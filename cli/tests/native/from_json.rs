//! `derive FromJson` and `json.decode`, on every backend (buri-lang/buri#279).
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

/// The issue's program, verbatim.
#[test]
fn a_derived_from_json_struct_decodes() {
    rows_or_skip!();
    agree(
        "from json issue",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { DecodeError, FromJson, Json };
from "core/str" import * as str;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive FromJson, Show for Thing;
struct Thing {
    name: Str,
    count: Int,
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let doc = json.parse(ctx, "{\"name\":\"a\",\"count\":3}").withDefault(Json.Null);
    let thing: Result<Thing, DecodeError> = json.decode(ctx, doc);
    let _ = io.println(ctx, str.format(ctx, "${thing.show(ctx)}")).ignore();
    .Ok(())
}
"#,
        ".Ok(Thing { name: \"a\", count: 3 })\n",
    );
}

/// Every field type `FromJson` reads, in one struct: the numbers, `Str`,
/// `Bool`, `Char`, `()`, a tuple, `Option` present and absent, lists, nesting,
/// and an enum at each of its variant shapes.
#[test]
fn every_field_type_decodes() {
    rows_or_skip!();
    agree(
        "from json every field",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { DecodeError, FromJson, Json };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive FromJson, Show for Point;
struct Point {
    x: Int,
    y: Int,
}

derive FromJson, Show for Shape;
enum Shape {
    Empty,
    Circle(Float),
    Line(Point, Point),
    Rect { width: Int, height: Int },
}

derive FromJson, Show for Meters;
struct Meters(Int);

derive FromJson, Show for All;
struct All {
    int: Int,
    float: Float,
    text: Str,
    flag: Bool,
    letter: Char,
    small: I8,
    byte: U8,
    medium: I32,
    wide: U64,
    single: F32,
    present: Option<Int>,
    absent: Option<Str>,
    numbers: [Int],
    nested: [[Str]],
    point: Point,
    meters: Meters,
    pair: (Int, Str),
    unit: (),
    shapes: [Shape],
}

fn doc<C: Allocator>(ctx: C, text: Str): Json {
    json.parse(ctx, text).withDefault(Json.Null)
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let text = "{\"int\":-42,\"float\":2.5,\"text\":\"h\\u00e9 \\\"q\\\"\",\"flag\":true,\"letter\":\"z\",\"small\":-8,\"byte\":255,\"medium\":70000,\"wide\":7,\"single\":0.5,\"present\":3,\"absent\":null,\"numbers\":[1,2,3],\"nested\":[[\"a\"],[],[\"b\",\"c\"]],\"point\":{\"x\":1,\"y\":2},\"meters\":[5],\"pair\":[7,\"z\"],\"unit\":null,\"shapes\":[\"Empty\",{\"Circle\":[1.5]},{\"Line\":[{\"x\":0,\"y\":0},{\"x\":3,\"y\":4}]},{\"Rect\":{\"width\":3,\"height\":4}}]}";
    let all: Result<All, DecodeError> = json.decode(ctx, doc(ctx, text));
    let _ = io.println(ctx, "${all.show(ctx)}").ignore();
    let top: Result<[Shape], DecodeError> = json.decode(ctx, doc(ctx, "[\"Empty\"]"));
    let _ = io.println(ctx, "${top.show(ctx)}").ignore();
    let n: Result<Int, DecodeError> = json.decode(ctx, doc(ctx, "7"));
    let _ = io.println(ctx, "${n.show(ctx)}").ignore();
    let s: Result<Option<Str>, DecodeError> = json.decode(ctx, doc(ctx, "\"x\""));
    let _ = io.println(ctx, "${s.show(ctx)}").ignore();
    .Ok(())
}
"#,
        ".Ok(All { int: -42, float: 2.5, text: \"hé \\\"q\\\"\", flag: true, letter: 'z', \
         small: -8, byte: 255, medium: 70000, wide: 7, single: 0.5, present: .Some(3), \
         absent: .None, numbers: [1, 2, 3], nested: [[\"a\"], [], [\"b\", \"c\"]], \
         point: Point { x: 1, y: 2 }, meters: Meters(5), pair: (7, \"z\"), unit: (), \
         shapes: [.Empty, .Circle(1.5), .Line(Point { x: 0, y: 0 }, Point { x: 3, y: 4 }), \
         .Rect { width: 3, height: 4 }] })\n\
         .Ok([.Empty])\n\
         .Ok(7)\n\
         .Ok(.Some(\"x\"))\n",
    );
}

/// Members the type does not name are ignored, and member order does not
/// matter.
#[test]
fn extra_members_are_ignored_and_order_is_free() {
    rows_or_skip!();
    agree(
        "from json extra members",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { DecodeError, FromJson, Json };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive FromJson, Show for Point;
struct Point {
    x: Int,
    y: Int,
}

derive FromJson, Show for Shape;
enum Shape {
    Rect { width: Int, height: Int },
}

fn doc<C: Allocator>(ctx: C, text: Str): Json {
    json.parse(ctx, text).withDefault(Json.Null)
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let a: Result<Point, DecodeError> = json.decode(
        ctx,
        doc(ctx, "{\"z\":[1,{\"q\":null}],\"y\":2,\"x\":1,\"w\":\"extra\"}"),
    );
    let _ = io.println(ctx, "${a.show(ctx)}").ignore();
    let b: Result<Shape, DecodeError> = json.decode(
        ctx,
        doc(ctx, "{\"Rect\":{\"height\":4,\"depth\":9,\"width\":3}}"),
    );
    let _ = io.println(ctx, "${b.show(ctx)}").ignore();
    .Ok(())
}
"#,
        ".Ok(Point { x: 1, y: 2 })\n\
         .Ok(.Rect { width: 3, height: 4 })\n",
    );
}

/// Every way a document can fail to be the type, each with its exact
/// `DecodeError`: a missing member, a wrong type at each primitive and at each
/// shape, `null` where a value is required, an element of a list, a tuple of
/// the wrong length, and every way an enum's tag can be wrong.
#[test]
fn every_failure_names_its_path_and_its_shapes() {
    rows_or_skip!();
    agree(
        "from json failures",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { DecodeError, FromJson, Json };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive FromJson, Show for Point;
struct Point {
    x: Int,
    y: Int,
}

derive FromJson, Show for Segment;
struct Segment {
    start: Point,
    stop: Point,
    label: Str,
    waypoints: [Point],
}

derive FromJson, Show for Shape;
enum Shape {
    Empty,
    Circle(Float),
    Line(Point, Point),
    Rect { width: Int, height: Int },
}

derive FromJson, Show for Prims;
struct Prims {
    b: Bool,
    s: Str,
    c: Char,
    f: Float,
    u: (),
}

fn doc<C: Allocator>(ctx: C, text: Str): Json {
    json.parse(ctx, text).withDefault(Json.Null)
}

fn point<C: Allocator + Stdout>(ctx: C, text: Str): () {
    let r: Result<Point, DecodeError> = json.decode(ctx, doc(ctx, text));
    io.println(ctx, "${r.show(ctx)}").ignore()
}

fn segment<C: Allocator + Stdout>(ctx: C, text: Str): () {
    let r: Result<Segment, DecodeError> = json.decode(ctx, doc(ctx, text));
    io.println(ctx, "${r.show(ctx)}").ignore()
}

fn shape<C: Allocator + Stdout>(ctx: C, text: Str): () {
    let r: Result<Shape, DecodeError> = json.decode(ctx, doc(ctx, text));
    io.println(ctx, "${r.show(ctx)}").ignore()
}

fn prims<C: Allocator + Stdout>(ctx: C, text: Str): () {
    let r: Result<Prims, DecodeError> = json.decode(ctx, doc(ctx, text));
    io.println(ctx, "${r.show(ctx)}").ignore()
}

fn pair<C: Allocator + Stdout>(ctx: C, text: Str): () {
    let r: Result<(Int, Str), DecodeError> = json.decode(ctx, doc(ctx, text));
    io.println(ctx, "${r.show(ctx)}").ignore()
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let _ = point(ctx, "{\"x\":1}");
    let _ = point(ctx, "{}");
    let _ = point(ctx, "{\"x\":1,\"y\":1.5}");
    let _ = point(ctx, "{\"x\":1,\"y\":\"two\"}");
    let _ = point(ctx, "{\"x\":null,\"y\":2}");
    let _ = point(ctx, "{\"x\":true,\"y\":2}");
    let _ = point(ctx, "{\"x\":[],\"y\":2}");
    let _ = point(ctx, "{\"x\":{},\"y\":2}");
    let _ = point(ctx, "[1,2]");
    let _ = point(ctx, "null");
    let _ = point(ctx, "\"p\"");
    let _ = segment(ctx, "{\"start\":{\"x\":0},\"stop\":{\"x\":1,\"y\":1},\"label\":\"d\",\"waypoints\":[]}");
    let _ = segment(ctx, "{\"start\":{\"x\":0,\"y\":0},\"stop\":{\"x\":1,\"y\":1},\"label\":\"d\",\"waypoints\":[{\"x\":1,\"y\":1},3]}");
    let _ = segment(ctx, "{\"start\":{\"x\":0,\"y\":0},\"stop\":{\"x\":1,\"y\":1},\"label\":\"d\",\"waypoints\":{}}");
    let _ = segment(ctx, "{\"start\":{\"x\":0,\"y\":0},\"stop\":{\"x\":1,\"y\":1},\"label\":7,\"waypoints\":[]}");
    let _ = prims(ctx, "{\"b\":1,\"s\":\"\",\"c\":\"x\",\"f\":1,\"u\":null}");
    let _ = prims(ctx, "{\"b\":true,\"s\":null,\"c\":\"x\",\"f\":1,\"u\":null}");
    let _ = prims(ctx, "{\"b\":true,\"s\":\"\",\"c\":\"xy\",\"f\":1,\"u\":null}");
    let _ = prims(ctx, "{\"b\":true,\"s\":\"\",\"c\":\"\",\"f\":1,\"u\":null}");
    let _ = prims(ctx, "{\"b\":true,\"s\":\"\",\"c\":7,\"f\":1,\"u\":null}");
    let _ = prims(ctx, "{\"b\":true,\"s\":\"\",\"c\":\"x\",\"f\":\"1\",\"u\":null}");
    let _ = prims(ctx, "{\"b\":true,\"s\":\"\",\"c\":\"x\",\"f\":1,\"u\":0}");
    let _ = prims(ctx, "{\"b\":false,\"s\":\"ok\",\"c\":\"\\u00e9\",\"f\":-0.25,\"u\":null}");
    let _ = pair(ctx, "[1]");
    let _ = pair(ctx, "[1,\"a\",2]");
    let _ = pair(ctx, "[1,2]");
    let _ = pair(ctx, "{}");
    let _ = shape(ctx, "\"Square\"");
    let _ = shape(ctx, "\"Circle\"");
    let _ = shape(ctx, "\"Rect\"");
    let _ = shape(ctx, "{\"Empty\":[]}");
    let _ = shape(ctx, "{\"Square\":[1]}");
    let _ = shape(ctx, "{\"Circle\":[1],\"Rect\":{}}");
    let _ = shape(ctx, "{}");
    let _ = shape(ctx, "7");
    let _ = shape(ctx, "{\"Circle\":1}");
    let _ = shape(ctx, "{\"Circle\":[]}");
    let _ = shape(ctx, "{\"Circle\":[\"r\"]}");
    let _ = shape(ctx, "{\"Line\":[{\"x\":0,\"y\":0},{\"x\":3}]}");
    let _ = shape(ctx, "{\"Rect\":[3,4]}");
    let _ = shape(ctx, "{\"Rect\":{\"width\":3}}");
    let _ = shape(ctx, "{\"Rect\":{\"width\":3,\"height\":false}}");
    .Ok(())
}
"#,
        ".Err(.Missing { path: \"$.y\" })\n\
         .Err(.Missing { path: \"$.x\" })\n\
         .Err(.WrongType { path: \"$.y\", wanted: \"an integer\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$.y\", wanted: \"an integer\", found: \"a string\" })\n\
         .Err(.WrongType { path: \"$.x\", wanted: \"an integer\", found: \"null\" })\n\
         .Err(.WrongType { path: \"$.x\", wanted: \"an integer\", found: \"a boolean\" })\n\
         .Err(.WrongType { path: \"$.x\", wanted: \"an integer\", found: \"an array\" })\n\
         .Err(.WrongType { path: \"$.x\", wanted: \"an integer\", found: \"an object\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an object\", found: \"an array\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an object\", found: \"null\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an object\", found: \"a string\" })\n\
         .Err(.Missing { path: \"$.start.y\" })\n\
         .Err(.WrongType { path: \"$.waypoints[1]\", wanted: \"an object\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$.waypoints\", wanted: \"an array\", found: \"an object\" })\n\
         .Err(.WrongType { path: \"$.label\", wanted: \"a string\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$.b\", wanted: \"a boolean\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$.s\", wanted: \"a string\", found: \"null\" })\n\
         .Err(.WrongType { path: \"$.c\", wanted: \"a one-character string\", found: \"a string\" })\n\
         .Err(.WrongType { path: \"$.c\", wanted: \"a one-character string\", found: \"a string\" })\n\
         .Err(.WrongType { path: \"$.c\", wanted: \"a one-character string\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$.f\", wanted: \"a number\", found: \"a string\" })\n\
         .Err(.WrongType { path: \"$.u\", wanted: \"null\", found: \"a number\" })\n\
         .Ok(Prims { b: false, s: \"ok\", c: 'é', f: -0.25, u: () })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an array of length 2\", found: \"an array\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an array of length 2\", found: \"an array\" })\n\
         .Err(.WrongType { path: \"$[1]\", wanted: \"a string\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an array\", found: \"an object\" })\n\
         .Err(.UnknownVariant { path: \"$\", tag: \"Square\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an object naming Circle's fields\", found: \"a string\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an object naming Rect's fields\", found: \"a string\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"the string Empty\", found: \"an object\" })\n\
         .Err(.UnknownVariant { path: \"$\", tag: \"Square\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an object with one member, naming the variant\", found: \"an object\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"an object with one member, naming the variant\", found: \"an object\" })\n\
         .Err(.WrongType { path: \"$\", wanted: \"a string or an object\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$.Circle\", wanted: \"an array\", found: \"a number\" })\n\
         .Err(.WrongType { path: \"$.Circle\", wanted: \"an array of length 1\", found: \"an array\" })\n\
         .Err(.WrongType { path: \"$.Circle[0]\", wanted: \"a number\", found: \"a string\" })\n\
         .Err(.Missing { path: \"$.Line[1].y\" })\n\
         .Err(.WrongType { path: \"$.Rect\", wanted: \"an object\", found: \"an array\" })\n\
         .Err(.Missing { path: \"$.Rect.height\" })\n\
         .Err(.WrongType { path: \"$.Rect.height\", wanted: \"an integer\", found: \"a boolean\" })\n",
    );
}

/// What `ToJson` writes, `FromJson` reads back to an equal value: through a
/// `Json` and through its text, at every shape, a recursive type included.
#[test]
fn to_json_and_from_json_round_trip() {
    rows_or_skip!();
    agree(
        "from json round trip",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { DecodeError, FromJson, Json, ToJson };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive Equal, FromJson, Show, ToJson for Point;
struct Point {
    x: Int,
    y: Int,
}

derive Equal, FromJson, Show, ToJson for Shape;
enum Shape {
    Empty,
    Circle(Float),
    Line(Point, Point),
    Rect { width: Int, height: Int },
}

derive Equal, FromJson, Show, ToJson for Rose;
enum Rose {
    Leaf(Int),
    Branch([Rose]),
}

derive Equal, FromJson, Show, ToJson for Chain;
struct Chain {
    head: Int,
    rest: Option<[Chain]>,
}

derive Equal, FromJson, Show, ToJson for Record;
struct Record {
    name: Str,
    letter: Char,
    ok: Bool,
    ratio: Float,
    tags: [Str],
    shape: Shape,
    maybe: Option<Point>,
    pair: (Int, Str),
    nothing: (),
}

fn rose(n: Int): Rose {
    if (n <= 0) { Rose.Leaf(0) } else { Rose.Branch([Rose.Leaf(n), rose(n - 1)]) }
}

fn chain(n: Int): Chain {
    if (n <= 0) {
        Chain { head: 0, rest: .None }
    } else {
        Chain { head: n, rest: .Some([chain(n - 1)]) }
    }
}

fn trip<T: ToJson + FromJson + Equal, C: Allocator + Stdout>(ctx: C, value: T): () {
    let text = json.stringify(ctx, json.encode(ctx, value));
    let direct: Result<T, DecodeError> = json.decode(ctx, json.encode(ctx, value));
    let parsed: Result<T, DecodeError> = json.decode(
        ctx,
        json.parse(ctx, text).withDefault(Json.Null),
    );
    io.println(ctx, "${text} ${direct == .Ok(value)} ${parsed == .Ok(value)}").ignore()
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let _ = trip(ctx, Point { x: -1, y: 2 });
    let _ = trip(ctx, Shape.Empty);
    let _ = trip(ctx, Shape.Circle(0.25));
    let _ = trip(ctx, Shape.Line(Point { x: 0, y: 0 }, Point { x: 3, y: 4 }));
    let _ = trip(ctx, Shape.Rect { width: 3, height: 4 });
    let _ = trip(ctx, rose(3));
    let _ = trip(ctx, chain(2));
    let _ = trip(
        ctx,
        Record {
            name: "a \"b\"",
            letter: 'q',
            ok: false,
            ratio: 1.5,
            tags: ["x", "y"],
            shape: Shape.Circle(2.0),
            maybe: .Some(Point { x: 1, y: 1 }),
            pair: (7, "z"),
            nothing: (),
        },
    );
    let _ = trip(ctx, [Point { x: 1, y: 2 }, Point { x: 3, y: 4 }]);
    let none: Option<Point> = .None;
    let _ = trip(ctx, none);
    .Ok(())
}
"#,
        "{\"x\":-1,\"y\":2} true true\n\
         \"Empty\" true true\n\
         {\"Circle\":[0.25]} true true\n\
         {\"Line\":[{\"x\":0,\"y\":0},{\"x\":3,\"y\":4}]} true true\n\
         {\"Rect\":{\"width\":3,\"height\":4}} true true\n\
         {\"Branch\":[[{\"Leaf\":[3]},{\"Branch\":[[{\"Leaf\":[2]},{\"Branch\":[[{\"Leaf\":[1]},{\"Leaf\":[0]}]]}]]}]]} true true\n\
         {\"head\":2,\"rest\":[{\"head\":1,\"rest\":[{\"head\":0,\"rest\":null}]}]} true true\n\
         {\"name\":\"a \\\"b\\\"\",\"letter\":\"q\",\"ok\":false,\"ratio\":1.5,\"tags\":[\"x\",\"y\"],\"shape\":{\"Circle\":[2]},\"maybe\":{\"x\":1,\"y\":1},\"pair\":[7,\"z\"],\"nothing\":null} true true\n\
         [{\"x\":1,\"y\":2},{\"x\":3,\"y\":4}] true true\n\
         null true true\n",
    );
}

/// A generic struct decodes at each instantiation, and a function generic
/// over `FromJson` decodes whatever it is instantiated at.
#[test]
fn a_generic_struct_decodes_at_each_instantiation() {
    rows_or_skip!();
    agree(
        "from json generic",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { DecodeError, FromJson, Json };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive FromJson, Show for Point;
struct Point {
    x: Int,
    y: Int,
}

derive FromJson, Show for Wrapper;
struct Wrapper<T> {
    value: T,
    tag: Str,
}

derive FromJson, Show for Either;
enum Either<A, B> {
    Left(A),
    Right { value: B },
}

fn read<T: FromJson + Show, C: Allocator + Stdout>(ctx: C, value: Json, witness: Option<T>): () {
    let r: Result<T, DecodeError> = json.decode(ctx, value);
    io.println(ctx, "${r.show(ctx)}").ignore()
}

fn doc<C: Allocator>(ctx: C, text: Str): Json {
    json.parse(ctx, text).withDefault(Json.Null)
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let a: Option<Wrapper<Point>> = .None;
    let _ = read(ctx, doc(ctx, "{\"value\":{\"x\":1,\"y\":2},\"tag\":\"p\"}"), a);
    let b: Option<Wrapper<[Int]>> = .None;
    let _ = read(ctx, doc(ctx, "{\"value\":[1,2],\"tag\":\"l\"}"), b);
    let _ = read(ctx, doc(ctx, "{\"value\":[1,\"2\"],\"tag\":\"l\"}"), b);
    let c: Option<Wrapper<Option<Str>>> = .None;
    let _ = read(ctx, doc(ctx, "{\"value\":null,\"tag\":\"o\"}"), c);
    let d: Option<Either<Int, Point>> = .None;
    let _ = read(ctx, doc(ctx, "{\"Left\":[4]}"), d);
    let _ = read(ctx, doc(ctx, "{\"Right\":{\"value\":{\"x\":5,\"y\":6}}}"), d);
    let _ = read(ctx, doc(ctx, "{\"Right\":{\"value\":{\"x\":5}}}"), d);
    .Ok(())
}
"#,
        ".Ok(Wrapper { value: Point { x: 1, y: 2 }, tag: \"p\" })\n\
         .Ok(Wrapper { value: [1, 2], tag: \"l\" })\n\
         .Err(.WrongType { path: \"$.value[1]\", wanted: \"an integer\", found: \"a string\" })\n\
         .Ok(Wrapper { value: .None, tag: \"o\" })\n\
         .Ok(.Left(4))\n\
         .Ok(.Right { value: Point { x: 5, y: 6 } })\n\
         .Err(.Missing { path: \"$.Right.value.y\" })\n",
    );
}

/// A large document: a list of twenty thousand records, and a tree nested a
/// hundred and fifty deep. Each decodes and gives back every block, and an
/// error deep inside names its whole path.
#[test]
fn a_large_document_decodes() {
    rows_or_skip!();
    agree(
        "from json large",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { DecodeError, FromJson, Json, ToJson };
from "core/list" import * as list;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive Equal, FromJson, Show, ToJson for Item;
struct Item {
    id: Int,
    name: Str,
    tags: [Str],
    score: Option<Float>,
}

derive Equal, FromJson, Show, ToJson for Rose;
enum Rose {
    Leaf(Int),
    Branch([Rose]),
}

fn deep(n: Int, acc: Rose): Rose {
    if (n <= 0) { acc } else { deep(n - 1, Rose.Branch([acc])) }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let items = list.range(ctx, 0, 20000).mapCtx(ctx, fn(c, i) => Item {
        id: i,
        name: "item",
        tags: ["a", "b"],
        score: if (i % 2 == 0) { .Some(0.5) } else { .None },
    });
    let text = json.stringify(ctx, json.encode(ctx, items));
    let doc = json.parse(ctx, text).withDefault(Json.Null);
    let back: Result<[Item], DecodeError> = json.decode(ctx, doc);
    let line = match (back) {
        .Ok(xs) => "${text.length()} ${xs.length()} ${xs == items} ${xs.last().show(ctx)}",
        .Err(e) => e.show(ctx),
    };
    let _ = io.println(ctx, line).ignore();
    let broken = json.parse(ctx, "[{\"id\":0,\"name\":\"a\",\"tags\":[],\"score\":null},{\"id\":1,\"name\":\"b\",\"tags\":[\"x\",3],\"score\":null}]").withDefault(Json.Null);
    let bad: Result<[Item], DecodeError> = json.decode(ctx, broken);
    let _ = io.println(ctx, "${bad.show(ctx)}").ignore();
    let tree = deep(150, Rose.Leaf(7));
    let tree_back: Result<Rose, DecodeError> = json.decode(
        ctx,
        json.parse(ctx, json.stringify(ctx, json.encode(ctx, tree))).withDefault(Json.Null),
    );
    let _ = io.println(ctx, "${tree_back == .Ok(tree)}").ignore();
    .Ok(())
}
"#,
        "1118891 20000 true .Some(Item { id: 19999, name: \"item\", tags: [\"a\", \"b\"], score: .None })\n\
         .Err(.WrongType { path: \"$[1].tags[1]\", wanted: \"a string\", found: \"a number\" })\n\
         true\n",
    );
}

/// `json.encode` at every shape, a list and a primitive included: the text
/// `stringify` writes of each.
#[test]
fn json_encode_writes_every_shape() {
    rows_or_skip!();
    agree(
        "to json every shape",
        r#"
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { ToJson };
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

derive ToJson for Point;
struct Point {
    x: Int,
    y: Int,
}

derive ToJson for Holder;
struct Holder {
    points: [Point],
    words: [Str],
    grid: [[Int]],
    empty: [Bool],
}

fn say<T: ToJson, C: Allocator + Stdout>(ctx: C, value: T): () {
    io.println(ctx, json.stringify(ctx, json.encode(ctx, value))).ignore()
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let _ = say(ctx, Holder {
        points: [Point { x: 1, y: 2 }],
        words: ["a", "b"],
        grid: [[1], [], [2, 3]],
        empty: [],
    });
    let _ = say(ctx, [Point { x: 3, y: 4 }]);
    let _ = say(ctx, 5);
    let _ = say(ctx, 2.5);
    let _ = say(ctx, "s");
    let _ = say(ctx, true);
    let _ = say(ctx, 'c');
    let small: U8 = 200;
    let _ = say(ctx, small);
    let _ = say(ctx, [1, 2]);
    .Ok(())
}
"#,
        "{\"points\":[{\"x\":1,\"y\":2}],\"words\":[\"a\",\"b\"],\"grid\":[[1],[],[2,3]],\"empty\":[]}\n\
         [{\"x\":3,\"y\":4}]\n\
         5\n\
         2.5\n\
         \"s\"\n\
         true\n\
         \"c\"\n\
         200\n\
         [1,2]\n",
    );
}
