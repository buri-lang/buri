//! Tables an analysis shares with the standard library snapshot it resumes
//! from, rather than copies.
//!
//! An analysis starts from what checking the standard library left behind
//! (`resolve::Base`) and adds its own modules' declarations after it. Every
//! table here keeps the base's entries behind an `Arc` and the analysis's own
//! in a vector or a map of its own, so starting an analysis costs a reference
//! count per table instead of a copy of the checked library. Ids keep running
//! after the base's, so an id means the same entry whichever half holds it.
//!
//! [`Layered`] is an append-only list, where the base's entries never change.
//! [`IdMap`] is a map from a dense id, where an analysis may replace one of the
//! base's entries: style extraction rewrites bodies, and a scoped analysis
//! checks base bodies the snapshot left unchecked. [`LayeredMap`] is a hash map
//! whose base entries are read through and copied before they're changed.

use crate::diagnostics::Invariant as _;
use crate::hash::Map as HashMap;
use std::hash::Hash;
use std::sync::Arc;

/// An append-only list whose leading entries are shared.
///
/// Index `i` reads the base below `base.len()` and the analysis's own entries
/// from there on. The base's entries are read-only: [`Layered::get_mut`]
/// answers `None` for one.
pub struct Layered<T> {
    base: Arc<[T]>,
    own: Vec<T>,
}

impl<T> Default for Layered<T> {
    fn default() -> Self {
        Layered { base: Arc::from(Vec::new()), own: Vec::new() }
    }
}

impl<T: Clone> Clone for Layered<T> {
    fn clone(&self) -> Self {
        Layered { base: Arc::clone(&self.base), own: self.own.clone() }
    }
}

impl<T> Layered<T> {
    pub fn len(&self) -> usize {
        self.base.len().saturating_add(self.own.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many entries are shared.
    pub fn base_len(&self) -> usize {
        self.base.len()
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        match i.checked_sub(self.base.len()) {
            None => self.base.get(i),
            Some(own) => self.own.get(own),
        }
    }

    /// The entry at `i`, when it is this analysis's own.
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        let own = i.checked_sub(self.base.len())?;
        self.own.get_mut(own)
    }

    pub fn push(&mut self, value: T) {
        self.own.push(value);
    }

    /// Grows the analysis's own entries until there are `len` in all.
    pub fn resize_with(&mut self, len: usize, f: impl FnMut() -> T) {
        let own = len.saturating_sub(self.base.len()).max(self.own.len());
        self.own.resize_with(own, f);
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.base.iter().chain(self.own.iter())
    }

    /// The analysis's own entries, the ones after the base.
    pub fn own_mut(&mut self) -> &mut [T] {
        &mut self.own
    }

    /// Replaces the analysis's own entries.
    pub fn set_own(&mut self, own: Vec<T>) {
        self.own = own;
    }

    /// Makes every entry shared, so a [`Layered::layer`] taken next starts
    /// after all of them.
    pub fn freeze(&mut self)
    where
        T: Clone,
    {
        if self.own.is_empty() {
            return;
        }
        let own = std::mem::take(&mut self.own);
        self.base = if self.base.is_empty() {
            Arc::from(own)
        } else {
            self.base.iter().cloned().chain(own).collect()
        };
    }

    /// A list that shares every entry of this one and has none of its own.
    pub fn layer(&self) -> Layered<T> {
        Layered { base: Arc::clone(&self.base), own: Vec::new() }
    }
}

impl<'a, T> IntoIterator for &'a Layered<T> {
    type Item = &'a T;
    type IntoIter = std::iter::Chain<std::slice::Iter<'a, T>, std::slice::Iter<'a, T>>;
    fn into_iter(self) -> Self::IntoIter {
        self.base.iter().chain(self.own.iter())
    }
}

/// An id that indexes a dense table.
pub trait DenseId: Copy {
    fn from_index(i: usize) -> Self;
    fn to_index(self) -> usize;
}

/// A map from a dense id to a value, for tables most ids have an entry in.
///
/// One slot per id instead of a hash: a lookup is an index, and walking it
/// goes in id order. An analysis's own slots override the base's, which is how
/// it replaces a base entry without copying the rest.
pub struct IdMap<I, T> {
    base: Arc<[Option<T>]>,
    own: Vec<Option<T>>,
    marker: std::marker::PhantomData<I>,
}

impl<I, T> Default for IdMap<I, T> {
    fn default() -> Self {
        IdMap { base: Arc::from(Vec::new()), own: Vec::new(), marker: std::marker::PhantomData }
    }
}

impl<I, T: Clone> Clone for IdMap<I, T> {
    fn clone(&self) -> Self {
        IdMap { base: Arc::clone(&self.base), own: self.own.clone(), marker: std::marker::PhantomData }
    }
}

impl<I: DenseId, T> IdMap<I, T> {
    pub fn get(&self, id: &I) -> Option<&T> {
        let i = id.to_index();
        match self.own.get(i) {
            Some(Some(value)) => Some(value),
            _ => self.base.get(i)?.as_ref(),
        }
    }

    pub fn contains_key(&self, id: &I) -> bool {
        self.get(id).is_some()
    }

    /// The entry for `id`, copied out of the base first when that is where it
    /// is.
    pub fn get_mut(&mut self, id: &I) -> Option<&mut T>
    where
        T: Clone,
    {
        let i = id.to_index();
        if !matches!(self.own.get(i), Some(Some(_))) {
            let shared = self.base.get(i)?.as_ref()?.clone();
            self.slot(i).replace(shared);
        }
        self.own.get_mut(i)?.as_mut()
    }

    pub fn insert(&mut self, id: I, value: T) {
        self.slot(id.to_index()).replace(value);
    }

    fn slot(&mut self, i: usize) -> &mut Option<T> {
        if self.own.len() <= i {
            self.own.resize_with(i.saturating_add(1), || None);
        }
        self.own.get_mut(i).or_ice("the slots were just grown past this one")
    }

    /// The entries this map holds itself rather than reads from its base:
    /// what was written since [`IdMap::layer`], in id order.
    pub fn written(&self) -> impl Iterator<Item = (I, &T)> {
        self.own.iter().enumerate().filter_map(|(i, value)| Some((I::from_index(i), value.as_ref()?)))
    }

    /// Every entry, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.iter_from(0)
    }

    /// Every entry whose id is `start` or later, in id order.
    pub fn iter_from(&self, start: usize) -> impl Iterator<Item = (I, &T)> {
        let len = self.base.len().max(self.own.len());
        (start..len).filter_map(move |i| {
            let id = I::from_index(i);
            self.get(&id).map(|value| (id, value))
        })
    }

    pub fn keys(&self) -> impl Iterator<Item = I> + '_ {
        self.iter().map(|(id, _)| id)
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.iter().map(|(_, value)| value)
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// Makes every entry shared. See [`Layered::freeze`].
    pub fn freeze(&mut self)
    where
        T: Clone,
    {
        if self.own.is_empty() {
            return;
        }
        let len = self.base.len().max(self.own.len());
        let all: Vec<Option<T>> = (0..len).map(|i| self.get(&I::from_index(i)).cloned()).collect();
        self.base = Arc::from(all);
        self.own = Vec::new();
    }

    /// A map that shares every entry of this one and has none of its own.
    pub fn layer(&self) -> IdMap<I, T> {
        IdMap { base: Arc::clone(&self.base), own: Vec::new(), marker: std::marker::PhantomData }
    }
}

impl<'a, I: DenseId + 'a, T> IntoIterator for &'a IdMap<I, T> {
    type Item = (I, &'a T);
    type IntoIter = Box<dyn Iterator<Item = (I, &'a T)> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl<I: DenseId, T> FromIterator<(I, T)> for IdMap<I, T> {
    fn from_iter<It: IntoIterator<Item = (I, T)>>(entries: It) -> Self {
        let mut map = IdMap::default();
        for (id, value) in entries {
            map.insert(id, value);
        }
        map
    }
}

/// A hash map whose base entries are shared.
///
/// A key the analysis has written is read from its own map, and every other
/// key falls through to the base. [`LayeredMap::get_mut`] copies a base entry
/// into the analysis's map before handing it out.
pub struct LayeredMap<K, V> {
    base: Arc<HashMap<K, V>>,
    own: HashMap<K, V>,
}

impl<K, V> Default for LayeredMap<K, V> {
    fn default() -> Self {
        LayeredMap { base: Arc::new(HashMap::default()), own: HashMap::default() }
    }
}

impl<K: Clone, V: Clone> Clone for LayeredMap<K, V> {
    fn clone(&self) -> Self {
        LayeredMap { base: Arc::clone(&self.base), own: self.own.clone() }
    }
}

impl<K: Eq + Hash, V> LayeredMap<K, V> {
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.own.get(key).or_else(|| self.base.get(key))
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.own.contains_key(key) || self.base.contains_key(key)
    }

    /// Whether the base holds `key`, whatever the analysis did since.
    pub fn in_base<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.base.contains_key(key)
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.own.insert(key, value);
    }

    /// The entry for `key`, made the analysis's own first, and filled with
    /// `V::default()` when neither half has one.
    pub fn entry_mut(&mut self, key: K) -> &mut V
    where
        K: Clone,
        V: Clone + Default,
    {
        let shared = &self.base;
        self.own.entry(key.clone()).or_insert_with(|| shared.get(&key).cloned().unwrap_or_default())
    }

    /// Every entry, the analysis's own and then the base's it hasn't
    /// replaced. Unordered, as a hash map's walk is.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.own.iter().chain(self.base.iter().filter(|(k, _)| !self.own.contains_key(*k)))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }

    /// Makes every entry shared. See [`Layered::freeze`].
    pub fn freeze(&mut self)
    where
        K: Clone,
        V: Clone,
    {
        if self.own.is_empty() {
            return;
        }
        let own = std::mem::take(&mut self.own);
        match Arc::get_mut(&mut self.base) {
            Some(base) => base.extend(own),
            None => {
                let mut all: HashMap<K, V> = (*self.base).clone();
                all.extend(own);
                self.base = Arc::new(all);
            }
        }
    }

    /// A map that shares every entry of this one and has none of its own.
    pub fn layer(&self) -> LayeredMap<K, V> {
        LayeredMap { base: Arc::clone(&self.base), own: HashMap::default() }
    }
}

impl<'a, K: Eq + Hash, V> IntoIterator for &'a LayeredMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Box<dyn Iterator<Item = (&'a K, &'a V)> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Debug)]
    struct Id(u32);
    impl DenseId for Id {
        fn from_index(i: usize) -> Self {
            Id(i as u32)
        }
        fn to_index(self) -> usize {
            self.0 as usize
        }
    }

    #[test]
    fn a_layer_reads_the_base_and_appends_after_it() {
        let mut base: Layered<u32> = Layered::default();
        base.push(1);
        base.push(2);
        base.freeze();
        let mut layer = base.layer();
        layer.push(3);
        assert_eq!(layer.len(), 3);
        assert_eq!(layer.get(1), Some(&2));
        assert_eq!(layer.get(2), Some(&3));
        assert!(layer.get_mut(0).is_none());
        assert_eq!(layer.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(base.len(), 2);
    }

    #[test]
    fn an_id_map_overrides_a_base_entry_without_changing_the_base() {
        let mut base: IdMap<Id, &str> = IdMap::default();
        base.insert(Id(0), "a");
        base.insert(Id(2), "c");
        base.freeze();
        let mut layer = base.layer();
        layer.insert(Id(2), "C");
        layer.insert(Id(4), "e");
        *layer.get_mut(&Id(0)).unwrap() = "A";
        assert_eq!(layer.iter().collect::<Vec<_>>(), vec![(Id(0), &"A"), (Id(2), &"C"), (Id(4), &"e")]);
        assert_eq!(base.iter().collect::<Vec<_>>(), vec![(Id(0), &"a"), (Id(2), &"c")]);
    }

    #[test]
    fn a_layered_map_copies_a_base_entry_before_changing_it() {
        let mut base: LayeredMap<u32, Vec<u32>> = LayeredMap::default();
        base.insert(1, vec![1]);
        base.freeze();
        let mut layer = base.layer();
        layer.entry_mut(1).push(2);
        layer.entry_mut(3).push(3);
        assert_eq!(layer.get(&1), Some(&vec![1, 2]));
        assert_eq!(layer.get(&3), Some(&vec![3]));
        assert_eq!(base.get(&1), Some(&vec![1]));
        assert_eq!(layer.iter().count(), 2);
    }
}
