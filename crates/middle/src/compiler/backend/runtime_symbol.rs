//! The runtime's symbol rule: which `buri_rt_*` symbol an intrinsic key
//! names.
//!
//! It is `buri-middle`'s rather than a backend's because
//! [`runtime_table`](super::runtime_table) names each row's symbol with it,
//! and the middle end reads that table. `buri-backend`'s `runtime_native`
//! re-exports it.

/// The prefix on every symbol the runtime exports.
///
/// One prefix and one rule, so that "is this symbol the runtime's" is a string
/// comparison and not a table. Both native backends import through it.
pub const SYMBOL_PREFIX: &str = "buri_rt_";

/// The symbol `cli/runtime/lib.rs` §1's rule names for an intrinsic key.
///
/// This names the symbol of a `runtime_table.rs` row (`Entry::symbol`); the
/// table, not this rule, decides which keys exist. A key the rule names is not
/// thereby a key the table has a row for: `str.concat` mangles to
/// `buri_rt_str_concat`, which the archive does export, and `runtime_table.rs`
/// still has no row for it and says why.
///
/// The rule is "[`SYMBOL_PREFIX`] followed by `snake_case`", plus one thing the
/// contract states by example rather than in words: `host.HostStdout.println`
/// is `buri_rt_host_stdout_println` and **not** `buri_rt_host_host_stdout_println`.
/// The effect type repeats its module in its name, and the symbol does not
/// repeat it twice — so a snake-cased segment that begins with the previous
/// segment drops that prefix. `host.HostAllocator.allocate` is
/// `buri_rt_host_allocator_allocate`, which is the same rule and keeps the
/// non-redundant repetition it happens to have.
///
/// One copy for every backend, because the rule is the runtime's and not any
/// one code generator's: two manglers that drifted would disagree about which
/// table is wrong.
pub fn symbol_for(key: &str) -> String {
    let mut out = String::from(SYMBOL_PREFIX);
    let mut previous = String::new();
    for (i, segment) in key.split('.').enumerate() {
        let mut piece = String::new();
        snake_into(segment, &mut piece);
        if !previous.is_empty() {
            if let Some(rest) = piece.strip_prefix(&format!("{previous}_")) {
                piece = rest.to_string();
            }
        }
        if i > 0 {
            out.push('_');
        }
        previous.clone_from(&piece);
        out.push_str(&piece);
    }
    out
}

/// `HostFileSystem` -> `host_file_system`, `readFile` -> `read_file`,
/// `nowMilliseconds` -> `now_milliseconds`. An underscore before an upper-case letter that follows a
/// lower-case one or a digit; runs of capitals are not split, because no key
/// has one.
fn snake_into(segment: &str, out: &mut String) {
    let mut previous_lower = false;
    for c in segment.chars() {
        if c.is_ascii_uppercase() {
            if previous_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            previous_lower = false;
        } else {
            out.push(c);
            previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
}
