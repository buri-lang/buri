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

/// The keyed functions of `core/crypto` take a `Secret<[U8]>`, and answer what
/// they answered for the same key bytes: RFC 4231 test case 2 for both HMACs,
/// and RFC 8439 section 2.8.2 for `seal` and `open`.
#[test]
fn a_secret_key_gives_the_known_answers() {
    rows_or_skip!();
    agree(
        "secret crypto",
        r#"
from "native" import { NativeHost };
from "core/bytes" import * as bytes;
from "core/crypto" import * as crypto;
from "core/io" import * as io;
from "core/secret" import * as secret;
from "platform/effect" import { Allocator, Entropy, Stdout };

struct FixedNonce([U8]);

impl Entropy for FixedNonce {
    fn bytes(self, count: Int): [U8] {
        self.0
    }
}

fn hex<C: Allocator>(ctx: C, text: Str): [U8] {
    bytes.fromHex(ctx, text).withDefault([])
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdout: host.stdout };
    let jefe = secret.of("Jefe").mapCtx(ctx, fn(c, text) => bytes.toUtf8(c, text));
    let question = bytes.toUtf8(ctx, "what do ya want for nothing?");
    let _ = io.println(ctx, "${crypto.hmacSha256(ctx, jefe, question).toHex(ctx)}").ignore();
    let _ = io.println(ctx, "${crypto.hmacSha512(ctx, jefe, question).toHex(ctx)}").ignore();

    let key = secret.of(hex(ctx, "808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f"));
    let aad = hex(ctx, "50515253c0c1c2c3c4c5c6c7");
    let plaintext = bytes.toUtf8(
        ctx,
        "Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.",
    );
    let fixed = context { Allocator: host.alloc, Entropy: FixedNonce(hex(ctx, "070000004041424344454647")) };
    let sealed = crypto.seal(fixed, key, plaintext, aad);
    let _ = io.println(ctx, "${bytes.toHex(ctx, sealed)}").ignore();
    let opened = crypto.open(ctx, key, sealed, aad)?;
    let _ = io.println(ctx, "${bytes.fromUtf8Lossy(ctx, opened)}").ignore();
    .Ok(())
}
"#,
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843\n\
         164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737\n\
         070000004041424344454647\
         d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca967\
         1282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b\
         58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b6116\
         1ae10b594f09e26a7e902ecbd0600691\n\
         Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the \
         future, sunscreen would be it.\n",
    );
}

/// `crypto.token` answers a `Secret<Str>`: it shows as `***`, and `reveal`
/// gives the hex.
#[test]
fn a_token_is_a_secret() {
    rows_or_skip!();
    agree(
        "secret token",
        r#"
from "native" import { NativeHost };
from "core/crypto" import * as crypto;
from "core/io" import * as io;
from "platform/effect" import { Allocator, Entropy, Stdout };

struct Fixed([U8]);

impl Entropy for Fixed {
    fn bytes(self, count: Int): [U8] {
        self.0
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Entropy: Fixed([0, 1, 171, 255]), Stdout: host.stdout };
    let t = crypto.token(ctx, 4);
    let _ = io.println(ctx, "${t.show(ctx)}").ignore();
    let _ = io.println(ctx, "Bearer ${t.reveal()}").ignore();
    .Ok(())
}
"#,
        "***\n\
         Bearer 0001abff\n",
    );
}

/// The example in `core/secret`'s proposal (buri-lang/buri#226), whole: a key
/// read from the environment goes to `hmacSha256` without being revealed, a
/// derived `Show` masks the one in `Config`, and `reveal` is the way out.
#[test]
fn the_proposal_example_runs() {
    rows_or_skip!();
    agree(
        "secret proposal",
        r#"
from "native" import { NativeHost };
from "core/bytes" import * as bytes;
from "core/crypto" import * as crypto;
from "core/crypto" import { Digest };
from "core/env" import * as env;
from "core/io" import * as io;
from "core/secret" import { Secret };
from "core/str" import * as str;
from "platform/effect" import { Allocator, Environment, Stdout };

/// An environment holding exactly two variables.
struct Fixed {}

impl Environment for Fixed {
    fn variable(self, name: Str): Option<Str> {
        if (name == "API_KEY") {
            .Some("s3cr3t")
        } else if (name == "SIGNING_KEY") {
            .Some("key")
        } else {
            .None
        }
    }

    fn arguments(self): [Str] {
        []
    }

    fn currentDirectory(self): Str {
        "/"
    }

    fn allVariables(self): [(Str, Str)] {
        []
    }

    fn operatingSystemName(self): Str {
        "fixed"
    }
}

derive Show for Config;
struct Config {
    region: Str,
    apiKey: Secret<Str>,
}

fn load<C: Environment>(ctx: C): Option<Config> {
    .Some(Config { region: "eu-west-1", apiKey: env.get(ctx, "API_KEY")? })
}

fn authorization<C: Allocator>(ctx: C, config: Config): Str {
    str.format(ctx, "Bearer ${config.apiKey.reveal()}")
}

fn sign<C: Allocator + Environment>(ctx: C, body: [U8]): Option<Digest> {
    let key = env.get(ctx, "SIGNING_KEY")?.mapCtx(ctx, fn(c, text) => bytes.toUtf8(c, text));
    let mac = crypto.hmacSha256(ctx, key, body);
    .Some(mac)
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Environment: Fixed {}, Stdout: host.stdout };
    let config = load(ctx).okOr("API_KEY is not set")?;
    let _ = io.println(ctx, "${config.show(ctx)}").ignore();
    let _ = io.println(ctx, "${authorization(ctx, config)}").ignore();
    let body = bytes.toUtf8(ctx, "The quick brown fox jumps over the lazy dog");
    let mac = match (sign(ctx, body)) {
        .Some(digest) => digest.toHex(ctx),
        .None => "unsigned",
    };
    let _ = io.println(ctx, "${mac}").ignore();
    .Ok(())
}
"#,
        "Config { region: \"eu-west-1\", apiKey: *** }\n\
         Bearer s3cr3t\n\
         f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8\n",
    );
}
