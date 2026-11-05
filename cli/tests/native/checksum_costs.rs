//! What `core/hash`'s `crc32c` costs an octet, natively, on every native
//! backend this toolchain has built in, counted rather than timed.

use crate::collection_costs::per_op;

/// 64 KiB hashed 32 times by `mode`'s function, the answers summed. `main`
/// takes the mode.
const HASHES: &str = r#"
from "core/env" import * as env;
from "core/hash" import * as hash;
from "core/io" import * as io;
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Environment, Stdout };

fn rounds(b: [U8], k: Int, acc: Int, f: fn([U8]) => U32): Int {
  if (k == 0) { acc } else { rounds(b, k - 1, acc + f(b).toI64(), f) }
}

export fn main(host: NativeHost): Result<(), Str> {
  let ctx = context { Allocator: host.alloc, Environment: host.env, Stdout: host.stdout };
  let b = list.range(ctx, 0, 65536).map(ctx, fn(i) => (i * 31 + 7).wrapToU8());
  let answer = match (env.arguments(ctx).first()) {
    .Some("crc32c") => rounds(b, 32, 0, fn(x) => hash.crc32c(x)),
    .Some("fnv1a32") => rounds(b, 32, 0, fn(x) => hash.fnv1a32(x)),
    _ => 0,
  };
  io.println(ctx, "${answer}").mapErr(fn(_e) => "stdout")
}
"#;

/// **`crc32c` costs less an octet than FNV-1a's octet-at-a-time fold.** It
/// was a bit at a time, eight shifts an octet: 432 instructions an octet
/// against `fnv1a32`'s 55 on the copy-and-patch backend, 58 against 4 on LLVM,
/// and 10% of a database simulator's test run. The processor's CRC
/// instructions fold eight octets in one, and the table fallback eight in
/// eight lookups.
#[test]
fn crc32c_costs_less_than_an_octet_at_a_time_hash() {
    let octets = 65536 * 32;
    let mut failures = Vec::new();
    for (backend, build) in crate::e2e::probed_backends() {
        let binary = build("checksum-costs", HASHES);
        let (crc, _) = per_op(backend, &binary, "crc32c", &[], octets, "46098004384\n");
        let (fnv, _) = per_op(backend, &binary, "fnv1a32", &[], octets, "62932629664\n");
        eprintln!("{backend}: crc32c {crc:?}, fnv1a32 {fnv:?} instructions an octet");
        let (Some(crc), Some(fnv)) = (crc, fnv) else {
            continue;
        };
        if crc >= fnv {
            failures.push(format!(
                "{backend}: crc32c is {crc} instructions an octet against fnv1a32's {fnv}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
