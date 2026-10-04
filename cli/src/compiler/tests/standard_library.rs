//! The standard library against the runtime. Here because the runtime's
//! sources are `buri`'s.

use crate::compiler::standard_library::find;

/// The mailbox bound is one number, written twice, and the two spellings
/// must agree.
///
/// `cli/runtime/rt.rs` refuses to take a message past the bound, and
/// `core/actor` is where the number a reader of the module is told about
/// lives. A number quoted in the documentation that could drift from the
/// one the runtime enforces is a claim nobody can check.
///
/// Read out of the two sources rather than shared as a constant, because
/// they are two crates that never link against each other — the archive is
/// `include_bytes!`d — which is the same reason `BURI_OK` is transcribed in
/// `backend/runtime_table.rs` rather than imported.
#[test]
fn the_default_mailbox_is_the_one_core_actor_names() {
    const RUNTIME: &str = include_str!("../../../runtime/rt.rs");
    let buri = find("core/actor").expect("`core/actor` is in the table").source;
    // `split` rather than `find` and a range, because a byte range into a
    // `&str` is `clippy::string_slice` and this needs no offset — what
    // follows the needle is what the second piece begins with.
    let named = |text: &str, needle: &str| -> String {
        text.split(needle)
            .nth(1)
            .unwrap_or_else(|| panic!("no `{needle}`"))
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
    };
    let module = named(buri, "let MAILBOX: Int = ");
    let runtime = named(RUNTIME, "pub const MAILBOX: i64 = ");
    assert!(!module.is_empty(), "`core/actor` names no default mailbox");
    assert_eq!(
        module, runtime,
        "`core/actor`'s MAILBOX is {module} and `cli/runtime/rt.rs`'s is {runtime}"
    );
}
