//! Unicode answers, on every backend.
//!
//! JavaScript asks the engine for some of these and the native backends read
//! `core/*`'s tables or Rust's `std` for the same answer, so each program
//! below prints what one Unicode version says and [`agree`] pins it on
//! JavaScript and on every native backend this binary was built with. Every
//! row holds characters Unicode 17.0 added or changed, so a backend whose
//! data is a version behind prints something else.
use crate::agreement::{agree, skip_reason};

macro_rules! rows_or_skip {
    () => {
        if let Some(why) = skip_reason() {
            crate::ci::skipped("backend agreement", &why);
            return;
        }
    };
}

/// Every `Char` question, one line per character: a code point, then what the
/// eight predicates say, then the case mappings as code points.
///
/// U+A7CE and U+A7CF, U+A7D2 and U+A7D3, and U+16EA0 and U+16EBB are case
/// pairs 17.0 added, U+1E6C0 is a Tai Yo letter, U+10ED0 a punctuation mark,
/// U+1ACF a combining mark and U+1FAC8 an emoji, all new in 17.0. U+0295 is a
/// letter that 17.0 moved from `Ll` to `Lo`, U+01C5 is title case, and U+0378
/// is still unassigned.
#[test]
fn every_char_question_agrees_at_unicode_17() {
    rows_or_skip!();
    agree(
        "char questions",
        r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(host: NativeHost): Result<(), Str> {
  let chars = [
    'A', 'z', '\u{df}', '\u{1c5}', '\u{295}', '\u{a7ce}', '\u{a7cf}', '\u{a7d2}',
    '\u{a7d3}', '\u{16ea0}', '\u{16ebb}', '\u{1e6c0}', '\u{10ed0}', '\u{1acf}',
    '\u{1fac8}', '!', '+', ' ', '\u{a0}', '\u{200d}', '\u{378}',
  ];
  let lines = chars.mapCtx(host.alloc, fn(a, c) => str.format(
    a,
    "${c.toU32()} alpha=${c.isAlpha()} upper=${c.isUpper()} lower=${c.isLower()} punct=${c.isPunctuation()} print=${c.isPrintable()} space=${c.isSpace()} up=${c.toUpper().toU32()} low=${c.toLower().toU32()}",
  ));
  let _ = io.println(host.stdout, lines.join(host.alloc, "\n")).ignore();
  .Ok(())
}
"#,
        "65 alpha=true upper=true lower=false punct=false print=true space=false up=65 low=97
122 alpha=true upper=false lower=true punct=false print=true space=false up=90 low=122
223 alpha=true upper=false lower=true punct=false print=true space=false up=83 low=223
453 alpha=true upper=false lower=false punct=false print=true space=false up=452 low=454
661 alpha=true upper=false lower=false punct=false print=true space=false up=661 low=661
42958 alpha=true upper=true lower=false punct=false print=true space=false up=42958 low=42959
42959 alpha=true upper=false lower=true punct=false print=true space=false up=42958 low=42959
42962 alpha=true upper=true lower=false punct=false print=true space=false up=42962 low=42963
42963 alpha=true upper=false lower=true punct=false print=true space=false up=42962 low=42963
93856 alpha=true upper=true lower=false punct=false print=true space=false up=93856 low=93883
93883 alpha=true upper=false lower=true punct=false print=true space=false up=93856 low=93883
124608 alpha=true upper=false lower=false punct=false print=true space=false up=124608 low=124608
69328 alpha=false upper=false lower=false punct=true print=true space=false up=69328 low=69328
6863 alpha=false upper=false lower=false punct=false print=true space=false up=6863 low=6863
129736 alpha=false upper=false lower=false punct=false print=true space=false up=129736 low=129736
33 alpha=false upper=false lower=false punct=true print=true space=false up=33 low=33
43 alpha=false upper=false lower=false punct=false print=true space=false up=43 low=43
32 alpha=false upper=false lower=false punct=false print=true space=true up=32 low=32
160 alpha=false upper=false lower=false punct=false print=false space=true up=160 low=160
8205 alpha=false upper=false lower=false punct=false print=false space=false up=8205 low=8205
888 alpha=false upper=false lower=false punct=false print=false space=false up=888 low=888
",
    );
}

/// `str.toUpper`, `str.toLower` and `str.caseFold` over 17.0's case pairs,
/// a title-case letter, and the two full mappings that change the length.
#[test]
fn string_case_agrees_at_unicode_17() {
    rows_or_skip!();
    agree(
        "string case",
        r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn codes(host: NativeHost, s: Str): Str {
  s.chars(host.alloc).mapCtx(host.alloc, fn(a, c) => c.toU32().show(a)).join(host.alloc, " ")
}

export fn main(host: NativeHost): Result<(), Str> {
  let upper = "\u{df}\u{a7cf}\u{a7d3}\u{16ebb}\u{1c5}".toUpper(host.alloc);
  let lower = "\u{a7ce}\u{a7d2}\u{16ea0}\u{1c5}\u{130}".toLower(host.alloc);
  let folded = "\u{a7ce}\u{df}\u{a7d2}\u{16ea0}K".caseFold(host.alloc);
  let _ = io.println(host.stdout, "upper ${codes(host, upper)}").ignore();
  let _ = io.println(host.stdout, "lower ${codes(host, lower)}").ignore();
  let _ = io.println(host.stdout, "fold ${codes(host, folded)}").ignore();
  .Ok(())
}
"#,
        "upper 83 83 42958 42962 93856 452
lower 42959 42963 93883 454 105 775
fold 42959 115 115 42963 93883 107
",
    );
}

/// The four normalization forms over a combining sequence, Hangul both ways,
/// a reordering, a compatibility ligature, a singleton, and two 17.0
/// characters: U+A7F1, whose compatibility decomposition is `S`, and U+1ACF,
/// a combining mark of class 230 that the class-220 U+0316 sorts ahead of.
#[test]
fn normalization_agrees_at_unicode_17() {
    rows_or_skip!();
    agree(
        "normalization",
        r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;
from "core/str" import { Normalization };

fn codes(host: NativeHost, s: Str): Str {
  s.chars(host.alloc).mapCtx(host.alloc, fn(a, c) => c.toU32().show(a)).join(host.alloc, " ")
}

fn forms(host: NativeHost, s: Str): Str {
  let nfc = codes(host, s.normalize(host.alloc, .Nfc));
  let nfd = codes(host, s.normalize(host.alloc, .Nfd));
  let nfkc = codes(host, s.normalize(host.alloc, .Nfkc));
  let nfkd = codes(host, s.normalize(host.alloc, .Nfkd));
  str.format(host.alloc, "${nfc} | ${nfd} | ${nfkc} | ${nfkd}")
}

export fn main(host: NativeHost): Result<(), Str> {
  let inputs = [
    "e\u{301}", "\u{e9}", "\u{1112}\u{1161}\u{11ab}", "\u{d55c}", "\u{1e0b}\u{323}",
    "\u{fb01}", "\u{212b}", "\u{a7f1}", "a\u{1acf}\u{316}", "plain",
  ];
  let lines = inputs.map(host.alloc, fn(s) => forms(host, s));
  let _ = io.println(host.stdout, lines.join(host.alloc, "\n")).ignore();
  .Ok(())
}
"#,
        "233 | 101 769 | 233 | 101 769
233 | 101 769 | 233 | 101 769
54620 | 4370 4449 4523 | 54620 | 4370 4449 4523
54620 | 4370 4449 4523 | 54620 | 4370 4449 4523
7693 775 | 100 803 775 | 7693 775 | 100 803 775
64257 | 64257 | 102 105 | 102 105
197 | 65 778 | 197 | 65 778
42993 | 42993 | 83 | 83
97 790 6863 | 97 790 6863 | 97 790 6863 | 97 790 6863
112 108 97 105 110 | 112 108 97 105 110 | 112 108 97 105 110 | 112 108 97 105 110
",
    );
}

/// `graphemes` and `graphemeCount`, as the count and each cluster's length in
/// scalars: a combining sequence, Hangul syllables spelled in jamo, an emoji
/// ZWJ family, regional indicators with one left over, `\r\n`, `\n\r`, a
/// Devanagari conjunct, a skin tone, and three 17.0 changes — a Myanmar
/// conjunct, which 17.0 joins through its virama, a combining mark 17.0
/// added, and a ZWJ sequence that starts with a 17.0 emoji.
#[test]
fn graphemes_agree_at_unicode_17() {
    rows_or_skip!();
    agree(
        "graphemes",
        r#"
from "native" import { NativeHost };
from "core/io" import * as io;
from "core/str" import * as str;

fn clusters(host: NativeHost, s: Str): Str {
  let sizes = s.graphemes(host.alloc).mapCtx(host.alloc, fn(a, g) => g.length().show(a));
  str.format(host.alloc, "${s.graphemeCount()}: ${sizes.join(host.alloc, " ")}")
}

export fn main(host: NativeHost): Result<(), Str> {
  let inputs = [
    "e\u{301}x",
    "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1161}",
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
    "\u{1f1fa}\u{1f1f8}\u{1f1ec}\u{1f1e7}\u{1f1eb}",
    "a\r\nb",
    "\n\r",
    "\u{915}\u{94d}\u{937}\u{93f}",
    "\u{1f44d}\u{1f3fd}!",
    "\u{1000}\u{1039}\u{1001}",
    "a\u{1acf}",
    "\u{1fac8}\u{200d}\u{1f525}",
    "",
  ];
  let lines = inputs.map(host.alloc, fn(s) => clusters(host, s));
  let _ = io.println(host.stdout, lines.join(host.alloc, "\n")).ignore();
  .Ok(())
}
"#,
        "2: 2 1
2: 3 2
1: 5
3: 2 2 1
3: 1 2 1
2: 1 1
1: 4
2: 2 1
1: 3
1: 2
1: 3
0: \n",
    );
}

/// `core/buri/ast` prints a string literal the way `buri format` does: a
/// joiner between two emoji stays itself and every other unprintable character
/// is an escape. U+1FAC8 is an emoji 17.0 assigned, so at 16.0 it and its
/// joiner were escapes.
#[test]
fn a_printed_literal_keeps_a_17_0_emoji_sequence() {
    rows_or_skip!();
    agree(
        "printed literal",
        r#"
from "native" import { NativeHost };
from "core/buri/ast" import * as ast;
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(host: NativeHost): Result<(), Str> {
  let source = "let a: Str = \"\u{1fac8}\u{200d}\u{1f525} \u{1f525}\u{200d}x \u{a0}\";\n";
  match (ast.parse(host.alloc, "a.buri", source)) {
    .Ok(tree) => {
      let _ = io.print(host.stdout, ast.print(host.alloc, tree).text).ignore();
      .Ok(())
    },
    .Err(errors) => .Err(errors.length().show(host.alloc)),
  }
}
"#,
        "let a: Str = \"\u{1fac8}\u{200d}\u{1f525} \u{1f525}\\u{200d}x \\u{a0}\";\n",
    );
}
