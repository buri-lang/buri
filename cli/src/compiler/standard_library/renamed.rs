//! The names this library used to answer to.
//!
//! [`RETIRED`] does this for module *paths*. This is the same promise one level
//! down: a name the rename pass moved gets told what it is called now, instead
//! of the nearest thing by edit distance. The two are next to each other
//! because they are one bargain — a rename is not an alias, the old spelling
//! resolves to nothing, and what a reader gets back for it is a sentence.
//!
//! It is a table here rather than a marker in `sources/*.buri` because a
//! marker would be a second export surface: every renamed declaration would
//! carry a name nothing resolves, the formatter and the docs renderer would
//! both have to know to skip it, and the library's own pages would be written
//! partly in names the library does not have. The rename happened once, in one
//! pass, and this is what that pass moved.
//!
//! Nothing here is loadable and nothing here resolves. The table is read only
//! after a lookup has already failed, so a name that came back into service
//! shadows its own row rather than colliding with it —
//! `no_renamed_name_is_still_exported` is what says a row cannot be both.
//!
//! [`RETIRED`]: super::RETIRED

/// What to write instead of a name this library used to have.
pub enum Now {
    /// The same thing under a new spelling.
    Named(&'static str),
    /// Gone, and what covers it now — a whole expression, because a name that
    /// was removed has no one word to offer.
    Write(&'static str),
}

pub struct Renamed {
    /// The module the name belonged to: the module that exported it, or, for a
    /// method, the module its type is declared in (SPEC 6.7.3).
    pub module: &'static str,
    pub old: &'static str,
    pub now: Now,
}

const fn r(module: &'static str, old: &'static str, now: &'static str) -> Renamed {
    Renamed { module, old, now: Now::Named(now) }
}

const fn gone(module: &'static str, old: &'static str, write: &'static str) -> Renamed {
    Renamed { module, old, now: Now::Write(write) }
}

/// Every name the rename pass moved, and the counts `core/time` took back.
///
/// Grouped by module and, within a module, in the order a reader would look
/// them up. A trait's or an effect's own methods are filed under the module
/// that declares it, and so is a method on a type — `len` belongs to `core/str`
/// as much as `length` does.
pub const RENAMED: &[Renamed] = &[
    r("core/bigint", "mul", "multiply"),
    r("core/bigint", "sub", "subtract"),
    r("core/bits", "sar", "shiftRightArithmetic"),
    r("core/bits", "shl", "shiftLeft"),
    r("core/bits", "shlU8", "shiftLeftU8"),
    r("core/bits", "shlU32", "shiftLeftU32"),
    r("core/bits", "shlU64", "shiftLeftU64"),
    r("core/bits", "shr", "shiftRight"),
    r("core/bits", "shrU8", "shiftRightU8"),
    r("core/bits", "shrU32", "shiftRightU32"),
    r("core/bits", "shrU64", "shiftRightU64"),
    r("core/bytes", "fromU32Be", "fromU32BigEndian"),
    r("core/bytes", "fromU32Le", "fromU32LittleEndian"),
    r("core/bytes", "fromU64Be", "fromU64BigEndian"),
    r("core/bytes", "fromU64Le", "fromU64LittleEndian"),
    r("core/bytes", "toU32Be", "toU32BigEndian"),
    r("core/bytes", "toU32Le", "toU32LittleEndian"),
    r("core/bytes", "toU64Be", "toU64BigEndian"),
    r("core/bytes", "toU64Le", "toU64LittleEndian"),
    r("core/date", "MILLIS_PER_DAY", "MILLISECONDS_PER_DAY"),
    r("core/date", "MILLIS_PER_HOUR", "MILLISECONDS_PER_HOUR"),
    r("core/date", "MILLIS_PER_MINUTE", "MILLISECONDS_PER_MINUTE"),
    r("core/date", "MILLIS_PER_SECOND", "MILLISECONDS_PER_SECOND"),
    r("core/decimal", "mul", "multiply"),
    r("core/decimal", "sub", "subtract"),
    r("core/effect", "Alloc", "Allocator"),
    r("core/effect", "Env", "Environment"),
    r("core/effect", "Net", "Network"),
    r("core/effect", "Proc", "Process"),
    r("core/effect", "Rand", "Random"),
    r("core/effect", "args", "arguments"),
    r("core/effect", "nowMillis", "nowMilliseconds"),
    r("core/effect", "sleepMillis", "sleepMilliseconds"),
    r("core/env", "args", "arguments"),
    r("core/fs", "FsRead", "FileSystemRead"),
    r("core/fs", "FsWrite", "FileSystemWrite"),
    r("core/host", "HostAlloc", "HostAllocator"),
    r("core/host", "HostEnv", "HostEnvironment"),
    r("core/host", "HostFs", "HostFileSystem"),
    r("core/host", "HostNet", "HostNetwork"),
    r("core/host", "HostProc", "HostProcess"),
    r("core/host", "HostRand", "HostRandom"),
    // The reader took the name `arguments`, so the builder that had it is
    // `withArguments` — the one row where a name's answer depends on which
    // module asked.
    r("core/host/testing", "args", "withArguments"),
    r("core/host/testing", "TestAlloc", "TestAllocator"),
    r("core/host/testing", "TestEnv", "TestEnvironment"),
    r("core/host/testing", "TestFs", "TestFileSystem"),
    r("core/host/testing", "TestNet", "TestNetwork"),
    r("core/host/testing", "TestProc", "TestProcess"),
    r("core/host/testing", "TestRand", "TestRandom"),
    r("core/json", "asNum", "asNumber"),
    r("core/list", "len", "length"),
    r("core/map", "len", "length"),
    r("core/math", "absFloat", "absoluteFloat"),
    r("core/math", "cbrt", "cubeRoot"),
    r("core/math", "ceil", "ceiling"),
    r("core/math", "pow", "power"),
    r("core/math", "sqrt", "squareRoot"),
    r("core/math", "trunc", "truncate"),
    r("core/number", "Div", "Divide"),
    r("core/number", "Mul", "Multiply"),
    r("core/number", "Neg", "Negate"),
    r("core/number", "Rem", "Remainder"),
    r("core/number", "Sub", "Subtract"),
    r("core/number", "checkedDiv", "checkedDivide"),
    r("core/number", "checkedMul", "checkedMultiply"),
    r("core/number", "checkedSub", "checkedSubtract"),
    r("core/number", "div", "divide"),
    r("core/number", "divFloor", "divideFloor"),
    r("core/number", "mul", "multiply"),
    r("core/number", "neg", "negate"),
    r("core/number", "rem", "remainder"),
    r("core/number", "remEuclid", "remainderEuclidean"),
    r("core/number", "saturatingMul", "saturatingMultiply"),
    r("core/number", "saturatingSub", "saturatingSubtract"),
    r("core/number", "sub", "subtract"),
    r("core/number", "wrappingMul", "wrappingMultiply"),
    r("core/number", "wrappingSub", "wrappingSubtract"),
    r("core/order", "Eq", "Equal"),
    r("core/order", "Ord", "Ordered"),
    r("core/order", "eq", "equal"),
    r("core/orderedmap", "OrdMap", "OrderedMap"),
    r("core/orderedmap", "len", "length"),
    r("core/orderedset", "OrdSet", "OrderedSet"),
    r("core/orderedset", "len", "length"),
    r("core/queue", "len", "length"),
    r("core/random", "Gen", "Generator"),
    r("core/random", "gen", "generator"),
    r("core/set", "len", "length"),
    r("core/simd", "ZERO_F", "ZERO_FLOAT"),
    r("core/simd", "ZERO_I", "ZERO_INT"),
    r("core/simd", "div", "divide"),
    r("core/simd", "mul", "multiply"),
    r("core/simd", "neg", "negate"),
    r("core/simd", "selectF", "selectFloat"),
    r("core/simd", "selectI", "selectInt"),
    r("core/simd", "shl", "shiftLeft"),
    r("core/simd", "shr", "shiftRight"),
    r("core/simd", "splatF", "splatFloat"),
    r("core/simd", "splatI", "splatInt"),
    r("core/simd", "sqrt", "squareRoot"),
    r("core/simd", "sub", "subtract"),
    r("core/str", "len", "length"),
    r("core/testing/assert", "approxEq", "approximatelyEqual"),
    r("core/testing/assert", "approxEqRelative", "approximatelyEqualRelative"),
    r("core/testing/assert", "eq", "equal"),
    r("core/testing/assert", "eqWith", "equalWith"),
    r("core/testing/assert", "ge", "greaterOrEqual"),
    r("core/testing/assert", "gt", "greaterThan"),
    r("core/testing/assert", "le", "lessOrEqual"),
    r("core/testing/assert", "len", "length"),
    r("core/testing/assert", "lt", "lessThan"),
    r("core/testing/assert", "notEq", "notEqual"),
    r("core/time", "micros", "microseconds"),
    r("core/time", "millis", "milliseconds"),
    r("core/time", "mul", "multiply"),
    r("core/time", "nanos", "nanoseconds"),
    r("core/time", "sub", "subtract"),
    // `core/time`'s counts went private in the same breath, so both spellings
    // of each answer with the `Duration` that carries the fact instead.
    gone("core/time", "ZERO", "time.nanoseconds(0)"),
    gone("core/time", "NANOS_PER_MICROSECOND", "time.microseconds(1).nanoseconds()"),
    gone("core/time", "NANOS_PER_MILLISECOND", "time.milliseconds(1).nanoseconds()"),
    gone("core/time", "NANOS_PER_SECOND", "time.seconds(1).nanoseconds()"),
    gone("core/time", "NANOS_PER_MINUTE", "time.minutes(1).nanoseconds()"),
    gone("core/time", "NANOS_PER_HOUR", "time.hours(1).nanoseconds()"),
    gone("core/time", "NANOS_PER_DAY", "time.hours(24).nanoseconds()"),
    gone("core/time", "NANOSECONDS_PER_MICROSECOND", "time.microseconds(1).nanoseconds()"),
    gone("core/time", "NANOSECONDS_PER_MILLISECOND", "time.milliseconds(1).nanoseconds()"),
    gone("core/time", "NANOSECONDS_PER_SECOND", "time.seconds(1).nanoseconds()"),
    gone("core/time", "NANOSECONDS_PER_MINUTE", "time.minutes(1).nanoseconds()"),
    gone("core/time", "NANOSECONDS_PER_HOUR", "time.hours(1).nanoseconds()"),
    gone("core/time", "NANOSECONDS_PER_DAY", "time.hours(24).nanoseconds()"),
    gone("core/time", "sleepMs", "time.sleep(ctx, time.milliseconds(n))"),
];

/// The note and the fix for a name this library used to have: what happened to
/// it, and what to type.
///
/// One wording for every diagnostic that names a missing member, method, type
/// or effect, because it is one fact and the reader does not care which pass
/// found it.
fn advice(old: &str, now: &Now) -> (String, String) {
    match now {
        Now::Named(name) => {
            (format!("`{old}` was renamed to `{name}`"), format!("write `{name}`"))
        }
        Now::Write(what) => {
            (format!("`{old}` was removed; write `{what}`"), format!("write `{what}`"))
        }
    }
}

/// What became of `old`, asked of the module that had it.
pub fn in_module(module: &str, old: &str) -> Option<(String, String)> {
    let canonical = module.strip_suffix("/lib.buri").unwrap_or(module);
    RENAMED
        .iter()
        .find(|row| row.module == canonical && row.old == old)
        .map(|row| advice(old, &row.now))
}

/// What became of `old`, asked with no module to narrow it — a bare name in a
/// bound, a `derive`, a `context`, or an expression.
///
/// Answers only where every module that had the name agrees about it, so
/// `args` — `arguments` in one module and `withArguments` in another — gets
/// nothing rather than a coin toss.
pub fn anywhere(old: &str) -> Option<(String, String)> {
    let mut rows = RENAMED.iter().filter(|row| row.old == old);
    let first = rows.next()?;
    let same = |a: &Now, b: &Now| match (a, b) {
        (Now::Named(x), Now::Named(y)) | (Now::Write(x), Now::Write(y)) => x == y,
        _ => false,
    };
    rows.all(|row| same(&row.now, &first.now)).then(|| advice(old, &first.now))
}
