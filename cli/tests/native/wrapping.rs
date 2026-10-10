//! `wrapTo*` from a float, on every backend: truncate toward zero, then keep
//! the low bits, with `NaN` and the infinities at `0` (SPEC 6.2.1).
//!
//! Each program runs through JavaScript and every native backend this binary
//! was built with, under the heap check: [`crate::agreement`]'s `agree`.
use crate::agreement::{agree, skip_reason};

macro_rules! rows_or_skip {
    () => {
        if let Some(why) = skip_reason() {
            crate::ci::skipped("backend agreement", &why);
            return;
        }
    };
}

/// Every integer width, both signs, fractions, `-0.0`, the 32- and 64-bit
/// edges, `2^64` and `2^65`, huge values, `NaN` and both infinities, from an
/// `F64` and from an `F32`. Each answer is the truncated value modulo `2^n`.
#[test]
fn wrap_to_from_a_float_is_modular() {
    rows_or_skip!();
    agree(
        "wrapTo from a float",
        r#"
from "core/io" import * as io;
from "native" import { NativeHost };

fn id(x: Float): Float { x }
fn id32(x: F32): F32 { x }
fn nan(): Float { id(0.0) / id(0.0) }
fn inf(): Float { id(1.0) / id(0.0) }

fn say(host: NativeHost, x: Float): () {
    io.println(host.stdout, "${x.wrapToI8()} ${x.wrapToI16()} ${x.wrapToI32()} ${x.wrapToI64()} ${x.wrapToU8()} ${x.wrapToU16()} ${x.wrapToU32()} ${x.wrapToU64()}").ignore()
}

fn say32(host: NativeHost, x: F32): () {
    io.println(host.stdout, "${x.wrapToI8()} ${x.wrapToI16()} ${x.wrapToI32()} ${x.wrapToI64()} ${x.wrapToU8()} ${x.wrapToU16()} ${x.wrapToU32()} ${x.wrapToU64()}").ignore()
}

export fn main(host: NativeHost): Result<(), Str> {
    let _ = say(host, id(0.0));
    let _ = say(host, id(-0.0));
    let _ = say(host, id(2.7));
    let _ = say(host, id(-2.7));
    let _ = say(host, id(-1.5));
    let _ = say(host, id(127.9));
    let _ = say(host, id(128.0));
    let _ = say(host, id(255.0));
    let _ = say(host, id(256.0));
    let _ = say(host, id(-129.0));
    let _ = say(host, id(65536.5));
    let _ = say(host, id(-32769.0));
    let _ = say(host, id(2147483648.0));
    let _ = say(host, id(-2147483649.0));
    let _ = say(host, id(4294967296.0));
    let _ = say(host, id(4294967297.0));
    let _ = say(host, id(9007199254740993.0));
    let _ = say(host, id(9223372036854775808.0));
    let _ = say(host, id(-9223372036854775808.0));
    let _ = say(host, id(-9223372036854777856.0));
    let _ = say(host, id(18446744073709549568.0));
    let _ = say(host, id(18446744073709551616.0));
    let _ = say(host, id(36893488147419103232.0));
    let _ = say(host, id(-36893488147419103232.0));
    let _ = say(host, id(1e20));
    let _ = say(host, id(-1e20));
    let _ = say(host, id(1e300));
    let _ = say(host, nan());
    let _ = say(host, inf());
    let _ = say(host, -inf());
    let _ = say32(host, id32(3000000000.0));
    let _ = say32(host, id32(-2.5));
    let _ = say32(host, id32(16777217.0));
    let _ = say32(host, id32(1e30));
    .Ok(())
}
"#,
        "0 0 0 0 0 0 0 0\n\
         0 0 0 0 0 0 0 0\n\
         2 2 2 2 2 2 2 2\n\
         -2 -2 -2 -2 254 65534 4294967294 18446744073709551614\n\
         -1 -1 -1 -1 255 65535 4294967295 18446744073709551615\n\
         127 127 127 127 127 127 127 127\n\
         -128 128 128 128 128 128 128 128\n\
         -1 255 255 255 255 255 255 255\n\
         0 256 256 256 0 256 256 256\n\
         127 -129 -129 -129 127 65407 4294967167 18446744073709551487\n\
         0 0 65536 65536 0 0 65536 65536\n\
         -1 32767 -32769 -32769 255 32767 4294934527 18446744073709518847\n\
         0 0 -2147483648 2147483648 0 0 2147483648 2147483648\n\
         -1 -1 2147483647 -2147483649 255 65535 2147483647 18446744071562067967\n\
         0 0 0 4294967296 0 0 0 4294967296\n\
         1 1 1 4294967297 1 1 1 4294967297\n\
         0 0 0 9007199254740992 0 0 0 9007199254740992\n\
         0 0 0 -9223372036854775808 0 0 0 9223372036854775808\n\
         0 0 0 -9223372036854775808 0 0 0 9223372036854775808\n\
         0 -2048 -2048 9223372036854773760 0 63488 4294965248 9223372036854773760\n\
         0 -2048 -2048 -2048 0 63488 4294965248 18446744073709549568\n\
         0 0 0 0 0 0 0 0\n\
         0 0 0 0 0 0 0 0\n\
         0 0 0 0 0 0 0 0\n\
         0 0 1661992960 7766279631452241920 0 0 1661992960 7766279631452241920\n\
         0 0 -1661992960 -7766279631452241920 0 0 2632974336 10680464442257309696\n\
         0 0 0 0 0 0 0 0\n\
         0 0 0 0 0 0 0 0\n\
         0 0 0 0 0 0 0 0\n\
         0 0 0 0 0 0 0 0\n\
         0 24064 -1294967296 3000000000 0 24064 3000000000 3000000000\n\
         -2 -2 -2 -2 254 65534 4294967294 18446744073709551614\n\
         0 0 16777216 16777216 0 0 16777216 16777216\n\
         0 0 0 0 0 0 0 0\n",
    );
}

/// The checked family from a float: `.Ok` exactly where the value is whole and
/// in range, `.Err` naming the value and the target everywhere else, `NaN` and
/// the infinities included.
#[test]
fn to_from_a_float_is_checked() {
    rows_or_skip!();
    agree(
        "checked to from a float",
        r#"
from "core/io" import * as io;
from "native" import { NativeHost };

fn id(x: Float): Float { x }
fn nan(): Float { id(0.0) / id(0.0) }
fn inf(): Float { id(1.0) / id(0.0) }

fn say(host: NativeHost, x: Float): () {
    io.println(host.stdout, "${x.toI8().isOk()} ${x.toU8().isOk()} ${x.toI64().isOk()} ${x.toU64().isOk()} ${x.toI8()}").ignore()
}

export fn main(host: NativeHost): Result<(), Str> {
    let _ = say(host, id(127.0));
    let _ = say(host, id(128.0));
    let _ = say(host, id(-128.0));
    let _ = say(host, id(-129.0));
    let _ = say(host, id(2.5));
    let _ = say(host, id(-0.0));
    let _ = say(host, id(-1.0));
    let _ = say(host, id(9223372036854775808.0));
    let _ = say(host, id(-9223372036854775808.0));
    let _ = say(host, id(18446744073709549568.0));
    let _ = say(host, id(18446744073709551616.0));
    let _ = say(host, nan());
    let _ = say(host, inf());
    let _ = say(host, -inf());
    .Ok(())
}
"#,
        "true true true true .Ok(127)\n\
         false true true true .Err(RangeError { value: \"128.0\", target: \"I8\" })\n\
         true false true false .Ok(-128)\n\
         false false true false .Err(RangeError { value: \"-129.0\", target: \"I8\" })\n\
         false false false false .Err(RangeError { value: \"2.5\", target: \"I8\" })\n\
         true true true true .Ok(0)\n\
         true false true false .Ok(-1)\n\
         false false false true .Err(RangeError { value: \"9223372036854776000.0\", target: \"I8\" })\n\
         false false true false .Err(RangeError { value: \"-9223372036854776000.0\", target: \"I8\" })\n\
         false false false true .Err(RangeError { value: \"18446744073709550000.0\", target: \"I8\" })\n\
         false false false false .Err(RangeError { value: \"18446744073709552000.0\", target: \"I8\" })\n\
         false false false false .Err(RangeError { value: \"NaN\", target: \"I8\" })\n\
         false false false false .Err(RangeError { value: \"inf\", target: \"I8\" })\n\
         false false false false .Err(RangeError { value: \"-inf\", target: \"I8\" })\n",
    );
}
