//! `core/secret`, on every backend.
//!
//! Each program runs through JavaScript and every native backend this binary
//! was built with, under the heap check: [`crate::agreement`]'s `agree`, so the
//! answer is pinned as well as compared.
use crate::agreement::{agree, skip_reason};

macro_rules! rows_or_skip {
    () => {
        if let Some(why) = skip_reason() {
            crate::ci::skipped("backend agreement", &why);
            return;
        }
    };
}

/// A `Secret` shows as `***`, alone and inside every derived `Show` that holds
/// one: a struct, an enum's tuple and record variants, an `Option` and a list.
#[test]
fn a_secret_shows_as_stars_wherever_it_is_held() {
    rows_or_skip!();
    agree(
        "secret show",
        r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/secret" import * as secret;
from "core/secret" import { Secret };
from "platform/effect" import { Allocator, Stdout };

derive Show for Login;
struct Login {
    user: Str,
    password: Secret<Str>,
}

derive Show for Credential;
enum Credential {
    Anonymous,
    Token(Secret<Str>),
    Basic { user: Str, password: Secret<Str> },
}

derive Show for Vault;
struct Vault {
    maybe: Option<Secret<Str>>,
    keys: [Secret<[U8]>],
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let s = secret.of("hunter2");
    let _ = io.println(ctx, "${s.show(ctx)}").ignore();
    let _ = io.println(ctx, "${secret.of(42).show(ctx)}").ignore();
    let login = Login { user: "ada", password: s };
    let _ = io.println(ctx, "${login.show(ctx)}").ignore();
    let token = Credential.Token(s);
    let _ = io.println(ctx, "${token.show(ctx)}").ignore();
    let basic = Credential.Basic { user: "ada", password: s };
    let _ = io.println(ctx, "${basic.show(ctx)}").ignore();
    let full = Vault { maybe: .Some(s), keys: [secret.of([1, 2]), secret.of([])] };
    let _ = io.println(ctx, "${full.show(ctx)}").ignore();
    let empty = Vault { maybe: .None, keys: [] };
    let _ = io.println(ctx, "${empty.show(ctx)}").ignore();
    .Ok(())
}
"#,
        "***\n\
         ***\n\
         Login { user: \"ada\", password: *** }\n\
         .Token(***)\n\
         .Basic { user: \"ada\", password: *** }\n\
         Vault { maybe: .Some(***), keys: [***, ***] }\n\
         Vault { maybe: .None, keys: [] }\n",
    );
}

/// `reveal` answers the value, and `map` and `mapCtx` change it while it stays
/// wrapped: the result still shows as `***` until it is revealed.
#[test]
fn a_secret_is_revealed_and_mapped() {
    rows_or_skip!();
    agree(
        "secret reveal and map",
        r#"
from "native" import { NativeHost };
from "core/bytes" import * as bytes;
from "core/io" import * as io;
from "core/secret" import * as secret;
from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let s = secret.of("hunter2");
    let _ = io.println(ctx, "${s.reveal()}").ignore();
    let longer = s.map(fn(text) => text.length() + 1);
    let _ = io.println(ctx, "${longer.show(ctx)}").ignore();
    let _ = io.println(ctx, "${longer.reveal()}").ignore();
    let raw = s.mapCtx(ctx, fn(c, text) => bytes.toUtf8(c, text));
    let _ = io.println(ctx, "${raw.show(ctx)}").ignore();
    let _ = io.println(ctx, "${bytes.toHex(ctx, raw.reveal())}").ignore();
    .Ok(())
}
"#,
        "hunter2\n***\n8\n***\n68756e74657232\n",
    );
}
