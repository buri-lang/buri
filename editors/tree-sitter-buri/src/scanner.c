// The two things tree-sitter's lexer cannot express on its own.
//
// **Templates.** `"a ${b} c"` needs the lexer to know whether a `}` closes a
// hole or resumes string text. The compiler's own lexer keeps a stack of open
// interpolations for this (`cli/src/lex.rs`). This scanner needs no stack: the
// parser tells it which tokens are valid at this point, and "a template span
// is valid here" is exactly the state a stack would be tracking. So the whole
// mode question is answered by `valid_symbols`, and there is nothing to
// serialize between runs.
//
// **Nestable block comments.** `/* /* */ */` is one comment. A regular
// expression cannot count.

#include "tree_sitter/parser.h"

enum TokenType {
  STRING_LITERAL,
  TEMPLATE_HEAD,
  TEMPLATE_SPAN,
  TEMPLATE_TAIL,
  BLOCK_COMMENT,
};

static void advance(TSLexer *lexer) { lexer->advance(lexer, false); }
static void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

// Consumes string text up to an unescaped `"` or an interpolation `${`.
//
// Returns 1 if it stopped at `${` (having consumed it), 0 if it stopped at the
// closing quote (having consumed it), and -1 if the string never ended.
static int scan_body(TSLexer *lexer) {
  for (;;) {
    if (lexer->eof(lexer)) return -1;
    // A newline inside a string is the file having gone wrong somewhere
    // earlier; stopping here keeps the error local instead of swallowing the
    // rest of the file into one token.
    if (lexer->lookahead == '\n') return -1;
    if (lexer->lookahead == '\\') {
      advance(lexer);
      if (lexer->eof(lexer)) return -1;
      advance(lexer);
      continue;
    }
    if (lexer->lookahead == '"') {
      advance(lexer);
      return 0;
    }
    if (lexer->lookahead == '$') {
      advance(lexer);
      if (lexer->lookahead == '{') {
        advance(lexer);
        return 1;
      }
      continue;
    }
    advance(lexer);
  }
}

static bool scan_block_comment(TSLexer *lexer) {
  // `/` is already consumed by the caller.
  if (lexer->lookahead != '*') return false;
  advance(lexer);
  unsigned depth = 1;
  for (;;) {
    if (lexer->eof(lexer)) return false;
    if (lexer->lookahead == '/') {
      advance(lexer);
      if (lexer->lookahead == '*') {
        advance(lexer);
        depth++;
      }
      continue;
    }
    if (lexer->lookahead == '*') {
      advance(lexer);
      if (lexer->lookahead == '/') {
        advance(lexer);
        if (--depth == 0) {
          lexer->result_symbol = BLOCK_COMMENT;
          return true;
        }
      }
      continue;
    }
    advance(lexer);
  }
}

bool tree_sitter_buri_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  (void)payload;

  // A `}` that resumes template text. Only reachable where the parser is
  // inside a hole, which is what makes this unambiguous with a closing brace.
  if (valid_symbols[TEMPLATE_SPAN] || valid_symbols[TEMPLATE_TAIL]) {
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
           lexer->lookahead == '\n' || lexer->lookahead == '\r') {
      skip(lexer);
    }
    if (lexer->lookahead == '}') {
      advance(lexer);
      int r = scan_body(lexer);
      if (r < 0) return false;
      lexer->result_symbol = r == 1 ? TEMPLATE_SPAN : TEMPLATE_TAIL;
      return true;
    }
    // Not a template continuation, so fall through: the same position may
    // still be a string or a comment.
  }

  if (valid_symbols[STRING_LITERAL] || valid_symbols[TEMPLATE_HEAD] ||
      valid_symbols[BLOCK_COMMENT]) {
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
           lexer->lookahead == '\n' || lexer->lookahead == '\r') {
      skip(lexer);
    }

    if ((valid_symbols[STRING_LITERAL] || valid_symbols[TEMPLATE_HEAD]) &&
        lexer->lookahead == '"') {
      advance(lexer);
      int r = scan_body(lexer);
      if (r < 0) return false;
      // Which token this was is decided by how it ended, not by looking ahead:
      // a string that reached its closing quote is a string, and one that
      // reached `${` is the head of a template.
      if (r == 1) {
        if (!valid_symbols[TEMPLATE_HEAD]) return false;
        lexer->result_symbol = TEMPLATE_HEAD;
      } else {
        if (!valid_symbols[STRING_LITERAL]) return false;
        lexer->result_symbol = STRING_LITERAL;
      }
      return true;
    }

    if (valid_symbols[BLOCK_COMMENT] && lexer->lookahead == '/') {
      advance(lexer);
      return scan_block_comment(lexer);
    }
  }

  return false;
}

// No state between runs, so these are the trivial versions.
unsigned tree_sitter_buri_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_buri_external_scanner_deserialize(void *payload, const char *buffer,
                                                   unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

void *tree_sitter_buri_external_scanner_create(void) { return NULL; }
void tree_sitter_buri_external_scanner_destroy(void *payload) { (void)payload; }
