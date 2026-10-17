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
//! base's entries: style extraction rewrites bodies, a scoped analysis checks
//! base bodies the snapshot left unchecked, and a method can be added to a
//! base type. [`LayeredMap`] is a hash map whose base entries are read
//! through.

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
        Layered {
            base: Arc::from(Vec::new()),
            own: Vec::new(),
        }
    }
}

impl<T: Clone> Clone for Layered<T> {
    fn clone(&self) -> Self {
        Layered {
            base: Arc::clone(&self.base),
            own: self.own.clone(),
        }
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
        let own = std::mem::take(&mut self.own);
        self.base = self.base.iter().cloned().chain(own).collect();
    }

    /// A list that shares every entry of this one and has none of its own.
    pub fn layer(&self) -> Layered<T> {
        Layered {
            base: Arc::clone(&self.base),
            own: Vec::new(),
        }
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
///
/// Only this module holds a [`Mint`], so an id comes from the index a
/// [`Table`] handed out or an [`IdMap`] was given.
pub trait DenseId: Copy {
    fn from_index(i: usize, mint: Mint) -> Self;
    fn to_index(self) -> usize;
}

/// What making an id from an index takes.
pub struct Mint(());

fn mint<I: DenseId>(i: usize) -> I {
    I::from_index(i, Mint(()))
}

/// A [`Layered`] list whose ids it mints itself, so reading one is total.
///
/// An analysis's own ids follow its base's, so a base id means the same entry
/// in every analysis layered on it, and an own id means something only in the
/// analysis that minted it and the layers taken from that.
pub struct Table<I, T> {
    rows: Layered<T>,
    id: std::marker::PhantomData<fn() -> I>,
}

impl<I, T: Clone> Clone for Table<I, T> {
    fn clone(&self) -> Self {
        Table {
            rows: self.rows.clone(),
            id: std::marker::PhantomData,
        }
    }
}

impl<I: DenseId, T> Table<I, T> {
    /// An empty table. Only this crate makes one, so no other code can mint
    /// ids the checker's tables never handed out.
    pub(crate) fn new() -> Self {
        Table {
            rows: Layered::default(),
            id: std::marker::PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn push(&mut self, row: T) -> I {
        self.push_with(|_| row)
    }

    /// Pushes the row `make` builds from the id it will have.
    pub fn push_with(&mut self, make: impl FnOnce(I) -> T) -> I {
        let id = mint(self.rows.len());
        self.rows.push(make(id));
        id
    }

    /// Grows a table keyed by another table's ids to `len` rows, that table's
    /// length, so every id it minted reads a row here too.
    pub(crate) fn fill(&mut self, len: usize, f: impl FnMut() -> T) {
        self.rows.resize_with(len, f);
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "only `push` mints an id, at the length it then grows past, and nothing shrinks a table"
    )]
    pub fn get(&self, id: I) -> &T {
        let i = id.to_index();
        match i.checked_sub(self.rows.base.len()) {
            None => &self.rows.base[i],
            Some(own) => &self.rows.own[own],
        }
    }

    /// The entry for `id`. A base entry is shared, so changing one copies the
    /// base first, and the change is this table's alone.
    #[expect(
        clippy::indexing_slicing,
        reason = "only `push` mints an id, at the length it then grows past, and nothing shrinks a table"
    )]
    pub fn get_mut(&mut self, id: I) -> &mut T
    where
        T: Clone,
    {
        let i = id.to_index();
        match i.checked_sub(self.rows.base.len()) {
            None => &mut Arc::make_mut(&mut self.rows.base)[i],
            Some(own) => &mut self.rows.own[own],
        }
    }

    /// Every entry, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.rows.iter()
    }

    /// Every entry with its id, in id order.
    pub fn entries(&self) -> impl Iterator<Item = (I, &T)> {
        self.rows.iter().enumerate().map(|(i, row)| (mint(i), row))
    }

    pub fn ids(&self) -> impl Iterator<Item = I> + use<I, T> {
        (0..self.rows.len()).map(mint)
    }

    /// The ids this table minted itself, after its base's.
    pub fn own_ids(&self) -> impl Iterator<Item = I> + use<I, T> {
        (self.rows.base_len()..self.rows.len()).map(mint)
    }

    /// See [`Layered::freeze`].
    pub fn freeze(&mut self)
    where
        T: Clone,
    {
        self.rows.freeze();
    }

    /// See [`Layered::layer`].
    pub fn layer(&self) -> Table<I, T> {
        Table {
            rows: self.rows.layer(),
            id: std::marker::PhantomData,
        }
    }
}

impl<'a, I, T> IntoIterator for &'a Table<I, T> {
    type Item = &'a T;
    type IntoIter = std::iter::Chain<std::slice::Iter<'a, T>, std::slice::Iter<'a, T>>;
    fn into_iter(self) -> Self::IntoIter {
        (&self.rows).into_iter()
    }
}

/// A list whose ids it mints itself, so reading one is total. [`Table`]
/// without a base: for lists that belong to one declaration or one body, such
/// as an enum's variants or a function's locals.
pub struct Dense<I, T> {
    rows: Vec<T>,
    id: std::marker::PhantomData<fn() -> I>,
}

impl<I, T> Dense<I, T> {
    pub const fn new() -> Self {
        Dense {
            rows: Vec::new(),
            id: std::marker::PhantomData,
        }
    }
}

impl<I, T> Default for Dense<I, T> {
    fn default() -> Self {
        Dense::new()
    }
}

impl<I, T: Clone> Clone for Dense<I, T> {
    fn clone(&self) -> Self {
        Dense {
            rows: self.rows.clone(),
            id: std::marker::PhantomData,
        }
    }
}

impl<I, T: std::fmt::Debug> std::fmt::Debug for Dense<I, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.rows.fmt(f)
    }
}

impl<I, T: PartialEq> PartialEq for Dense<I, T> {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows
    }
}

impl<I, T: Eq> Eq for Dense<I, T> {}

impl<I, T> FromIterator<T> for Dense<I, T> {
    fn from_iter<It: IntoIterator<Item = T>>(rows: It) -> Self {
        Dense {
            rows: rows.into_iter().collect(),
            id: std::marker::PhantomData,
        }
    }
}

impl<I: DenseId, T> Dense<I, T> {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn push(&mut self, row: T) -> I {
        let id = mint(self.rows.len());
        self.rows.push(row);
        id
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "only `push` and `entries` mint an id, below the length, and nothing shrinks a list"
    )]
    pub fn get(&self, id: I) -> &T {
        &self.rows[id.to_index()]
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "only `push` and `entries` mint an id, below the length, and nothing shrinks a list"
    )]
    pub fn get_mut(&mut self, id: I) -> &mut T {
        &mut self.rows[id.to_index()]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.rows.iter()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.rows
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.rows.iter_mut()
    }

    /// Every entry with its id, in id order.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = (I, &T)> {
        self.rows.iter().enumerate().map(|(i, row)| (mint(i), row))
    }

    pub fn ids(&self) -> impl DoubleEndedIterator<Item = I> + use<I, T> {
        (0..self.rows.len()).map(mint)
    }

    /// Every entry with its id, in id order, to change in place.
    pub fn entries_mut(&mut self) -> impl DoubleEndedIterator<Item = (I, &mut T)> {
        self.rows
            .iter_mut()
            .enumerate()
            .map(|(i, row)| (mint(i), row))
    }

    /// The id of the first entry `f` accepts.
    pub fn position(&self, f: impl FnMut(&T) -> bool) -> Option<I> {
        self.rows.iter().position(f).map(mint)
    }

    /// One `U` per entry, under the same ids.
    pub fn map<U>(&self, f: impl FnMut(&T) -> U) -> Dense<I, U> {
        self.rows.iter().map(f).collect()
    }

    /// Appends another list's entries, and answers where its ids landed.
    pub fn append(&mut self, other: &Dense<I, T>) -> Shift<I>
    where
        T: Clone,
    {
        let by = self.rows.len();
        self.rows.extend(other.rows.iter().cloned());
        Shift {
            by,
            id: std::marker::PhantomData,
        }
    }
}

/// Where [`Dense::append`] put the appended list's ids.
pub struct Shift<I> {
    by: usize,
    id: std::marker::PhantomData<fn() -> I>,
}

impl<I: DenseId> Shift<I> {
    /// The id the appended list's `id` has now.
    pub fn of(&self, id: I) -> I {
        mint(id.to_index().saturating_add(self.by))
    }
}

impl<'a, I, T> IntoIterator for &'a Dense<I, T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

impl<'a, I, T> IntoIterator for &'a mut Dense<I, T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter_mut()
    }
}

/// A map from a dense id to a value, for tables most ids have an entry in.
///
/// One slot per id instead of a hash: a lookup is an index, and walking it
/// goes in id order. The analysis's own slots start after the base's, and a
/// base entry it replaces is kept beside them, so its memory and the cost of
/// copying it are its own entries' and not the base's.
pub struct IdMap<I, T> {
    base: Arc<[Option<T>]>,
    /// Slots for the ids from `base.len()` on.
    own: Vec<Option<T>>,
    /// The base entries this map replaced, by index.
    replaced: HashMap<usize, T>,
    marker: std::marker::PhantomData<I>,
}

impl<I, T> Default for IdMap<I, T> {
    fn default() -> Self {
        IdMap {
            base: Arc::from(Vec::new()),
            own: Vec::new(),
            replaced: HashMap::default(),
            marker: std::marker::PhantomData,
        }
    }
}

impl<I, T: Clone> Clone for IdMap<I, T> {
    fn clone(&self) -> Self {
        IdMap {
            base: Arc::clone(&self.base),
            own: self.own.clone(),
            replaced: self.replaced.clone(),
            marker: std::marker::PhantomData,
        }
    }
}

impl<I: DenseId, T> IdMap<I, T> {
    pub fn get(&self, id: &I) -> Option<&T> {
        let i = id.to_index();
        match i.checked_sub(self.base.len()) {
            Some(own) => self.own.get(own)?.as_ref(),
            None if self.replaced.is_empty() => self.base.get(i)?.as_ref(),
            None => self.replaced.get(&i).or_else(|| self.base.get(i)?.as_ref()),
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
        match i.checked_sub(self.base.len()) {
            Some(own) => self.own.get_mut(own)?.as_mut(),
            None => {
                if !self.replaced.contains_key(&i) {
                    let shared = self.base.get(i)?.as_ref()?.clone();
                    self.replaced.insert(i, shared);
                }
                self.replaced.get_mut(&i)
            }
        }
    }

    /// The entry for `id`, made this map's own first, and filled with
    /// `T::default()` when there was none.
    pub fn get_or_default(&mut self, id: I) -> &mut T
    where
        T: Clone + Default,
    {
        let i = id.to_index();
        match i.checked_sub(self.base.len()) {
            Some(own) => self.slot(own).get_or_insert_with(T::default),
            None => {
                let base = &self.base;
                self.replaced.entry(i).or_insert_with(|| {
                    base.get(i)
                        .and_then(Option::as_ref)
                        .cloned()
                        .unwrap_or_default()
                })
            }
        }
    }

    pub fn insert(&mut self, id: I, value: T) {
        let i = id.to_index();
        match i.checked_sub(self.base.len()) {
            Some(own) => {
                self.slot(own).replace(value);
            }
            None => {
                self.replaced.insert(i, value);
            }
        }
    }

    /// The slot for the analysis's own entry at `own`, past the base.
    fn slot(&mut self, own: usize) -> &mut Option<T> {
        if self.own.len() <= own {
            self.own.resize_with(own.saturating_add(1), || None);
        }
        #[expect(
            clippy::indexing_slicing,
            reason = "the slots were just grown past this one"
        )]
        let slot = &mut self.own[own];
        slot
    }

    /// The entries this map holds itself rather than reads from its base:
    /// what was written since [`IdMap::layer`]. Unordered.
    pub fn written(&self) -> impl Iterator<Item = (I, &T)> {
        let from = self.base.len();
        let replaced = self.replaced.iter().map(|(i, value)| (mint(*i), value));
        let own =
            self.own.iter().enumerate().filter_map(move |(i, value)| {
                Some((mint(from.saturating_add(i)), value.as_ref()?))
            });
        replaced.chain(own)
    }

    /// Every entry, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.iter_from(0)
    }

    /// Every entry whose id is `start` or later, in id order.
    pub fn iter_from(&self, start: usize) -> impl Iterator<Item = (I, &T)> {
        let len = self.base.len().saturating_add(self.own.len());
        (start..len).filter_map(move |i| {
            let id = mint(i);
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
        let len = self.base.len().saturating_add(self.own.len());
        let all: Vec<Option<T>> = (0..len).map(|i| self.get(&mint(i)).cloned()).collect();
        self.base = Arc::from(all);
        self.own = Vec::new();
        self.replaced = HashMap::default();
    }

    /// A map that shares every entry of this one and has none of its own.
    pub fn layer(&self) -> IdMap<I, T> {
        IdMap {
            base: Arc::clone(&self.base),
            own: Vec::new(),
            replaced: HashMap::default(),
            marker: std::marker::PhantomData,
        }
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
/// key falls through to the base.
pub struct LayeredMap<K, V> {
    base: Arc<HashMap<K, V>>,
    own: HashMap<K, V>,
}

impl<K, V> Default for LayeredMap<K, V> {
    fn default() -> Self {
        LayeredMap {
            base: Arc::new(HashMap::default()),
            own: HashMap::default(),
        }
    }
}

impl<K: Clone, V: Clone> Clone for LayeredMap<K, V> {
    fn clone(&self) -> Self {
        LayeredMap {
            base: Arc::clone(&self.base),
            own: self.own.clone(),
        }
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

    pub fn insert(&mut self, key: K, value: V) {
        self.own.insert(key, value);
    }

    /// Every entry, the analysis's own and then the base's it hasn't
    /// replaced. Unordered, as a hash map's walk is.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.own
            .iter()
            .chain(self.base.iter().filter(|(k, _)| !self.own.contains_key(*k)))
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
        LayeredMap {
            base: Arc::clone(&self.base),
            own: HashMap::default(),
        }
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
        fn from_index(i: usize, _: Mint) -> Self {
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
        assert_eq!(
            layer.iter().collect::<Vec<_>>(),
            vec![(Id(0), &"A"), (Id(2), &"C"), (Id(4), &"e")]
        );
        assert_eq!(
            base.iter().collect::<Vec<_>>(),
            vec![(Id(0), &"a"), (Id(2), &"c")]
        );
        let mut written: Vec<_> = layer.written().collect();
        written.sort_by_key(|(id, _)| id.0);
        assert_eq!(written, vec![(Id(0), &"A"), (Id(2), &"C"), (Id(4), &"e")]);
    }

    #[test]
    fn an_id_map_fills_a_missing_entry_with_its_default() {
        let mut base: IdMap<Id, Vec<u32>> = IdMap::default();
        base.insert(Id(1), vec![1]);
        base.freeze();
        let mut layer = base.layer();
        layer.get_or_default(Id(1)).push(2);
        layer.get_or_default(Id(0)).push(0);
        layer.get_or_default(Id(3)).push(3);
        assert_eq!(layer.get(&Id(0)), Some(&vec![0]));
        assert_eq!(layer.get(&Id(1)), Some(&vec![1, 2]));
        assert_eq!(layer.get(&Id(2)), None);
        assert_eq!(layer.get(&Id(3)), Some(&vec![3]));
        assert_eq!(base.get(&Id(1)), Some(&vec![1]));
    }

    #[test]
    fn a_layered_map_reads_through_to_its_base() {
        let mut base: LayeredMap<u32, &str> = LayeredMap::default();
        base.insert(1, "a");
        base.freeze();
        let mut layer = base.layer();
        layer.insert(1, "A");
        layer.insert(3, "c");
        assert_eq!(layer.get(&1), Some(&"A"));
        assert_eq!(layer.get(&3), Some(&"c"));
        assert_eq!(base.get(&1), Some(&"a"));
        assert!(base.get(&3).is_none());
        assert_eq!(layer.iter().count(), 2);
    }
}
