//! What the build schema says about a field, read out of the schema itself.
//!
//! `BUILD.buri` and `REPO.buri` parse as two messages whose normative
//! definitions are `docs/reference/schema/build.proto` and `docs/reference/schema/repo.proto`, and
//! every field in them is documented there — in prose written for the person
//! editing the file. That prose had no way of reaching an editor: `textproto`
//! knows a field's name and `buildfile` knows its meaning in code, and neither
//! holds the sentence explaining it.
//!
//! So the two schemas are read once, as text. They are already compiled into
//! the binary for `buri docs`, which is what makes this a parse rather than a
//! file read, and one reader means a hover and a completion item cannot
//! disagree about what a field is for: both take their prose from here.
//!
//! The reader is deliberately shallow. It is not a `.proto` compiler — it wants
//! the comment above a declaration, the declaration's own line, and the nesting
//! it is in, and it recognizes exactly the constructs those two files use.

use crate::documentation::topics::{BUILD_PROTO, REPO_PROTO};
use std::collections::HashMap;

/// One field of one message.
pub struct Field {
    pub name: String,
    /// The type as the schema writes it: `string`, `Platform`, `TestSuite`.
    pub type_name: String,
    /// The declaration as it stands in the schema, without the field number —
    /// `repeated string sources`. This is what a hover shows.
    pub signature: String,
    pub repeated: bool,
    pub docs: Vec<String>,
}

/// A `message` or an `enum` block, by the name it declares.
pub struct Block {
    /// `Library`, or `Tag.Forbids` for a nested one.
    pub name: String,
    pub docs: Vec<String>,
    pub fields: Vec<Field>,
    /// The constants, for an `enum`. A message has none.
    pub constants: Vec<Constant>,
}

pub struct Constant {
    pub name: String,
    /// `Platform.LINUX = 1`, which is the whole of what an enum constant
    /// declares.
    pub signature: String,
    pub docs: Vec<String>,
}

/// Both schemas, read once per process.
pub struct Schema {
    blocks: Vec<Block>,
    /// The messages a textproto block of this name holds — `library` ->
    /// `Library`, `forbids` -> `Tag.Forbids`. Keyed by the field name a build
    /// file writes, which is how `textproto::schema_order` names them too, and
    /// a list because the empty name is two messages: a `BUILD.buri` is a
    /// `BuildFile` and a `REPO.buri` is a `RepoConfig`.
    by_field: HashMap<String, Vec<usize>>,
}

/// The two schemas, parsed on first use and kept.
pub fn schema() -> &'static Schema {
    static SCHEMA: std::sync::OnceLock<Schema> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| {
        let mut blocks = Vec::new();
        read(BUILD_PROTO, &mut blocks);
        read(REPO_PROTO, &mut blocks);
        let by_field = message_blocks(&blocks);
        Schema { blocks, by_field }
    })
}

impl Schema {
    /// The message a build file's block of this name is written against.
    pub fn block(&self, name: &str) -> Option<&Block> {
        self.blocks_named(name).next()
    }

    /// Every field a block of this name may hold. The empty name is the file
    /// itself, and its fields are the two roots' together — no file has both,
    /// which is the same union `textproto::schema_order` keeps.
    pub fn fields(&self, block: &str) -> Vec<&Field> {
        self.blocks_named(block).flat_map(|b| b.fields.iter()).collect()
    }

    pub fn field(&self, block: &str, name: &str) -> Option<&Field> {
        self.fields(block).into_iter().find(|f| f.name == name)
    }

    /// The enum a field's values are drawn from, for the fields whose type is
    /// one. A `string` or a `uint32` field has none.
    pub fn enumeration(&self, block: &str, field: &str) -> Option<&Block> {
        let ty = &self.field(block, field)?.type_name;
        self.blocks.iter().find(|b| !b.constants.is_empty() && ends_with_name(&b.name, ty))
    }

    /// Whether a field of this name in this block holds `true` or `false`.
    pub fn is_boolean(&self, block: &str, field: &str) -> bool {
        self.field(block, field).is_some_and(|f| f.type_name == "bool")
    }

    fn blocks_named(&self, name: &str) -> impl Iterator<Item = &Block> {
        self.by_field
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|i| self.blocks.get(*i))
    }
}

/// The block each textproto field name opens, found by walking down from the
/// two root messages.
///
/// A build file names its blocks by *field*, and the schema names them by
/// type, so `outputs { platform: JS }` is an `Output` reached through
/// `Binary.outputs`. Walking the fields is what maps one onto the other, and it
/// is why nothing here holds a hand-written table of the pairing.
fn message_blocks(blocks: &[Block]) -> HashMap<String, Vec<usize>> {
    let mut out: HashMap<String, Vec<usize>> = HashMap::new();
    let mut queue: Vec<(String, usize)> = Vec::new();
    for root in ["BuildFile", "RepoConfig"] {
        if let Some(i) = blocks.iter().position(|b| b.name == root) {
            queue.push((String::new(), i));
        }
    }
    while let Some((name, index)) = queue.pop() {
        let found = out.entry(name).or_default();
        if found.contains(&index) {
            continue;
        }
        found.push(index);
        let Some(block) = blocks.get(index) else { continue };
        for field in &block.fields {
            let Some(i) = blocks
                .iter()
                .position(|b| b.constants.is_empty() && ends_with_name(&b.name, &field.type_name))
            else {
                continue;
            };
            queue.push((field.name.clone(), i));
        }
    }
    out
}

/// Whether a qualified block name names this type: `Tag.Forbids` is `Forbids`
/// written from outside, and `Lint` is itself.
fn ends_with_name(qualified: &str, name: &str) -> bool {
    qualified == name || qualified.ends_with(&format!(".{name}"))
}

/// The block's own name, without what it is nested in.
fn last_segment(qualified: &str) -> &str {
    qualified.rsplit('.').next().unwrap_or(qualified)
}

/// Reads one schema into blocks.
///
/// A line at a time, with the comment lines above a declaration collected as
/// they go. That is the whole grammar these two files use: a `message` or an
/// `enum` opens a block, a `}` closes it, and everything else inside one is a
/// field or a constant.
fn read(text: &str, out: &mut Vec<Block>) {
    let mut docs: Vec<String> = Vec::new();
    // The blocks currently open, innermost last, as indices into `out`.
    let mut open: Vec<usize> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            docs.clear();
            continue;
        }
        if let Some(comment) = line.strip_prefix("//") {
            docs.push(comment.strip_prefix(' ').unwrap_or(comment).to_string());
            continue;
        }
        if line == "}" {
            open.pop();
            docs.clear();
            continue;
        }
        if let Some(name) = opens_block(line) {
            let qualified = match open.last().and_then(|i| out.get(*i)) {
                Some(outer) => format!("{}.{name}", outer.name),
                None => name.to_string(),
            };
            out.push(Block {
                name: qualified,
                docs: std::mem::take(&mut docs),
                fields: Vec::new(),
                constants: Vec::new(),
            });
            open.push(out.len().saturating_sub(1));
            continue;
        }
        let Some(index) = open.last().copied() else {
            docs.clear();
            continue;
        };
        let declaration = line.split(';').next().unwrap_or(line).trim().to_string();
        let taken = std::mem::take(&mut docs);
        let Some(block) = out.get_mut(index) else { continue };
        // A constant when the block is an enum, a field when it is a message.
        // The two are told apart by the declaration itself: `NAME = 1` has no
        // type in front of the name.
        let owner = last_segment(&block.name).to_string();
        match field_of(&declaration, taken) {
            Ok(field) => block.fields.push(field),
            Err(taken) => {
                if let Some(constant) = constant_of(&declaration, &owner, taken) {
                    block.constants.push(constant);
                }
            }
        }
    }
}

/// The name a `message X {` or an `enum X {` opens.
fn opens_block(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("message ").or_else(|| line.strip_prefix("enum "))?;
    rest.split_whitespace().next()
}

/// A field declaration: an optional `repeated`, a type, a name, and a number.
/// The prose comes back on a line that is not one, so the caller can try it as
/// a constant instead.
fn field_of(declaration: &str, docs: Vec<String>) -> Result<Field, Vec<String>> {
    let words: Vec<&str> = declaration.split_whitespace().collect();
    let (repeated, rest) = match words.split_first() {
        Some((&"repeated", rest)) => (true, rest),
        _ => (false, &words[..]),
    };
    let [type_name, name, "=", _number] = rest else { return Err(docs) };
    Ok(Field {
        name: (*name).to_string(),
        type_name: (*type_name).to_string(),
        signature: if repeated {
            format!("repeated {type_name} {name}")
        } else {
            format!("{type_name} {name}")
        },
        repeated,
        docs,
    })
}

/// An enum constant: a name and a number, with a trailing comment kept as its
/// prose — `MODULE_UNSPECIFIED = 0;  // ESM` is where that default is written
/// down.
fn constant_of(declaration: &str, owner: &str, mut docs: Vec<String>) -> Option<Constant> {
    let declaration = match declaration.split_once("//") {
        Some((before, after)) => {
            docs.push(after.trim().to_string());
            before.trim()
        }
        None => declaration,
    };
    let words: Vec<&str> = declaration.split_whitespace().collect();
    let [name, "=", _number] = words[..] else { return None };
    if !name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
        return None;
    }
    Some(Constant {
        name: name.to_string(),
        signature: format!("{owner}.{declaration}"),
        docs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_a_build_file_writes_is_found() {
        let schema = schema();
        for name in ["library", "binary", "test", "testing", "outputs", "js", "tag", "lint"] {
            assert!(schema.block(name).is_some(), "no message for `{name}`");
        }
        // Nested messages, reached through the field that holds them.
        assert_eq!(schema.block("forbids").map(|b| b.name.as_str()), Some("Tag.Forbids"));
        assert_eq!(schema.block("requires").map(|b| b.name.as_str()), Some("Tag.Requires"));
    }

    /// The schemas and `textproto::schema_order` name the same fields. A field
    /// in one and not the other is a schema the formatter or the editor does
    /// not know about.
    #[test]
    fn the_field_lists_agree_with_the_formatter() {
        let schema = schema();
        for block in ["", "library", "binary", "test", "testing", "outputs", "js", "tag", "lint"] {
            let mut ordered: Vec<&str> = crate::build::textproto::schema_order(block).to_vec();
            let mut declared: Vec<&str> =
                schema.fields(block).iter().map(|f| f.name.as_str()).collect();
            ordered.sort_unstable();
            declared.sort_unstable();
            assert_eq!(ordered, declared, "`{block}`");
        }
    }

    #[test]
    fn a_field_carries_the_prose_written_above_it() {
        let field = schema().field("library", "sources").expect("library.sources");
        assert_eq!(field.signature, "repeated string sources");
        assert!(field.repeated);
        assert!(field.docs[0].starts_with("Every .buri file"), "{:?}", field.docs);
    }

    #[test]
    fn a_platform_field_names_the_enum_its_values_come_from() {
        let platform = schema().enumeration("library", "platforms").expect("Platform");
        let names: Vec<&str> = platform.constants.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["PLATFORM_UNSPECIFIED", "LINUX", "MACOS", "JS", "WEB"]);
        let web = platform.constants.iter().find(|c| c.name == "WEB").expect("WEB");
        assert_eq!(web.signature, "Platform.WEB = 4");
        assert!(web.docs[0].starts_with("A page in a browser"), "{:?}", web.docs);
    }
}
