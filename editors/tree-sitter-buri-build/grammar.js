// The grammar for BUILD.buri and REPO.buri, which are textproto rather than
// Buri. Hand-written, and held to `cli/src/build/textproto.rs` by `check.sh`.
//
// That reader is the normative one, and it is small enough to mirror rule for
// rule: a file is a list of fields, a field is `name: value` or `name { ... }`,
// values are strings, integers, bare words, lists and messages, separators are
// optional everywhere, and `#` starts a comment.

module.exports = grammar({
  name: 'buri_build',

  // The reader's `skip_trivia`: spaces, tabs, carriage returns, newlines, and
  // comments, anywhere at all.
  extras: $ => [/[ \t\r\n]+/, $.comment],

  rules: {
    document: $ => repeat($._field),

    // "A message field takes no colon; a scalar does" — textproto.rs. The
    // trailing comma is optional because the reader steps over one if it is
    // there and does not miss it if it is not.
    _field: $ => choice($.block, $.field),

    block: $ => seq(
      field('name', $.identifier),
      field('body', $.message),
      optional(','),
    ),

    field: $ => seq(
      field('name', $.identifier),
      ':',
      field('value', $._value),
      optional(','),
    ),

    message: $ => seq('{', repeat($._field), '}'),

    list: $ => seq('[', repeat(seq($._value, optional(','))), ']'),

    _value: $ => choice(
      $.string,
      $.number,
      // A bare word is an enum constant or a bool. The reader spells both the
      // same way, so the tree does too, and it is named for the common case.
      alias($.identifier, $.constant),
      $.message,
      $.list,
    ),

    // One token, so that no comment or newline can be taken for part of a
    // string. An escape names a character rather than a byte, and the reader
    // lets it name a newline.
    string: $ => token(seq(
      '"',
      repeat(choice(/[^"\\\n]/, seq('\\', /[\s\S]/))),
      '"',
    )),

    number: $ => /-?[0-9]+/,

    identifier: $ => /[A-Za-z_][A-Za-z0-9_]*/,

    comment: $ => token(seq('#', /[^\n]*/)),
  },
});
