// The Buri runtime for the JavaScript backend.
//
// Every global here is `$`-prefixed, which is what lets the minifier rename
// them safely and drop the ones a program does not reach. Nothing in this file
// is emitted unless something in the generated program names it.
//
// The value representation, which the backend and this file have to agree on:
//
//   ()              0
//   Bool            boolean
//   I8..I32, U8..U32  number -- a double holds every integer of these widths
//   I64, U64          bigint -- `Int` is `I64`, and a double stops being exact
//   I128, U128        number    at 2^53, which is inside all four of these
//   F32, F64        number
//   Char            a one-scalar string
//   Str             string
//   Template        a string -- the backend renders every hole from its
//                   static type and joins the parts, so nothing here has to
//   struct          an array of fields, in declaration order
//   enum            a number (the tag) when no variant has a payload,
//                   otherwise [tag, ...payload]
//   Option<T>       `None` is undefined and `Some(x)` is `x`. Absence is the
//                   only undefined there is, so nothing else is needed to
//                   tell them apart -- and no array is built. The one
//                   collision is `Option<Option<T>>`, where `Some(None)`
//                   would be undefined too; see $some/$val
//   tuple, [T]      an array
//   fn              a function
//   context         an array of implementations, in binding order

// --- Failure ----------------------------------------------------------------

// The program has no way to say "this cannot happen" — every case is handled —
// so this is only ever reached from a runtime failure the language does define.
function $abort(m) {
  const e = new Error(m);
  e.$buri = true;
  throw e;
}

// Overflow and underflow are undefined and go unchecked. Division by zero
// still aborts: there is no answer to give.
function $divz() {
  $abort("division by zero");
}

// Truncating toward zero, so `a == (a / b) * b + (a % b)` holds.
function $divi(a, b) {
  if (b === 0) $divz();
  return Math.trunc(a / b);
}

function $remi(a, b) {
  if (b === 0) $divz();
  return a % b;
}

// The same two at a `BigInt` width, where `/` already truncates toward zero.
function $divb(a, b) {
  if (b === 0n) $divz();
  return a / b;
}

function $remb(a, b) {
  if (b === 0n) $divz();
  return a % b;
}

// `abs` at a `BigInt` width: `Math.abs` is a double operation and refuses one.
function $absBig(v) {
  return v < 0n ? -v : v;
}

// Taking the low `bits` of a value, for checksums and wire formats where
// wrapping is the intent. The target is one of the `number` widths — the
// backend spells the `BigInt` ones as `asIntN` on the spot — but the *source*
// may be either, so a value that is already exact is not sent through a double.
function $wrapTo(v, bits, signed) {
  const b = typeof v === "bigint" ? v : BigInt(Math.trunc(v));
  const w = signed ? BigInt.asIntN(bits, b) : BigInt.asUintN(bits, b);
  return Number(w);
}

// One wrapping operation, computed where it is exact: `op` is 0 for `+`, 1 for
// `-` and 2 for `*`.
//
// `$wrapTo` takes the low bits of a value it is *handed*, and the caller used
// to hand it a double it had already computed. Above 2^53 that double is
// rounded, and the low bits of a rounded value are not the low bits of the
// answer — `U32.wrappingMultiply(0xffffffff, 0xffffffff)` is 1, its exact product
// 18446744065119617025 rounds to an even double, and the wrap of that is 0. So
// the arithmetic itself happens in BigInt and only the wrapped result, which
// is inside the type's range by construction, comes back as a `number`.
//
// Emitted only at the `number` widths, where the intermediate can leave the
// exact range although both operands and the answer are inside it — a product
// at 32 bits, and nothing else. The `BigInt` widths need none of this: the
// operation is already exact and the wrap is one `asIntN`.
function $wrapOp(op, a, b, bits, signed) {
  const x = BigInt(Math.trunc(a));
  const y = BigInt(Math.trunc(b));
  const r = op === 0 ? x + y : op === 1 ? x - y : x * y;
  return Number(signed ? BigInt.asIntN(bits, r) : BigInt.asUintN(bits, r));
}

// --- Rendering ---------------------------------------------------------------

// Rendering a value whose type the backend could not settle statically: the
// derived `Show`, and the fallback hole.
function $str(v) {
  const t = typeof v;
  if (t === "string") return v;
  if (t === "boolean") return v ? "true" : "false";
  // A `number` is both an integer type and a float type here, so the backend
  // chooses the rendering from the static type and only reaches this for a
  // float. A `bigint` is an integer and nothing else, so `String` is right.
  if (t === "number") return $f64(v);
  return String(v);
}

// A float always shows a point, so `1.0` does not read as an integer.
function $f64(n) {
  if (Number.isNaN(n)) return "NaN";
  if (n === Infinity) return "inf";
  if (n === -Infinity) return "-inf";
  if (Number.isInteger(n) && Math.abs(n) < 1e21) return (Object.is(n, -0) ? "-0" : n) + ".0";
  return String(n);
}

// --- The structural operations `derive` stands for ---------------------------
//
// A descriptor is [kind, ...]. Kinds: 0 primitive, 1 unit, 2 struct, 3 enum,
// 4 array, 5 tuple, 6 opaque.

// `==` at a float, where the operands cannot be written twice. SPEC 7.2 rules
// `NaN == NaN`, so this is `===` widened by exactly one pair. It is not
// `Object.is`, which separates `-0.0` from `0.0`; SPEC 6.2 keeps those equal.
function $feq(a, b) {
  return a === b || (a !== a && b !== b);
}

function $eq(a, b) {
  // A fast path that is also the answer: SPEC 7.2 makes `==` an equivalence
  // relation, so one value compared with itself is equal at every type.
  if (a === b) return true;
  // The one value `===` denies is itself. Both sides NaN is equal; one side
  // NaN and the other anything is not, at every depth.
  if (a !== a) return b !== b;
  // Two `Some(None)`s at the same nesting depth are the same value; they are
  // distinct objects, so `===` does not say so.
  if (a !== null && typeof a === "object" && !Array.isArray(a) && a.$n !== undefined) {
    return b !== null && typeof b === "object" && b.$n === a.$n;
  }
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!$eq(a[i], b[i])) return false;
    return true;
  }
  return false;
}

// Returns the index of an `Order` variant: 0 Less, 1 Equal, 2 Greater.
function $cmp(a, b) {
  // `None` sorts before every `Some`, and it is the only `undefined` there
  // is. Without this the comparisons below answer Equal for it, because
  // `undefined < x` and `undefined > x` are both false.
  if (a === undefined || b === undefined) {
    return a === b ? 1 : a === undefined ? 0 : 2;
  }
  if (Array.isArray(a)) {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) {
      const c = $cmp(a[i], b[i]);
      if (c !== 1) return c;
    }
    return a.length < b.length ? 0 : a.length > b.length ? 2 : 1;
  }
  // `Str` and `Char` are both JavaScript strings, and `<` on one is UTF-16
  // code-unit order rather than the scalar order the language specifies. This
  // is the derived path — a struct field, a list element, a tuple — and it has
  // to answer what `Str.compare` answers, or an `[Str]` and an `[(Str, Int)]`
  // would sort differently.
  if (typeof a === "string") return $str_compare(a, b);
  // Floats order -0.0 equal to 0.0 and report NaN as unordered, which falls
  // out of the comparisons below.
  return a < b ? 0 : a > b ? 2 : 1;
}

// FNV-1a over 32 bits, which a double holds exactly. `Hash` returns a `U64`,
// and the top bits are simply always zero.
//
// The accumulator is threaded through as an argument rather than captured,
// because a closure over a mutable local has to be allocated on every call and
// every hashed container calls this on every lookup. Same numbers, nothing
// allocated. The array arm is an indexed loop for the same reason: `for..of`
// allocates an iterator.
function $mix(h, x) {
  h = (h ^ (x >>> 0)) >>> 0;
  return Math.imul(h, 0x01000193) >>> 0;
}

function $hashInto(h, x) {
  if (Array.isArray(x)) {
    h = $mix(h, x.length);
    for (let i = 0; i < x.length; i++) h = $hashInto(h, x[i]);
    return h;
  }
  if (x === undefined) return $mix(h, 0);
  if (x !== null && typeof x === "object" && x.$n !== undefined) return $mix(h, x.$n + 1);
  if (typeof x === "string") {
    for (let i = 0; i < x.length; i++) h = $mix(h, x.charCodeAt(i));
    return h;
  }
  if (typeof x === "boolean") return $mix(h, x ? 1 : 0);
  // The low 32 bits either way: `>>> 0` on a double and `asUintN` on a
  // `BigInt` are the same reduction, so a value that fits both hashes the same.
  if (typeof x === "bigint") return $mix(h, Number(BigInt.asUintN(32, x)));
  return $mix(h, Math.trunc(x) || 0);
}

// `Hash` answers a `U64`, which is a `BigInt`. The mixing above stays on
// doubles — it is 32 bits wide and runs on every lookup — and only the answer
// crosses over.
function $hash(v) {
  return BigInt($hashInto(0x811c9dc5, v));
}

// The joins `$show` needs, as loops. Each of these was a `.map(…).join(", ")`,
// which allocates a fresh arrow *and* an intermediate array on every call —
// and `$show` runs at least once per assertion in every test suite, so it is
// worth not doing.
function $showFields(fields, types, xs) {
  let out = "";
  for (let i = 0; i < fields.length; i++) {
    if (i) out += ", ";
    out += fields[i] + ": " + $show(xs[i], types[i]);
  }
  return out;
}

function $showArgs(xs, types) {
  let out = "";
  for (let i = 0; i < xs.length; i++) {
    if (i) out += ", ";
    out += $show(xs[i], types[i]);
  }
  return out;
}

// One shared element type, which is what a list has.
function $showEach(xs, t) {
  let out = "";
  for (let i = 0; i < xs.length; i++) {
    if (i) out += ", ";
    out += $show(xs[i], t);
  }
  return out;
}

function $show(v, d) {
  const k = d[0];
  if (k === 0) {
    const p = d[1];
    if (p === "s") return JSON.stringify(v);
    if (p === "c") return "'" + v + "'";
    if (p === "f") return $f64(v);
    // "i" is an integer and "I" a `BigInt` one; a number is also how a float
    // is stored, so the tag is what tells them apart. `String` renders both
    // in decimal, with no `n`.
    if (p === "i" || p === "I") return String(v);
    return $str(v);
  }
  if (k === 1) return "()";
  if (k === 2) {
    // [2, name, record, fields, types]
    const [, name, record, fields, types] = d;
    // A struct with no fields is still written with its delimiters — `Hollow {}`
    // is a value and `Hollow` is a type — so the rendering is the source syntax,
    // the same one `middle/derives.rs` generates for `Show`.
    if (!fields.length) return record ? name + " {}" : name + "()";
    if (record) {
      return name + " { " + $showFields(fields, types, v) + " }";
    }
    return name + "(" + $showArgs(v, types) + ")";
  }
  if (k === 3) {
    // [3, name, variants, payloadless]
    const [, , variants, flat] = d;
    const tag = flat ? v : v[0];
    const [vname, record, fields, types] = variants[tag];
    if (!fields.length) return "." + vname;
    const args = flat ? [] : v.slice(1);
    if (record) {
      return "." + vname + " { " + $showFields(fields, types, args) + " }";
    }
    return "." + vname + "(" + $showArgs(args, types) + ")";
  }
  // [7, payload] -- an `Option`, which has no tag to read.
  if (k === 7) {
    return v === undefined ? ".None" : ".Some(" + $show($val(v), d[1]) + ")";
  }
  if (k === 4) return "[" + $showEach(v, d[1]) + "]";
  if (k === 5) return "(" + $showArgs(v, d[1]) + ")";
  return $str(v);
}

// --- core/json ----------------------------------------------------------------
//
// `derive ToJson` and `derive FromJson` are one walk over a type descriptor
// each, in place of an encoder and a decoder generated per type. What the walk
// means — which Buri shape becomes which JSON shape — is written down in
// `core/json`'s own source, and this is the half that runs.
//
// A `Json` is the enum `core/json` declares, so its variant tags are that
// declaration's order and nothing else: 0 Null, 1 Bool, 2 Num, 3 Str, 4 Array,
// 5 Object. A runtime walker is the one place that builds a value of a library
// type without the library's help, so those five numbers are the seam. The
// conformance suite asserts `json.encode(ctx, 1) == Json.Num(1.0)` and its four
// siblings, which is what keeps the two ends in step.

function $json_bool(b) {
  return [1, b];
}

function $json_num(x) {
  return [2, x];
}

function $json_str(s) {
  return [3, s];
}

// The three loops the encoding walk needs. Each was a `.map(…)` once, which
// allocates an arrow per call, and encoding is a per-element operation.
function $jsonFields(fields, types, xs) {
  const out = new Array(fields.length);
  for (let i = 0; i < fields.length; i++) out[i] = [fields[i], $json_of(xs[i], types[i])];
  return out;
}

function $jsonArgs(xs, types) {
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = $json_of(xs[i], types[i]);
  return out;
}

// One shared element type, which is what a list has.
function $jsonEach(xs, t) {
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = $json_of(xs[i], t);
  return out;
}

function $json_of(v, d) {
  const k = d[0];
  if (k === 0) {
    const p = d[1];
    if (p === "b") return [1, v];
    // A `Char` is a one-scalar string, so it is a JSON string like `Str`.
    if (p === "s" || p === "c") return [3, v];
    // JSON's one number type is a double, so a `BigInt` narrows on the way
    // out. Above 2^53 that rounds — a document cannot say the value.
    return [2, p === "I" ? Number(v) : v];
  }
  if (k === 1) return [0];
  if (k === 2) {
    // [2, name, record, fields, types]
    const [, , record, fields, types] = d;
    if (record) return [5, $jsonFields(fields, types, v)];
    return [4, $jsonArgs(v, types)];
  }
  if (k === 3) {
    // [3, name, variants, payloadless] — externally tagged: a variant with no
    // fields is its own name, and one with fields is a single-member object.
    const [, , variants, flat] = d;
    const tag = flat ? v : v[0];
    const [vname, record, fields, types] = variants[tag];
    if (!fields.length) return [3, vname];
    const args = flat ? [] : v.slice(1);
    const payload = record
      ? [5, $jsonFields(fields, types, args)]
      : [4, $jsonArgs(args, types)];
    return [5, [[vname, payload]]];
  }
  // [7, payload] — an `Option`, which is its payload or `null`.
  if (k === 7) return v === undefined ? [0] : $json_of($val(v), d[1]);
  if (k === 4) return [4, $jsonEach(v, d[1])];
  if (k === 5) return [4, $jsonArgs(v, d[1])];
  return [0];
}

// What the document actually held, for the message. Never the value itself: a
// decode error names a place and a shape, and a document is not something to
// paste into a terminal.
function $jsonFound(j) {
  const k = j[0];
  if (k === 0) return "null";
  if (k === 1) return "a boolean";
  if (k === 2) return "a number";
  if (k === 3) return "a string";
  if (k === 4) return "an array";
  return "an object";
}

// A failure is thrown rather than returned, so the walk carries no result
// wrapper down every level and the succeeding path allocates only what it
// keeps. `$json_decode` is the one place that catches it, and the one place
// the error crosses back into Buri.
function $jsonThrow(e) {
  const err = new Error("json decode failed");
  err.$json = e;
  throw err;
}

function $jsonWrong(p, wanted, j) {
  $jsonThrow([1, p, wanted, $jsonFound(j)]);
}

function $jsonMember(entries, key, p) {
  for (let i = 0; i < entries.length; i++) if (entries[i][0] === key) return entries[i][1];
  $jsonThrow([0, p + "." + key]);
}

function $jsonVariant(variants, name, p) {
  for (let i = 0; i < variants.length; i++) if (variants[i][0] === name) return i;
  $jsonThrow([2, p, name]);
}

function $jsonVariantInto(j, d, p) {
  const variants = d[2];
  const flat = d[3];
  // A variant with no fields is written as its name, and nothing else is.
  if (j[0] === 3) {
    const t = $jsonVariant(variants, j[1], p);
    if (variants[t][2].length) $jsonWrong(p, "an object naming " + j[1] + "'s fields", j);
    return flat ? t : [t];
  }
  if (j[0] !== 5) $jsonWrong(p, "a string or an object", j);
  const entries = j[1];
  if (entries.length !== 1) $jsonWrong(p, "an object with one member, naming the variant", j);
  const name = entries[0][0];
  const t = $jsonVariant(variants, name, p);
  const record = variants[t][1];
  const fields = variants[t][2];
  const types = variants[t][3];
  if (!fields.length) $jsonWrong(p, "the string " + name, j);
  const inner = entries[0][1];
  const q = p + "." + name;
  const out = new Array(fields.length + 1);
  out[0] = t;
  if (record) {
    if (inner[0] !== 5) $jsonWrong(q, "an object", inner);
    for (let i = 0; i < fields.length; i++) {
      out[i + 1] = $json_into($jsonMember(inner[1], fields[i], q), types[i], q + "." + fields[i]);
    }
    return out;
  }
  if (inner[0] !== 4) $jsonWrong(q, "an array", inner);
  if (inner[1].length !== fields.length) $jsonWrong(q, "an array of length " + fields.length, inner);
  for (let i = 0; i < fields.length; i++) {
    out[i + 1] = $json_into(inner[1][i], types[i], q + "[" + i + "]");
  }
  return out;
}

function $json_into(j, d, p) {
  const k = d[0];
  if (k === 0) {
    const t = d[1];
    if (t === "b") {
      if (j[0] !== 1) $jsonWrong(p, "a boolean", j);
      return j[1];
    }
    if (t === "s") {
      if (j[0] !== 3) $jsonWrong(p, "a string", j);
      return j[1];
    }
    if (t === "c") {
      // A `Char` is one Unicode scalar value, which is one iteration step of a
      // string rather than one UTF-16 unit of it.
      if (j[0] !== 3 || Array.from(j[1]).length !== 1) {
        $jsonWrong(p, "a one-character string", j);
      }
      return j[1];
    }
    if (j[0] !== 2) $jsonWrong(p, t === "f" ? "a number" : "an integer", j);
    // JSON has one number type, so an integer field is a number that happens
    // to be whole — and a document that says `1.5` is not one.
    if (t !== "f" && !Number.isInteger(j[1])) $jsonWrong(p, "an integer", j);
    // A `BigInt` field is built from that whole number, which is as much as a
    // document carries: JSON's number is a double.
    return t === "I" ? BigInt(j[1]) : j[1];
  }
  if (k === 1) {
    if (j[0] !== 0) $jsonWrong(p, "null", j);
    return 0;
  }
  if (k === 2) {
    const [, , record, fields, types] = d;
    const out = new Array(fields.length);
    if (record) {
      if (j[0] !== 5) $jsonWrong(p, "an object", j);
      for (let i = 0; i < fields.length; i++) {
        out[i] = $json_into($jsonMember(j[1], fields[i], p), types[i], p + "." + fields[i]);
      }
      return out;
    }
    if (j[0] !== 4) $jsonWrong(p, "an array", j);
    if (j[1].length !== fields.length) $jsonWrong(p, "an array of length " + fields.length, j);
    for (let i = 0; i < fields.length; i++) {
      out[i] = $json_into(j[1][i], types[i], p + "[" + i + "]");
    }
    return out;
  }
  if (k === 3) return $jsonVariantInto(j, d, p);
  if (k === 7) return j[0] === 0 ? undefined : $some($json_into(j, d[1], p));
  if (k === 4) {
    if (j[0] !== 4) $jsonWrong(p, "an array", j);
    const xs = j[1];
    const out = new Array(xs.length);
    for (let i = 0; i < xs.length; i++) out[i] = $json_into(xs[i], d[1], p + "[" + i + "]");
    return out;
  }
  if (k === 5) {
    if (j[0] !== 4) $jsonWrong(p, "an array", j);
    const types = d[1];
    if (j[1].length !== types.length) $jsonWrong(p, "an array of length " + types.length, j);
    const out = new Array(types.length);
    for (let i = 0; i < types.length; i++) {
      out[i] = $json_into(j[1][i], types[i], p + "[" + i + "]");
    }
    return out;
  }
  $jsonWrong(p, "a type with a shape", j);
}

// `$` is the document, and the path grows from there, so an error names a
// place a reader can find in the text in front of them.
function $json_decode(j, d) {
  try {
    return [0, $json_into(j, d, "$")];
  } catch (e) {
    if (e !== null && typeof e === "object" && e.$json !== undefined) return [1, e.$json];
    throw e;
  }
}

// --- sharing ------------------------------------------------------------------
//
// `$u` is one bit per list, and it only ever moves one way. `$u === true` says
// this runtime allocated the list and nothing else holds it, so an operation
// that grows it may write through instead of copying. **Absence means not
// ours**, and therefore shared: an array that arrived from the host carries no
// `$u`, so it is copied and never written to. The fast path tests for `true`
// rather than for the absence of a mark, because absence is the answer for
// everything this backend did not make. design/native/MEMORY.md §5.5.
//
// An aggregate — a struct, a tuple, an enum, all of them arrays here — carries
// no bit, because nothing writes into one: a functional update spells its
// fields out or copies. What it needs is the *other* half of the question, so
// that a field read out of it can pass the sharing on, and that is `$shared`.
//
// `$shared` is a symbol-keyed property rather than a `$u`, so the mark is
// invisible to everything a host reads by name: `JSON.stringify`, `Object.keys`,
// `for…in` and spread all skip a symbol key. A host array marked here still
// carries no `$u`, so it still reads as not ours. It is a property rather than
// a `WeakSet` entry because every element handed to a `fold` or a `map` is
// marked, and a property store is several times cheaper than a `WeakSet.add`
// in both bun and node.
//
// A frozen or sealed object cannot take the property, and writing one would
// throw, so its mark goes into `$sharedFrozen` instead. `$fromShared` reads that
// set only for an object that cannot be extended, so a program that never
// meets one never looks there.

// A fresh list this runtime allocated. Called on the way out of everything in
// `core/list` that builds one.
function $own(a) {
  a.$u = true;
  return a;
}

const $shared = Symbol("shared");
const $sharedFrozen = new WeakSet();

// A second reference to a value has come into existence. Sticky: nothing ever
// puts a value back, because the cost of an over-set mark is one copy and the
// cost of a cleared one is an aliasing bug.
//
// The mark is asked about first because most calls find it: an aggregate is
// marked once and then shared again and again, and a list never carries it, so
// asking first answers the common case with one property read instead of two.
function $share(v) {
  if (v !== null && typeof v === "object" && v[$shared] !== true) {
    const u = v.$u;
    if (u === true) v.$u = false;
    else if (u === undefined) {
      if (Object.isExtensible(v)) v[$shared] = true;
      else $sharedFrozen.add(v);
    }
  }
  return v;
}

// Every element of `xs` marked, as the higher-order functions below hand each
// one to a callback. `$each` on the list records that this was done, so a list
// that is searched or folded over again and again is walked to mark it once
// rather than once per search. The elements are marked up front, even the ones
// a search that stops early never reaches: an over-set mark costs one copy, and
// the record is what saves the walk.
//
// Only adding an element can make the record false, so the two operations that
// add one in place, `$list_push` and `$list_concat`, clear it. Taking elements
// away or reordering them leaves every element still marked.
const $each = Symbol("each");

function $shareEach(xs) {
  if (xs[$each] !== true) {
    for (let i = 0; i < xs.length; i++) $share(xs[i]);
    if (Object.isExtensible(xs)) xs[$each] = true;
  }
  return xs;
}

// A field read out of a parent this expression is the last use of: a second
// reference only if the parent was one. Perceus's drop specialisation with the
// answer left until run time, because a garbage collector hides the count that
// would have decided it statically.
function $fromShared(p, v) {
  if (
    p !== null &&
    typeof p === "object" &&
    (p.$u === false ||
      p[$shared] === true ||
      (!Object.isExtensible(p) && $sharedFrozen.has(p)))
  ) {
    return $share(v);
  }
  return v;
}

// --- core/list ----------------------------------------------------------------
//
// Indexing yields Option<T>, so `get` is where the absence shows up.

// `None` is `undefined` and `Some(x)` is `x`: absence is the only thing in the
// value representation that is ever `undefined`, so nothing else is needed to
// tell them apart.
//
// `Option<Option<T>>` is the one case where that collides, since `Some(None)`
// would be `undefined` too. The generated code knows its types and wraps only
// there; these two are for the runtime, which is shared across every element
// type and so has to check. The counter carries the nesting depth.
function $some(x) {
  if (x === undefined) return { $n: 0 };
  if (x !== null && typeof x === "object" && !Array.isArray(x) && x.$n !== undefined) {
    return { $n: x.$n + 1 };
  }
  return x;
}

function $val(x) {
  if (x !== null && typeof x === "object" && !Array.isArray(x) && x.$n !== undefined) {
    return x.$n === 0 ? undefined : { $n: x.$n - 1 };
  }
  return x;
}

// `0n` to `1023n`, so a count, a length or an index is read rather than made:
// `BigInt()` is a call into the engine, and these are most of what is asked.
const $ints = Array.from({ length: 1024 }, (_, i) => BigInt(i));

// A length or an index, which is a non-negative integer, as an `Int`.
function $int(n) {
  return n < 1024 ? $ints[n] : BigInt(n);
}

function $list_length(xs) {
  return $int(xs.length);
}

function $list_get(xs, i) {
  const n = Number(i);
  return n >= 0 && n < xs.length ? $some(xs[n]) : undefined;
}

// `get` over a list whose element is not itself an `Option`, which the
// generated code chooses (`js/intrinsics.rs`) because it knows the element
// type. Such an element is never `undefined` and never a `$some` counter, so
// it is its own `Some` and needs no check. `$list_findFlat` is the same.
function $list_getFlat(xs, i) {
  const n = Number(i);
  return n >= 0 && n < xs.length ? xs[n] : undefined;
}

// A higher-order runtime function marks what it hands to a callback: the
// element belongs to `xs`, so a callback that keeps one keeps a second name
// for it.
//
// The **seed does not**, and that is a decision rather than an omission. A
// fold's accumulator parameter is owned (`middle/rc.rs`'s `TAKEN_BY`), so a
// caller still holding what it seeded the fold with has already marked it, and
// marking again here would cost the first step a copy of the whole
// accumulator. Once per fold reads as a constant until the fold is inside a
// walk, and then it is one copy of everything built so far per step of the
// walk.
function $list_fold(xs, f, acc) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) acc = f(acc, xs[i]);
  return acc;
}

function $list_foldCtx(xs, c, f, acc) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) acc = f(c, acc, xs[i]);
  return acc;
}

// Stops at the first .Err, which is how a fallible fold is written without an
// early exit.
function $list_foldResult(xs, f, acc) {
  $shareEach(xs);
  let cur = [0, acc];
  for (let i = 0; i < xs.length; i++) {
    cur = f(cur[1], xs[i]);
    if (cur[0] !== 0) return cur;
  }
  return cur;
}

function $list_foldResultCtx(xs, c, f, acc) {
  $shareEach(xs);
  let cur = [0, acc];
  for (let i = 0; i < xs.length; i++) {
    cur = f(c, cur[1], xs[i]);
    if (cur[0] !== 0) return cur;
  }
  return cur;
}

function $list_any(xs, p) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return true;
  return false;
}

function $list_all(xs, p) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) if (!p(xs[i])) return false;
  return true;
}

function $list_find(xs, p) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return $some(xs[i]);
  return undefined;
}

function $list_findFlat(xs, p) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return xs[i];
  return undefined;
}

function $list_findIndex(xs, p) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return $int(i);
  return undefined;
}

function $list_count(xs, p) {
  $shareEach(xs);
  let n = 0;
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) n++;
  return $int(n);
}

function $list_map(xs, c, f) {
  $shareEach(xs);
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = f(xs[i]);
  return $own(out);
}

function $list_mapCtx(xs, c, f) {
  $shareEach(xs);
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = f(c, xs[i]);
  return $own(out);
}

function $list_filter(xs, c, p) {
  $shareEach(xs);
  const out = [];
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) out.push(xs[i]);
  return $own(out);
}

function $list_filterCtx(xs, c, p) {
  $shareEach(xs);
  const out = [];
  for (let i = 0; i < xs.length; i++) if (p(c, xs[i])) out.push(xs[i]);
  return $own(out);
}

// --- the same four, awaiting their step --------------------------------------
//
// A `*Ctx` combinator hands its step the caller's **whole context**, so the
// step may do anything the caller may: dial a socket, sleep on a clock, ask an
// actor, open a task scope. On this backend a step that waits is an `async`
// arrow, and calling one returns a promise rather than an answer — so
// `xs.mapCtx(ctx, fn(c, x) => …)` over a body that parks produced a list of
// promises, `main` returned before any of them settled, and the work the step
// was written to do silently did not happen. That is the bug these four exist
// to fix, and `$list_mapCtx` above is why the plain loop still exists: a step
// that never waits must stay synchronous, because an `async` combinator makes
// its caller `async`, and this compiler hands function values to JavaScript
// that cannot await one — a `view` given to `mount`, a sort comparator, the
// row callbacks inside `ui.each`.
//
// Which of the two an instantiation is compiled to is not a decision this file
// makes: `middle::rc`'s `can_park` column decides it, from the step that
// actually arrived at the call (`intrinsic_keys::ctx_step_key`), and the same
// column is what puts the `await` at the call site. The suffix is the whole
// of the convention — `$x` and `$xAwait` — and `js/intrinsics.rs` reads it.
//
// Each is its synchronous twin with one `await` in it and nothing else
// changed, including `$shareEach` on the list and the seed left unmarked, so
// the two cannot disagree about ownership.

async function $list_foldCtxAwait(xs, c, f, acc) {
  $shareEach(xs);
  for (let i = 0; i < xs.length; i++) acc = await f(c, acc, xs[i]);
  return acc;
}

async function $list_foldResultCtxAwait(xs, c, f, acc) {
  $shareEach(xs);
  let cur = [0, acc];
  for (let i = 0; i < xs.length; i++) {
    cur = await f(c, cur[1], xs[i]);
    if (cur[0] !== 0) return cur;
  }
  return cur;
}

async function $list_mapCtxAwait(xs, c, f) {
  $shareEach(xs);
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = await f(c, xs[i]);
  return $own(out);
}

async function $list_filterCtxAwait(xs, c, p) {
  $shareEach(xs);
  const out = [];
  for (let i = 0; i < xs.length; i++) if (await p(c, xs[i])) out.push(xs[i]);
  return $own(out);
}

// The six operations below are the whole of the in-place half. Each is the
// same shape: ask whether this list is ours and unshared, write through if it
// is, and otherwise copy exactly as before — where the copy is fresh, so it is
// ours, so a loop that grows a list pays for at most one copy per sharing
// event rather than one per iteration.

function $list_concat(xs, c, ys) {
  if (xs.$u === true) {
    // `ys` may be `xs`, so the length is read once before anything is added.
    const n = ys.length;
    if (n > 0 && xs[$each] === true) xs[$each] = false;
    for (let i = 0; i < n; i++) xs.push(ys[i]);
    return xs;
  }
  return $own(xs.concat(ys));
}

function $list_push(xs, c, x) {
  if (xs.$u === true) {
    if (xs[$each] === true) xs[$each] = false;
    xs.push(x);
    return xs;
  }
  const out = xs.slice();
  out.push(x);
  return $own(out);
}

function $list_reverse(xs, c) {
  if (xs.$u === true) return xs.reverse();
  return $own(xs.slice().reverse());
}

// Stable, so a tie-break the comparator does not decide keeps source order.
// `Array.prototype.sort` is stable, and `Order`'s tags are 0, 1 and 2, so the
// tag less one is the comparator's answer. A `None` is `undefined`, which the
// built-in sort moves to the end unasked, so a list holding one sorts pairs.
function $list_sortBy(xs, c, order) {
  $shareEach(xs);
  if (!xs.includes(undefined)) return $own(xs.slice().sort((a, b) => order(a, b) - 1));
  return $own(
    xs
      .map((v, i) => [v, i])
      .sort((a, b) => {
        const o = order(a[0], b[0]);
        return o === 1 ? a[1] - b[1] : o === 0 ? -1 : 1;
      })
      .map((p) => p[0]),
  );
}

// `core/map`'s splices of a node's children. Natively they write into a list
// nothing else holds; here they copy, because no plan says the receiver is
// given up.
function $map_insertAt(c, xs, at, x) {
  const k = Math.min(Math.max(0, Number(at)), xs.length);
  const out = xs.slice();
  out.splice(k, 0, x);
  return $own(out);
}

function $map_replaceAt(c, xs, at, x) {
  const k = Number(at);
  const out = xs.slice();
  if (k >= 0 && k < out.length) out[k] = x;
  return $own(out);
}

function $map_removeAt(c, xs, at) {
  const k = Number(at);
  const out = xs.slice();
  if (k >= 0 && k < out.length) out.splice(k, 1);
  return $own(out);
}

function $list_take(xs, c, n) {
  const k = Math.min(Math.max(0, Number(n)), xs.length);
  if (xs.$u === true) {
    xs.length = k;
    return xs;
  }
  return $own(xs.slice(0, k));
}

function $list_drop(xs, c, n) {
  const k = Math.min(Math.max(0, Number(n)), xs.length);
  if (xs.$u === true) {
    xs.copyWithin(0, k);
    xs.length = xs.length - k;
    return xs;
  }
  return $own(xs.slice(k));
}

function $list_slice(xs, c, a, b) {
  const lo = Math.min(Math.max(0, Number(a)), xs.length);
  const hi = Math.min(Math.max(0, Number(b)), xs.length);
  if (xs.$u === true) {
    if (hi <= lo) {
      xs.length = 0;
    } else {
      xs.copyWithin(0, lo, hi);
      xs.length = hi - lo;
    }
    return xs;
  }
  return $own(xs.slice(lo, hi));
}

function $list_zip(xs, c, ys) {
  const n = Math.min(xs.length, ys.length);
  const out = new Array(n);
  for (let i = 0; i < n; i++) out[i] = [xs[i], ys[i]];
  return $own(out);
}

function $list_flatten(xs, c) {
  const out = [];
  for (const x of xs) for (const y of x) out.push(y);
  return $own(out);
}

function $list_empty() {
  return $own([]);
}

// The counter is the element type, because the elements are what it produces.
// The loop counts in a `number`, which is cheaper to test than the `BigInt`
// it produces. Pushed rather than preallocated: `new Array(n)` is a holey
// array in V8, and every later `map` over one is slower.
function $list_range(c, a, b) {
  const out = [];
  if (!(a < b)) return $own(out);
  const n = Number(b - a);
  let v = a;
  for (let i = 0; i < n; i++) out.push(v++);
  return $own(out);
}

function $list_repeat(c, x, n) {
  const out = [];
  const k = Number(n);
  for (let i = 0; i < k; i++) out.push(x);
  return $own(out);
}

function $list_join(xs, c, sep) {
  return xs.join(sep);
}

// --- core/str -----------------------------------------------------------------
//
// `Str` is an immutable UTF-8 string, and `len` counts Unicode scalar values
// rather than UTF-8 bytes or UTF-16 code units.

function $chars(s) {
  return Array.from(s);
}

// A JavaScript string is a sequence of UTF-16 code units, and a code unit is a
// whole scalar value *unless* it is a surrogate — the astral scalars, and only
// those, are written as a surrogate pair. So a string containing no surrogate
// has exactly one scalar per code unit, its `length` is the scalar count, and
// `s[i]` is the scalar at index `i`.
//
// A string that does hold one is indexed through a table of where each scalar
// starts: entry `i` is the code unit scalar `i` begins at, and one last entry
// is `s.length`, so scalar `i` is `s.slice(t[i], t[i + 1])` and the count is
// `t.length - 1`. A pair is one scalar, and so is a lone unpaired surrogate —
// the same split `Array.from` makes.
//
// `length`, `charAt` and `slice` get asked of one long string over and over —
// the `core/buri/ast` lexer slices its whole source once per token, and the
// ordinary `for i in 0..s.length() { s.charAt(i) }` scan asks once per scalar
// — so the answer for the last long string asked about is kept: its table, or
// `null` when it holds no surrogate. Without that, every question about a
// string with one emoji in it cost a pass and an allocation as long as the
// string. The key is compared with `===`, which on strings compares the
// characters, so a different string never reads another's table; and the
// table is never handed out, so nothing but this can change it. A short string
// is cheap to measure again, and is not kept, so it cannot push out a long one.
const $surrogate = /[\uD800-\uDFFF]/;

function $wide(s) {
  return $surrogate.test(s);
}

const $startsKept = 256;
let $startsKey = "";
let $startsTable = null;

function $starts(s) {
  if (s === $startsKey) return $startsTable;
  const t = $wide(s) ? $scalarStarts(s) : null;
  if (s.length >= $startsKept) {
    $startsKey = s;
    $startsTable = t;
  }
  return t;
}

function $scalarStarts(s) {
  const n = s.length;
  const t = [];
  let i = 0;
  while (i < n) {
    t.push(i);
    const c = s.charCodeAt(i);
    const paired = c >= 0xd800 && c <= 0xdbff && i + 1 < n && (s.charCodeAt(i + 1) & 0xfc00) === 0xdc00;
    i += paired ? 2 : 1;
  }
  t.push(n);
  return t;
}

function $str_length(s) {
  const t = $starts(s);
  return $int(t === null ? s.length : t.length - 1);
}

// A character is a string and an index a bigint, so neither is ever
// `undefined`: each is its own `Some`, here and in `indexOf` and `findIndex`.
function $str_charAt(s, i) {
  const n = Number(i);
  const t = $starts(s);
  if (t === null) return n >= 0 && n < s.length ? s[n] : undefined;
  return n >= 0 && n < t.length - 1 ? s.slice(t[n], t[n + 1]) : undefined;
}

function $str_slice(s, a, b) {
  const lo = Math.max(0, Number(a));
  const hi = Math.max(0, Number(b));
  // `String.prototype.slice` clamps past the end and answers "" when the end
  // is at or before the start, and the table path clamps to the scalar count
  // and answers the same.
  const t = $starts(s);
  if (t === null) return s.slice(lo, hi);
  const m = t.length - 1;
  const l = Math.min(lo, m);
  const h = Math.min(hi, m);
  return h <= l ? "" : s.slice(t[l], t[h]);
}

function $str_trim(s) {
  return s.trim();
}

function $str_trimStart(s) {
  return s.trimStart();
}

function $str_trimEnd(s) {
  return s.trimEnd();
}

function $str_startsWith(s, p) {
  return s.startsWith(p);
}

function $str_endsWith(s, p) {
  return s.endsWith(p);
}

function $str_contains(s, n) {
  return s.includes(n);
}

function $str_indexOf(s, n) {
  const i = s.indexOf(n);
  if (i < 0) return undefined;
  // The answer is a scalar index, and `indexOf` gives a code-unit index, so
  // what has to be counted is the scalars that start before it.
  const t = $starts(s);
  if (t === null) return $int(i);
  let lo = 0;
  let hi = t.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (t[mid] < i) lo = mid + 1;
    else hi = mid;
  }
  return $int(lo);
}

// Two slices, or .None when the separator does not occur. Pure, because
// neither half is a copy. The pair is its own `Some`: a tuple is never
// `undefined`, so it needs no `$some`.
function $str_splitOnce(s, sep) {
  const i = s.indexOf(sep);
  return i < 0 ? undefined : [s.slice(0, i), s.slice(i + sep.length)];
}

// `Str.compare`, and through `Ordered` every `<`, `sort` and `OrderedMap` key order.
//
// **Unicode scalar value order**, which for a valid string is byte-for-byte
// UTF-8 order — the same answer `str::cmp` gives in Rust, `<` gives in Go and
// `<` gives in Python, and the same answer `buri_rt_str_compare` gives on the
// native backends. It is deliberately *not* JavaScript's own `<`, which
// compares UTF-16 code units: an astral scalar is a surrogate pair beginning
// at 0xD800, so `<` puts every astral character *below* every character in
// U+E000..U+FFFF, where the scalar values say the opposite. `"\u{1F600}" <
// "\u{E000}"` is the case that names it — true in JavaScript, false here.
//
// Only the first code unit the two differ at decides, so that is all this
// reads: below 0xD800 both units are whole scalars and `<` on them is scalar
// order. It used to ask first whether either string held a surrogate at all,
// which read both strings to the end on every call, and keys sorted in a map
// share long prefixes and differ early.
function $str_compare(a, b) {
  if (a === b) return 1;
  const n = a.length < b.length ? a.length : b.length;
  for (let i = 0; i < n; i++) {
    const x = a.charCodeAt(i);
    const y = b.charCodeAt(i);
    if (x === y) continue;
    if (x < 0xd800 && y < 0xd800) return x < y ? 0 : 2;
    // Re-rank the two differing code units so that they order the way the
    // scalars they encode do. A surrogate stands for something above U+FFFF,
    // so it belongs above the whole 0xE000..0xFFFF block rather than below it;
    // sliding that block down by 0x800 and the surrogates up by 0x2000 puts
    // 0x0000..0xD7FF, then 0xD800..0xF7FF (was 0xE000..0xFFFF), then
    // 0xF800..0xFFFF (the surrogates) in one increasing run. Two surrogates at
    // the same index are either both leads or both trails — the units before
    // them are equal — so comparing them shifted is comparing the scalars.
    const p = x >= 0xe000 ? x - 0x800 : x >= 0xd800 ? x + 0x2000 : x;
    const q = y >= 0xe000 ? y - 0x800 : y >= 0xd800 ? y + 0x2000 : y;
    return p < q ? 0 : 2;
  }
  // One is a prefix of the other, in code units and so in scalars too.
  return a.length < b.length ? 0 : a.length > b.length ? 2 : 1;
}

const $i64Min = -(2n ** 63n);
const $i64Max = 2n ** 63n - 1n;

function $str_toInt(s) {
  const t = s.trim();
  if (!/^[+-]?\d+$/.test(t)) return undefined;
  try {
    // `Int` is `I64` and holds its whole range here, so the only string this
    // refuses is one naming a number that is not an `I64` at all.
    const v = BigInt(t);
    if (v < $i64Min || v > $i64Max) return undefined;
    return $some(v);
  } catch {
    return undefined;
  }
}

function $str_toFloat(s) {
  const t = s.trim();
  if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t)) return undefined;
  return $some(Number(t));
}

function $str_concat(s, c, o) {
  return s + o;
}

function $str_split(s, c, sep) {
  return sep === "" ? $chars(s) : s.split(sep);
}

function $str_splitAny(s, c, seps) {
  const set = new Set($chars(seps));
  const out = [];
  let cur = "";
  for (const ch of s) {
    if (set.has(ch)) {
      if (cur) out.push(cur);
      cur = "";
    } else cur += ch;
  }
  if (cur) out.push(cur);
  return out;
}

function $str_lines(s, c) {
  return s.split("\n");
}

function $str_replace(s, c, a, b) {
  return a === "" ? s : s.split(a).join(b);
}

function $str_repeat(s, c, n) {
  return s.repeat(Math.max(0, Number(n)));
}

function $str_toUpper(s, c) {
  return s.toUpperCase();
}

function $str_toLower(s, c) {
  return s.toLowerCase();
}

function $str_chars(s, c) {
  return $chars(s);
}

function $str_fromChars(c, cs) {
  return cs.join("");
}

function $str_fromInt(c, n) {
  return n.toString();
}

function $str_fromFloat(c, x) {
  return $f64(x);
}

function $str_padStart(s, c, w, fill) {
  const n = Number(w) - Number($str_length(s));
  return n > 0 ? fill.repeat(n) + s : s;
}

function $str_padEnd(s, c, w, fill) {
  const n = Number(w) - Number($str_length(s));
  return n > 0 ? s + fill.repeat(n) : s;
}

// --- core/character -------------------------------------------------------------

function $character_isDigit(c) {
  return c >= "0" && c <= "9";
}

function $character_isAlpha(c) {
  return /^\p{L}$/u.test(c);
}

function $character_isSpace(c) {
  return /^\s$/u.test(c);
}

function $character_isUpper(c) {
  return c !== c.toLowerCase() && c === c.toUpperCase();
}

function $character_isLower(c) {
  return c !== c.toUpperCase() && c === c.toLowerCase();
}

function $character_toLower(c) {
  return c.toLowerCase();
}

function $character_toUpper(c) {
  return c.toUpperCase();
}

function $character_toU32(c) {
  return c.codePointAt(0);
}

function $character_toDigit(c, radix) {
  const n = parseInt(c, Number(radix));
  return Number.isNaN(n) ? undefined : $some(BigInt(n));
}

// --- core/math --------------------------------------------------------------------

const $math_squareRoot = Math.sqrt;
const $math_cubeRoot = Math.cbrt;
const $math_power = Math.pow;
const $math_exp = Math.exp;
const $math_ln = Math.log;
const $math_log10 = Math.log10;
const $math_log2 = Math.log2;
const $math_sin = Math.sin;
const $math_cos = Math.cos;
const $math_tan = Math.tan;
const $math_asin = Math.asin;
const $math_acos = Math.acos;
const $math_atan = Math.atan;
const $math_atan2 = Math.atan2;
const $math_floor = Math.floor;
const $math_ceiling = Math.ceil;
const $math_round = Math.round;
const $math_truncate = Math.trunc;
const $math_absoluteFloat = Math.abs;
const $math_isNan = Number.isNaN;
function $math_isInfinite(x) {
  return x === Infinity || x === -Infinity;
}
const $math_isFinite = Number.isFinite;

// --- core/bits ----------------------------------------------------------------------
//
// Shifting by a count at or beyond the width of the type is a crash, the same
// way overflow is.

// `core/bits` is declared over `Int`, which is a `BigInt` here, so the shifts
// are the operators themselves. The narrow unsigned forms below take a
// `number` and give one back. Shifting by a count at or beyond the width of
// the type aborts.
function $shiftCount(n, bits) {
  const k = Number(n);
  if (k < 0 || k >= bits) $abort("shift out of range");
  return $ints[k];
}


function $big(x) {
  return BigInt(Math.trunc(x));
}

// `$big` for a value that may already be one: the `U64` entries below are handed
// a `BigInt`, and `Math.trunc` throws on those.
function $toBig(x) {
  return typeof x === "bigint" ? x : BigInt(Math.trunc(x));
}

// --- Narrow unsigned bitwise ---------------------------------------------------------
//
// JavaScript's bitwise operators produce a *signed* 32-bit result, so
// `0x80000000 | 0` came back as `-2147483648` and `~0` on a `U8` came back as
// `-1` instead of `255`. The operands are in range and the answer is in range;
// only the representation was wrong, so narrowing the result to the type's own
// width is the whole fix.
//
// The wide types need none of this: they are `BigInt`s, and the backend emits
// the operator itself.
function $umask(v, bits) {
  return bits >= 32 ? v >>> 0 : v & ((1 << bits) - 1);
}

function $bits_shiftLeft(x, n) {
  return BigInt.asIntN(64, x << $shiftCount(n, 64));
}

function $bits_shiftRight(x, n) {
  // Logical: reinterpret as unsigned, shift, then narrow back, so a shift by
  // zero is the identity rather than the unsigned reinterpretation.
  return BigInt.asIntN(64, BigInt.asUintN(64, x) >> $shiftCount(n, 64));
}

function $bits_shiftRightArithmetic(x, n) {
  return x >> $shiftCount(n, 64);
}

// The three counts read the word as two 32-bit halves of `number`s: walking a
// `BigInt` a bit at a time allocates at every step.
function $pop32(n) {
  n -= (n >>> 1) & 0x55555555;
  n = (n & 0x33333333) + ((n >>> 2) & 0x33333333);
  return (Math.imul((n + (n >>> 4)) & 0x0f0f0f0f, 0x01010101) >>> 24);
}

function $ctz32(n) {
  return n === 0 ? 32 : 31 - Math.clz32(n & -n);
}

function $bits_popCount(x) {
  const v = BigInt.asUintN(64, x);
  if (v <= 0xffffffffn) return $ints[$pop32(Number(v))];
  return $ints[$pop32(Number(v & 0xffffffffn)) + $pop32(Number(v >> 32n))];
}

function $bits_leadingZeros(x) {
  const v = BigInt.asUintN(64, x);
  const hi = Number(v >> 32n);
  return $ints[hi !== 0 ? Math.clz32(hi) : 32 + Math.clz32(Number(v & 0xffffffffn))];
}

function $bits_trailingZeros(x) {
  const v = BigInt.asUintN(64, x);
  const lo = Number(v & 0xffffffffn);
  return $ints[lo !== 0 ? $ctz32(lo) : 32 + $ctz32(Number(v >> 32n))];
}

function $bits_rotateLeft(x, n) {
  const k = $shiftCount(n, 64);
  const v = BigInt.asUintN(64, x);
  return BigInt.asIntN(64, (v << k) | (v >> (64n - k)));
}

function $bits_rotateRight(x, n) {
  const k = $shiftCount(n, 64);
  const v = BigInt.asUintN(64, x);
  return BigInt.asIntN(64, (v >> k) | (v << (64n - k)));
}

// The narrow widths, where the value is a `number` and only the count is not.
function $bits_shiftLeftU8(x, n) {
  return Number(BigInt.asUintN(8, $big(x) << $shiftCount(n, 8)));
}
function $bits_shiftRightU8(x, n) {
  return Number($big(x) >> $shiftCount(n, 8));
}
function $bits_shiftLeftU32(x, n) {
  return Number(BigInt.asUintN(32, $big(x) << $shiftCount(n, 32)));
}
function $bits_shiftRightU32(x, n) {
  return Number($big(x) >> $shiftCount(n, 32));
}
function $bits_shiftLeftU64(x, n) {
  return BigInt.asUintN(64, x << $shiftCount(n, 64));
}
function $bits_shiftRightU64(x, n) {
  return x >> $shiftCount(n, 64);
}

// The rotates at the three unsigned widths. Each wraps inside its **own** width,
// so the count is checked against that width and the value is masked to it.
// `x << k | x >> (w - k)` is the whole rotate; at `k === 0` the second shift is
// by the full width, which a BigInt handles as a plain shift rather than as the
// undefined behaviour a machine word would have.
function $rotate(x, n, bits, left) {
  const k = $shiftCount(n, bits);
  const v = BigInt.asUintN(bits, $toBig(x));
  const w = $ints[bits];
  const spun = left ? (v << k) | (v >> (w - k)) : (v >> k) | (v << (w - k));
  return BigInt.asUintN(bits, spun);
}

function $bits_rotateLeftU8(x, n) {
  return Number($rotate(x, n, 8, true));
}
function $bits_rotateRightU8(x, n) {
  return Number($rotate(x, n, 8, false));
}
function $bits_rotateLeftU32(x, n) {
  return Number($rotate(x, n, 32, true));
}
function $bits_rotateRightU32(x, n) {
  return Number($rotate(x, n, 32, false));
}
function $bits_rotateLeftU64(x, n) {
  return $rotate(x, n, 64, true);
}
function $bits_rotateRightU64(x, n) {
  return $rotate(x, n, 64, false);
}

// The byte reversals. One byte at a time from the bottom, which is what a
// `bswap` instruction does and what a BigInt can say without a typed array.
function $swapBytes(x, bytes) {
  let v = BigInt.asUintN(bytes * 8, $toBig(x));
  let out = 0n;
  for (let i = 0; i < bytes; i++) {
    out = (out << 8n) | (v & 0xffn);
    v >>= 8n;
  }
  return out;
}

function $bits_byteSwapU32(x) {
  return Number($swapBytes(x, 4));
}
function $bits_byteSwapU64(x) {
  return $swapBytes(x, 8);
}

// The three counts at `U64`. The signed counts read the word unsigned anyway,
// so they take a `U64` as it is.
function $bits_popCountU64(x) {
  return $bits_popCount($toBig(x));
}
function $bits_leadingZerosU64(x) {
  return $bits_leadingZeros($toBig(x));
}
function $bits_trailingZerosU64(x) {
  return $bits_trailingZeros($toBig(x));
}

// --- Conversions ---------------------------------------------------------------------

function $ok(x) {
  return [0, x];
}

function $err(x) {
  return [1, x];
}

// --- Bytes -----------------------------------------------------------------------------
//
// A `[U8]` is an ordinary array of numbers, like every other list. These two
// are intrinsics rather than Buri because the encoding lives in the platform:
// a JavaScript string is UTF-16, and turning one into UTF-8 bytes is the
// engine's job, not something to reimplement on top of `charAt`.

function $bytes_toUtf8(_c, s) {
  const out = [];
  for (const ch of s) {
    const cp = ch.codePointAt(0);
    if (cp < 0x80) {
      out.push(cp);
    } else if (cp < 0x800) {
      out.push(0xc0 | (cp >> 6), 0x80 | (cp & 0x3f));
    } else if (cp < 0x10000) {
      out.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
    } else {
      out.push(
        0xf0 | (cp >> 18),
        0x80 | ((cp >> 12) & 0x3f),
        0x80 | ((cp >> 6) & 0x3f),
        0x80 | (cp & 0x3f),
      );
    }
  }
  return out;
}

// Strict: an overlong encoding, a truncated sequence, or a surrogate is an
// error rather than a replacement character. Bytes that are not text should
// say so, not decode to `�` and be discovered three layers later.
function $bytes_fromUtf8(_c, b) {
  let out = "";
  let i = 0;
  while (i < b.length) {
    const c = b[i] & 0xff;
    let cp;
    let n;
    if (c < 0x80) {
      cp = c;
      n = 0;
    } else if ((c & 0xe0) === 0xc0) {
      cp = c & 0x1f;
      n = 1;
    } else if ((c & 0xf0) === 0xe0) {
      cp = c & 0x0f;
      n = 2;
    } else if ((c & 0xf8) === 0xf0) {
      cp = c & 0x07;
      n = 3;
    } else {
      return $err([BigInt(i)]);
    }
    // The continuation bytes have to be there. A tuple struct is an array
    // of its fields, so a `Utf8Error(i)` is `[i]`.
    if (n > 0 && i + n >= b.length) return $err([BigInt(i)]);
    for (let k = 1; k <= n; k++) {
      const cc = b[i + k] & 0xff;
      if ((cc & 0xc0) !== 0x80) return $err([BigInt(i)]);
      cp = (cp << 6) | (cc & 0x3f);
    }
    const min = n === 0 ? 0 : n === 1 ? 0x80 : n === 2 ? 0x800 : 0x10000;
    if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return $err([BigInt(i)]);
    out += String.fromCodePoint(cp);
    i += n + 1;
  }
  return $ok(out);
}

// The IEEE 754 byte patterns, little-endian. Intrinsics for the same reason
// the UTF-8 pair above are: the bit pattern of a double belongs to the
// platform, and reconstructing it from arithmetic would be a second definition
// of the same thing. `Option<T>` is the value or `undefined`, so a short input
// simply returns nothing.
//
// A decoded NaN is the canonical quiet NaN, as SPEC 6.2 promises. The engine
// doesn't do this: JavaScriptCore canonicalizes a NaN read out of a buffer but
// V8 keeps the payload, and writes it back out on `setFloat64`. So the
// decoders below replace every NaN with the `NaN` literal themselves.
const $f64buf = new DataView(new ArrayBuffer(8));

function $canonicalNan(x) {
  return x !== x ? NaN : x;
}

function $bytes_f64ToBytes(_c, x) {
  $f64buf.setFloat64(0, x, true);
  const out = [];
  for (let i = 0; i < 8; i++) out.push($f64buf.getUint8(i));
  return out;
}

function $bytes_f64FromBytes(b, where) {
  const at = Number(where);
  if (at < 0 || at + 8 > b.length) return undefined;
  for (let i = 0; i < 8; i++) $f64buf.setUint8(i, b[at + i] & 0xff);
  return $canonicalNan($f64buf.getFloat64(0, true));
}

function $bytes_f32ToBytes(_c, x) {
  $f64buf.setFloat32(0, x, true);
  const out = [];
  for (let i = 0; i < 4; i++) out.push($f64buf.getUint8(i));
  return out;
}

function $bytes_f32FromBytes(b, where) {
  const at = Number(where);
  if (at < 0 || at + 4 > b.length) return undefined;
  for (let i = 0; i < 4; i++) $f64buf.setUint8(i, b[at + i] & 0xff);
  return $canonicalNan($f64buf.getFloat32(0, true));
}

// A RangeError is a struct { value: Str, target: Str }.
// The message quotes the value as it was written, so the rendering follows the
// source type rather than the runtime one: every number is a `number` here.
function $rangeErr(v, t, flt) {
  return $err([flt ? $f64(v) : String(v), t]);
}

// `lo` and `hi` are the target's own range, written in the target's own
// representation, so an `.Ok` is always the value that was converted — never
// one that merely rounded into range. A comparison between a `number` and a
// `BigInt` is exact in JavaScript, so the two sides need not match.
//
// `big` says which representation the answer is in, which is the target's and
// not the source's: `9007199254740993` is an `I128` a double cannot hold and an
// `I64` that a `BigInt` can.
function $convChecked(v, lo, hi, target, flt, big) {
  // `isInteger` is false for NaN and for both infinities, so this is the
  // finiteness test as well. A `BigInt` source is an integer by construction.
  if (typeof v === "number" && !Number.isInteger(v)) return $rangeErr(v, target, flt);
  if (v < lo || v > hi) return $rangeErr(v, target, flt);
  return $ok(big ? BigInt(v) : Number(v));
}

// Not every U32 is a Unicode scalar value: the surrogate range and anything
// above U+10FFFF have no `Char`.
function $toChar(n) {
  const v = n;
  if (v > 0x10ffff || (v >= 0xd800 && v <= 0xdfff)) return $rangeErr(n, "Char", false);
  return $ok(String.fromCodePoint(v));
}

// `F64 -> F32` rounds to binary32, and fails when the value does not survive
// as a finite one.
function $convF32(v, target) {
  if (Number.isNaN(v)) return $ok(v);
  const r = Math.fround(v);
  if (!Number.isFinite(r) && Number.isFinite(v)) return $rangeErr(v, target, true);
  return $ok(r);
}

// --- The platform --------------------------------------------------------------------
//
// **The boundary rule.** Everything past this line is a JavaScript API, and a
// JavaScript API counts in `number`s: a file descriptor, a byte count, a
// millisecond, a node id. So an `Int` handed to one is narrowed with `Number`
// at the call, and an `Int` handed back is widened with `BigInt` — the
// conversion happens here, at the edge, and never in the middle of a program.
// The one place the narrowing could lose something is a byte count above 2^53,
// which no allocator on the other side of it would honour anyway.

// Output is buffered, so the write a program *calls* and the write the platform
// *performs* are two different moments: `println` fills `out`, and only a full
// buffer — or the exit path — reaches the descriptor. A failure discovered then
// belongs to some earlier print, and the honest thing a buffered stream can
// promise is that it reaches the next caller rather than being dropped. So it
// is held in `pending` and answered by the next write, which is what makes
// `Result<(), IoError>` a claim this runtime can keep. `cli/runtime/host.rs`'s
// `PENDING` is the same mechanism on the other backend.
//
// The first failure wins: a closed pipe fails every write after the first, and
// the one worth reporting is the one that says what went wrong before the
// program was writing into nothing.
const $host = {
  out: [],
  err: [],
  pending: null,
  note(e) {
    if (this.pending === null) this.pending = e;
  },
  // The `Result` arm the next write answers with, and the point at which a held
  // failure stops being held.
  reported() {
    const e = this.pending;
    this.pending = null;
    return e === null ? $ok(0) : $err($ioErr(e));
  },
  flush() {
    if (this.out.length) {
      const text = this.out.join("");
      this.out = [];
      try {
        $write(1, text);
      } catch (e) {
        this.note(e);
      }
    }
    if (this.err.length) {
      const text = this.err.join("");
      this.err = [];
      try {
        $write(2, text);
      } catch (e) {
        this.note(e);
      }
    }
  },
};

// **What was printed goes out before the program waits.** The buffer above
// batches a *run of consecutive prints* and nothing longer: a sleep, a read of
// standard input, a `fetch`, a mailbox with no room and a child being waited on
// are each a moment somebody reading a redirected log is entitled to what the
// program has already said. Without it a server's "listening" line sat in the
// buffer for the whole life of the process (buri-lang/buri#66).
//
// `cli/runtime/host.rs`'s `about_to_block` is the same rule on the native
// backend, kept at the same places, which is what keeps the two backends'
// output one promise rather than two.
function $aboutToBlock() {
  $host.flush();
}

// **Synchronous, wherever the platform has a descriptor to write to.** Every
// asynchronous writer a JavaScript host offers — `Bun.stdout.write`,
// `process.stdout.write` on a pipe — hands the text to the event loop and
// answers before it has landed, and `process.exit` does not wait for the loop.
// So an exit truncated whatever was still in flight: `Bun.stdout.write` to a
// file dropped it whole, and `process.stdout.write` to a pipe kept the first
// sixty-four kilobytes and lost the rest (buri-lang/buri#37,
// buri-lang/buri#42). A flush a following `process.exit` may discard is not a
// flush, and the buffer above exists precisely so that the exit path can empty
// it.
//
// `writeSync` on the descriptor is the one writer with no queue behind it. A
// browser has no descriptor and no `process.exit` either, so nothing there can
// be truncated and the asynchronous writer is what it keeps.
function $write(fd, s) {
  const fs = $fsOrNull();
  if (fs !== null) {
    $writeAll(fs, fd, typeof Buffer !== "undefined" ? Buffer.from(s, "utf8") : new TextEncoder().encode(s));
  } else if (typeof process !== "undefined") {
    (fd === 1 ? process.stdout : process.stderr).write(s);
  } else {
    (fd === 1 ? console.log : console.error)(s);
  }
}

// One buffer, written whole. `write(2)` may take less than it was offered, and
// a descriptor someone else put in non-blocking mode answers `EAGAIN` rather
// than waiting — neither is a failure, and both would silently lose text if the
// count came back unchecked. Anything else throws, and the caller holds it as
// the stream's `pending` failure.
function $writeAll(fs, fd, buf) {
  let at = 0;
  while (at < buf.length) {
    try {
      at += fs.writeSync(fd, buf, at, buf.length - at);
    } catch (e) {
      if (!e || (e.code !== "EAGAIN" && e.code !== "EWOULDBLOCK")) throw e;
    }
  }
}

// What a program's `main` answered, and what that means for the process.
// `.Ok(())` exits 0; `.Err(msg)` prints `msg` on stderr and exits 1. The
// message joins the stream's buffer rather than jumping the queue, so a
// program's own stderr and the runtime's last word arrive in the order they
// were written — and both arrive, which they did not while the exit went
// through an asynchronous write (buri-lang/buri#42).
function $done(r) {
  if (r[0] === 0) {
    $host.flush();
    return;
  }
  $host.err.push($str(r[1]) + "\n");
  $exit(1);
}

// The same, for a failure the program had no name for: an abort, or anything
// else that reached the top without being a `Result`.
function $failed(e) {
  $host.err.push((e && e.message ? e.message : String(e)) + "\n");
  if (e && e.stack) $host.err.push(e.stack + "\n");
  $exit(1);
}

// The one exit, and the only place `process.exit` is spelled. Flushing first is
// what makes the status and the output arrive together: `$write` above has had
// the queue taken out from under it, so by the time this returns to the host
// there is nothing left in flight to lose.
function $exit(code) {
  $host.flush();
  if (typeof process !== "undefined") process.exit(code);
}

// The platform's allocator: unbounded, and it counts nothing. `core/alloc`'s
// three count; this one is what a program gets when it asks for none of them,
// and it is a `Region` of the bytes requested (`effect.buri`'s cost model, last
// row).
function $host_HostAllocator_allocate(self, n) {
  return [Number(n)];
}

// `Region` is one number, and `core/alloc` hands its handle around as an
// `I64`, so the two cross the boundary in opposite directions.

// --- core/alloc, the counting allocators ------------------------------------
//
// One counter behind `GeneralPurpose`, `Arena` and `FixedBuffer`. The state is
// here rather than in the struct because Buri has no mutation, exactly as the
// test platform's handles below; the struct carries the index.
//
// The charges are the *defined* ones (`effect.buri`), so these numbers are the
// numbers `cli/runtime/memory.rs` produces for the same program. That is what
// makes a count meaningful on a backend with a garbage collector under it:
// nothing here is measured, on either backend.
const $alloc = { c: [] };

function $alloc_newCounter(budget) {
  $alloc.c.push({ n: 0, bytes: 0, budget: Number(budget) });
  return BigInt($alloc.c.length - 1);
}

// A budget is checked *before* the charge lands, and exceeding it ends the
// process: `allocate` answers `Region` and not `Result`, so there is no value
// to report the failure with (SPEC 6.9, MEMORY.md §7.2). The message is
// `cli/runtime/abort.rs`'s, word for word.
function $alloc_charge(h, bytes) {
  const c = $alloc.c[Number(h)];
  const n = Number(bytes);
  if (c.budget >= 0 && c.bytes + n > c.budget) {
    $abort(
      "allocation budget exhausted: " +
        n +
        " bytes requested against a budget of " +
        c.budget,
    );
  }
  c.n += 1;
  c.bytes += n;
  return BigInt(n);
}

function $alloc_count(h) {
  return BigInt($alloc.c[Number(h)].n);
}

function $alloc_total(h) {
  return BigInt($alloc.c[Number(h)].bytes);
}

// --- core/alloc's scope (G4) -------------------------------------------------
//
// `Scoped<C>` forwards every effect but `Allocator`, and its `Allocator` charges the
// arena named by these handles. What the *native* runtime does on top of this
// — reserve the bytes as anonymous pages and `munmap` them at release — has no
// counterpart here and needs none: this backend has a garbage collector under
// it, and `core/alloc`'s whole doctrine is that a charge is a definition rather
// than a measurement. So the two backends agree on every number a program can
// observe, which is `stats()`, and disagree only about pages, which no program
// can ask about.
//
// A handle is `(generation << 32) | slot`, `cli/runtime/memory.rs`'s packing,
// because the generation is *observable*: a `Scoped` that outlives its scope
// charges nothing, and that has to be true here too.
const $arena = { a: [], free: [] };

function $arena_slot(h) {
  const bits = BigInt.asUintN(64, BigInt(h));
  return [Number(bits & 0xffffffffn), Number(bits >> 32n)];
}

function $arena_handle(slot, gen) {
  return BigInt.asIntN(64, (BigInt(gen) << 32n) | BigInt(slot));
}

function $alloc_arenaCreate() {
  const reused = $arena.free.pop();
  if (reused !== undefined) {
    const a = $arena.a[reused];
    a.n = 0;
    a.bytes = 0;
    a.live = true;
    return $arena_handle(reused, a.gen);
  }
  $arena.a.push({ n: 0, bytes: 0, gen: 0, live: true });
  return $arena_handle($arena.a.length - 1, 0);
}

function $alloc_arenaAllocate(h, bytes) {
  const [slot, gen] = $arena_slot(h);
  const a = $arena.a[slot];
  if (a !== undefined && a.live && a.gen === gen) {
    a.n += 1;
    a.bytes += Number(bytes);
  }
  return BigInt(bytes);
}

// Answers the bytes given back, which is zero on a backend that maps none.
// `scoped` discards it on both backends, so nothing a program can see turns on
// the number.
function $alloc_arenaRelease(h) {
  const [slot, gen] = $arena_slot(h);
  const a = $arena.a[slot];
  if (a !== undefined && a.live && a.gen === gen) {
    a.live = false;
    a.gen += 1;
    $arena.free.push(slot);
  }
  return 0n;
}

function $alloc_arenaCount(h) {
  const [slot, gen] = $arena_slot(h);
  const a = $arena.a[slot];
  return a !== undefined && a.gen === gen ? BigInt(a.n) : 0n;
}

function $alloc_arenaTotal(h) {
  const [slot, gen] = $arena_slot(h);
  const a = $arena.a[slot];
  return a !== undefined && a.gen === gen ? BigInt(a.bytes) : 0n;
}

// --- core/alloc's copy out of a scope (G5) -----------------------------------
//
// **On this backend a copy-out is the identity, and that is the honest answer
// rather than a stub.** The three things `copyOut` exists to do natively are
// all facts about a reference-counted heap with a bump allocator beside it:
//
//   * a value in the arena would dangle when the pages went back — there are no
//     pages here, `$alloc_arenaRelease` unmaps nothing, and a value the body
//     built is kept alive by the reference the caller holds to it, which is
//     what a garbage collector *is*;
//   * a copy must not be a share, so that the original's counts stay put —
//     there are no counts here, because `middle/mod.rs`'s pipeline does not run
//     `rc` for this backend;
//   * a copy must be freshly *unique*, so that a later in-place write is
//     licensed — but uniqueness here is the sticky `$u` bit, which the value
//     already carries or already lacks, and duplicating it would change nothing
//     about the answer while making every scope cost the size of its result.
//
// So the two backends agree on every number and every value a program can
// observe, and disagree only about pages and blocks, which no program can ask
// about — the same division `$alloc_arenaRelease` above already draws.
//
// Entering and leaving a scope go the same way: there is one allocator here,
// and nothing to switch it to.
function $alloc_copyOut(v) {
  return v;
}

function $alloc_arenaEnter(h) {
  return h;
}

function $alloc_arenaLeave(previous) {
  return previous;
}

function $host_HostStdout_print(self, t) {
  $host.out.push(t);
  if ($host.out.length > 64) $host.flush();
  return $host.reported();
}

function $host_HostStdout_println(self, t) {
  $host.out.push(t + "\n");
  if ($host.out.length > 64) $host.flush();
  return $host.reported();
}

// Octets, written through unchanged. The buffered text stream is flushed
// first, so the two orderings a program can see are the one it wrote.
//
// Unbuffered, so unlike the four text writers this one can answer its *own*
// failure — and it reports a held one first, because that failure is older.
function $host_HostStdout_writeBytes(self, b) {
  $host.flush();
  try {
    $writeRaw(1, b);
  } catch (e) {
    $host.note(e);
  }
  return $host.reported();
}

function $writeRaw(fd, bytes) {
  // Bun's stdout writer is async; `writeSync` on the file descriptor is not,
  // and a protocol that answers a request has to have answered before it
  // reads the next one.
  const buf = typeof Buffer !== "undefined" ? Buffer.from(bytes) : Uint8Array.from(bytes);
  $writeAll($fs(), fd, buf);
}

function $host_HostStderr_eprint(self, t) {
  $host.err.push(t);
  return $host.reported();
}

function $host_HostStderr_eprintln(self, t) {
  $host.err.push(t + "\n");
  return $host.reported();
}

// --- Standard input ---------------------------------------------------------
//
// Both readers are `async`, and the event loop is what waits: a program
// blocked on input is a program something else can run inside. What was here
// before was `readSync` on the descriptor with a `continue` on `EAGAIN` — a
// non-blocking pipe answers that until it has something to say — which burned
// a core for the whole of a wait and let nothing else happen during one.
//
// `process.stdin` in paused mode is what replaces it: `read()` takes what has
// already arrived, and `readable`/`end` say when to ask again. Node and Bun
// both implement it; a browser has no standard input at all, and no platform
// that grants `Stdin` is a browser (`standard_library`'s grant table), so the
// failure there is a refusal rather than a wrong answer.
//
// The buffer is a queue of chunks rather than one growing `Buffer`, because
// `readBytes` is a framed protocol's reader: a thousand four-byte headers off
// the front of one megabyte must not each copy the megabyte.
const $stdin = { chunks: [], at: 0, size: 0, ended: false };

function $stdinStream() {
  if (typeof process === "undefined" || !process.stdin) {
    $abort("this platform grants no standard input");
  }
  return process.stdin;
}

// The next chunk, or `null` at end of input.
//
// The listeners come off before the promise settles, and the stream is paused
// with them: a reader that has stopped asking must not hold the event loop
// open, or a program that read to the end would never exit.
function $stdinChunk() {
  const s = $stdinStream();
  const first = s.read();
  if (first !== null && first !== undefined) return Promise.resolve(first);
  if (s.readableEnded) return Promise.resolve(null);
  return new Promise((resolve) => {
    const settle = (v) => {
      s.off("readable", onReadable);
      s.off("end", onDone);
      s.off("error", onDone);
      s.pause();
      resolve(v);
    };
    // `readable` fires when there *may* be something; `read()` still answers
    // null when there is not, and then the next one is waited for.
    const onReadable = () => {
      const c = s.read();
      if (c !== null && c !== undefined) settle(c);
    };
    const onDone = () => settle(null);
    s.on("readable", onReadable);
    s.on("end", onDone);
    s.on("error", onDone);
  });
}

// One chunk into the queue, or the end of input recorded. Both readers ask
// `$stdin.ended` afterwards rather than reading an answer from here, because
// what they do about it differs.
async function $stdinPull() {
  const c = await $stdinChunk();
  if (c === null) {
    $stdin.ended = true;
  } else if (c.length) {
    $stdin.chunks.push(c);
    $stdin.size += c.length;
  }
}

// Exactly `n` octets off the front of the queue, which the caller has already
// established are there.
function $stdinTake(n) {
  const parts = [];
  let left = n;
  while (left > 0) {
    const head = $stdin.chunks[0];
    const avail = head.length - $stdin.at;
    if (avail > left) {
      parts.push(head.subarray($stdin.at, $stdin.at + left));
      $stdin.at += left;
      left = 0;
    } else {
      parts.push(head.subarray($stdin.at));
      $stdin.chunks.shift();
      $stdin.at = 0;
      left -= avail;
    }
  }
  $stdin.size -= n;
  return parts.length === 1 ? parts[0] : Buffer.concat(parts);
}

// The offset of the first newline in the queue, or -1. A line boundary is a
// byte boundary — no octet of a multi-byte character is 0x0A — so cutting
// here and decoding after is safe across a chunk that split one.
function $stdinNewline() {
  let seen = 0;
  for (let i = 0; i < $stdin.chunks.length; i++) {
    const c = $stdin.chunks[i];
    const from = i === 0 ? $stdin.at : 0;
    const at = c.indexOf(10, from);
    if (at >= 0) return seen + (at - from);
    seen += c.length - from;
  }
  return -1;
}

// A line at a time rather than the whole stream at once, which is the other
// half of what the spin cost: a reader that has to see end of input before it
// answers its first line cannot hold up one end of a conversation.
async function $host_HostStdin_readLine(self) {
  $aboutToBlock();
  for (;;) {
    const at = $stdinNewline();
    if (at >= 0) {
      const line = $stdinTake(at).toString("utf8");
      $stdinTake(1);
      return $some(line);
    }
    // What is left when the stream ends is a last line without one, and
    // nothing left is end of input.
    if ($stdin.ended) {
      return $stdin.size === 0 ? undefined : $some($stdinTake($stdin.size).toString("utf8"));
    }
    await $stdinPull();
  }
}

// Exactly `n` octets, waiting until they arrive. A short read at end of input
// yields what it got, or nothing at all.
async function $host_HostStdin_readBytes(self, want) {
  $aboutToBlock();
  const n = Number(want);
  if (n <= 0) return [];
  while ($stdin.size < n && !$stdin.ended) await $stdinPull();
  const got = Math.min(n, $stdin.size);
  if (got === 0) return undefined;
  return Array.from($stdinTake(got));
}

// `IoError` has a variant with a payload (`Other(Str)`), so every value of it
// is `[tag, ...payload]` — a bare tag is only the representation when *no*
// variant carries anything.
function $ioErr(e) {
  const c = e && e.code;
  if (c === "ENOENT") return [0];
  if (c === "EACCES" || c === "EPERM") return [1];
  if (c === "EROFS") return [2];
  if (c === "EEXIST") return [3];
  if (c === "ENOTDIR") return [4];
  if (c === "EXDEV") return [5];
  return [6, String((e && e.message) || e)];
}

// UTF-8 with U+FFFD for what is not, which is what `readFileSync(p, "utf8")`
// does and what the native runtime's `String::from_utf8_lossy` does.
function $utf8Lossy(b) {
  return new TextDecoder().decode(Uint8Array.from(b));
}

// `require` does not exist in an ES module on node, so the backend emits a
// `createRequire` prologue when — and only when — a program actually reaches
// one of the two modules below. A program whose `main` binds neither `FileSystem` nor
// `Stdout.writeBytes` never gets one.
//
// The synchronous half answers the two writers that must not wait:
// `$writeRaw`, behind `Stdout.writeBytes`, because a protocol that answers a
// request has to have answered before it reads the next one; and `$write`,
// because an exit does not wait for a queue.
function $fs() {
  const fs = $fsOrNull();
  if (fs === null) $abort("this platform grants no filesystem");
  return fs;
}

// The same, for the caller that has something else to do when there is none.
// `typeof` rather than a bare read, because `$require` is a `const` the backend
// emits only for a platform that can have one.
function $fsOrNull() {
  if (typeof $require === "function") return $require("fs");
  if (typeof require === "function") return require("fs");
  return null;
}

// The filesystem every `FileSystem` method reaches: `node:fs/promises`, so that a read
// is a wait rather than a stall. **Node and Bun only.** A browser has no such
// module and no browser platform grants `FileSystem` (`standard_library`'s grant
// table), so the abort below is unreachable from a `WEB` artifact and is what
// a mis-grant would say out loud rather than silently.
// A `Path` is a one-field struct, and generated code represents a struct as an
// array of its fields — so what arrives here is `[text]` and what node wants is
// the text. Unwrapping is the whole of the conversion, because `core/path`
// normalized the spelling before the value became a `Path` (VALUE-MODEL.md §5
// on the native side flattens the same one field to the same three C
// parameters a `Str` was, which is why `cli/runtime/host.rs` needed no edit at
// all for this).
function $osPath(p) {
  return p[0];
}

function $fsp() {
  if (typeof $require === "function") return $require("node:fs/promises");
  if (typeof require === "function") return require("node:fs/promises");
  $abort("this platform grants no filesystem");
}

async function $host_HostFileSystem_readFile(self, at) {
  const p = $osPath(at);
  try {
    return $ok(await $fsp().readFile(p, "utf8"));
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFileSystem_writeFile(self, at, b) {
  const p = $osPath(at);
  try {
    await $fsp().writeFile(p, b);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `access` rather than a `stat`: the question is whether the name resolves,
// and the answer to every failure is the same `false`.
async function $host_HostFileSystem_fileExists(self, at) {
  const p = $osPath(at);
  try {
    await $fsp().access(p);
    return true;
  } catch {
    return false;
  }
}

async function $host_HostFileSystem_readDir(self, at) {
  const p = $osPath(at);
  try {
    return $ok(await $fsp().readdir(p));
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFileSystem_readFileBytes(self, at) {
  const p = $osPath(at);
  try {
    return $ok(Array.from(await $fsp().readFile(p)));
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFileSystem_writeFileBytes(self, at, b) {
  const p = $osPath(at);
  try {
    await $fsp().writeFile(p, Uint8Array.from(b));
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `"a"` is `O_APPEND | O_CREAT`, so the position is taken and the octets
// written as one operation and the file appears when it was absent.
async function $host_HostFileSystem_appendFile(self, at, b) {
  const p = $osPath(at);
  try {
    await $fsp().appendFile(p, Uint8Array.from(b));
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFileSystem_renameFile(self, source, destination) {
  const from = $osPath(source);
  const to = $osPath(destination);
  try {
    await $fsp().rename(from, to);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFileSystem_removeFile(self, at) {
  const p = $osPath(at);
  try {
    await $fsp().unlink(p);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `rmdir` and not `rm -r`: the directory must be empty, and one that is not is
// `ENOTEMPTY`, which `$ioErr` has no classified variant for and so reports as
// `.Other` carrying the platform's own sentence. `core/fs`'s `removeDir` is
// where the argument for having no recursive form lives.
async function $host_HostFileSystem_removeDir(self, at) {
  const p = $osPath(at);
  try {
    await $fsp().rmdir(p);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `recursive` is what makes the parents and the already-there case both work;
// a path naming a file is still `EEXIST`, which is `.AlreadyExists`.
async function $host_HostFileSystem_makeDir(self, at) {
  const p = $osPath(at);
  try {
    await $fsp().mkdir(p, { recursive: true });
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `fsync` on a directory flushes its entries, which is what makes a preceding
// rename durable. Opened read-only: `fsync(2)` needs no write access, and a
// directory cannot be opened for writing at all.
async function $host_HostFileSystem_syncFile(self, at) {
  const p = $osPath(at);
  let fh;
  try {
    fh = await $fsp().open(p, "r");
    await fh.sync();
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  } finally {
    if (fh !== undefined) {
      try {
        await fh.close();
      } catch {}
    }
  }
}

// `EntryKind`'s variant index, in `core/fs`'s declaration order. A link is
// asked about first, because a link to a directory is both.
function $entryKind(st) {
  if (st.isSymbolicLink()) return 2;
  if (st.isDirectory()) return 1;
  if (st.isFile()) return 0;
  return 3;
}

// `lstat` and not `stat`: `metadata` does not follow a link, which is what
// makes `.Symlink` reachable and what keeps a walk out of a loop. `Metadata` is
// `[kind, size, modified]` and `Instant` is a one-field struct, so the milliseconds
// arrive wrapped.
async function $host_HostFileSystem_metadata(self, at) {
  const p = $osPath(at);
  try {
    const st = await $fsp().lstat(p);
    return $ok([$entryKind(st), BigInt(Math.trunc(st.size)), [BigInt(Math.trunc(st.mtimeMs))]]);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// A window of the file without reading the rest of it. An offset past the end
// is the empty list, which is what a read at end of file is; a negative offset
// or count is the one failure this can produce that the platform would not.
async function $host_HostFileSystem_readRange(self, at, from, count) {
  const p = $osPath(at);
  const start = Number(from);
  const want = Number(count);
  if (start < 0 || want < 0) return $err([6, "a negative offset or count"]);
  let fh;
  try {
    fh = await $fsp().open(p, "r");
    const buffer = Buffer.alloc(want);
    const got = await fh.read(buffer, 0, want, start);
    return $ok(Array.from(buffer.subarray(0, got.bytesRead)));
  } catch (e) {
    return $err($ioErr(e));
  } finally {
    if (fh !== undefined) {
      try {
        await fh.close();
      } catch {}
    }
  }
}

async function $host_HostFileSystem_realPath(self, at) {
  const p = $osPath(at);
  try {
    return $ok(await $fsp().realpath(p));
  } catch (e) {
    return $err($ioErr(e));
  }
}

// Contents only, and not atomic: a reader of the destination can see half of
// it. `EISDIR` on a directory source, which `$ioErr` reports as `.Other`.
async function $host_HostFileSystem_copyFile(self, source, destination) {
  try {
    await $fsp().copyFile($osPath(source), $osPath(destination));
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// The wire spellings of `Method`, in the enum's declaration order
// (`effect.buri`). A payloadless enum is its variant index in generated code,
// so the index *is* the row, and this array is the whole of the mapping: the
// three letters live here and nowhere in Buri.
const $HTTP_METHOD = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

// A `Headers` iterates `[name, value]` pairs whose names the engine has already
// lowercased — which is the casing `Header` states, so nothing is normalized
// twice. The iterator is also where a repeated field is settled: `set-cookie`
// arrives one entry per cookie and every other repeat arrives joined, which is
// what the platform guarantees and not something to redo here.
function $httpResponseHeaders(response) {
  const out = [];
  for (const [name, value] of response.headers) out.push([name, value]);
  return out;
}

// The platform's own `fetch`, awaited. What was here was a synchronous
// `XMLHttpRequest` with an apology attached: Buri had no way to wait, so a
// request stalled the one thread there was, and off Bun there was no blocking
// path at all. `fetch` is the opposite of every part of that — it is in node,
// in Bun and in every browser, and it is the one host call whose asynchrony the
// language now has a word for. What crosses is unchanged: a `Request` in, a
// `Response` out, octets both ways.
//
// `Request` is `[method, url, headers, body, timeoutMillis]` and `Response` is
// `[status, headers, body]`: a struct is its fields in order, a `Header` is
// `[name, value]`, a payloadless enum is its variant index, and a `[U8]` is an
// ordinary array of numbers.
//
// `timeoutMillis` is `Request.withTimeout`'s, and zero means the platform's own
// bound — which for `fetch` is whatever the engine does. An `AbortController`
// rather than `AbortSignal.timeout`: the controller is in every engine this
// runs on and the static is not, and what an expired request has to answer is
// `.Timeout` either way rather than the `AbortError` the platform throws.
async function $host_HostNetwork_fetch(self, request) {
  $aboutToBlock();
  const method = $HTTP_METHOD[Number(request[0])] || "GET";
  const url = request[1];
  const headers = request[2];
  const body = request[3];
  const timeout = Number(request[4] ?? 0n);
  const stopper = timeout > 0 ? new AbortController() : undefined;
  const alarm =
    stopper === undefined
      ? undefined
      : setTimeout(() => stopper.abort(), timeout);
  try {
    // A `GET` or a `HEAD` may carry no body at all, which `fetch` enforces
    // rather than ignores, so an empty one is left off entirely.
    const sends = body.length !== 0 && method !== "GET" && method !== "HEAD";
    const init = { method, headers: Array.from(headers, (h) => [h[0], h[1]]) };
    if (sends) init.body = new Uint8Array(body);
    if (stopper !== undefined) init.signal = stopper.signal;
    const r = await fetch(url, init);
    // Every byte back unchanged, which is what the `overrideMimeType` trick was
    // standing in for: a response that is not text used to arrive as
    // replacement characters, and a `[U8]` body exists to avoid exactly that.
    const octets = new Uint8Array(await r.arrayBuffer());
    return $ok([BigInt(r.status), $httpResponseHeaders(r), Array.from(octets)]);
  } catch (e) {
    // The deadline fired, so this is `.Timeout` — `NetError`'s first variant —
    // rather than the transport failing. Every engine aborts with a name of
    // `AbortError`, and the controller is this runtime's own, so nothing else
    // can have raised it.
    if (stopper !== undefined && stopper.signal.aborted) return $err([0]);
    // `.Transport(Str)`, the fourth variant of `NetError` — a request that did
    // not reach an answer, whatever stopped it.
    return $err([3, String((e && e.message) || e)]);
  } finally {
    if (alarm !== undefined) clearTimeout(alarm);
  }
}

// --- Crossing to a platform's `js` file ---------------------------------------
//
// A repository platform's `js` file calls its entry and implements its
// production structs' methods, and these are the rows of the crossing table
// that need more than an expression: the Fetch standard's `Request` and
// `Response` each way, and `Result<T, Str>` as a thrown `Error`. The rest is
// written inline by `backend/js/crossing.rs`.

// A Fetch `Request`, as `platform/effect`'s: `[method, url, headers, body,
// timeoutMillis]`. A request the platform handed over carries no timeout.
async function $crossRequestIn(request) {
  const method = $HTTP_METHOD.indexOf(request.method);
  const headers = [];
  for (const [name, value] of request.headers) headers.push([name, value]);
  const carries = request.method !== "GET" && request.method !== "HEAD";
  const body = carries ? Array.from(new Uint8Array(await request.arrayBuffer())) : [];
  return [method < 0 ? 0 : method, request.url, headers, body, 0n];
}

// The other way: `platform/effect`'s `Request`, as a Fetch one.
function $crossRequestOut(r) {
  const method = $HTTP_METHOD[Number(r[0])] || "GET";
  const init = { method, headers: Array.from(r[2], (h) => [h[0], h[1]]) };
  if (r[3].length !== 0 && method !== "GET" && method !== "HEAD") init.body = new Uint8Array(r[3]);
  return new Request(r[1], init);
}

// A Fetch `Response`, as `platform/effect`'s: `[status, headers, body]`.
async function $crossResponseIn(response) {
  const octets = new Uint8Array(await response.arrayBuffer());
  return [BigInt(response.status), $httpResponseHeaders(response), Array.from(octets)];
}

// The other way: `platform/effect`'s `Response`, as a Fetch one. A status that
// may carry no body gets none, not an empty one: Fetch refuses either.
function $crossResponseOut(r) {
  const status = Number(r[0]);
  const bodiless = status === 101 || status === 103 || status === 204 || status === 205 || status === 304;
  return new Response(bodiless ? null : new Uint8Array(r[2]), {
    status,
    headers: Array.from(r[1], (h) => [h[0], h[1]]),
  });
}

// `Result<T, Str>` leaving for the file: the value, or the message thrown.
function $crossThrow(r) {
  if (r[0] === 1) throw new Error(r[1]);
  return r[1];
}

// And arriving from it: `.Ok` of what the method answered, or `.Err` of the
// message of whatever it threw.
async function $crossCatch(thunk) {
  try {
    return $ok(await thunk());
  } catch (e) {
    return $err(String((e && e.message) || e));
  }
}

function $host_HostClock_nowMilliseconds(self) {
  return BigInt(Date.now());
}

// A timer, waited on. What was here spun on `Date.now()` — or called
// `Bun.sleepSync`, which is the same stall with the core given back — and
// either way nothing else in the program could run for the duration. This is
// the plainest statement of what the whole transform is for: a sleeping
// program is now a program with a free event loop.
//
// `setTimeout` is universal — node, Bun and every browser — so there is
// nothing to split on here.
async function $host_HostClock_sleepMilliseconds(self, ms) {
  $aboutToBlock();
  const n = Number(ms);
  await new Promise((wake) => setTimeout(wake, n > 0 ? n : 0));
  return 0;
}

// A clock that only goes forward. `performance.now()` is milliseconds with a
// fraction, and it is what node, Bun and every browser agree on; its zero is
// wherever the host put it, which is exactly what `Monotonic` promises nothing
// about. `Date.now()` is the fallback for a host that has no `performance`, and
// it is a worse answer rather than no answer: it steps when the wall clock does.
function $host_HostClock_monotonicNanoseconds(self) {
  const ms =
    typeof performance === "object" && performance !== null
      ? performance.now()
      : Date.now();
  return BigInt(Math.round(ms * 1e6));
}

function $host_HostRandom_nextInt(self, lo, hi) {
  if (hi <= lo) $abort("random range is empty");
  const span = Number(hi - lo);
  return lo + BigInt(Math.floor(Math.random() * span));
}

function $host_HostRandom_nextFloat(self) {
  return Math.random();
}

// `Entropy.bytes` — WebCrypto, and the one line worth writing down is that it
// is **synchronous**. `crypto.subtle` is promise-shaped and `getRandomValues`
// is not: it fills a typed array and returns it, in node, in Bun, in every
// browser, and in a page that is not a secure context. So this key is absent
// from `rc::suspends`, no caller of it is coloured `async`, and a program that
// mints a token pays nothing for the privilege.
//
// `Math.random` is deliberately not a fallback for a runtime that has no
// `crypto`. A generator that is merely uniform answering a call for one that is
// unpredictable is the exact failure this effect exists to make impossible, and
// it would be invisible: the octets look the same.
function $host_HostEntropy_bytes(self, count) {
  const n = Number(count);
  if (n < 0) $abort("entropy count is negative");
  if (n === 0) return [];
  const c = globalThis.crypto;
  if (!c || typeof c.getRandomValues !== "function") {
    $abort("this platform grants no cryptographic randomness");
  }
  const out = new Uint8Array(n);
  // The specification caps one call at 65536 octets — a quota rather than an
  // implementation limit, so it is the same on every engine — and a longer
  // request is filled a window at a time rather than refused.
  for (let i = 0; i < n; i += 65536) {
    c.getRandomValues(out.subarray(i, Math.min(i + 65536, n)));
  }
  return Array.from(out);
}

// --- Cryptography --------------------------------------------------------------
//
// `core/crypto`'s sealing and signature checks. Written out here rather than
// handed to WebCrypto, because `crypto.subtle` is promise-shaped and these are
// ordinary synchronous functions, and because WebCrypto has no ChaCha20 at all.
// The native runtime answers the same keys through `ring`, and both are checked
// against the RFC 8439, RFC 8032 and RFC 7515 vectors and a Wycheproof subset in
// the conformance corpus.
//
// Nothing below branches on a secret except Poly1305's BigInt arithmetic, whose
// timing depends on the one-time key's magnitude and nothing an attacker picks.
// The signature half only ever sees public values.

// Little-endian bytes to a BigInt, and back.
function $cryptoLe(b, from, to) {
  let n = 0n;
  for (let i = to - 1; i >= from; i--) n = (n << 8n) | BigInt(b[i]);
  return n;
}
function $cryptoLeBytes(n, width) {
  const out = new Array(width);
  for (let i = 0; i < width; i++) {
    out[i] = Number(n & 0xffn);
    n >>= 8n;
  }
  return out;
}
function $cryptoBe(b, from, to) {
  let n = 0n;
  for (let i = from; i < to; i++) n = (n << 8n) | BigInt(b[i]);
  return n;
}

// One ChaCha20 block, RFC 8439 §2.3, as sixteen little-endian words.
function $chachaBlock(key, counter, nonce) {
  const w = (b, i) => (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0;
  const s = new Uint32Array(16);
  s[0] = 0x61707865;
  s[1] = 0x3320646e;
  s[2] = 0x79622d32;
  s[3] = 0x6b206574;
  for (let i = 0; i < 8; i++) s[4 + i] = w(key, i * 4);
  s[12] = counter >>> 0;
  for (let i = 0; i < 3; i++) s[13 + i] = w(nonce, i * 4);
  const x = Uint32Array.from(s);
  const quarter = (a, b, c, d) => {
    x[a] += x[b]; x[d] ^= x[a]; x[d] = (x[d] << 16) | (x[d] >>> 16);
    x[c] += x[d]; x[b] ^= x[c]; x[b] = (x[b] << 12) | (x[b] >>> 20);
    x[a] += x[b]; x[d] ^= x[a]; x[d] = (x[d] << 8) | (x[d] >>> 24);
    x[c] += x[d]; x[b] ^= x[c]; x[b] = (x[b] << 7) | (x[b] >>> 25);
  };
  for (let i = 0; i < 10; i++) {
    quarter(0, 4, 8, 12);
    quarter(1, 5, 9, 13);
    quarter(2, 6, 10, 14);
    quarter(3, 7, 11, 15);
    quarter(0, 5, 10, 15);
    quarter(1, 6, 11, 12);
    quarter(2, 7, 8, 13);
    quarter(3, 4, 9, 14);
  }
  const out = new Uint8Array(64);
  for (let i = 0; i < 16; i++) {
    const v = (x[i] + s[i]) >>> 0;
    out[i * 4] = v & 0xff;
    out[i * 4 + 1] = (v >>> 8) & 0xff;
    out[i * 4 + 2] = (v >>> 16) & 0xff;
    out[i * 4 + 3] = v >>> 24;
  }
  return out;
}

// The keystream from block 1 on, xor-ed over `data`.
function $chachaXor(key, nonce, data) {
  const out = new Array(data.length);
  for (let at = 0, counter = 1; at < data.length; at += 64, counter++) {
    const block = $chachaBlock(key, counter, nonce);
    const end = Math.min(at + 64, data.length);
    for (let i = at; i < end; i++) out[i] = (data[i] ^ block[i - at]) & 0xff;
  }
  return out;
}

// Poly1305 over RFC 8439 §2.8's padded AAD and ciphertext.
function $chachaTag(key, nonce, aad, ciphertext) {
  const otk = $chachaBlock(key, 0, nonce);
  const r = $cryptoLe(otk, 0, 16) & 0x0ffffffc0ffffffc0ffffffc0fffffffn;
  const s = $cryptoLe(otk, 16, 32);
  const p = (1n << 130n) - 5n;
  let acc = 0n;
  const absorb = (b) => {
    for (let i = 0; i < b.length; i += 16) {
      const end = Math.min(i + 16, b.length);
      acc = ((acc + $cryptoLe(b, i, end) + (1n << BigInt(8 * (end - i)))) * r) % p;
    }
  };
  const padded = (b) => b.concat(new Array((16 - (b.length % 16)) % 16).fill(0));
  absorb(
    padded(aad)
      .concat(padded(ciphertext))
      .concat($cryptoLeBytes(BigInt(aad.length), 8))
      .concat($cryptoLeBytes(BigInt(ciphertext.length), 8)),
  );
  return $cryptoLeBytes((acc + s) & ((1n << 128n) - 1n), 16);
}

function $crypto_chacha20Poly1305Seal(_c, key, nonce, plaintext, aad) {
  if (key.length !== 32) $abort("a sealing key is 32 bytes");
  if (nonce.length !== 12) $abort("a sealing nonce is 12 bytes");
  const ciphertext = $chachaXor(key, nonce, plaintext);
  return ciphertext.concat($chachaTag(key, nonce, aad, ciphertext));
}

// `.None` for anything that does not authenticate. The tag is compared without
// an early exit, and nothing is decrypted until it matches.
function $crypto_chacha20Poly1305Open(_c, key, nonce, sealed, aad) {
  if (key.length !== 32 || nonce.length !== 12 || sealed.length < 16) return undefined;
  const ciphertext = sealed.slice(0, sealed.length - 16);
  const expected = $chachaTag(key, nonce, aad, ciphertext);
  let diff = 0;
  for (let i = 0; i < 16; i++) diff |= expected[i] ^ sealed[sealed.length - 16 + i];
  if (diff !== 0) return undefined;
  return $chachaXor(key, nonce, ciphertext);
}

// SHA-256 and SHA-512, for the signature checks to hash with. `core/crypto`
// has its own in Buri; the runtime cannot call back into it.
const $sha256K = Uint32Array.from([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

// Padding shared by both: a one bit, zeros, and the bit length big-endian in
// the block's last `lengthBytes` bytes.
function $shaPad(message, block, lengthBytes) {
  const n = message.length;
  const total = Math.ceil((n + 1 + lengthBytes) / block) * block;
  const out = new Uint8Array(total);
  out.set(message);
  out[n] = 0x80;
  let bits = BigInt(n) * 8n;
  for (let i = total - 1; bits > 0n; i--) {
    out[i] = Number(bits & 0xffn);
    bits >>= 8n;
  }
  return out;
}

function $sha256(message) {
  const m = $shaPad(message, 64, 8);
  const h = Uint32Array.from([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  const w = new Uint32Array(64);
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  for (let at = 0; at < m.length; at += 64) {
    for (let i = 0; i < 16; i++) {
      const j = at + i * 4;
      w[i] = (m[j] << 24) | (m[j + 1] << 16) | (m[j + 2] << 8) | m[j + 3];
    }
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i++) {
      const t1 =
        (hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + $sha256K[i] + w[i]) >>> 0;
      const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) >>> 0;
      hh = g;
      g = f;
      f = e;
      e = (d + t1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) >>> 0;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d;
    h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
  }
  const out = [];
  for (const word of h) out.push(word >>> 24, (word >>> 16) & 0xff, (word >>> 8) & 0xff, word & 0xff);
  return out;
}

const $sha512K = [
  "428a2f98d728ae22", "7137449123ef65cd", "b5c0fbcfec4d3b2f", "e9b5dba58189dbbc", "3956c25bf348b538",
  "59f111f1b605d019", "923f82a4af194f9b", "ab1c5ed5da6d8118", "d807aa98a3030242", "12835b0145706fbe",
  "243185be4ee4b28c", "550c7dc3d5ffb4e2", "72be5d74f27b896f", "80deb1fe3b1696b1", "9bdc06a725c71235",
  "c19bf174cf692694", "e49b69c19ef14ad2", "efbe4786384f25e3", "0fc19dc68b8cd5b5", "240ca1cc77ac9c65",
  "2de92c6f592b0275", "4a7484aa6ea6e483", "5cb0a9dcbd41fbd4", "76f988da831153b5", "983e5152ee66dfab",
  "a831c66d2db43210", "b00327c898fb213f", "bf597fc7beef0ee4", "c6e00bf33da88fc2", "d5a79147930aa725",
  "06ca6351e003826f", "142929670a0e6e70", "27b70a8546d22ffc", "2e1b21385c26c926", "4d2c6dfc5ac42aed",
  "53380d139d95b3df", "650a73548baf63de", "766a0abb3c77b2a8", "81c2c92e47edaee6", "92722c851482353b",
  "a2bfe8a14cf10364", "a81a664bbc423001", "c24b8b70d0f89791", "c76c51a30654be30", "d192e819d6ef5218",
  "d69906245565a910", "f40e35855771202a", "106aa07032bbd1b8", "19a4c116b8d2d0c8", "1e376c085141ab53",
  "2748774cdf8eeb99", "34b0bcb5e19b48a8", "391c0cb3c5c95a63", "4ed8aa4ae3418acb", "5b9cca4f7763e373",
  "682e6ff3d6b2b8a3", "748f82ee5defb2fc", "78a5636f43172f60", "84c87814a1f0ab72", "8cc702081a6439ec",
  "90befffa23631e28", "a4506cebde82bde9", "bef9a3f7b2c67915", "c67178f2e372532b", "ca273eceea26619c",
  "d186b8c721c0c207", "eada7dd6cde0eb1e", "f57d4f7fee6ed178", "06f067aa72176fba", "0a637dc5a2c898a6",
  "113f9804bef90dae", "1b710b35131c471b", "28db77f523047d84", "32caab7b40c72493", "3c9ebe0a15c9bebc",
  "431d67c49c100d4c", "4cc5d4becb3e42b6", "597f299cfc657e2a", "5fcb6fab3ad6faec", "6c44198c4a475817",
].map((k) => BigInt("0x" + k));

function $sha512(message) {
  const m = $shaPad(message, 128, 16);
  const u = (x) => BigInt.asUintN(64, x);
  const rotr = (x, n) => u((x >> n) | (x << (64n - n)));
  const h = [
    0x6a09e667f3bcc908n, 0xbb67ae8584caa73bn, 0x3c6ef372fe94f82bn, 0xa54ff53a5f1d36f1n,
    0x510e527fade682d1n, 0x9b05688c2b3e6c1fn, 0x1f83d9abfb41bd6bn, 0x5be0cd19137e2179n,
  ];
  const w = new Array(80);
  for (let at = 0; at < m.length; at += 128) {
    for (let i = 0; i < 16; i++) w[i] = $cryptoBe(m, at + i * 8, at + i * 8 + 8);
    for (let i = 16; i < 80; i++) {
      const s0 = rotr(w[i - 15], 1n) ^ rotr(w[i - 15], 8n) ^ (w[i - 15] >> 7n);
      const s1 = rotr(w[i - 2], 19n) ^ rotr(w[i - 2], 61n) ^ (w[i - 2] >> 6n);
      w[i] = u(w[i - 16] + s0 + w[i - 7] + s1);
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 80; i++) {
      const ch = (e & f) ^ (u(~e) & g);
      const t1 = u(hh + (rotr(e, 14n) ^ rotr(e, 18n) ^ rotr(e, 41n)) + ch + $sha512K[i] + w[i]);
      const t2 = u((rotr(a, 28n) ^ rotr(a, 34n) ^ rotr(a, 39n)) + ((a & b) ^ (a & c) ^ (b & c)));
      hh = g;
      g = f;
      f = e;
      e = u(d + t1);
      d = c;
      c = b;
      b = a;
      a = u(t1 + t2);
    }
    [a, b, c, d, e, f, g, hh].forEach((v, i) => (h[i] = u(h[i] + v)));
  }
  const out = [];
  for (const word of h) for (let i = 56n; i >= 0n; i -= 8n) out.push(Number((word >> i) & 0xffn));
  return out;
}

function $cryptoPow(base, exp, m) {
  let result = 1n;
  base %= m;
  for (; exp > 0n; exp >>= 1n) {
    if (exp & 1n) result = (result * base) % m;
    base = (base * base) % m;
  }
  return result;
}

// ECDSA over P-256 with SHA-256, the way JWS writes it: a 65-byte uncompressed
// point, and `r ++ s` as 64 bytes. Jacobian coordinates, so one inversion.
const $p256 = {
  p: 0xffffffff00000001000000000000000000000000ffffffffffffffffffffffffn,
  n: 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n,
  b: 0x5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604bn,
  gx: 0x6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296n,
  gy: 0x4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5n,
};

function $p256Double([x, y, z]) {
  const p = $p256.p;
  if (z === 0n || y === 0n) return [0n, 1n, 0n];
  const delta = (z * z) % p;
  const gamma = (y * y) % p;
  const beta = (x * gamma) % p;
  const alpha = (3n * (((x - delta + p) * (x + delta)) % p)) % p;
  const x3 = (((alpha * alpha - 8n * beta) % p) + p) % p;
  const z3 = (((((y + z) * (y + z) - gamma - delta) % p) + p) % p);
  const y3 = (((alpha * ((4n * beta - x3 + p) % p) - 8n * gamma * gamma) % p) + p) % p;
  return [x3, y3, z3];
}

function $p256Add(a, b) {
  const p = $p256.p;
  if (a[2] === 0n) return b;
  if (b[2] === 0n) return a;
  const [x1, y1, z1] = a;
  const [x2, y2, z2] = b;
  const z1z1 = (z1 * z1) % p;
  const z2z2 = (z2 * z2) % p;
  const u1 = (x1 * z2z2) % p;
  const u2 = (x2 * z1z1) % p;
  const s1 = (((y1 * z2) % p) * z2z2) % p;
  const s2 = (((y2 * z1) % p) * z1z1) % p;
  const h = (u2 - u1 + p) % p;
  const r = (s2 - s1 + p) % p;
  if (h === 0n) return r === 0n ? $p256Double(a) : [0n, 1n, 0n];
  const hh = (h * h) % p;
  const hhh = (h * hh) % p;
  const v = (u1 * hh) % p;
  const x3 = (((r * r - hhh - 2n * v) % p) + p) % p;
  const y3 = (((r * (v - x3) - s1 * hhh) % p) + p) % p;
  const z3 = (((z1 * z2) % p) * h) % p;
  return [x3, y3, z3];
}

function $crypto_ecdsaP256Sha256Verify(key, message, signature) {
  const { p, n, b, gx, gy } = $p256;
  if (key.length !== 65 || key[0] !== 4 || signature.length !== 64) return false;
  const qx = $cryptoBe(key, 1, 33);
  const qy = $cryptoBe(key, 33, 65);
  if (qx >= p || qy >= p) return false;
  if ((qy * qy) % p !== (((qx * qx - 3n) % p) * qx + b + p) % p) return false;
  const r = $cryptoBe(signature, 0, 32);
  const s = $cryptoBe(signature, 32, 64);
  if (r === 0n || r >= n || s === 0n || s >= n) return false;
  const e = $cryptoBe($sha256(message), 0, 32) % n;
  const w = $cryptoPow(s, n - 2n, n);
  const u1 = (e * w) % n;
  const u2 = (r * w) % n;
  // Shamir's trick: one pass over both scalars' bits.
  const g = [gx, gy, 1n];
  const q = [qx, qy, 1n];
  const gq = $p256Add(g, q);
  let acc = [0n, 1n, 0n];
  for (let i = 255n; i >= 0n; i--) {
    acc = $p256Double(acc);
    const bits = Number(((u1 >> i) & 1n) | (((u2 >> i) & 1n) << 1n));
    if (bits === 1) acc = $p256Add(acc, g);
    else if (bits === 2) acc = $p256Add(acc, q);
    else if (bits === 3) acc = $p256Add(acc, gq);
  }
  if (acc[2] === 0n) return false;
  const zinv = $cryptoPow(acc[2], p - 2n, p);
  const x = (acc[0] * ((zinv * zinv) % p)) % p;
  return x % n === r;
}

// Ed25519 per RFC 8032 §5.1.7, over extended coordinates. The checks are
// `ring`'s, so both backends refuse the same inputs: `S` must be below the group
// order, the key's `y` is read mod p, and `R` must match the recomputed point's
// encoding byte for byte.
const $ed25519 = (() => {
  const p = (1n << 255n) - 19n;
  const d = (((-121665n * $cryptoPow(121666n, p - 2n, p)) % p) + p) % p;
  const l = (1n << 252n) + 27742317777372353535851937790883648493n;
  const sqrtMinusOne = $cryptoPow(2n, (p - 1n) / 4n, p);
  return { p, d, l, sqrtMinusOne };
})();

function $edAdd(a, b) {
  const { p, d } = $ed25519;
  const [x1, y1, z1, t1] = a;
  const [x2, y2, z2, t2] = b;
  const aa = ((y1 - x1 + p) * (y2 - x2 + p)) % p;
  const bb = ((y1 + x1) * (y2 + x2)) % p;
  const c = (2n * t1 * t2 * d) % p;
  const dd = (2n * z1 * z2) % p;
  const e = (bb - aa + p) % p;
  const f = (dd - c + p) % p;
  const g = (dd + c) % p;
  const h = (bb + aa) % p;
  return [(e * f) % p, (g * h) % p, (f * g) % p, (e * h) % p];
}

function $edMultiply(k, point) {
  let acc = [0n, 1n, 1n, 0n];
  for (let i = 255n; i >= 0n; i--) {
    acc = $edAdd(acc, acc);
    if ((k >> i) & 1n) acc = $edAdd(acc, point);
  }
  return acc;
}

// The point whose encoding is these 32 bytes, or null where there is none.
function $edDecode(bytes) {
  const { p, d, sqrtMinusOne } = $ed25519;
  const y = ($cryptoLe(bytes, 0, 32) & ((1n << 255n) - 1n)) % p;
  const u = (y * y - 1n + p) % p;
  const v = (d * y * y + 1n) % p;
  const v3 = (v * v * v) % p;
  let x = (u * v3 * $cryptoPow(u * v3 * v3 * v, (p - 5n) / 8n, p)) % p;
  const vxx = (v * x * x) % p;
  if (vxx !== u) {
    if (vxx !== (p - u) % p) return null;
    x = (x * sqrtMinusOne) % p;
  }
  if (Number(x & 1n) !== bytes[31] >> 7) x = (p - x) % p;
  return [x, y, 1n, (x * y) % p];
}

function $edEncode([x, y, z]) {
  const p = $ed25519.p;
  const zinv = $cryptoPow(z, p - 2n, p);
  const ax = (x * zinv) % p;
  const ay = (y * zinv) % p;
  return $cryptoLeBytes(ay | ((ax & 1n) << 255n), 32);
}

function $crypto_ed25519Verify(key, message, signature) {
  const { p, l } = $ed25519;
  if (key.length !== 32 || signature.length !== 64) return false;
  const s = $cryptoLe(signature, 32, 64);
  if (s >= l) return false;
  const a = $edDecode(key);
  if (a === null) return false;
  const base = $edDecode($cryptoLeBytes((4n * $cryptoPow(5n, p - 2n, p)) % p, 32));
  const r = signature.slice(0, 32);
  const h = $cryptoLe($sha512(r.concat(key, message)), 0, 64) % l;
  const minusA = [(p - a[0]) % p, a[1], a[2], (p - a[3]) % p];
  const check = $edEncode($edAdd($edMultiply(s, base), $edMultiply(h, minusA)));
  let diff = 0;
  for (let i = 0; i < 32; i++) diff |= check[i] ^ r[i];
  return diff === 0;
}

// RSASSA-PKCS1-v1_5 with SHA-256, refusing what `ring`'s
// `RSA_PKCS1_2048_8192_SHA256` refuses. `n` and `e` are big-endian.
const $rsaSha256DigestInfo = [
  0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
  0x00, 0x04, 0x20,
];

function $crypto_rsaPkcs1Sha256Verify(n, e, message, signature) {
  const k = n.length;
  // An odd modulus with no leading zero, 256 to 1024 octets: `ring` measures
  // the lower bound in whole octets.
  if (k < 256 || k > 1024 || n[0] === 0 || (n[k - 1] & 1) === 0) return false;
  // An odd exponent with no leading zero, in [3, 2^33).
  if (e.length === 0 || e.length > 5 || e[0] === 0) return false;
  const exponent = $cryptoBe(e, 0, e.length);
  if (exponent < 3n || exponent >= 1n << 33n || (exponent & 1n) === 0n) return false;
  if (signature.length !== k) return false;
  const modulus = $cryptoBe(n, 0, k);
  const s = $cryptoBe(signature, 0, k);
  if (s === 0n || s >= modulus) return false;
  let m = $cryptoPow(s, exponent, modulus);
  // The whole encoding is rebuilt and compared, so nothing in it is parsed.
  const expected = [0, 1]
    .concat(new Array(k - 3 - $rsaSha256DigestInfo.length - 32).fill(0xff), [0])
    .concat($rsaSha256DigestInfo, $sha256(message));
  let diff = 0;
  for (let i = k - 1; i >= 0; i--) {
    diff |= Number(m & 0xffn) ^ expected[i];
    m >>= 8n;
  }
  return diff === 0;
}

// Where `variable` and `allVariables` read: the process environment, where
// the engine has one.
function $environmentVariables() {
  return typeof process !== "undefined" ? process.env : {};
}

function $host_HostEnvironment_variable(self, name) {
  const env = $environmentVariables();
  const v = env[name];
  return typeof v === "string" ? $some(v) : undefined;
}

function $host_HostEnvironment_arguments(self) {
  if (typeof Bun !== "undefined") return Bun.argv.slice(2);
  if (typeof process !== "undefined") return process.argv.slice(2);
  return [];
}

function $host_HostEnvironment_currentDirectory(self) {
  return typeof process !== "undefined" ? process.cwd() : "/";
}

// In the engine's own order, which is insertion order over the object it built
// the environment into. `core/env` promises no order at all.
function $host_HostEnvironment_allVariables(self) {
  const env = $environmentVariables();
  const out = [];
  for (const name of Object.keys(env)) {
    if (typeof env[name] === "string") out.push([name, env[name]]);
  }
  return out;
}

// node's word for the platform, mapped to the one `core/env` documents.
// Anything else passes through, which is the honest answer for a platform this
// toolchain does not build for.
function $host_HostEnvironment_operatingSystemName(self) {
  if (typeof process === "undefined") return "unknown";
  const p = process.platform;
  if (p === "darwin") return "macos";
  if (p === "win32") return "windows";
  return String(p);
}

// The child process module, or null where there is none. `$fsOrNull`'s shape,
// for the reason that file states: `$require` is a `const` the backend emits
// only for a platform that can have one.
function $childProcessOrNull() {
  if (typeof $require === "function") return $require("node:child_process");
  if (typeof require === "function") return require("node:child_process");
  return null;
}

// A signal name as the number a shell reports, so `Output.code` is `128 +
// signal` on both backends.
function $signalNumber(name) {
  try {
    const os = typeof $require === "function" ? $require("node:os") : require("node:os");
    return os.constants.signals[name] || 0;
  } catch {
    return 0;
  }
}

// `Spawn.spawnProcess` — start it, feed it, drain both streams, wait.
//
// Draining while it runs is what keeps a child that writes more than a pipe
// holds from deadlocking, and it is why this is `spawn` and a promise rather
// than `spawnSync`. `plan` is the program, the working directory — empty for
// this process's own — and then the arguments, and `environment` is name and
// value alternating and used only when `replace` says so: the encoding
// `core/proc`'s `run` writes and `effect Spawn` argues for.
async function $host_HostSpawn_spawnProcess(self, plan, environment, replace, input) {
  $aboutToBlock();
  const cp = $childProcessOrNull();
  if (cp === null) return $err([6, "this platform cannot start a process"]);
  const program = plan.length > 0 ? plan[0] : "";
  const at = plan.length > 1 ? plan[1] : "";
  const args = plan.slice(2);
  const options = {};
  if (at !== "") options.cwd = at;
  if (replace) {
    const env = {};
    for (let i = 0; i + 1 < environment.length; i += 2) env[environment[i]] = environment[i + 1];
    options.env = env;
  }
  return await new Promise(function (resolve) {
    let child;
    try {
      child = cp.spawn(program, args, options);
    } catch (e) {
      resolve($err($ioErr(e)));
      return;
    }
    const out = [];
    const err = [];
    let settled = false;
    const answer = function (v) {
      if (!settled) {
        settled = true;
        resolve(v);
      }
    };
    child.stdout.on("data", function (c) {
      out.push(c);
    });
    child.stderr.on("data", function (c) {
      err.push(c);
    });
    child.on("error", function (e) {
      answer($err($ioErr(e)));
    });
    child.on("close", function (code, signal) {
      const status = code === null ? 128 + $signalNumber(signal) : code;
      answer(
        $ok([
          BigInt(status),
          Array.from(Buffer.concat(out)),
          Array.from(Buffer.concat(err)),
        ]),
      );
    });
    // A child that never reads its input closes the pipe, and writing into a
    // closed pipe is `EPIPE` rather than a failure of the run.
    child.stdin.on("error", function () {});
    child.stdin.end(Uint8Array.from(input));
  });
}

// `Tasks.parallel(self, ctx, items, f)` — every task started, then every task
// awaited. This is the one place the JavaScript backend is *ahead* of the
// natives rather than level with them: `Promise.all` over an array of started
// calls is genuine concurrency the day the key lands, where the native runtime
// runs the same steps in index order and gains its fan-out a slice later.
//
// Three things are load-bearing and none of them is the promise machinery:
//
//  * **`items.map` starts them all before the first `await`.** Awaiting inside
//    the loop would be the sequential answer spelled concurrently, which is the
//    one mistake this shape exists to avoid.
//  * **The answer is `Promise.all`'s array**, which is in *argument* order and
//    not completion order — the promise `core/tasks` makes, met by the method
//    that already meets it rather than by sorting afterwards.
//  * **`f` need not be async.** A step that never waits returns a value, which
//    `Promise.all` passes through unchanged, so a program that uses `parallel`
//    for its indices and not for its concurrency pays one microtask and no
//    more.
//
// `$share` on the element, as every other combinator here does: the step owns
// what it is handed.
//
// **The step is handed `ctx`, not `self`.** `self` is the `HostTasks` that
// scheduled the work and grants one effect; `ctx` is the caller's whole
// context, which is what `Tasks` promises. They are the same shape here — both
// are empty — and were not on the day a context bound a stateful double, which
// is how passing the wrong one stayed invisible.
async function $host_HostTasks_parallel(self, ctx, xs, f) {
  $aboutToBlock();
  // Each task is handed a context of its own, which is how `core/actor` tells
  // two of them apart: see `$taskFrom`.
  const line = $lineFrom(ctx);
  const out = await Promise.all(xs.map((x, i) => f($taskFrom(ctx, line), $int(i), $share(x))));
  return $own(out);
}

// --- core/tasks: the scope a task is spawned into ----------------------------
//
// Six functions and one table, and they are the same table `cli/runtime/rt.rs`
// holds: a queue per scope, the round it last cut, and one flag saying who is
// draining it.
//
// **Nothing here calls a spawned task.** `core/tasks::running` does, in Buri,
// after asking for it back — which is what lets one implementation serve both
// this backend and the native one, where calling a Buri closure from the
// runtime is the boundary that does not exist. What a task crosses as is a
// one-element list, exactly as an actor's message does, so `$share` on the way
// in for the same reason.
//
// None of them waits, so none of them is `async`: the waiting is
// `Tasks.parallel`'s, one level up, where `core/tasks::draining` runs a round.
//
// **A scope is never removed from this table.** `scope` returns when its body
// and every task spawned into it have finished, and the handle stays live so
// that a handler spawning later still names somewhere to go. On a page that is
// the whole of what keeps a scope alive: the page is the outer scope.
const $scopes = [];

function $scopeAt(handle) {
  const i = Number(handle);
  return i >= 0 && i < $scopes.length ? $scopes[i] : undefined;
}

function $tasks_scopeOpen(c) {
  $scopes.push({ waiting: [], round: [], draining: true });
  return BigInt($scopes.length - 1);
}

function $tasks_scopePush(c, handle, task) {
  const s = $scopeAt(handle);
  if (s === undefined) return undefined;
  s.waiting.push($share(task));
  return $some(BigInt(s.waiting.length));
}

function $tasks_scopeRound(c, handle) {
  const s = $scopeAt(handle);
  if (s === undefined) return $own([]);
  s.round = s.waiting.splice(0);
  return $own(s.round.map((_, i) => BigInt(i)));
}

function $tasks_scopeTaskAt(c, handle, index) {
  const s = $scopeAt(handle);
  if (s === undefined) return undefined;
  const at = Number(index);
  if (at < 0 || at >= s.round.length) return undefined;
  const held = s.round[at];
  if (held === undefined) return undefined;
  s.round[at] = undefined;
  return $some(held);
}

function $tasks_scopeEnter(c, handle) {
  const s = $scopeAt(handle);
  if (s === undefined || s.draining) return false;
  s.draining = true;
  return true;
}

function $tasks_scopeLeave(c, handle) {
  const s = $scopeAt(handle);
  if (s === undefined) return false;
  s.draining = false;
  return s.waiting.length > 0;
}

// A page's tasks wait for the scope's body, so a scope here never runs them
// beside it, and the three worker entries answer as if every worker were done.
function $tasks_scopeBeside(c, handle) {
  return false;
}

function $tasks_scopeClaim(c, handle) {
  return -1n;
}

function $tasks_scopeSpare(c, handle) {
  return false;
}

function $tasks_scopeRan(c, handle) {
  return true;
}

// `core/tasks`'s timers: a `setTimeout` each, which is what keeps a process
// alive while one is pending and lets it end once none is. `run` is handed the
// context `after` was called with, and its own handle. What it printed goes
// out when it returns, because nothing else flushes once `main` has.
const $task_timers = new Map();
let $task_timer_next = 1;

function $tasks_timerStart(c, millis, run) {
  const handle = $task_timer_next++;
  const ms = Number(millis);
  const id = setTimeout(
    async () => {
      $task_timers.delete(handle);
      try {
        await run(c, BigInt(handle));
        $host.flush();
      } catch (e) {
        $failed(e);
      }
    },
    ms > 0 ? ms : 0,
  );
  $task_timers.set(handle, id);
  return BigInt(handle);
}

function $tasks_timerStop(c, handle) {
  const at = Number(handle);
  const id = $task_timers.get(at);
  if (id !== undefined) {
    $task_timers.delete(at);
    clearTimeout(id);
  }
  return 0;
}

function $host_HostProcess_exitWith(self, code) {
  $exit(Number(code));
  return 0;
}

// --- Sockets and WebSocketClient --------------------------------------------
//
// A page cannot hold a port open, so `Listen` is not granted here and there is
// no acceptor in this file. What a page *can* do is dial somebody else's
// socket, and these five bodies are that: `connectSocket` mints a socket,
// `connectReceive` reads it, and `Sockets`' three methods write on it.
//
// All of it runs on the platform's own `WebSocket` — node 22 and later, Bun,
// and every browser — so there is no dependency and no build step behind any
// of this.
//
// `$wsLive` is the whole of the state: one row per socket a program still
// holds, keyed by the number it carries around. A row is the `WebSocket`, a
// queue of frames that have arrived and nobody has asked for, and a queue of
// callers waiting for one. **That pairing is the point.** A message that lands
// while nobody is asking goes on the first queue and is answered by the next
// `connectReceive`, so nothing is lost between two reads — which is exactly
// what awaiting the event itself would get wrong.
const $wsLive = new Map();
let $wsNext = 1;

// `Frame`'s three variants and the three `ServeFailure` causes this file
// answers with, as the indices `platform/effect` declares them in. A payloadless
// enum is its variant index in generated code, so these are the whole of the
// mapping.
const $FRAME_TEXT = 0;
const $FRAME_BINARY = 1;
const $FRAME_CLOSED = 2;
const $SERVE_UNSUPPORTED = 3;
const $SERVE_CLOSED = 5;
const $SERVE_TRANSPORT = 6;

// One thing that arrived, handed to whoever is waiting or queued for whoever
// asks next.
function $wsArrived(row, item) {
  const waiting = row.waiting.shift();
  if (waiting) {
    waiting(item);
  } else {
    row.frames.push(item);
  }
}

// A close event as the wire number `Received.code` carries. RFC 6455 reserves
// 1005 for "the far side sent no code" and 1006 for "there was no close frame
// at all"; `core/net/server`'s `reasonOf` reads both as `.Abnormal`.
function $wsCloseCode(event) {
  if (event.wasClean === false) return 1006;
  const code = Number(event.code);
  return code > 0 ? code : 1005;
}

// A binary message, as the array of numbers a `[U8]` is. `binaryType` asks for
// an `ArrayBuffer` and the engines here honour it, but a `Blob` costs one
// `await` to read and a runtime that hands one over should still work.
async function $wsOctets(data) {
  if (data instanceof ArrayBuffer) return Array.from(new Uint8Array(data));
  if (ArrayBuffer.isView(data)) {
    return Array.from(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
  }
  return Array.from(new Uint8Array(await data.arrayBuffer()));
}

// What a page can know about the handshake, and it is less than a native
// program knows. The browser's `WebSocket` never shows the response, so the
// two fields it does expose are the two headers here — the negotiated
// subprotocol and extensions — and nothing else. On LINUX and MACOS
// `Connected.headers` is the head the server actually sent, so a reader should
// not take the two for the same list. Names are lowercase, which is what
// `Header` states.
function $wsHandshakeHeaders(ws) {
  const out = [];
  if (ws.protocol) out.push(["sec-websocket-protocol", ws.protocol]);
  if (ws.extensions) out.push(["sec-websocket-extensions", ws.extensions]);
  return out;
}

// Whether this is a URL a WebSocket can be dialled on at all, and the scheme to
// name when it is not. A scheme the platform does not speak is `.Unsupported`,
// which is a fact about the platform; everything that goes wrong once the dial
// has started is `.Transport`, which is a fact about the attempt. Keeping the
// two apart is what lets this backend and the native one answer the same value.
function $wsDials(url) {
  return url.startsWith("ws://") || url.startsWith("wss://");
}

function $wsScheme(url) {
  const mark = url.indexOf("://");
  return mark > 0 ? url.slice(0, mark) : url;
}

// The close reason rides in the close frame, which leaves 123 octets for it,
// and a longer one throws. So it is cut to fit rather than losing the close —
// and cut on a character boundary, by stepping back over any continuation
// octets (`10xxxxxx`) the cut would have orphaned.
function $wsReason(text) {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length <= 123) return text;
  let end = 123;
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end -= 1;
  return new TextDecoder().decode(bytes.subarray(0, end));
}

// `Sockets`' three, over the sockets `connectSocket` minted. Every one of them
// is total and none of them waits, which is what `effect Sockets` declares: a
// handle naming no open socket is one that has already gone, so the call is a
// no-op. `()` is `0`, as everywhere else in this file.
function $host_HostSockets_socketSendText(self, socket, text) {
  const row = $wsLive.get(Number(socket));
  if (row === undefined) return 0;
  row.ws.send(text);
  return 0;
}

function $host_HostSockets_socketSendBytes(self, socket, body) {
  const row = $wsLive.get(Number(socket));
  if (row === undefined) return 0;
  row.ws.send(new Uint8Array(body));
  return 0;
}

// A page's `WebSocket` has no way to send a ping, so there is nothing to do.
function $host_HostSockets_socketPing(self, socket) {
  return 0;
}

function $host_HostSockets_socketClose(self, socket, code, reason) {
  const row = $wsLive.get(Number(socket));
  if (row === undefined) return 0;
  // A page may only send 1000 or a private code in 3000–4999; every other
  // number throws. So the rest — 1001 for going away, 1011 for an internal
  // error — go out as 1000, because closing for the wrong reason beats not
  // closing at all.
  const wanted = Number(code);
  const sent = wanted === 1000 || (wanted >= 3000 && wanted <= 4999) ? wanted : 1000;
  row.ws.close(sent, $wsReason(reason));
  // The row stays until `connectReceive` answers the close: `.Closed` is the
  // last thing a socket says, and it has not said it yet.
  return 0;
}

// Dials a socket and completes the handshake.
//
// The handlers go on before the open is awaited, so a message that arrives in
// the same turn as the open lands on the queue instead of in the gap between
// two `addEventListener` calls. Before the socket is open an `error` or a
// `close` is the handshake failing; after it, both are the socket ending.
async function $host_HostWebSocketClient_connectSocket(self, url) {
  $aboutToBlock();
  if (typeof globalThis.WebSocket !== "function") {
    return $err([
      $SERVE_UNSUPPORTED,
      "this runtime has no WebSocket: node before 22 is the usual reason",
    ]);
  }
  // The scheme is checked before anything is constructed, so a scheme this
  // platform cannot speak is `.Unsupported` rather than whatever sentence the
  // constructor would have thrown.
  if (!$wsDials(url)) {
    const said = "a WebSocket speaks ws:// and wss://, and not " + $wsScheme(url);
    return $err([$SERVE_UNSUPPORTED, said]);
  }
  let ws;
  try {
    ws = new globalThis.WebSocket(url);
  } catch (e) {
    // A malformed URL, or a page refusing an insecure socket on a secure
    // origin — the constructor is where both are reported, and its own message
    // says which.
    return $err([$SERVE_TRANSPORT, String((e && e.message) || e)]);
  }
  ws.binaryType = "arraybuffer";
  const row = { ws, frames: [], waiting: [], open: false };
  let settle;
  const opening = new Promise((r) => {
    settle = r;
  });
  ws.onopen = () => {
    row.open = true;
    settle(null);
  };
  ws.onmessage = (e) => {
    if (typeof e.data === "string") {
      $wsArrived(row, { frame: $FRAME_TEXT, text: e.data });
    } else {
      $wsArrived(row, { frame: $FRAME_BINARY, data: e.data });
    }
  };
  ws.onerror = () => {
    if (row.open) {
      // An `error` on an open socket is the connection breaking, and the
      // wire has no number for that: 1006 is what it is called.
      $wsArrived(row, { frame: $FRAME_CLOSED, code: 1006 });
    } else {
      settle("the connection failed before the handshake finished");
    }
  };
  ws.onclose = (e) => {
    if (row.open) {
      $wsArrived(row, { frame: $FRAME_CLOSED, code: $wsCloseCode(e) });
    } else {
      settle("the socket closed before the handshake finished");
    }
  };
  // One await on a promise three handlers resolve. Nothing spins and nothing
  // blocks, so a page keeps rendering while this is in flight — the same
  // thing that let `Network.fetch` onto a page.
  const failed = await opening;
  if (failed !== null) return $err([$SERVE_TRANSPORT, failed]);
  const handle = $wsNext++;
  $wsLive.set(handle, row);
  // The status is `101` and the body is empty because a handshake that
  // finished answered `101` with no body, and a page never sees either.
  return $ok([BigInt(handle), 101n, $wsHandshakeHeaders(ws), []]);
}

// The next thing on that socket, awaited if nothing has arrived yet.
//
// A ping and a pong never reach here: the browser answers them and a page
// cannot see one, which is why `Frame` has three variants and not five.
async function $host_HostWebSocketClient_connectReceive(self, socket) {
  $aboutToBlock();
  const key = Number(socket);
  const row = $wsLive.get(key);
  if (row === undefined) return $err([$SERVE_CLOSED, "this socket has already gone"]);
  const item =
    row.frames.length !== 0
      ? row.frames.shift()
      : await new Promise((arrive) => row.waiting.push(arrive));
  if (item.frame === $FRAME_CLOSED) {
    // `.Closed` is the last answer a socket gives. It leaves the table here,
    // so a later `connectReceive` is `.Err(.Closed)` and a send to it is
    // dropped — which is what makes "the close hook is the last thing that
    // runs" true with no second call to release the socket.
    $wsLive.delete(key);
    return $ok([$FRAME_CLOSED, "", [], BigInt(item.code)]);
  }
  if (item.frame === $FRAME_TEXT) return $ok([$FRAME_TEXT, item.text, [], 0n]);
  return $ok([$FRAME_BINARY, "", await $wsOctets(item.data), 0n]);
}

// --- core/actor -------------------------------------------------------------
//
// The mailbox, the state and the reply slots. Nine functions, and between them
// they are the same table `cli/runtime/rt.rs` holds — one queue per actor, one
// state beside it, one baton that says who may step it, and a slot per message.
//
// Everything crosses as a one-element list, which is what makes the native
// half possible at all (a `[T]` is two words whatever `T` is) and costs nothing
// here, where a list is an array. `$share` on the way in, because a value the
// runtime holds onto is a second reference by definition and a list this
// backend allocated must stop being writable in place the moment there are two
// names for it.
//
// The three waits are `async` because `middle/rc.rs`'s `suspends` says they
// are, and the two lists must agree: `mailboxPush` waits for room, and
// `mailboxClose` and `stateTake` wait for the step in flight to give the state
// back. Nothing else here waits.
const $actors = [];

// How long `mailboxPush` waits for room and `stateTake` waits for another
// task's step before each answers `.TimedOut`: `cli/runtime/rt.rs`'s
// `ACTOR_DEADLINE`, for its reason. Both waits are ones a program can make
// unbounded — two actors each posting into the other's full mailbox, two steps
// each sending to the actor the other is stepping — so they carry a number, and
// the number is long enough that reaching it is a bug.
const $ACTOR_DEADLINE_MS = 30000;

// `core/actor`'s `SendError`, by variant: what `mailboxPush` and `stateTake`
// answer in `.Err`.
const $SEND_STOPPED = 0;
const $SEND_TIMED_OUT = 1;
const $SEND_WOULD_DEADLOCK = 2;

// Waits on `list` until something calls the resolver it pushed there with
// `true`, or until `deadline`. Answers which it was, and a wait that ran out
// takes its resolver back off the list.
async function $actorWait(list, deadline) {
  const left = deadline - Date.now();
  if (left <= 0) return false;
  $aboutToBlock();
  let wake;
  let alarm;
  const woken = await new Promise((resolve) => {
    wake = resolve;
    list.push(resolve);
    alarm = setTimeout(() => resolve(false), left);
  });
  clearTimeout(alarm);
  if (!woken) {
    const at = list.indexOf(wake);
    if (at >= 0) list.splice(at, 1);
  }
  return woken;
}

// **A task is the context it was handed.** `stateTake` has to tell a sender
// that may wait for the step holding the state from one that is part of that
// step, which would be waiting for itself, and the native runtime asks the
// thread and the tasks that started it. JavaScript has neither. What it has is
// the context: a lambda may not capture one and only an entry may build one
// (SPEC 10.6, 11.3), so the context a step was handed reaches everything that
// step calls, and nothing else. A drive records the context it took the state
// with, and a send is part of that step when its context is that one, or one a
// task started from it was handed.
//
// The one place two tasks are handed one context is where the runtime starts
// them, so that is where each gets a copy of its own: `$taskFrom` for a task
// its caller waits for, which carries the caller in its lineage, and
// `$taskApart` for a handler nobody waits for, which carries none. A copy
// reads the same effects, because a context is an array of handles and the
// handles are shared.
function $taskFrom(ctx, line) {
  if (!Array.isArray(ctx)) return ctx;
  const child = ctx.slice();
  child.$line = line;
  return child;
}

// The lineage every task `$taskFrom` starts from `ctx` carries.
function $lineFrom(ctx) {
  if (!Array.isArray(ctx)) return undefined;
  return ctx.$line === undefined ? [ctx] : ctx.$line.concat([ctx]);
}

function $taskApart(ctx) {
  return Array.isArray(ctx) ? ctx.slice() : ctx;
}

// Whether the state `holder` took is held by `c`'s own step: `c` is the
// context that step was handed, or one a task it started was.
function $actorWithin(holder, c) {
  if (holder === c) return true;
  const line = Array.isArray(c) ? c.$line : undefined;
  return line !== undefined && line.includes(holder);
}

function $actor_mailboxOpen(c, state, bound) {
  const room = Number(bound) > 0 ? Number(bound) : 1;
  $actors.push({
    queue: [],
    state: $share(state),
    closed: false,
    bound: room,
    room: [],
    baton: true,
    holder: undefined,
    free: [],
    takers: [],
  });
  return BigInt($actors.length - 1);
}

function $actorAt(handle) {
  const i = Number(handle);
  return i >= 0 && i < $actors.length ? $actors[i] : undefined;
}

async function $actor_mailboxPush(c, handle, message) {
  const a = $actorAt(handle);
  if (a === undefined || a.closed) return $err($SEND_STOPPED);
  const deadline = Date.now() + $ACTOR_DEADLINE_MS;
  while (a.queue.length >= a.bound) {
    if (!(await $actorWait(a.room, deadline))) return $err($SEND_TIMED_OUT);
    if (a.closed) return $err($SEND_STOPPED);
  }
  a.queue.push($share(message));
  return $ok(BigInt(a.queue.length));
}

function $actor_mailboxPop(c, handle) {
  const a = $actorAt(handle);
  if (a === undefined || a.queue.length === 0) return undefined;
  const held = a.queue.shift();
  const wake = a.room.shift();
  if (wake !== undefined) wake(true);
  return $some(held);
}

async function $actor_mailboxClose(c, handle) {
  const a = $actorAt(handle);
  if (a === undefined || a.closed) return undefined;
  a.closed = true;
  for (const wake of a.room.splice(0)) wake(true);
  // A `stateTake` still waiting is answered now rather than at its deadline:
  // there is no state left to wait for.
  for (const wake of a.takers.splice(0)) wake(true);
  while (!a.baton) {
    $aboutToBlock();
    await new Promise((resolve) => a.free.push(resolve));
  }
  // Kept, never given back: the baton is what a `stateTake` needs, so holding
  // it forever is what makes a stopped actor unsteppable.
  a.baton = false;
  const held = a.state;
  a.state = undefined;
  return held === undefined ? undefined : $some(held);
}

// **Waits while another task is stepping the actor**, so a sender that arrives
// mid-step gets its answer once that step is done (buri-lang/buri#205). Never
// for itself: `.WouldDeadlock` at once where the state is held by the caller's
// own step. `.Stopped` for a stopped actor, and `.TimedOut` past the deadline.
//
// Not `async` on the path that does not wait, so a send to an idle actor pays
// the one `await` at its call and nothing more.
function $actor_stateTake(c, handle) {
  const a = $actorAt(handle);
  if (a === undefined || a.closed) return $err($SEND_STOPPED);
  if (a.baton) return $actorHanded(a, c);
  if ($actorWithin(a.holder, c)) return $err($SEND_WOULD_DEADLOCK);
  return $actorAwaitState(a, c, Date.now() + $ACTOR_DEADLINE_MS);
}

async function $actorAwaitState(a, c, deadline) {
  for (;;) {
    if (!(await $actorWait(a.takers, deadline))) return $err($SEND_TIMED_OUT);
    if (a.closed) return $err($SEND_STOPPED);
    if (a.baton) return $actorHanded(a, c);
  }
}

// The state out of `a`, to `c`, whose baton it now is.
function $actorHanded(a, c) {
  if (a.state === undefined) return $err($SEND_STOPPED);
  a.baton = false;
  a.holder = c;
  const held = a.state;
  a.state = undefined;
  return $ok(held);
}

function $actor_statePut(c, handle, state) {
  const a = $actorAt(handle);
  if (a === undefined) return undefined;
  a.state = $share(state);
  a.baton = true;
  a.holder = undefined;
  for (const wake of a.free.splice(0)) wake();
  for (const wake of a.takers.splice(0)) wake(true);
  return $some(BigInt(a.queue.length));
}

// The reply slots, reused behind a generation, exactly as the native table
// reuses them: a server answering a million messages would otherwise grow a
// table for the length of its uptime, and a stale handle must name nothing
// rather than somebody else's answer.
const $replies = { slots: [], free: [] };
const $REPLY_INDEX_BITS = 20n;
const $REPLY_INDEX_MASK = (1n << $REPLY_INDEX_BITS) - 1n;

function $actor_replyOpen(c) {
  const index = $replies.free.pop();
  if (index !== undefined) {
    const slot = $replies.slots[index];
    slot.state = 0;
    slot.value = undefined;
    return (slot.generation << $REPLY_INDEX_BITS) | BigInt(index);
  }
  $replies.slots.push({ generation: 0n, state: 0, value: undefined });
  return BigInt($replies.slots.length - 1);
}

function $replyAt(handle) {
  const index = Number(handle & $REPLY_INDEX_MASK);
  const generation = handle >> $REPLY_INDEX_BITS;
  const slot = $replies.slots[index];
  if (slot === undefined || slot.generation !== generation) return undefined;
  return slot;
}

function $actor_replyPut(c, handle, value) {
  const slot = $replyAt(handle);
  if (slot === undefined || slot.state !== 0) return undefined;
  slot.state = 1;
  slot.value = $share(value);
  return $some(0n);
}

// A take spends the slot even when there was nothing in it: one `sendMessage`
// opens one slot and takes it once, so an empty take is a sender giving up and
// the slot goes back either way. The generation moves with it, so a `replyPut`
// that arrives afterwards writes nothing.
function $actor_replyTake(c, handle) {
  const slot = $replyAt(handle);
  if (slot === undefined) return undefined;
  const answered = slot.state === 1;
  const value = slot.value;
  slot.state = 2;
  slot.value = undefined;
  slot.generation = slot.generation + 1n;
  $replies.free.push(Number(handle & $REPLY_INDEX_MASK));
  return answered ? $some(value) : undefined;
}

// No arenas here, so nothing is ever in one.
function $actor_scopesLive() {
  return false;
}

// --- The reactive graph -----------------------------------------------------
//
// Auto-tracking, in the shape design/ui-reactivity.md commits to: the runtime
// holds a pointer to the computation that is running, `read` records a
// source -> computation edge, `write` marks dependents out of date and
// schedules them, and dependencies are collected afresh on every run, so a
// read behind an `if` is tracked exactly.
//
// One array of nodes, indexed by the `Int` a Buri `Signal<T>` carries. Four
// kinds, told apart by `kind`:
//
//   0  cell        a value, written from outside
//   1  memo        a value, computed from other nodes, lazily
//   2  watcher     run for its effect on the world, eagerly
//   3  owner       runs nothing; exists so that something else can be disposed
//                  with it. A keyed list's rows hang off one of these, which is
//                  what lets a row outlive the run that decided it belongs.
//
// A memo is lazy and a watcher is not, and that is the whole difference in
// scheduling: an out-of-date memo recomputes when something reads it, while a
// watcher is pushed onto the queue and drained at the end of the batch.
//
// `deps` and `subs` are the same edges from the two ends. Both are arrays
// rather than sets: a computation reads a handful of cells, and a linear scan
// over three elements beats a hash.

const $ui = {
  nodes: [],
  // What a node created right now belongs to, or -1. Disposal is keyed on it.
  current: -1,
  // What a read right now subscribes, or -1 for a read nobody is listening to.
  // Separate from `current` because building a keyed list's row is two
  // different questions at once: the row belongs to the list, and what the row
  // read while it was being built is nobody's dependency — the list is already
  // subscribed to the list.
  tracking: -1,
  queue: [],
  // Open batches. A write inside one defers the drain, so N writes cause one
  // pass rather than N.
  depth: 0,
  // Whether a drain is running. A write a watcher makes while the graph is
  // draining — a keyed list's row builder correcting the signal its key list
  // reads (buri#170) — queues the watcher and lets the running drain's own loop
  // take it, rather than starting a nested drain that re-enters the reconciler
  // mid-pass and recurses until the stack overflows.
  draining: false,
};

// A runaway is a program whose watchers write what they read. The limit is not
// a policy, it is the difference between a diagnosis and a hung tab.
const $UI_STEPS = 100000;

// **What a node holds, it holds against everyone else.** A cell keeps its value
// until the next write and a memo keeps the one it computed, so every value
// stored below goes through `$share` on the way in — this backend's currency
// for the debt the native runtime pays with the retain glue a cell's entry
// carries (`cli/runtime/ui.rs`, "the retain glue, and what a cell owes the
// value in it").
//
// Without it, `get` answers a list this runtime believes only the caller holds,
// `concat` on it writes through, and `xs.set(ctx, xs.get(ctx).concat(…))` hands
// `set` the value it is replacing — where the equality cutoff drops the write,
// and nothing that read the cell re-runs. buri-lang/buri#143.

function $ui_cell(kind, value, compute) {
  const owner = $ui.current;
  $ui.nodes.push({
    kind,
    value: $share(value),
    compute,
    deps: [],
    subs: [],
    // A memo has never run, so it is out of date by construction.
    dirty: kind === 1,
    queued: false,
    disposed: false,
    owner,
    children: [],
  });
  const id = $ui.nodes.length - 1;
  // Disposal is keyed on which computation was executing when the node was
  // created, so a nested computation dies with the run that made it.
  if (owner >= 0) $ui.nodes[owner].children.push(id);
  return id;
}

function $ui_at(id) {
  const n = $ui.nodes[id];
  if (n === undefined) $abort("this signal does not exist");
  return n;
}

function $ui_read(cell) {
  const id = Number(cell);
  const n = $ui_at(id);
  // Reading is what makes a memo run: until then it has computed nothing, and
  // a memo nothing reads never runs at all.
  if (n.kind === 1 && n.dirty && !n.disposed) $ui_run(id);
  const c = $ui.tracking;
  if (c >= 0 && c !== id) {
    if (!n.subs.includes(c)) n.subs.push(c);
    const reader = $ui.nodes[c];
    if (!reader.deps.includes(id)) reader.deps.push(id);
  }
  return n.value;
}

function $ui_unsubscribe(id, n) {
  for (const d of n.deps) {
    const source = $ui.nodes[d];
    if (source === undefined) continue;
    const at = source.subs.indexOf(id);
    if (at >= 0) source.subs.splice(at, 1);
  }
  n.deps = [];
}

// A computation may own something outside the graph — a document-level listener
// an `onPressOutside` registered is the one there is — and that thing has to be
// let go when the computation is. `cleanups` is where such a release is parked,
// and both a re-run and a disposal run it: a subtree torn down and a subtree
// rebuilt both mean the listener the last run put on the document is no longer
// the reader's, so it goes. The field is absent until something needs it,
// because almost nothing does.
function $ui_cleanups(n) {
  const cleanups = n.cleanups;
  if (cleanups === undefined) return;
  n.cleanups = undefined;
  for (const release of cleanups) release();
}

// Parks a release against the computation running now, so it fires when that
// computation re-runs or is disposed. Outside any computation — a mount's own
// top level — there is nothing to hang it on and nothing that would ever
// dispose it, which is the page's listeners going on running, so it is kept
// alive by the document it was put on.
function $ui_dispose_with(release) {
  const owner = $ui.current;
  if (owner < 0) return;
  const n = $ui.nodes[owner];
  if (n === undefined) return;
  (n.cleanups === undefined ? (n.cleanups = []) : n.cleanups).push(release);
}

function $ui_dispose(id) {
  const n = $ui.nodes[id];
  if (n === undefined || n.disposed) return;
  n.disposed = true;
  $ui_cleanups(n);
  for (const c of n.children) $ui_dispose(c);
  n.children = [];
  $ui_unsubscribe(id, n);
  n.subs = [];
  n.compute = null;
}

function $ui_run(id) {
  const n = $ui_at(id);
  if (n.disposed) return;
  // Everything the previous run created belongs to the previous run — its
  // child computations, and the listeners it parked on the document.
  $ui_cleanups(n);
  for (const c of n.children) $ui_dispose(c);
  n.children = [];
  // Per-run dependency re-collection: the edges are dropped before the body
  // runs, so what it reads this time is exactly what it is subscribed to.
  $ui_unsubscribe(id, n);
  const outerCurrent = $ui.current;
  const outerTracking = $ui.tracking;
  $ui.current = id;
  $ui.tracking = id;
  try {
    // The `Scope` a Buri closure receives: a one-field struct naming the
    // computation it belongs to.
    const v = n.compute([id]);
    if (n.kind === 1) n.value = $share(v);
  } finally {
    $ui.current = outerCurrent;
    $ui.tracking = outerTracking;
    n.dirty = false;
  }
}

// Runs `body` with everything it creates belonging to `owner`, and with what
// it reads subscribing nothing. Both halves are needed together exactly once:
// a keyed list builds a row that must outlive the run that decided to build
// it, and whose reads are the list's dependencies and not the row's.
function $ui_under(owner, body) {
  const outerCurrent = $ui.current;
  const outerTracking = $ui.tracking;
  $ui.current = owner;
  $ui.tracking = -1;
  try {
    return body();
  } finally {
    $ui.current = outerCurrent;
    $ui.tracking = outerTracking;
  }
}

// Drops `id` from its owner's children, so that a list which adds and removes
// a row a thousand times holds a thousand disposed nodes for no longer than it
// holds the row.
function $ui_forget(owner, id) {
  const n = $ui.nodes[owner];
  if (n === undefined) return;
  const at = n.children.indexOf(id);
  if (at >= 0) n.children.splice(at, 1);
}

// Marking out of date, transitively. A memo is only marked — it recomputes
// when read — while a watcher is queued, since nothing will ever read it.
function $ui_notify(n) {
  for (const s of n.subs.slice()) {
    const c = $ui.nodes[s];
    if (c === undefined || c.disposed) continue;
    if (c.kind === 1) {
      if (!c.dirty) {
        c.dirty = true;
        $ui_notify(c);
      }
    } else if (c.kind === 2 && !c.queued) {
      c.queued = true;
      $ui.queue.push(s);
    }
  }
}

function $ui_drain() {
  // A drain already running takes new work through its own loop below, so a
  // re-entrant call — a write made while draining — does nothing and returns.
  if ($ui.draining) return;
  $ui.draining = true;
  try {
    let steps = 0;
    // Not `for (const id of queue)`: a watcher may schedule another, and the
    // one it schedules belongs to this pass. Index-walking is what makes the
    // order the order they were scheduled in.
    for (let i = 0; i < $ui.queue.length; i++) {
      if (++steps > $UI_STEPS) $abort("a reactive update did not settle");
      const id = $ui.queue[i];
      const n = $ui.nodes[id];
      if (n === undefined) continue;
      n.queued = false;
      if (!n.disposed) $ui_run(id);
    }
  } finally {
    // A drain that threw — a reactive update that did not settle — must not
    // leave its half-processed queue or its flush-in-progress flag behind. The
    // queue, the per-node `queued` flags and `draining` are that state, so a
    // batch sibling's next write would drain these stale nodes (or find the
    // graph wedged "draining") and fail with the same error, which is the
    // poisoning buri#187 reports. Clearing them here scopes a failing update to
    // the test that caused it.
    for (const id of $ui.queue) {
      const n = $ui.nodes[id];
      if (n !== undefined) n.queued = false;
    }
    $ui.queue = [];
    $ui.draining = false;
  }
}

function $ui_write(cell, v) {
  const id = Number(cell);
  const n = $ui_at(id);
  // An equal write is not a change. This is what makes "wrote the same value,
  // so nothing re-ran" a thing a test can assert — and "the same value" is
  // `==`, which is structural (SPEC 7.2), so it is `$eq` rather than `===`.
  // Reference identity would call two lists of the same elements two values,
  // and a cell holding one would re-render on every write of what it already
  // held. The native backends compare with the type's own generated `Equal`
  // (`cli/runtime/ui.rs`), which is the same answer at every type.
  if ($eq(n.value, v)) return 0;
  n.value = $share(v);
  $ui_notify(n);
  if ($ui.depth === 0) $ui_drain();
  return 0;
}

// One update transaction: N writes cause one pass over the watchers rather
// than N.
//
// The transaction is the handler's *synchronous* run, and that is the whole of
// the decision now that a handler can wait. A page grants `Network`, so a press
// may write "asking the server", suspend on the answer, and write again when
// it arrives — and those are two transactions, not one held open across the
// wait. Holding it open would mean the first notice did not reach the document
// until the request it announced had already finished, which is the opposite
// of what it is for.
//
// What the promise still owes is its failure. A synchronous handler that
// aborts throws out of the listener; an awaiting one would settle a rejected
// promise nobody is holding, and the abort would be a line in a console rather
// than an error. Rethrowing it from a fresh task is what makes the two behave
// alike.
function $ui_flush(f) {
  $ui.depth++;
  let pending;
  try {
    pending = f();
  } finally {
    $ui.depth--;
  }
  if ($ui.depth === 0) $ui_drain();
  if (pending !== null && typeof pending === "object" && typeof pending.then === "function") {
    pending.then(undefined, (e) => {
      setTimeout(() => {
        throw e;
      }, 0);
    });
  }
  return 0;
}

function $host_HostUi_signal(self, initial) {
  return BigInt($ui_cell(0, initial, null));
}

function $host_HostUi_read(self, id) {
  return $ui_read(id);
}

function $host_HostUi_write(self, id, value) {
  return $ui_write(id, value);
}

function $host_HostUi_memo(self, compute) {
  return BigInt($ui_cell(1, undefined, compute));
}

function $host_HostUi_watch(self, run) {
  // Eager, and that is not an optimization: a watcher learns what it depends
  // on by running, so one that has never run is subscribed to nothing and
  // would never run again.
  $ui_run($ui_cell(2, undefined, run));
  return 0;
}

// A page's timers, on a real clock. `after` schedules `run` for the graph's own
// turn once the duration has passed, so the write it does lands in an update
// transaction of its own — a `setTimeout` whose body is a `$ui_flush`, exactly
// as a handler's is. The handle a `Timer` carries names the pending run in
// `$ui_page_timers`, so `cancel` can clear the one that is still waiting; a
// timer that has fired has already dropped itself from the map, so cancelling it
// is the no-op the interface promises.
const $ui_page_timers = new Map();
let $ui_page_timer_next = 1;

function $host_HostUi_schedule(self, millis, run) {
  const handle = $ui_page_timer_next++;
  const ms = Number(millis);
  const id = setTimeout(
    () => {
      $ui_page_timers.delete(handle);
      $ui_flush(() => run(self));
    },
    ms > 0 ? ms : 0,
  );
  $ui_page_timers.set(handle, () => clearTimeout(id));
  return BigInt(handle);
}

function $host_HostUi_unschedule(self, id) {
  const handle = Number(id);
  const clear = $ui_page_timers.get(handle);
  if (clear !== undefined) {
    $ui_page_timers.delete(handle);
    clear();
  }
  return 0;
}

function $host_HostWatch_read(self, id) {
  return $ui_read(id);
}

function $effect_Scope_read(self, id) {
  return $ui_read(id);
}

// --- The document -----------------------------------------------------------
//
// There are two, and every operation below asks which one it was handed rather
// than asking the program. In a browser a node is a real DOM node and each
// operation is the DOM call it looks like. Everywhere else — `bun`, `node`, and
// every test in this suite — there is no document, so the runtime supplies one:
// plain objects carrying `$shim`, with the same handful of operations, which is
// enough to build a tree, change it, fire a listener at it and write it out as
// markup.
//
// The substitute is not a second renderer. `$tree_render` below is the only
// renderer there is and it is the one a browser runs; a test drives the
// shipping code against a document it can look at. What a substitute cannot
// cover is only what a browser itself does — layout, painting, focus order, and
// the browser's own dispatch of a press — and that needs a browser rather than
// a stand-in for one.
//
// Only three kinds of node exist, because the tree vocabulary needs no more:
// an element, a run of text, and a marker. A marker is a comment in a real
// document; it holds the place of a region that can change, so that removing
// what the region rendered last time is "everything between these two", which
// stays right when regions nest.

const $dom = { identities: 0, body: null };

function $dom_make(kind, name) {
  $dom.identities++;
  return {
    $shim: true,
    // 0 element, 1 text, 2 marker.
    kind,
    name,
    // Given once, at creation, and never given to another node. This is what
    // lets a test tell a row that was moved from a row that was rebuilt.
    identity: $dom.identities,
    attributes: {},
    classes: "",
    styles: {},
    children: [],
    parent: null,
    listeners: {},
    data: "",
    value: "",
    checked: false,
    disabled: false,
    // A `<dialog>`'s own flag. In a browser this is the property the element
    // reflects; here it is what `$dom_markup` writes and what says which side
    // of an open modal an element is on.
    open: false,
  };
}

// The three constructors take the parent they are destined for, because which
// document a node belongs to is decided by what it will hang off rather than
// by what the platform happens to have.
// `svg` puts the element in the SVG namespace, which is what an icon needs: a
// browser decides what an element *is* by its namespace, and an `<svg>` made
// with `createElement` is an unknown HTML element that paints nothing. The
// substitute has one namespace, because markup is all it answers.
const $DOM_SVG_NS = "http://www.w3.org/2000/svg";

function $dom_element(parent, name, svg) {
  if (parent.$shim) return $dom_make(0, name);
  return svg ? document.createElementNS($DOM_SVG_NS, name) : document.createElement(name);
}

function $dom_text(parent, data) {
  return parent.$shim ? $dom_data($dom_make(1, ""), data) : document.createTextNode(data);
}

function $dom_marker(parent) {
  return parent.$shim ? $dom_make(2, "") : document.createComment("");
}

// A `Text` node has `data` in a real document too, so this is an assignment in
// both. It answers the node so that a constructor can end with it.
function $dom_data(node, data) {
  node.data = data;
  return node;
}

// Inserting a node that is already in the tree moves it, in both documents.
// The keyed reconciler depends on that: a row that is still in the list is
// moved, and moving it is what keeps it the same node.
function $dom_insert(parent, node, before) {
  if (!parent.$shim) {
    parent.insertBefore(node, before);
    return;
  }
  if (node.parent !== null) $dom_remove(node);
  node.parent = parent;
  const at = before === null ? parent.children.length : parent.children.indexOf(before);
  parent.children.splice(at, 0, node);
}

function $dom_remove(node) {
  if (!node.$shim) {
    if (node.parentNode !== null) node.parentNode.removeChild(node);
    return;
  }
  const parent = node.parent;
  if (parent === null) return;
  const at = parent.children.indexOf(node);
  if (at >= 0) parent.children.splice(at, 1);
  node.parent = null;
}

function $dom_next(parent, node) {
  if (!parent.$shim) return node.nextSibling;
  const at = parent.children.indexOf(node);
  return at < 0 || at + 1 >= parent.children.length ? null : parent.children[at + 1];
}

// Everything strictly between two markers, which is exactly what a region
// rendered last time — including whatever a region nested inside it rendered.
function $dom_between(parent, start, end) {
  const out = [];
  if (!parent.$shim) {
    for (let n = start.nextSibling; n !== null && n !== end; n = n.nextSibling) out.push(n);
    return out;
  }
  const from = parent.children.indexOf(start);
  const to = parent.children.indexOf(end);
  for (let i = from + 1; i < to; i++) out.push(parent.children[i]);
  return out;
}

function $dom_attribute(element, name, value) {
  if (element.$shim) {
    element.attributes[name] = value;
    return;
  }
  element.setAttribute(name, value);
}

// An attribute that is there or is not, rather than one that says `true` or
// `false`. `aria-invalid="false"` on every control in a page is markup nobody
// asked for, and the rule the sheet writes for `On(.Invalid, …)` asks whether
// the attribute is there and says `true`.
function $dom_flag(element, name, on) {
  if (element.$shim) {
    if (on) element.attributes[name] = "true";
    else delete element.attributes[name];
    return;
  }
  if (on) element.setAttribute(name, "true");
  else element.removeAttribute(name);
}

// An attribute whose empty value means "not there". A field's hint is the one:
// `placeholder=""` is an attribute that says nothing, and a hint the program
// cleared has to leave rather than linger as an empty one.
function $dom_optional(element, name, value) {
  if (value === "") {
    if (element.$shim) delete element.attributes[name];
    else element.removeAttribute(name);
    return;
  }
  $dom_attribute(element, name, value);
}

// Open or shut, in the top layer. `showModal` is the whole of what the widget
// is for: the browser stops the page behind from scrolling, makes it inert,
// paints the `::backdrop`, traps the focus and answers Escape — none of which
// a style can say and none of which a `<div>` can have.
//
// A server wrote `<dialog open>`, which is open and *not* modal, and
// `showModal` on an open dialog throws. So a resume shuts it first: that is
// what promotes the panel the reader is already looking at into the top layer.
function $dom_modal(element, on) {
  if (element.$shim) {
    element.open = on;
    return;
  }
  if (element.open) element.close();
  if (on) element.showModal();
}

// The classes an element has, all of them at once. Replacing rather than
// adding is what makes re-applying a style list idempotent: a `When` that
// switched back has to lose the class it gained.
function $dom_classes(element, value) {
  if (element.$shim) {
    element.classes = value;
    return;
  }
  // Nothing to say is nothing to write. Assigning "" to an element that has no
  // class *adds* `class=""` to it, which on a resume is markup the server did
  // not write appearing on every element the reader can see.
  // An SVG element's `className` is a read-only `SVGAnimatedString`, so an
  // icon's classes went nowhere at all. The attribute is the same thing on
  // every other element and the only thing that works on this one.
  if (element.namespaceURI === $DOM_SVG_NS) {
    if (value === "" && element.getAttribute("class") === null) return;
    element.setAttribute("class", value);
    return;
  }
  if (value === "" && element.className === "") return;
  element.className = value;
}

// The inline declarations an element has, all of them at once, for the same
// reason. Everything static is a class, so what is left here is small.
function $dom_styles(element, declarations) {
  if (element.$shim) {
    element.styles = {};
    for (const entry of declarations) element.styles[entry[0]] = entry[1];
    return;
  }
  // `$dom_classes`'s rule, for the same attribute-shaped reason.
  if (declarations.size === 0 && element.style.cssText === "") return;
  element.style.cssText = "";
  for (const entry of declarations) element.style.setProperty(entry[0], entry[1]);
}

function $dom_listen(element, type, handler) {
  if (element.$shim) {
    element.listeners[type] = handler;
    return;
  }
  element.addEventListener(type, handler);
}

// The top of the tree an element hangs off. In a real document that is the
// `document` the listener a dismissable overlay registers goes on; in the
// substitute it is the host `render` built, which is where a test's press is
// dispatched from, so the two documents answer the same question the same way.
function $dom_root(node) {
  if (!node.$shim) return node.ownerDocument || document;
  let at = node;
  while (at.parent !== null) at = at.parent;
  return at;
}

// Whether `target` is `element` or sits inside it. This is the whole of what an
// outside press is: a press whose target this answers `false` for is one that
// landed outside the subtree.
function $dom_within(element, target) {
  if (!element.$shim) return element === target || element.contains(target);
  for (let at = target; at !== null && at !== undefined; at = at.parent) {
    if (at === element) return true;
  }
  return false;
}

// Registers `onDown` to see every press on the document `element` is in, and
// answers the release that takes it away again. A browser hears the press
// through a capturing `pointerdown`, so an overlay shuts before the press it
// landed on is acted on — the way Basecoat closes a dropdown, a popover or a
// select. The substitute keeps the same listeners in a list on its host, which
// a test's press walks.
function $dom_outside(element, onDown) {
  const root = $dom_root(element);
  if (!element.$shim) {
    root.addEventListener("pointerdown", onDown, true);
    return () => root.removeEventListener("pointerdown", onDown, true);
  }
  const listeners = root.outside === undefined ? (root.outside = []) : root.outside;
  listeners.push(onDown);
  return () => {
    const at = listeners.indexOf(onDown);
    if (at >= 0) listeners.splice(at, 1);
  };
}

// Where `mount` puts a tree. A program built for a browser and run under `bun`
// mounts into the substitute rather than failing: what it is being asked is
// whether the tree builds and reacts, and that question has an answer without
// a browser.
function $dom_body() {
  // A real document is always the real one, even when it has no body yet —
  // mounting a page into a substitute because the browser had not parsed its
  // body would be the worst of both, so that answers nothing and `mount`
  // reports it.
  if (typeof document !== "undefined") return document.body || null;
  if ($dom.body === null) $dom.body = $dom_make(0, "body");
  return $dom.body;
}

// --- Reading the substitute document ----------------------------------------
//
// Only the substitute: these are what `platform/effect/testing` is, and a test holds one of
// its trees. Markers are left out of the markup deliberately — they are the
// runtime's own bookkeeping, not something a reader sees, and pinning a test to
// them would pin it to how a region is anchored.

function $dom_escape(text, quotes) {
  let out = text.split("&").join("&amp;").split("<").join("&lt;").split(">").join("&gt;");
  if (quotes) out = out.split('"').join("&quot;");
  return out;
}

// The elements this vocabulary writes that hold nothing. A trailing slash
// closes one of these and *nothing else*: an HTML parser reads `<div />` as an
// opening `<div>` and puts everything after it inside, so an empty region
// written that way swallows the rest of the page. Every other name gets its
// closing tag, empty or not.
const $DOM_VOID = { img: true, input: true, hr: true };

function $dom_markup(node) {
  if (node.kind === 2) return "";
  if (node.kind === 1) return $dom_escape(node.data, false);
  let out = "<" + node.name;
  for (const name of Object.keys(node.attributes)) {
    out += " " + name + '="' + $dom_escape(node.attributes[name], true) + '"';
  }
  if (node.classes !== "") out += ' class="' + $dom_escape(node.classes, true) + '"';
  // What is typed into a `textarea` is its text and not an attribute, which is
  // the one element where the two spellings are not the same markup.
  if (node.name === "textarea") return out + ">" + $dom_escape(node.value, false) + "</textarea>";
  if (node.value !== "") out += ' value="' + $dom_escape(node.value, true) + '"';
  if (node.checked) out += " checked";
  if (node.disabled) out += " disabled";
  if (node.open) out += " open";
  const styles = Object.keys(node.styles);
  if (styles.length > 0) {
    const parts = [];
    for (const property of styles) parts.push(property + ": " + node.styles[property]);
    out += ' style="' + $dom_escape(parts.join("; "), true) + '"';
  }
  if ($DOM_VOID[node.name]) return out + " />";
  let inner = "";
  for (const child of node.children) inner += $dom_markup(child);
  return out + ">" + inner + "</" + node.name + ">";
}

// --- The tree ---------------------------------------------------------------
//
// `ui/node`'s vocabulary, lowered. A `Node` is the one-field struct that keeps
// the tree opaque, so it is `[kind]`; the kind inside is `[tag, ...payload]`
// and the tags are the order `ui/node` declares `NodeKind`'s variants in:
//
//   0 Empty    1 Text    2 Heading  3 Stack   4 Region  5 Button  6 Link
//   7 Image    8 Icon    9 Field   10 Slider 11 Toggle 12 Picker 13 Form
//  14 Submit  15 Dialog 16 Disclosure 17 Progress 18 When 19 Computed 20 Each
//  21 Raw     22 Fragment
//
// Every element-producing node ends with an `Events` struct — eight optional
// listeners `$tree_events` wires on — and `Raw` and `Fragment` are the
// renderer's own, minted by a reactive widget's rebuild rather than by any
// program.
//
// A component runs once. What re-runs is what `When`, `Computed` and `Each`
// stand for, and each re-runs the smallest thing it can: a `Prop` on a leaf
// changes one run of text or one attribute; `When` and `Computed` rebuild one
// subtree; and `Each` moves the rows that are still there and builds only the
// rows that are not.

// Meaning, lowered. Each entry is an element name followed by attribute
// name-and-value pairs — the `role=` fallback of design/ui-reactivity.md, used
// wherever HTML has no element that carries the meaning by itself.
const $TREE_ROLES = [
  ["nav"],
  ["main"],
  ["header"],
  ["footer"],
  ["aside"],
  ["article"],
  ["search", "role", "search"],
  ["ul"],
  ["li"],
  ["div", "role", "group"],
  ["hr"],
  ["div", "role", "status", "aria-live", "polite"],
  ["div", "role", "alert", "aria-live", "assertive"],
  ["table"],
  ["tr"],
  ["th", "scope", "row"],
  ["th", "scope", "col"],
  ["td"],
];

// `FieldKind`, lowered. `Multiline` is a `textarea` and has no type; the entry
// keeps the arrays the same shape. No variant carries a payload any more — a
// number picked off a track is the `slider` widget — so the enum lowers to a
// bare tag and a kind is read with no unwrap.
const $TREE_FIELD_KINDS = [
  "text",
  "text",
  "password",
  "email",
  "number",
  "search",
];

const $TREE_WEIGHTS = ["400", "500", "600", "700"];

const $TREE_ALIGNMENTS = [
  "flex-start",
  "center",
  "flex-end",
  "stretch",
  "space-between",
  "space-around",
  "space-evenly",
];

// Text has no leftover room to distribute, so every distribution means
// justified. The compiler's own table says the same thing (`semantics::styles`).
const $TREE_TEXT_ALIGNMENTS = [
  "start",
  "center",
  "end",
  "justify",
  "justify",
  "justify",
  "justify",
];

const $TREE_CURSORS = ["default", "pointer", "text", "not-allowed"];

const $TREE_LIST_MARKERS = ["none", "disc", "decimal"];

// Logical edges, so a right-to-left page is right by construction.
const $TREE_EDGES = ["block-start", "block-end", "inline-start", "inline-end"];
// CSS names a corner by the two logical edges that meet at it, block first,
// which is the order `Corner` declares them in.
const $TREE_CORNERS = ["start-start", "start-end", "end-start", "end-end"];

// `PinAnchor` is `fixed` as well: it takes the box out of every scroll
// container's clip. The painter keeps it against its `Flow` ancestor from the
// `--buri-pin-anchor` marker the stylesheet writes beside it; a browser stays
// on the viewport until CSS anchor positioning closes that gap.
const $TREE_POSITIONS = ["relative", "sticky", "fixed", "fixed"];

const $TREE_BORDER_STYLES = ["none", "solid", "dashed"];

const $TREE_TEXT_CASES = ["none", "uppercase", "lowercase", "capitalize"];

const $TREE_TEXT_LINES = ["none", "underline", "line-through"];

const $TREE_TEXT_WRAPS = ["wrap", "nowrap", "balance"];

// `flex-direction`, in `Layout`'s own order. The two reversed ones paint
// backwards and leave the document's order alone.
const $TREE_DIRECTIONS = ["column", "row", "column-reverse", "row-reverse"];

const $TREE_FONTS = [
  "ui-sans-serif,system-ui,sans-serif",
  "ui-serif,Georgia,serif",
  "ui-monospace,SFMono-Regular,monospace",
];

// One `name` per radio group rendered, so two groups on a page never share a
// selection. It has to be unique rather than meaningful — a browser groups the
// radios that carry one `name`, and only a group's own radios may — so a
// counter is the whole of it. It never resets, which is what keeps a group
// rendered after a reconcile from colliding with one rendered before it.
let $tree_radio_groups = 0;

// The stylesheet the compiler extracted, assigned by one statement the backend
// emits ahead of the program and empty in a program that styles nothing.
// Nothing here ever writes to it: every rule in it was written at compile time,
// which is what "nothing is generated at run time" means.
let $ui_sheet = "";

// The inline tier's lowering, reached through a hole rather than by name.
//
// `$tree_declare` below is the run-time lowering of all fifty-three properties
// and is 3.5 KB of an artifact. `$tree_style_collect` is the only thing that
// needs it, and a call by name is a reference dead-code elimination cannot
// argue with — so every user interface carried the whole tier, including one
// whose styles are all static and are therefore all classes before the artifact
// is written. The backend assigns this when `Program::inline_styles` says some
// style in the program can reach the tier, and emits nothing when it cannot,
// which is the same mechanism `$ui_sheet` above uses.
let $tree_declare_hook = null;

// The artwork renderer, through the same kind of hole and for the same reason:
// `$tree_icon` below is a parser and two allow lists, 2.5 KB of an artifact,
// and only a tree holding an `icon` ever reaches it. The backend assigns this
// when `Program::icons` says the program can build one.
let $tree_icon_hook = null;

// The theme installer, through the same kind of hole and for the same reason:
// `$ui_node_mount` installs themes before it renders, so the seven functions
// under "Themes" below — 1.7 KB of resolution, rendering and switching — shipped
// in every user interface, including one with no design tokens, which can only
// ever pass an empty list. The backend assigns this when `Program::themes` says
// the program can build one.
let $ui_theme_hook = null;

function $tree_length(length) {
  const tag = length[0];
  if (tag === 0) return length[1] + "px";
  if (tag === 1) return length[1] + "rem";
  // A rem follows the root's text size and an em follows this element's.
  if (tag === 2) return length[1] + "em";
  if (tag === 3) return length[1] + "%";
  if (tag === 4) return "auto";
  return "100%";
}

function $tree_color(color) {
  const tag = color[0];
  if (tag === 0) return "rgb(" + color[1] + "," + color[2] + "," + color[3] + ")";
  if (tag === 1) {
    return "rgba(" + color[1] + "," + color[2] + "," + color[3] + "," + color[4] + ")";
  }
  // A design token, in the inline tier. The same custom property the compiler
  // writes into the stylesheet, so a style that folded and one that did not
  // look the same on the page.
  if (tag === 2) return "var(--" + $ui_theme_name(color[1]) + ")";
  if (tag === 3) return "transparent";
  if (tag === 4) return "inherit";
  // A faded token. The token stays a token, so a theme decides the hue and the
  // mix decides only how much of it there is.
  return (
    "color-mix(in srgb,var(--" +
    $ui_theme_name(color[1]) +
    ") " +
    color[2] * 100 +
    "%,transparent)"
  );
}

// One layer of a `box-shadow`.
function $tree_shadow(shadow) {
  return (
    $tree_length(shadow[0]) +
    " " +
    $tree_length(shadow[1]) +
    " " +
    $tree_length(shadow[2]) +
    " " +
    $tree_length(shadow[3]) +
    " " +
    $tree_color(shadow[4])
  );
}

function $tree_track(track) {
  const tag = track[0];
  if (tag === 0) return track[1] + "fr";
  if (tag === 1) return $tree_length(track[1]);
  return "auto";
}

function $tree_font(family) {
  if (family[0] === 3) return '"' + family[1] + '",ui-sans-serif,sans-serif';
  return $TREE_FONTS[family[0]];
}

// Applies a style list to an element.
//
// A static style arrived from the compiler already extracted: a conflict slot,
// and the name of a class that is already in the stylesheet. Everything else —
// a `Computed`, and anything the compiler could not evaluate — is written out
// inline. Nothing here builds a rule.
//
// A list holding a `When` or a `Computed` is applied inside a computation, so
// that a change re-picks the classes and re-serialises the inline half; a list
// holding neither is applied once and registers nothing at all. That is the
// whole cost difference between the two tiers.
function $tree_styles(element, styles) {
  if ($tree_style_static(styles)) {
    $tree_style_apply(element, styles, null);
    return;
  }
  $ui_run(
    $ui_cell(2, undefined, (scope) => {
      $tree_style_apply(element, styles, scope);
      return 0;
    }),
  );
}

// Whether applying this list can be done once. `Group` is transparent; the two
// reactive constructors are not.
function $tree_style_static(styles) {
  for (const style of styles) {
    const tag = style[0];
    if (tag === 3 || tag === 4) return false;
    if (tag === 0 && !$tree_style_static(style[1])) return false;
  }
  return true;
}

// Collects the classes the list chose and the declarations it has to write
// out, then applies both at once — so a re-run replaces an element's styling
// rather than adding to it.
function $tree_style_apply(element, styles, scope) {
  const slots = new Map();
  const inline = new Map();
  $tree_style_collect(styles, scope, slots, inline);
  $dom_classes(element, Array.from(slots.values()).join(" "));
  $dom_styles(element, inline);
}

function $tree_style_collect(styles, scope, slots, inline) {
  for (const style of styles) {
    const tag = style[0];
    if (tag === 5) {
      // Compiler-assigned `(slot, class)` pairs. Last slot wins, and every
      // name is one the stylesheet already has.
      for (const pair of style[1][0]) slots.set(pair[0], pair[1]);
    } else if (tag === 0) {
      $tree_style_collect(style[1], scope, slots, inline);
    } else if (tag === 3) {
      const branch = $tree_value(style[1], scope) ? style[2] : style[3];
      $tree_style_collect(branch, scope, slots, inline);
    } else if (tag === 4) {
      $tree_style_collect(style[1](scope), scope, slots, inline);
    } else if (tag === 1 || tag === 2) {
      // Reachable only from a program the compiler could not evaluate under a
      // condition, which it rejects — so this is the invariant, said out loud.
      $abort("a pseudo-class or a breakpoint exists only in the stylesheet");
    } else if ($tree_declare_hook !== null) {
      const abbreviation = $TREE_VAR_ABBR[tag];
      if (abbreviation !== undefined) {
        // A dynamic value the compiler could not fold: write it out as the
        // `--buri-…` custom property a stylesheet rule reads, and take that
        // rule's class, so an `At`/`On` rule can still win over it (#195). The
        // one declaration `$tree_declare` produces is the value; the class the
        // compiler already put in the sheet reads it back.
        const declared = new Map();
        $tree_declare_hook(style, declared);
        for (const value of declared.values()) inline.set("--buri-" + abbreviation, value);
        slots.set($tree_var_slot(tag), abbreviation + "-var");
      } else {
        $tree_declare_hook(style, inline);
      }
    } else {
      // The compiler said no style here could reach the inline tier, so it
      // left the lowering out of the artifact. Reaching this is that decision
      // being wrong, and saying so beats a `TypeError` about `null`.
      $abort("a style reached the inline tier in a program that was said to have none");
    }
  }
}

// A dynamic value's variable-backed lowering (#195): the class abbreviation per
// `Style` variant, and the conflict slot it takes — `styles.rs::slot(variant,
// 0, plain)`, which is `variant * SUB_KEYS * CONDITIONS`. Only the properties
// `styles.rs::variable_property` names; the rest still write out inline. Kept
// in step with that table and with `ui_node.buri`'s `variableProperty`.
const $TREE_VAR_ABBR = {
  12: "grow",
  13: "shrink",
  14: "span",
  17: "gap",
  18: "gapx",
  19: "gapy",
  20: "p",
  21: "px",
  22: "py",
  24: "w",
  25: "h",
  26: "minw",
  27: "maxw",
  28: "minh",
  29: "maxh",
  30: "ar",
  31: "bg",
  32: "fg",
  35: "bc",
  37: "r",
  39: "op",
  40: "sh",
  41: "sh",
  43: "fs",
  46: "lh",
  47: "ls",
  59: "bdblur",
  60: "caret",
};

function $tree_var_slot(tag) {
  return (tag === 41 ? 40 : tag) * 200;
}

// One property, written out as inline declarations.
//
// This is the tier a style lands in when the compiler could not evaluate it,
// and the tier `Computed` always lands in. A static style never reaches here:
// it arrived as a class. The two lowerings are deliberately the same CSS —
// `semantics::styles::declaration` is the other half — so that whether a style
// folded changes what it costs and not what it looks like.
function $tree_declare(style, out) {
  const tag = style[0];
  const value = style[1];
  if (tag === 6) {
    if (value[0] === 4) {
      out.set("display", "grid");
      out.set("grid-template-columns", value[1].map($tree_track).join(" "));
    } else if (value[0] === 5) {
      // The children's half of `Layers` — every child in one cell — is a rule
      // about descendants, which an element's own style attribute cannot say.
      // A `Layers` that reached this tier stacks nothing.
      out.set("display", "grid");
    } else {
      out.set("display", "flex");
      out.set("flex-direction", $TREE_DIRECTIONS[value[0]]);
    }
  } else if (tag === 7) {
    out.set("justify-content", $TREE_ALIGNMENTS[value]);
  } else if (tag === 8) {
    out.set("align-items", $TREE_ALIGNMENTS[value]);
  } else if (tag === 9) {
    out.set("align-self", $TREE_ALIGNMENTS[value]);
  } else if (tag === 10) {
    out.set("flex-wrap", value ? "wrap" : "nowrap");
  } else if (tag === 11) {
    if (value === 0) out.set("overflow-x", "auto");
    else if (value === 1) out.set("overflow-y", "auto");
    else out.set("overflow", "auto");
  } else if (tag === 12) {
    out.set("flex-grow", String(value));
  } else if (tag === 13) {
    out.set("flex-shrink", String(value));
  } else if (tag === 14) {
    out.set("grid-column", "span " + value);
  } else if (tag === 15) {
    out.set("position", "absolute");
    out.set("inset-" + $TREE_EDGES[value], $tree_length(style[2]));
  } else if (tag === 16) {
    out.set("position", $TREE_POSITIONS[value]);
    // The painter's marker, so a reactively-applied `PinAnchor` reads the same
    // as the stylesheet's class does; a browser ignores the custom property.
    if (value === 3) out.set("--buri-pin-anchor", "1");
  } else if (tag === 17) {
    out.set("gap", $tree_length(value));
  } else if (tag === 18) {
    out.set("column-gap", $tree_length(value));
  } else if (tag === 19) {
    out.set("row-gap", $tree_length(value));
  } else if (tag === 20) {
    out.set("padding", $tree_length(value));
  } else if (tag === 21) {
    // Logical rather than left-and-right, so a right-to-left page is right by
    // construction rather than by a second stylesheet.
    out.set("padding-inline", $tree_length(value));
  } else if (tag === 22) {
    out.set("padding-block", $tree_length(value));
  } else if (tag === 23) {
    out.set("padding-" + $TREE_EDGES[value], $tree_length(style[2]));
  } else if (tag === 24) {
    out.set("width", $tree_length(value));
  } else if (tag === 25) {
    out.set("height", $tree_length(value));
  } else if (tag === 26) {
    out.set("min-width", $tree_length(value));
  } else if (tag === 27) {
    out.set("max-width", $tree_length(value));
  } else if (tag === 28) {
    out.set("min-height", $tree_length(value));
  } else if (tag === 29) {
    out.set("max-height", $tree_length(value));
  } else if (tag === 30) {
    out.set("aspect-ratio", String(value));
  } else if (tag === 31) {
    out.set("background-color", $tree_color(value));
  } else if (tag === 32) {
    out.set("color", $tree_color(value));
  } else if (tag === 33) {
    // A width on its own draws a solid border, because a border nobody can see
    // is not what asking for one means. `BorderStyle` is applied after it.
    out.set("border-style", "solid");
    out.set("border-width", $tree_length(value));
  } else if (tag === 34) {
    // One edge, and the same solid a whole-box width implies. `BorderStyle` is
    // applied after it, and `BorderColor` stays whole-box.
    const edge = "border-" + $TREE_EDGES[value];
    out.set(edge + "-style", "solid");
    out.set(edge + "-width", $tree_length(style[2]));
  } else if (tag === 35) {
    out.set("border-color", $tree_color(value));
  } else if (tag === 36) {
    out.set("border-style", $TREE_BORDER_STYLES[value]);
  } else if (tag === 37) {
    out.set("border-radius", $tree_length(value));
  } else if (tag === 38) {
    out.set("border-" + $TREE_CORNERS[value] + "-radius", $tree_length(style[2]));
  } else if (tag === 39) {
    out.set("opacity", String(value));
  } else if (tag === 40) {
    out.set("box-shadow", $tree_shadow(value));
  } else if (tag === 41) {
    // One declaration, the layers in the order they were written — which is
    // the order a browser paints them, first over last.
    out.set("box-shadow", value.map($tree_shadow).join(","));
  } else if (tag === 42) {
    out.set("font-family", $tree_font(value));
  } else if (tag === 43) {
    out.set("font-size", $tree_length(value));
  } else if (tag === 44) {
    out.set("font-weight", $TREE_WEIGHTS[value]);
  } else if (tag === 45) {
    out.set("font-style", value ? "italic" : "normal");
  } else if (tag === 46) {
    out.set("line-height", String(value));
  } else if (tag === 47) {
    out.set("letter-spacing", $tree_length(value));
  } else if (tag === 48) {
    out.set("text-align", $TREE_TEXT_ALIGNMENTS[value]);
  } else if (tag === 49) {
    out.set("text-transform", $TREE_TEXT_CASES[value]);
  } else if (tag === 50) {
    out.set("text-decoration-line", $TREE_TEXT_LINES[value]);
  } else if (tag === 51) {
    out.set("text-wrap", $TREE_TEXT_WRAPS[value]);
  } else if (tag === 52) {
    if (value > 0) {
      out.set("display", "-webkit-box");
      out.set("-webkit-box-orient", "vertical");
      out.set("-webkit-line-clamp", String(value));
      out.set("overflow", "hidden");
    } else {
      out.set("-webkit-line-clamp", "none");
      out.set("overflow", "visible");
    }
  } else if (tag === 53) {
    out.set("cursor", $TREE_CURSORS[value]);
  } else if (tag === 54) {
    out.set("list-style-type", $TREE_LIST_MARKERS[value]);
  } else if (tag === 55) {
    out.set("margin-" + $TREE_EDGES[value], $tree_outwards(style[2]));
  } else if (tag === 56) {
    out.set("transform", "translate(" + $tree_length(value) + "," + $tree_length(style[2]) + ")");
  } else if (tag === 57) {
    // `clip` rather than `hidden`: both stop the paint, and only `hidden` also
    // makes a scroll container a keyboard can land in.
    out.set("overflow", value ? "clip" : "visible");
  } else if (tag === 58) {
    out.set("pointer-events", value ? "none" : "auto");
  } else if (tag === 59) {
    // Only the page behind the box is blurred; the box paints over the blur.
    out.set("backdrop-filter", "blur(" + $tree_length(value) + ")");
  } else if (tag === 60) {
    // The caret on its own, so a field with transparent text still has a
    // visible one. `Foreground` is otherwise the only thing that reaches it.
    out.set("caret-color", $tree_color(value));
  } else {
    // Out of sight, in the tree: the "sr-only" clip. The element paints nothing
    // and takes no space while its text stays for the accessible name. The
    // `--buri-visually-hidden` marker is what the snapshot painter reads; a
    // browser ignores the custom property.
    out.set("position", "absolute");
    out.set("width", "1px");
    out.set("height", "1px");
    out.set("padding", "0");
    out.set("margin", "-1px");
    out.set("overflow", "hidden");
    out.set("clip", "rect(0,0,0,0)");
    out.set("white-space", "nowrap");
    out.set("border", "0");
    out.set("--buri-visually-hidden", "1");
  }
}

// A bleed's length, as the margin it writes: a distance outwards, so the margin
// is its negation. A negative distance and `Auto` — which is no distance at all
// — bleed nothing, because inward is the space between things and that belongs
// to the container.
function $tree_outwards(length) {
  const tag = length[0];
  if (tag === 5) return "-100%";
  if (tag === 4 || !(length[1] > 0)) return "0px";
  if (tag === 0) return "-" + length[1] + "px";
  if (tag === 1) return "-" + length[1] + "rem";
  return "-" + length[1] + (tag === 2 ? "em" : "%");
}

// A `Prop<T>` is `[tag, payload]`: 0 Const, 1 Cell, 2 Computed.
function $tree_value(prop, scope) {
  const tag = prop[0];
  if (tag === 0) return prop[1];
  if (tag === 1) return $ui_read(prop[1][0]);
  return prop[1](scope);
}

// Applies a prop now, and again whenever it changes. A `Const` registers
// nothing — that is the whole reason it is a visible constructor — so a static
// interface holds no computations at all.
function $tree_bind(prop, apply) {
  if (prop[0] === 0) {
    apply(prop[1]);
    return;
  }
  $ui_run(
    $ui_cell(2, undefined, (scope) => {
      apply($tree_value(prop, scope));
      return 0;
    }),
  );
}

// Whether a control refuses what a reader does to it. An attribute rather than
// a style: it is what takes the control out of the tab order, what refuses the
// press before a handler is reached, and what tells a reader the control is
// unavailable rather than absent. `element.disabled` is the property in both
// documents — a real element reflects it into the attribute, and the substitute
// holds it in the field `$dom_markup` writes out.
function $tree_disabled(element, prop) {
  $tree_bind(prop, (off) => {
    element.disabled = off;
  });
}

// --- Resuming what a server rendered ------------------------------------------
//
// `ops` while a resume runs, and null the rest of the time. Everything under it
// is named in `$ui_web_resume` and nowhere else, so a program that mounts drops
// all of it and keeps the comparisons.

const $adopt = { ops: null, at: new Map() };

// One renderer. A resume runs `$tree_render`, the walk a mount runs, and what
// changes is where a node comes from: the two constructors answer with the node
// already sitting in the document instead of making one. So the page registers
// every computation and every listener a fresh mount would — the button works —
// and the markup the reader is looking at is the markup that arrived.
//
// `$adopt.at` is the node each parent has left to offer. A parent is entered
// once, because the renderer walks a tree, so a map keyed by the parent is the
// whole of the bookkeeping.
function $adopt_first(parent) {
  if (parent.$shim) return parent.children.length > 0 ? parent.children[0] : null;
  return parent.firstChild;
}

// The node this parent has to offer now, or null where it has run out.
function $adopt_at(parent) {
  if (!$adopt.at.has(parent)) $adopt.at.set(parent, $adopt_first(parent));
  return $adopt.at.get(parent);
}

// 0 an element, 1 a run of text, 2 a marker — the substitute's own three kinds,
// which a real document spells as node types.
function $adopt_kind(node) {
  if (node.$shim) return node.kind;
  return node.nodeType === 1 ? 0 : node.nodeType === 3 ? 1 : 2;
}

function $adopt_name(node) {
  return node.$shim ? node.name : node.nodeName.toLowerCase();
}

// The node the server wrote where the renderer wants one. Anything else is the
// page and the server disagreeing about what the tree is, and a resume that
// guessed would leave the reader looking at both answers — so it stops here and
// `resume` answers `.Err` naming what it wanted and what was there.
function $adopt_claim(parent, kind, name) {
  const node = $adopt_at(parent);
  if (node !== null && $adopt_kind(node) === kind && (kind !== 0 || $adopt_name(node) === name)) {
    $adopt.at.set(parent, $dom_next(parent, node));
    return node;
  }
  const wanted = kind === 1 ? "a run of text" : "<" + name + ">";
  const found =
    node === null
      ? "nothing left"
      : $adopt_kind(node) === 1
        ? "a run of text"
        : "<" + $adopt_name(node) + ">";
  throw { $resume: "this page is not the markup the server sent: wanted " + wanted + ", found " + found };
}

// A browser parses two runs of text into one node, so a tree with two beside
// each other has to take its own back. What is left over becomes the node this
// parent offers next, which is exactly what the run after this one asks for.
function $adopt_split(parent, node, value) {
  if (node.data.length <= value.length || node.data.slice(0, value.length) !== value) return;
  if (!node.$shim) {
    $adopt.at.set(parent, node.splitText(value.length));
    return;
  }
  const rest = $dom_data($dom_make(1, ""), node.data.slice(value.length));
  $dom_insert(parent, rest, $adopt_at(parent));
  $adopt.at.set(parent, rest);
}

// What the tree did not account for. Everything a server wrote came out of the
// tree, so a node left over inside one is the same disagreement a missing node
// is, found from the other end — a page whose tree stops early would otherwise
// leave the rest of the markup on screen and dead. The body is the exception:
// `shell` puts the state script in it, and that is the server's own furniture
// rather than part of any tree.
function $adopt_leftovers(body) {
  for (const entry of $adopt.at) {
    if (entry[0] !== body && entry[1] !== null) {
      return (
        "this page is not the markup the server sent: <" +
        $adopt_name(entry[0]) +
        "> holds more than the tree does"
      );
    }
  }
  return null;
}

// A marker the server did not write, inserted where the walk has reached rather
// than at the anchor a fresh render would use.
function $tree_mark(parent, anchor) {
  const marker = $dom_marker(parent);
  $dom_insert(parent, marker, $adopt.ops === null ? anchor : $adopt.ops.at(parent));
  return marker;
}

function $tree_element(parent, name, anchor, svg) {
  if ($adopt.ops !== null) return $adopt.ops.claim(parent, 0, name);
  const element = $dom_element(parent, name, svg);
  $dom_insert(parent, element, anchor);
  return element;
}

function $tree_text(prop, parent, anchor) {
  if ($adopt.ops !== null) {
    // The run of text the server wrote may be several of these — a browser
    // parses one node however many the tree has — so the first value says how
    // much of it belongs here and `split` leaves the rest for the next run.
    const ops = $adopt.ops;
    const adopted = ops.claim(parent, 1, "");
    let first = true;
    $tree_bind(prop, (value) => {
      if (first) {
        first = false;
        ops.split(parent, adopted, value);
      }
      $dom_data(adopted, value);
    });
    return;
  }
  const node = $dom_text(parent, "");
  $dom_insert(parent, node, anchor);
  $tree_bind(prop, (value) => $dom_data(node, value));
}

function $tree_children(ctx, element, styles, children) {
  $tree_styles(element, styles);
  for (const child of children) $tree_render(ctx, child, element, null);
}

// A region whose contents are decided by something that can change. Two
// markers hold the place; a re-run removes everything between them and renders
// what `build` answers now. Everything the run created belongs to the run, so
// the computations inside a subtree are disposed with the subtree.
function $tree_dynamic(ctx, parent, anchor, build) {
  const start = $tree_mark(parent, anchor);
  // A region being adopted does not know where it ends until the markup for it
  // has been walked, so that marker goes in after the first run.
  let adopting = $adopt.ops !== null;
  const end = adopting ? $dom_marker(parent) : $tree_mark(parent, anchor);
  $ui_run(
    $ui_cell(2, undefined, (scope) => {
      if (adopting) {
        adopting = false;
        $tree_render(ctx, build(scope), parent, end);
        $dom_insert(parent, end, $adopt.ops.at(parent));
        return 0;
      }
      for (const node of $dom_between(parent, start, end)) $dom_remove(node);
      $tree_render(ctx, build(scope), parent, end);
      return 0;
    }),
  );
}

// One row of a keyed list: two markers of its own, so that moving it moves
// whatever it rendered, and an owner of its own, so that disposing it disposes
// what it created. The row is built under that owner and untracked — the list
// is already subscribed to the list, and what a row read while it was being
// built is not a reason to rebuild the list.
function $tree_row(ctx, parent, anchor, owner, key, index, rowAt) {
  const start = $tree_mark(parent, anchor);
  const adopting = $adopt.ops !== null;
  const end = adopting ? $dom_marker(parent) : $tree_mark(parent, anchor);
  const rowOwner = $ui_under(owner, () => $ui_cell(3, undefined, null));
  $ui_under(rowOwner, () => {
    $tree_render(ctx, rowAt(ctx, [rowOwner], index), parent, end);
    return 0;
  });
  if (adopting) $dom_insert(parent, end, $adopt.ops.at(parent));
  // `list` names the `each` the row is in. The markers carry the row, which is
  // how a pointer finds the row it is over (`$tree_rowOf`).
  const row = { key, start, end, owner: rowOwner, list: owner };
  start.$row = row;
  end.$row = row;
  return row;
}

function $tree_detach(parent, row) {
  const nodes = $dom_between(parent, row.start, row.end);
  $dom_remove(row.start);
  for (const node of nodes) $dom_remove(node);
  $dom_remove(row.end);
}

function $tree_move(parent, row, anchor) {
  if ($dom_next(parent, row.end) === anchor) return;
  const nodes = $dom_between(parent, row.start, row.end);
  $dom_insert(parent, row.start, anchor);
  for (const node of nodes) $dom_insert(parent, node, anchor);
  $dom_insert(parent, row.end, anchor);
}

// Keyed reconciliation. Walking backwards means the anchor for each row is the
// row that follows it, which is already in place, so one pass positions
// everything. A row whose key is still in the list is moved and never rebuilt:
// that is what keyed means, and it is what keeps the focus, the scroll
// position and the computations inside a row alive across a reorder.
function $tree_reconcile(ctx, parent, end, owner, rows, keys, rowAt) {
  // A resume walks forwards, so the rows the server wrote are taken in order.
  // Nothing moves on that pass: every row is already where it belongs.
  if ($adopt.ops !== null) {
    const adopted = [];
    for (let i = 0; i < keys.length; i++) {
      adopted.push($tree_row(ctx, parent, end, owner, keys[i], i, rowAt));
    }
    return adopted;
  }
  const byKey = new Map();
  for (const row of rows) byKey.set(row.key, row);
  const next = new Array(keys.length);
  let anchor = end;
  for (let i = keys.length - 1; i >= 0; i--) {
    const key = keys[i];
    let row = byKey.get(key);
    if (row === undefined) {
      row = $tree_row(ctx, parent, anchor, owner, key, i, rowAt);
    } else {
      byKey.delete(key);
      $tree_move(parent, row, anchor);
    }
    next[i] = row;
    anchor = row.start;
  }
  for (const row of byKey.values()) {
    $tree_detach(parent, row);
    $ui_dispose(row.owner);
    $ui_forget(owner, row.owner);
  }
  return next;
}

function $tree_each(ctx, parent, anchor, count, keyAt, rowAt) {
  const start = $tree_mark(parent, anchor);
  let adopting = $adopt.ops !== null;
  const end = adopting ? $dom_marker(parent) : $tree_mark(parent, anchor);
  // The rows hang off this rather than off the computation below, because that
  // computation re-runs and a row must survive it.
  const owner = $ui_cell(3, undefined, null);
  let rows = [];
  $ui_run(
    $ui_cell(2, undefined, (scope) => {
      const keys = [];
      const seen = new Set();
      const n = count(scope);
      for (let i = 0; i < n; i++) {
        const key = keyAt(scope, i);
        // Two rows with one key is not a thing to resolve: whichever way it is
        // resolved, one of the two rows is wrong, and the list will go on
        // rebuilding both. Refusing it at the point it happens is the only
        // report that names the key.
        if (seen.has(key)) $abort('two rows share the key "' + key + '"');
        seen.add(key);
        keys.push(key);
      }
      rows = $tree_reconcile(ctx, parent, end, owner, rows, keys, rowAt);
      if (adopting) {
        adopting = false;
        $dom_insert(parent, end, $adopt.ops.at(parent));
      }
      return 0;
    }),
  );
}

// The artwork an `icon` holds, lowered.
//
// An icon is drawn *in* the tree rather than pointed at, and that is the whole
// of what it is for: an `<svg>` in the document reads the colour of the element
// around it, so `currentColor` is whatever `Foreground` the cascade gives it
// and a theme switch recolours every icon for free. An `<img>` cannot — its
// source is a document of its own.
//
// Only these elements are built and only these attributes are set. The compiler
// already refused a source holding anything else (`icon-not-drawable`), so this
// is the second half of one rule rather than a check of its own: there is no
// path here that creates a script, an event handler, or a reference to anywhere
// else, whatever a source says. `xmlns` is left out because the element is in
// that namespace already, and setting it would put a second one beside the
// namespaced attribute a parser wrote.
const $TREE_ARTWORK = [
  "svg",
  "g",
  "path",
  "rect",
  "circle",
  "ellipse",
  "line",
  "polyline",
  "polygon",
];

const $TREE_ARTWORK_ATTRIBUTES = [
  "viewBox",
  "width",
  "height",
  "fill",
  "stroke",
  "stroke-width",
  "stroke-linecap",
  "stroke-linejoin",
  "opacity",
  "fill-opacity",
  "stroke-opacity",
  "transform",
  "d",
  "points",
  "x",
  "y",
  "x1",
  "y1",
  "x2",
  "y2",
  "cx",
  "cy",
  "r",
  "rx",
  "ry",
];

// The source as tags: `[name, attribute names and values, closing, empty]`.
// Nothing else in the document is read, because nothing else may be in one —
// no text, no comments, no declarations, no entities.

const $ARTWORK_SPACE = " \t\r\n";

function $tree_artwork_space(c) {
  return $ARTWORK_SPACE.indexOf(c) >= 0;
}

// How far a run of characters the predicate accepts reaches from `at`.
function $tree_artwork_run(source, at, ok) {
  let i = at;
  while (i < source.length && ok(source[i])) i++;
  return i;
}

function $tree_artwork_tags(source) {
  const out = [];
  const named = (c) => !$tree_artwork_space(c) && c !== "/" && c !== ">";
  let at = 0;
  for (;;) {
    const open = source.indexOf("<", at);
    if (open < 0) return out;
    let i = open + 1;
    const closing = source[i] === "/";
    if (closing) i++;
    const from = i;
    i = $tree_artwork_run(source, i, named);
    const name = source.slice(from, i);
    const attributes = [];
    let empty = false;
    for (;;) {
      i = $tree_artwork_run(source, i, $tree_artwork_space);
      if (i >= source.length || source[i] === ">") {
        i++;
        break;
      }
      // A self-closing tag's slash, which says only that the tag holds nothing.
      if (source[i] === "/") {
        empty = true;
        i++;
        continue;
      }
      const nameFrom = i;
      i = $tree_artwork_run(source, i, (c) => named(c) && c !== "=");
      const attribute = source.slice(nameFrom, i);
      i = $tree_artwork_run(source, i, $tree_artwork_space);
      let value = "";
      if (source[i] === "=") {
        i = $tree_artwork_run(source, i + 1, $tree_artwork_space);
        const quote = source[i];
        const quoted = quote === '"' || quote === "'";
        const valueFrom = quoted ? i + 1 : i;
        const ends = quoted ? (c) => c !== quote : (c) => named(c);
        i = $tree_artwork_run(source, valueFrom, ends);
        value = source.slice(valueFrom, i);
        if (quoted) i++;
      }
      attributes.push(attribute, value);
    }
    out.push([name, attributes, closing, empty]);
    at = i;
  }
}

// Answers the `<svg>` root it built, so the caller can wire a listener onto the
// drawing itself, and `null` when the source held no drawable root at all.
function $tree_icon(parent, styles, source, anchor) {
  const stack = [];
  let root = null;
  for (const tag of $tree_artwork_tags(source)) {
    if (tag[2]) {
      stack.pop();
      // Everything after the root's closing tag is outside the artwork.
      if (stack.length === 0) return root;
      continue;
    }
    if ($TREE_ARTWORK.indexOf(tag[0]) < 0) continue;
    const into = stack.length === 0 ? parent : stack[stack.length - 1];
    const element = $tree_element(into, tag[0], stack.length === 0 ? anchor : null, true);
    const attributes = tag[1];
    for (let i = 0; i + 1 < attributes.length; i += 2) {
      if ($TREE_ARTWORK_ATTRIBUTES.indexOf(attributes[i]) >= 0) {
        $dom_attribute(element, attributes[i], attributes[i + 1]);
      }
    }
    if (root === null) {
      root = element;
      // Decorative by construction: what an icon means is said by what it is
      // inside, and a reader told about the glyph as well hears it twice.
      $dom_attribute(element, "aria-hidden", "true");
      // The `<svg>` is the element, so the styles land on the artwork itself.
      $tree_styles(element, styles);
    }
    if (!tag[3]) stack.push(element);
    if (tag[3] && stack.length === 0) return root;
  }
  return root;
}

// The eight generic listeners a node may carry, wired onto its element. `events`
// is the `Events` struct every element-producing node ends with — an array of
// eight `Option<fn>`, each the handler itself or `undefined` — so a node that
// listens for nothing registers nothing. Every side-effecting call runs inside
// one transaction, the rule a press already follows: a handler that writes three
// signals causes one pass over the watchers rather than three.
function $tree_events(ctx, element, events) {
  const onHover = events[0];
  if (onHover !== undefined) {
    // Pointer enter is `true`, leave is `false` — the two edges of a hover.
    $dom_listen(element, "mouseenter", () => $ui_flush(() => onHover($taskApart(ctx), true)));
    $dom_listen(element, "mouseleave", () => $ui_flush(() => onHover($taskApart(ctx), false)));
  }
  const onFocus = events[1];
  if (onFocus !== undefined) {
    // `focusin`/`focusout` rather than `focus`/`blur`, so focus moving *within*
    // the subtree is one thing gaining focus rather than a leave and an enter.
    $dom_listen(element, "focusin", () => $ui_flush(() => onFocus($taskApart(ctx), true)));
    $dom_listen(element, "focusout", () => $ui_flush(() => onFocus($taskApart(ctx), false)));
  }
  const onScroll = events[2];
  if (onScroll !== undefined) {
    // A `ScrollOffset` is the struct `{x, y}`, so an array of the two.
    $dom_listen(element, "scroll", () =>
      $ui_flush(() => onScroll($taskApart(ctx), [element.scrollLeft, element.scrollTop])),
    );
  }
  const onKey = events[3];
  if (onKey !== undefined) {
    // The handler answers whether it claimed the key. When it did,
    // `preventDefault` suppresses the platform's own handling — Tab leaving the
    // field, Enter submitting the form, an arrow moving the caret, Escape
    // shutting the dialog — so a combobox or completion list can drive those
    // keys. The answer is captured out of the flushed run (which itself always
    // returns 0), and `preventDefault` still lands synchronously inside the
    // browser's keydown dispatch.
    $dom_listen(element, "keydown", (event) => {
      let consumed = false;
      $ui_flush(() => {
        consumed = onKey($taskApart(ctx), event.key);
        return consumed;
      });
      if (consumed === true) event.preventDefault();
    });
  }
  const onPressOutside = events[4];
  if (onPressOutside !== undefined) {
    // The mechanism the old `onPressOutside` widget used: a document-level press
    // whose target is not inside this element fires the handler. Registered
    // while the subtree is mounted and let go when it is disposed, so a wrapper
    // inside a `choose` that shuts leaves no listener on the document. The
    // handler is `fn(C)` — no event — so it takes only the context.
    const onDown = (event) => {
      const target = event ? event.target : null;
      if (target !== null && target !== undefined && $dom_within(element, target)) return;
      $ui_flush(() => onPressOutside($taskApart(ctx)));
    };
    $ui_dispose_with($dom_outside(element, onDown));
  }
  const onPointerDown = events[5];
  const onPointerMove = events[6];
  const onPointerUp = events[7];
  if (onPointerDown !== undefined || onPointerMove !== undefined || onPointerUp !== undefined) {
    // The innermost element with a pointer handler captures the pointer, so the
    // moves and the release reach it after the pointer leaves it — a drag
    // outrunning its row keeps going. The press bubbles on to the handlers
    // around it, which the event's mark keeps from taking the capture over.
    $dom_listen(element, "pointerdown", (event) => {
      if (event.$captured !== true && element.setPointerCapture !== undefined) {
        event.$captured = true;
        element.setPointerCapture(event.pointerId);
      }
      if (onPointerDown !== undefined) {
        $ui_flush(() => onPointerDown($taskApart(ctx), $tree_pointerAt(element, event)));
      }
    });
    if (onPointerMove !== undefined) {
      $dom_listen(element, "pointermove", (event) =>
        $ui_flush(() => onPointerMove($taskApart(ctx), $tree_pointerAt(element, event))),
      );
    }
    if (onPointerUp !== undefined) {
      $dom_listen(element, "pointerup", (event) =>
        $ui_flush(() => onPointerUp($taskApart(ctx), $tree_pointerAt(element, event))),
      );
    }
  }
}

// A `PointerAt`: the pointer against `element`'s box and against the viewport,
// and the key of the row under it in the list `element` is a row of.
function $tree_pointerAt(element, event) {
  const box = element.getBoundingClientRect();
  return [
    event.clientX - box.left,
    event.clientY - box.top,
    event.clientX,
    event.clientY,
    $tree_overRow(element, event.clientX, event.clientY),
  ];
}

// The innermost keyed row holding `node`, or null. `$tree_row` marks its two
// markers with the row, so walking back over the siblings finds the start of
// the row the node is in — skipping any row whose end came first, which is a
// whole row nested beside it — and a level up when there is none.
function $tree_rowOf(node) {
  for (let at = node; at !== null && at.parentNode !== null; at = at.parentNode) {
    const ended = new Set();
    for (let n = at.previousSibling; n !== null; n = n.previousSibling) {
      const row = n.$row;
      if (row === undefined) continue;
      if (n === row.end) ended.add(row);
      else if (!ended.has(row)) return row;
    }
  }
  return null;
}

// The key of the row of `element`'s list under the viewport point, looking
// through `element`'s own row so a row that follows the pointer reports the
// row beneath it — undefined when there is none.
function $tree_overRow(element, x, y) {
  const own = $tree_rowOf(element);
  if (own === null) return undefined;
  let found = undefined;
  for (const hit of element.ownerDocument.elementsFromPoint(x, y)) {
    for (let row = $tree_rowOf(hit); row !== null; row = $tree_rowOf(row.start)) {
      if (row.list !== own.list) continue;
      if (row !== own) return row.key;
      found = own.key;
      break;
    }
  }
  return found;
}

// Renders one node into `parent`, before `anchor` — or at the end of `parent`
// when there is none.
//
// A `Node` is the one-field struct `ui/node` keeps the tree opaque with, so
// the tagged value is one unwrap in.
function $tree_render(ctx, wrapper, parent, anchor) {
  const node = wrapper[0];
  const tag = node[0];
  if (tag === 0) {
    // Empty: no element, no text, no place held. A `when` that answers this is
    // a `when` whose region is empty, and its own markers hold the place.
    return;
  }
  if (tag === 1) {
    $tree_text(node[1], parent, anchor);
    return;
  }
  if (tag === 2) {
    // The level is the document's outline rather than a size. There is no
    // seventh level to lower to, so it clamps.
    const level = node[1] < 1 ? 1 : node[1] > 6 ? 6 : node[1];
    const element = $tree_element(parent, "h" + level, anchor);
    // The size and the weight are the styles', because the level is not one:
    // the sheet's reset drops what a browser paints on a heading by itself.
    $tree_styles(element, node[2]);
    $tree_text(node[3], element, null);
    return;
  }
  if (tag === 3) {
    const element = $tree_element(parent, "div", anchor);
    $tree_children(ctx, element, node[1], node[2]);
    $tree_events(ctx, element, node[3]);
    return;
  }
  if (tag === 4) {
    const role = $TREE_ROLES[node[1]];
    const element = $tree_element(parent, role[0], anchor);
    for (let i = 1; i + 1 < role.length; i += 2) $dom_attribute(element, role[i], role[i + 1]);
    $tree_children(ctx, element, node[2], node[3]);
    $tree_events(ctx, element, node[4]);
    return;
  }
  if (tag === 5) {
    const element = $tree_element(parent, "button", anchor);
    // Not a submit button: a form usually holds a Cancel as well, and a button
    // that submits the form it happens to be inside is the surprise this
    // vocabulary exists to remove. `submit` is the one that submits.
    $dom_attribute(element, "type", "button");
    // The label is the accessible name however the button is drawn, so it is
    // an attribute rather than the glyphs: a button holding an icon and a word
    // is still announced as the one thing the program named it.
    $tree_bind(node[1], (label) => $dom_attribute(element, "aria-label", label));
    // The styles go through `$tree_styles`, the same reactive path `stack`
    // reaches through `$tree_children` (issue #164): a `.When`/`.Computed` class
    // swap on a button is watched and updates live rather than being applied
    // once.
    $tree_styles(element, node[2]);
    const children = node[3];
    // A button with no children shows its label. That is the only place the
    // name and the glyphs are the same string.
    if (children.length === 0) $tree_text(node[1], element, null);
    else for (const child of children) $tree_render(ctx, child, element, null);
    $tree_disabled(element, node[5]);
    // A button that opens a menu, a popover or a select says so, and says
    // whether it is open. Written only when it is `true`, the way `aria-invalid`
    // is: an ordinary button expands nothing and carries none of it.
    $tree_bind(node[6], (expanded) => $dom_flag(element, "aria-expanded", expanded));
    // A button that marks the current page, step or item says so, written only
    // when it is `true`, the way `aria-expanded` is.
    $tree_bind(node[7], (current) => $dom_flag(element, "aria-current", current));
    const onPress = node[4];
    $dom_listen(element, "click", () =>
      // One transaction, so that a handler which writes three signals causes
      // one pass over the watchers rather than three.
      $ui_flush(() => onPress($taskApart(ctx), [0])),
    );
    $tree_events(ctx, element, node[8]);
    return;
  }
  if (tag === 6) {
    const element = $tree_element(parent, "a", anchor);
    // `onFollow` is an `Option<fn>`: `undefined` is a plain anchor the browser
    // owns, and a handler is a `routeLink` — the same `<a href>`, so a reader
    // gets the middle-click, the ⌘-click, "open in new tab", the status-bar
    // preview and the "link", but a plain left-click runs the handler in place
    // of the browser's own navigation.
    const onFollow = node[4];
    if (onFollow === undefined) {
      $tree_bind(node[1], (dest) => $dom_attribute(element, "href", dest));
    } else {
      // The href a browser follows, kept so the plain-click handler navigates to
      // the same address the reader sees in the status bar. `.Cell` and
      // `.Computed` re-run this, so it is always what the anchor points at now.
      let dest = "";
      $tree_bind(node[1], (to) => {
        dest = to;
        $dom_attribute(element, "href", to);
      });
      // A real anchor, so the browser keeps middle-click, ⌘-click, "open in new
      // tab", the status bar and the reader's "link". Only a plain left-click is
      // the app's: a modified click — the middle button, or ⌘/Ctrl/Shift/Alt
      // with the left one — falls through to the anchor the browser already has,
      // and one another listener already handled is left alone.
      $dom_listen(element, "click", (event) => {
        if (event.defaultPrevented) return;
        if (event.button !== undefined && event.button !== 0) return;
        if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        event.preventDefault();
        // One transaction, the way a press is.
        $ui_flush(() => onFollow($taskApart(ctx), dest));
      });
    }
    // Styles and children the reactive way `stack` does them (issue #164), so a
    // `.When`/`.Computed` class swap on a link — plain or routing — updates live.
    $tree_children(ctx, element, node[2], node[3]);
    // The current page, written only when it is `true`, the way a button's is.
    $tree_bind(node[5], (current) => $dom_flag(element, "aria-current", current));
    $tree_events(ctx, element, node[6]);
    return;
  }
  if (tag === 7) {
    const element = $tree_element(parent, "img", anchor);
    $tree_bind(node[1], (source) => $dom_attribute(element, "src", source));
    // `alt` is a plain string here rather than a `Prop`, so it is written once:
    // an image's text does not change under it the way a label does.
    $dom_attribute(element, "alt", node[2]);
    // The styles are the picture's rather than a box's around it: sizing a
    // picture, cropping it and rounding it are things only the picture can be
    // told.
    $tree_styles(element, node[3]);
    $tree_events(ctx, element, node[4]);
    return;
  }
  if (tag === 8) {
    if ($tree_icon_hook === null) {
      // The compiler said no tree here holds artwork, so it left the renderer
      // out of the artifact. Reaching this is that decision being wrong, and
      // saying so beats a `TypeError` about `null`.
      $abort("an icon was rendered in a program that was said to have none");
    }
    // The artwork string and its styles, lowered through the icon hole. The
    // hook answers the `<svg>` root it built, so a listener wires onto the
    // drawing itself.
    const element = $tree_icon_hook(parent, node[1], node[2], anchor);
    if (element !== null && element !== undefined) $tree_events(ctx, element, node[3]);
    return;
  }
  if (tag === 9) {
    // The label wraps the field rather than pointing at it by an identifier,
    // which is what makes the pair correct with nothing generated: there is no
    // identifier to collide, and no way to render a field whose label is
    // attached to something else.
    const wrapper = $tree_element(parent, "label", anchor);
    // `around` is the label's, because the label is the box a surrounding row
    // lays out and nothing on the input can reach it.
    $tree_styles(wrapper, node[5]);
    $tree_text(node[1], $tree_element(wrapper, "span", null), null);
    // `FieldKind` carries no payload any more, so it lowers to a bare tag rather
    // than an array — read it with no unwrap.
    const kind = node[2];
    const element = $tree_element(wrapper, kind === 1 ? "textarea" : "input", null);
    if (kind !== 1) $dom_attribute(element, "type", $TREE_FIELD_KINDS[kind]);
    // The hint inside the empty box. `placeholder` is announced after the
    // accessible name rather than instead of it, which is why the label beside
    // it is still required.
    $tree_bind(node[3], (hint) => $dom_optional(element, "placeholder", hint));
    // Failing validation is announced as well as painted, and the attribute is
    // both: a reader hears it, and `On(.Invalid, …)` is a rule about it.
    $tree_bind(node[8], (invalid) => $dom_flag(element, "aria-invalid", invalid));
    // The styles are the input's rather than the label's: the input is what a
    // reader focuses and what a browser disables.
    $tree_styles(element, node[4]);
    $tree_disabled(element, node[9]);
    const cell = node[6][0];
    $tree_bind([1, node[6]], (value) => {
      // Writing what is already there moves the caret in a real browser.
      if (element.value !== value) element.value = value;
    });
    $dom_listen(element, "input", () => $ui_flush(() => $ui_write(cell, element.value)));
    // The caret and selection, when the field binds one. `selection` is an
    // `Option<Signal<(Int,Int)>>`: `undefined` is a field that keeps none.
    // Writing the signal moves the caret (`setSelectionRange`), and the browser
    // writes it back as the reader moves — guarded so the program's write does
    // not read back as the reader's.
    const selection = node[7];
    if (selection !== undefined) {
      const selCell = selection[0];
      let ours = false;
      $tree_bind([1, selection], (pair) => {
        const a = Number(pair[0]);
        const b = Number(pair[1]);
        if (element.selectionStart !== a || element.selectionEnd !== b) {
          ours = true;
          element.setSelectionRange(a, b);
          ours = false;
        }
      });
      const report = () => {
        if (ours) return;
        const a = element.selectionStart;
        const b = element.selectionEnd;
        if (a === null || a === undefined || b === null || b === undefined) return;
        $ui_flush(() => $ui_write(selCell, [BigInt(a), BigInt(b)]));
      };
      // The events that move a caret or a selection in a text control: the
      // browser's own `select`, and the key and pointer gestures a plain caret
      // move fires no `select` for.
      $dom_listen(element, "select", report);
      $dom_listen(element, "keyup", report);
      $dom_listen(element, "mouseup", report);
      $dom_listen(element, "focus", report);
    }
    $tree_events(ctx, wrapper, node[10]);
    return;
  }
  if (tag === 10) {
    // A slider is a `<label>` wrapping its name and a `<input type=range>`. The
    // browser gives back the thumb, the drag, the arrow and Home/End keys, and
    // the `role="slider"` announcement with its `aria-valuenow`, `aria-valuemin`
    // and `aria-valuemax` — there is nothing here to draw or announce for
    // itself.
    const wrapper = $tree_element(parent, "label", anchor);
    // `around` is the label's, the box a surrounding row lays out — the same
    // rule a `field`'s and a `toggle`'s wrapper follows.
    $tree_styles(wrapper, node[7]);
    $tree_text(node[1], $tree_element(wrapper, "span", null), null);
    const element = $tree_element(wrapper, "input", null);
    $dom_attribute(element, "type", "range");
    // The track's bounds, which a reader is told it is dragging inside.
    $dom_attribute(element, "min", $f64(node[3]));
    $dom_attribute(element, "max", $f64(node[4]));
    // `step` is an `Option<Float>`: `undefined` leaves the browser's default of
    // one, and a number sets the size of a move.
    const step = node[5];
    if (step !== undefined) $dom_attribute(element, "step", $f64(step));
    $tree_styles(element, node[6]);
    $tree_disabled(element, node[8]);
    // The signal holds a `Float`, and the control's `value` is a string, so this
    // boundary is the one place a number is coerced across — `String` out and
    // `Number` back in.
    const cell = node[2][0];
    $tree_bind([1, node[2]], (value) => {
      const text = String(value);
      if (element.value !== text) element.value = text;
    });
    $dom_listen(element, "input", () => $ui_flush(() => $ui_write(cell, Number(element.value))));
    $tree_events(ctx, wrapper, node[9]);
    return;
  }
  if (tag === 11) {
    const wrapper = $tree_element(parent, "label", anchor);
    $tree_styles(wrapper, node[4]);
    const element = $tree_element(wrapper, "input", null);
    $dom_attribute(element, "type", "checkbox");
    // A switch is the same control with a different mark and a different
    // announcement — "on" and "off" rather than "ticked". The mark itself is
    // the sheet's, drawn on the box by the reset, because an `<input>` holds
    // no children.
    if (node[2] === 1) $dom_attribute(element, "role", "switch");
    $tree_bind(node[6], (invalid) => $dom_flag(element, "aria-invalid", invalid));
    $tree_styles(element, node[3]);
    $tree_disabled(element, node[7]);
    $tree_text(node[1], $tree_element(wrapper, "span", null), null);
    const cell = node[5][0];
    $tree_bind([1, node[5]], (value) => {
      element.checked = value;
    });
    $dom_listen(element, "change", () => $ui_flush(() => $ui_write(cell, element.checked)));
    $tree_events(ctx, wrapper, node[8]);
    return;
  }
  if (tag === 12) {
    // The group carries `role="radiogroup"` and its own accessible name; the
    // caller's styles land on it, the box a surrounding row lays out. Its
    // options are the real inputs — the whole point, since a stack of buttons
    // gets none of the browser's model.
    const group = $tree_element(parent, "div", anchor);
    $dom_attribute(group, "role", "radiogroup");
    $tree_bind(node[1], (label) => $dom_attribute(group, "aria-label", label));
    $tree_styles(group, node[3]);
    // `style` (Radio, Segmented, Menu, Dropdown) is carried but not yet drawn
    // apart: every choice style renders as the accessible radio model for now —
    // one tab stop, one selected key, the arrow keys, Space and `aria-checked`
    // the browser gives a `role="radiogroup"` of radios. A segmented or menu
    // look is a later pass, not something invented here.
    const name = "buri-radio-" + $tree_radio_groups++;
    const cell = node[4][0];
    const disabled = node[6];
    for (const option of node[2]) {
      const key = option[0];
      const wrapper = $tree_element(group, "label", null);
      const input = $tree_element(wrapper, "input", null);
      $dom_attribute(input, "type", "radio");
      $dom_attribute(input, "name", name);
      // The key is the input's `value`, and what the signal holds when this is
      // the one picked.
      $dom_attribute(input, "value", key);
      // The caption is a node now rather than a run of text, so it is rendered
      // as one: an option may be a word, an icon and a word, or any tree.
      $tree_render(ctx, option[1], wrapper, null);
      // A disabled group refuses each option: the attribute takes the input out
      // of the tab order and refuses the press, written only when it is `true`.
      $tree_disabled(input, disabled);
      // Checked is the signal: the input whose key equals what it holds. A key
      // no option carries checks none of them, which is an unset group.
      $tree_bind([1, node[4]], (selected) => {
        input.checked = selected === key;
      });
      // Picking one writes its key back. A radio fires `change` only on its way
      // to checked, so this reads the key rather than a boolean, and never
      // writes the signal back to what it already is.
      $dom_listen(input, "change", () => {
        if (input.checked) $ui_flush(() => $ui_write(cell, key));
      });
    }
    $tree_events(ctx, group, node[7]);
    return;
  }
  if (tag === 13) {
    const element = $tree_element(parent, "form", anchor);
    const onSubmit = node[1];
    $dom_listen(element, "submit", (event) => {
      // The page must not navigate: submission is the handler, and there is
      // nowhere for a browser to post to.
      if (event && event.preventDefault) event.preventDefault();
      $ui_flush(() => onSubmit($taskApart(ctx), [0]));
    });
    $tree_children(ctx, element, node[2], node[3]);
    $tree_events(ctx, element, node[4]);
    return;
  }
  if (tag === 14) {
    const element = $tree_element(parent, "button", anchor);
    // The form's action. This attribute is the whole of what makes Enter in a
    // field submit: HTML submits a form implicitly through its submit button,
    // and a form of two fields with none discards the keypress. There is no
    // listener here — submitting is the form's own handler, and a click that
    // ran it as well would run it twice.
    $dom_attribute(element, "type", "submit");
    $tree_bind(node[1], (label) => $dom_attribute(element, "aria-label", label));
    $tree_styles(element, node[2]);
    // `disabled` takes the button out of the tab order and refuses the submit,
    // both the press and the implicit Enter — a browser fires no `submit` for a
    // disabled submit button, so the attribute is the whole of it.
    $tree_disabled(element, node[3]);
    $tree_text(node[1], element, null);
    $tree_events(ctx, element, node[4]);
    return;
  }
  if (tag === 15) {
    const element = $tree_element(parent, "dialog", anchor);
    // The label is the accessible name however the panel is drawn, the rule a
    // button's label follows: a reader is announced into the dialog by what it
    // is for, and the heading inside it may not be there yet.
    $tree_bind(node[2], (label) => $dom_attribute(element, "aria-label", label));
    $tree_styles(element, node[3]);
    for (const child of node[4]) $tree_render(ctx, child, element, null);
    // `scrimStyles` (node[5]) style the scrim behind the panel. A real
    // `<dialog>` draws its scrim as `::backdrop`, which no inline class on the
    // element can reach and for which this renderer mints no element of its own,
    // so they are left unrendered here for now. The native painter and the
    // snapshot honour them — there the scrim is the widget's own box, drawn from
    // these styles (see `ui/node`'s `describe`).
    const cell = node[1][0];
    // Whether the shutting is ours or the reader's. `close()` fires the same
    // event either way, and only the reader's is news to the signal.
    let ours = false;
    $tree_bind([1, node[1]], (on) => {
      ours = true;
      $dom_modal(element, on);
      ours = false;
    });
    // Escape, and every other way a browser shuts a dialog by itself. A signal
    // that went on saying `true` would leave the program holding a panel
    // nobody can see.
    $dom_listen(element, "close", () => {
      if (!ours) $ui_flush(() => $ui_write(cell, false));
    });
    $tree_events(ctx, element, node[6]);
    return;
  }
  if (tag === 16) {
    // A disclosure. `<details>` is the open-and-shut state, the toggle keys and
    // the announcement, all the browser's own; `<summary>` is the row that
    // opens it and the one thing shown when it is shut.
    const element = $tree_element(parent, "details", anchor);
    $tree_styles(element, node[3]);
    const summary = $tree_element(element, "summary", null);
    $tree_text(node[1], summary, null);
    for (const child of node[4]) $tree_render(ctx, child, element, null);
    const cell = node[2][0];
    // Whether the opening is ours or the reader's, the way a dialog's close is:
    // `toggle` fires for both, and only the reader's is news to the signal.
    let ours = false;
    $tree_bind([1, node[2]], (on) => {
      ours = true;
      element.open = on;
      ours = false;
    });
    // A reader opens and shuts a `<details>` without asking, so the signal
    // follows what the reader chose rather than staying where the program left
    // it.
    $dom_listen(element, "toggle", () => {
      if (!ours) $ui_flush(() => $ui_write(cell, element.open));
    });
    $tree_events(ctx, element, node[5]);
    return;
  }
  if (tag === 17) {
    // A progress bar. The role and the three `aria-value*` are the whole of
    // what a widget adds over the nested boxes a program drew before: a reader
    // is told it is a progress bar, what it measures, and how far along it is.
    const element = $tree_element(parent, "div", anchor);
    $dom_attribute(element, "role", "progressbar");
    $tree_bind(node[1], (label) => $dom_attribute(element, "aria-label", label));
    $dom_attribute(element, "aria-valuemin", "0");
    $dom_attribute(element, "aria-valuemax", "100");
    // `value` is an `Option<Prop<Float>>`. A prop is a determinate bar whose
    // `aria-valuenow` is the whole number of hundredths it stands for, out of
    // the hundred `min` and `max` name. `undefined` is an indeterminate bar: it
    // carries no `aria-valuenow` at all, which is how a reader is told the
    // progress is not yet known — a spinner.
    const value = node[2];
    if (value !== undefined) {
      $tree_bind(value, (v) =>
        $dom_attribute(element, "aria-valuenow", String(Math.round(v * 100))),
      );
    }
    // The fill is the caller's own children, drawn inside the bar.
    $tree_children(ctx, element, node[3], node[4]);
    $tree_events(ctx, element, node[5]);
    return;
  }
  if (tag === 18) {
    const cond = node[1];
    const then = node[2];
    const otherwise = node[3];
    $tree_dynamic(ctx, parent, anchor, (scope) => ($tree_value(cond, scope) ? then : otherwise));
    return;
  }
  if (tag === 19) {
    const build = node[1];
    $tree_dynamic(ctx, parent, anchor, (scope) => build(scope));
    return;
  }
  if (tag === 20) {
    $tree_each(ctx, parent, anchor, node[1], node[2], node[3]);
    return;
  }
  if (tag === 21) {
    // Renderer-internal: an element of an arbitrary name with a stack's styles
    // and children. Nothing a program builds is a `Raw`; a reactive widget's
    // rebuild through `$tree_dynamic` is.
    const element = $tree_element(parent, node[1], anchor);
    $tree_children(ctx, element, node[2], node[3]);
    return;
  }
  if (tag === 22) {
    // Renderer-internal: several children with no element of their own, shown
    // directly under their parent — a disclosure's body is the one that reaches
    // here.
    for (const child of node[1]) $tree_render(ctx, child, parent, anchor);
    return;
  }
  if (tag === 23) {
    // A file picker. The button is the control a reader focuses, hears and
    // styles; the `<input type=file>` beside it is `hidden` and only opens the
    // platform's chooser, which a click on the button asks it to.
    const button = $tree_element(parent, "button", anchor);
    $dom_attribute(button, "type", "button");
    $tree_bind(node[1], (label) => $dom_attribute(button, "aria-label", label));
    $tree_styles(button, node[2]);
    $tree_text(node[1], button, null);
    $tree_disabled(button, node[3]);
    const input = $tree_element(parent, "input", anchor);
    $dom_attribute(input, "type", "file");
    $dom_attribute(input, "hidden", "");
    // Only a hint to the chooser — a reader can pick "All files" — so `admit`
    // checks the entries again.
    if (node[4].length > 0) $dom_attribute(input, "accept", node[4].join(","));
    const admit = node[5];
    const land = node[6];
    $dom_listen(button, "click", () => input.click());
    $dom_listen(input, "change", () => {
      const file = input.files ? input.files[0] : undefined;
      // Cleared, so choosing the same file again is a new pick.
      input.value = "";
      if (file === undefined) return;
      const name = file.name;
      const type = file.type;
      const size = BigInt(file.size);
      // A refused file is never read: over the cap, it may be any size at all.
      if (admit(name, type, size) !== "") {
        $ui_flush(() => land($taskApart(ctx), [name, type, size, undefined]));
        return;
      }
      file.arrayBuffer().then(
        (buffer) => {
          // Strict, the way `bytes.fromUtf8` is, and a leading byte-order mark
          // is dropped. Bytes that are not UTF-8 land as no text at all.
          let text;
          try {
            text = new TextDecoder("utf-8", { fatal: true }).decode(buffer);
          } catch {
            text = undefined;
          }
          $ui_flush(() => land($taskApart(ctx), [name, type, size, text]));
        },
        () => $ui_flush(() => land($taskApart(ctx), [name, type, size, undefined])),
      );
    });
    $tree_events(ctx, button, node[7]);
    return;
  }
  // Every tag the vocabulary has an arm above. Reaching here is a node carrying
  // one the renderer does not, which beats a `TypeError` about `node` shape.
  $abort("a node carried a tag the renderer has no arm for: " + tag);
}

// `ui/node`'s one operation with a body in the runtime. Everything else in that
// module is ordinary Buri building a value; this is where the value meets a
// document.
// Puts the compiler's stylesheet in the document, once, before anything is
// rendered against it.
//
// The text is a string constant in the artifact — the compiler wrote every
// rule in it — so this copies rather than generates, and a second mount finds
// the element already there. Off a browser there is nowhere to put it and
// nothing to look at it, which is why `platform/effect/testing` reads `$ui_sheet` instead.
// --- Themes ------------------------------------------------------------------
//
// A design token is a namespaced custom property, and a theme is the block of
// values those properties take. That split is what makes a theme cost nothing:
// every class in the stylesheet was decided at compile time and names
// `var(--cardlib-surface)`, so installing a theme, or switching one, writes one
// `:root` block and touches no element at all.
//
// Resolution is one pass. Every binding every theme holds goes into one map,
// keyed by the token it names; then each value is followed while it is itself a
// token, which is how a chain — a library's token to the app's token to a
// colour — arrives at a value. A chain that leaves the map, or one that closes
// on itself, names nothing and is left out rather than guessed at: an undefined
// custom property is one the browser ignores, and inventing a colour for it
// would hide the missing binding rather than show it.

// The tag `ui/style`'s `Color.Token` carries. That vocabulary's variant order
// is load-bearing and its module header says so; this is one of the places that
// rests on it.
const $UI_COLOR_TOKEN = 2;

// The custom-property block installed right now, without the `<style>` element
// around it. Off a browser this is all there is, which is what `platform/effect/testing`
// reads.
let $ui_theme_text = "";

// `namespace-name` — the custom property's name, without the leading dashes.
function $ui_theme_name(reference) {
  return reference[0] + "-" + reference[1];
}

// `ui/theme`'s `Scheme`, lowered. What the platform paints its own things in:
// a native control, the document scrollbar, a date picker, the form autofill.
const $UI_THEME_SCHEMES = ["light", "dark"];

// Which themes currently apply, in the order they were passed: a switch is
// followed to whichever branch its condition picks, and what comes back is a
// list of `Values` and `Scheme` kinds. Deciding this in one place is what keeps
// the map used for resolution and the blocks that are written from ever
// disagreeing about which side of a switch the page is on.
function $ui_theme_applied(themes, scope, out) {
  for (const wrapper of themes) {
    const theme = wrapper[0];
    if (theme[0] === 1) {
      $ui_theme_applied($tree_value(theme[1], scope) ? theme[2] : theme[3], scope, out);
    } else {
      out.push(theme);
    }
  }
}

// One value, followed while it is a token. The step budget is the number of
// bindings there are, so a chain that closes on itself stops instead of
// hanging.
function $ui_theme_resolve(bindings, color) {
  let steps = bindings.size;
  while (color[0] === $UI_COLOR_TOKEN) {
    if (steps-- <= 0) return null;
    const next = bindings.get($ui_theme_name(color[1]));
    if (next === undefined) return null;
    color = next;
  }
  return $tree_color(color);
}

// The whole custom-property text: one `:root` block per theme, in the order
// they were passed — a theme *is* a block of values, so reading the installed
// text shows which package each variable came from.
function $ui_theme_render(themes, scope) {
  const applied = [];
  $ui_theme_applied(themes, scope, applied);

  // Every binding, in declaration order, a later one for the same token
  // replacing an earlier one. This is what a chain is followed through.
  const bindings = new Map();
  for (const theme of applied) {
    if (theme[0] !== 0) continue;
    for (const binding of theme[1]) {
      if (binding[0][0] === $UI_COLOR_TOKEN) {
        bindings.set($ui_theme_name(binding[0][1]), binding[1]);
      }
    }
  }

  let out = "";
  for (const theme of applied) {
    // A scheme is a theme that binds no token: one declaration, in a block of
    // its own, so a later one wins the way a later value does.
    if (theme[0] === 2) {
      out += ":root{color-scheme:" + $UI_THEME_SCHEMES[theme[1]] + "}\n";
      continue;
    }
    // A page's ground is the other one: the document's own two colours, on
    // `body` rather than `:root` — a background there is what the browser
    // paints the canvas with, so it reaches an overscroll and the whole window
    // and not just the box the tree drew. A token is left as the `var()` a
    // class would have held, because the properties are declared in this same
    // text and the cascade resolves them wherever they are used.
    if (theme[0] === 3) {
      out +=
        "body{background-color:" +
        $tree_color(theme[1]) +
        ";color:" +
        $tree_color(theme[2]) +
        "}\n";
      continue;
    }
    const body = [];
    for (const binding of theme[1]) {
      if (binding[0][0] !== $UI_COLOR_TOKEN) continue;
      const value = $ui_theme_resolve(bindings, binding[1]);
      if (value !== null) body.push("--" + $ui_theme_name(binding[0][1]) + ":" + value);
    }
    if (body.length !== 0) out += ":root{" + body.join(";") + "}\n";
  }
  return out;
}

// Whether the block can be written once. A `switching` theme is the only thing
// in a theme list that can change.
function $ui_theme_static(themes) {
  for (const wrapper of themes) {
    if (wrapper[0][0] === 1) return false;
  }
  return true;
}

function $ui_theme_write(text) {
  $ui_theme_text = text;
  if (typeof document === "undefined") return;
  let element = document.getElementById("buri-theme");
  if (element === null) {
    // A program with no tokens leaves no trace of the machinery in its page.
    if (text === "") return;
    element = document.createElement("style");
    element.id = "buri-theme";
    (document.head || document.body).appendChild(element);
  }
  element.textContent = text;
}

// Resolves the themes and puts their values in the document, writing them again
// whenever a switching theme's condition changes. The stylesheet is never
// touched and no element's classes are re-applied — which is the whole claim
// dark mode rests on.
function $ui_theme_install(themes) {
  if ($ui_theme_static(themes)) {
    $ui_theme_write($ui_theme_render(themes, null));
    return;
  }
  $ui_run(
    $ui_cell(2, undefined, (scope) => {
      $ui_theme_write($ui_theme_render(themes, scope));
      return 0;
    }),
  );
}

// The `:root`/`body`/scheme text a *flattened* theme document resolves to — the
// twin of `cli/runtime/ui.rs`'s `render`, byte for byte, so `platform/effect/testing`'s
// `install` reads the same block on both backends. `ui/theme`'s `document`
// flattens the switches away (through the tracked scope its reactive caller
// passes); this follows the chains and writes the blocks. The format is that
// file's header: `buri-theme 1`, then a `theme`/`scheme`/`page` line per applied
// theme with `bind` lines under a `theme`.
function $ui_theme_render_doc(doc) {
  const lines = doc.split("\n");
  if (lines.shift() !== "buri-theme 1") return "";
  // A block is a theme's values ({v:[[name,kind,rest]]}), a scheme ({s:name}) or
  // a page ({g:ground, i:ink}). Only a values block binds a token.
  const blocks = [];
  for (const line of lines) {
    if (line === "theme") {
      blocks.push({ v: [] });
    } else if (line.startsWith("scheme ")) {
      blocks.push({ s: line.slice(7) });
    } else if (line.startsWith("page ")) {
      const rest = line.slice(5);
      const at = rest.indexOf(" ");
      if (at >= 0) blocks.push({ g: rest.slice(0, at), i: rest.slice(at + 1) });
    } else if (line.startsWith("bind ")) {
      // `bind <name> token|value <rest>` — the rest keeps its own spaces.
      const rest = line.slice(5);
      const a = rest.indexOf(" ");
      const b = rest.indexOf(" ", a + 1);
      if (a < 0 || b < 0) continue;
      const name = rest.slice(0, a);
      const kind = rest.slice(a + 1, b);
      const value = rest.slice(b + 1);
      if (kind !== "token" && kind !== "value") continue;
      const last = blocks[blocks.length - 1];
      if (last && last.v) last.v.push([name, kind, value]);
    }
  }
  // Every binding, in declaration order, a later one for the same token
  // replacing an earlier: what a chain is followed through.
  const bindings = new Map();
  for (const block of blocks) {
    if (!block.v) continue;
    for (const [name, kind, value] of block.v) bindings.set(name, [kind, value]);
  }
  // One value, followed while it is a token, with a step budget of the map's
  // size so a chain that closes on itself stops.
  const resolve = (kind, value) => {
    let steps = bindings.size;
    let k = kind;
    let v = value;
    while (k === "token") {
      if (steps-- <= 0) return null;
      const next = bindings.get(v);
      if (next === undefined) return null;
      k = next[0];
      v = next[1];
    }
    return v;
  };
  let out = "";
  for (const block of blocks) {
    if (block.v) {
      const body = [];
      for (const [name, kind, value] of block.v) {
        const resolved = resolve(kind, value);
        if (resolved !== null) body.push("--" + name + ":" + resolved);
      }
      if (body.length !== 0) out += ":root{" + body.join(";") + "}\n";
    } else if (block.s !== undefined) {
      out += ":root{color-scheme:" + block.s + "}\n";
    } else {
      out += "body{background-color:" + block.g + ";color:" + block.i + "}\n";
    }
  }
  return out;
}

// `ui/theme`'s `installDoc(doc)` — resolve a flattened document to its block,
// store it where `variables` reads it, and answer it. `platform/effect/testing`'s `install`
// is the caller, through the reactive computation `$ui_node_reactive` registers,
// so a switching theme re-runs this with the other branch's document.
function $ui_theme_installDoc(doc) {
  $ui_theme_write($ui_theme_render_doc(doc));
  return $ui_theme_text;
}

// `ui/theme`'s `variables()` — the block installed right now, which is what
// `platform/effect/testing`'s `variables` answers.
function $ui_theme_variables() {
  return $ui_theme_text;
}

function $ui_inject(sheet) {
  if (sheet === "" || typeof document === "undefined") return;
  if (document.getElementById("buri-styles") !== null) return;
  const element = document.createElement("style");
  element.id = "buri-styles";
  element.textContent = sheet;
  (document.head || document.body).appendChild(element);
}

function $ui_node_mount(ctx, root, themes) {
  const body = $dom_body();
  if (!body) return $err("there is nowhere to mount: this platform has no document");
  $ui_inject($ui_sheet);
  // Before anything is rendered, so that the first paint already has the values
  // the classes ask for. Through the hook, so that a program with no themes
  // carries none of the machinery: with nothing assigned there is nothing to
  // install, because the only list `themes` can be is an empty one.
  if ($ui_theme_hook !== null) $ui_theme_hook(themes);
  $tree_render(ctx, root, body, null);
  // The page stays live. The entry wrapper exits only on an `.Err`, and the
  // listeners this registered go on running.
  return $ok(0);
}

// The scope `ui/node`'s `describe` reads props under: `-1`, which is what
// `$ui.tracking` already holds outside a computation. A snapshot looks at the
// graph once and subscribes to nothing, so a read through this records no
// dependency and nothing here ever re-runs.
function $ui_node_rootScope() {
  return [-1];
}

// `ui/theme`'s own, for the walk that flattens a theme list to the document a
// snapshot's painter reads. The same untracked scope under a second name,
// because the symbol an intrinsic key produces is the key's own.
function $ui_theme_rootScope() {
  return [-1];
}

// --- The headless user-interface platform ------------------------------------
//
// The same graph, with no document attached: `platform/effect/testing` is about what the
// runtime does, and a second implementation of it would be a second thing to
// be right. The handles are unused — the graph is the state — but the structs
// carry one so that the shape matches every other test double.

const $host_testing_Headless_signal = $host_HostUi_signal;
const $host_testing_Headless_read = $host_HostUi_read;
const $host_testing_Headless_write = $host_HostUi_write;
const $host_testing_Headless_memo = $host_HostUi_memo;
const $host_testing_Headless_watch = $host_HostUi_watch;
const $host_testing_Observer_read = $host_HostWatch_read;

// The headless double's timers, on a virtual clock. A test has no wall clock to
// wait on, so what `after` schedules is queued here against `$ui_fake_now`
// rather than handed to `setTimeout`, and `elapse` is what moves the clock and
// fires what has come due. Each fires on the graph's own turn (`$ui_flush`), so
// the signals it writes settle before the next timer runs — the parity with a
// page a test is asserting.
const $ui_fake_timers = [];
let $ui_fake_timer_next = 1;
let $ui_fake_now = 0;

function $host_testing_Headless_schedule(self, millis, run) {
  const handle = $ui_fake_timer_next++;
  const ms = Number(millis);
  $ui_fake_timers.push({
    handle,
    due: $ui_fake_now + (ms > 0 ? ms : 0),
    run,
    self,
    done: false,
  });
  return BigInt(handle);
}

function $host_testing_Headless_unschedule(self, id) {
  const handle = Number(id);
  for (const timer of $ui_fake_timers) {
    if (timer.handle === handle) timer.done = true;
  }
  return 0;
}

function $host_testing_elapse(millis) {
  const target = $ui_fake_now + Number(millis);
  // Fire due timers in due-time order — a timer scheduled by a timer that just
  // fired joins this pass if it too is due by `target`, the way a page's clock
  // would reach it. Ties go to the one scheduled first, which is the order the
  // array already holds them in.
  for (;;) {
    let next;
    for (const timer of $ui_fake_timers) {
      if (timer.done || timer.due > target) continue;
      if (next === undefined || timer.due < next.due) next = timer;
    }
    if (next === undefined) break;
    next.done = true;
    $ui_fake_now = next.due;
    $ui_flush(() => next.run(next.self));
  }
  $ui_fake_now = target;
  return 0;
}

function $host_testing_headless() {
  return $handle(0);
}

function $host_testing_observer() {
  return $handle(0);
}

// The stylesheet the compiler extracted for this artifact, as text. Reading it
// is how a test asserts what a class *means* rather than only what it is
// called, and how it sees that two modules asking for one padding produced one
// rule.
function $host_testing_stylesheet() {
  return $ui_sheet;
}

// `install` and `variables` are Buri bodies now (#53 phase 5): `install`
// flattens its themes through `ui/theme`'s `document` and installs the result
// with `$ui_theme_installDoc`, registering the switching computation through
// `$ui_node_reactive`; `variables` reads `$ui_theme_variables`. So the block a
// test asserts is resolved by the same `ui/theme` Buri on both backends, and
// there is nothing named `$host_testing_install`/`$host_testing_variables` for this
// side to hold.

// A snapshot is painted by the native runtime — taffy, cosmic-text and
// tiny-skia, in `cli/runtime/paint.rs` — and there is no painter here. A suite
// that declares `platforms: [JS]` and calls `snapshot` therefore fails rather
// than passing without having painted anything.
// A snapshot's themes, for a painter this side does not have. `paint` below
// fails the block whatever was installed, so there is nothing here to keep.
function $host_testing_installThemes(document) {
  return 0;
}

function $host_testing_paint(name, scene, state) {
  $testing_assert_failWith(
    'the snapshot "' + name + '" was not painted: snapshots run natively, and this suite is JS',
  );
  return 0;
}

function $host_testing_recorder() {
  return $handle({ tags: [], values: [] });
}

function $host_testing_Recorder_record(self, tag) {
  $slot(self).tags.push(tag);
  return 0;
}

function $host_testing_Recorder_recorded(self) {
  return $slot(self).tags.slice();
}

function $host_testing_Recorder_note(self, value) {
  $slot(self).values.push(value);
  return value;
}

function $host_testing_Recorder_noted(self) {
  return $slot(self).values.slice();
}

// --- platform/effect/testing: the scene document (issue #53, phase 6) ----------------------
//
// `render(ctx, root)` is `Rendered(mount(ctx, root, renderInto))`. On this
// backend `mount` now drives the `renderInto` walk (`ui/node`) into a scene
// document — the element arena the native runtime (`cli/runtime/document.rs`)
// builds — rather than the HTML document double `$tree_render` builds for a
// real page. So `markup()` is the headerless `describe` scene on both backends,
// byte for byte, and every `Rendered` reader below answers from the same
// reconciled arena the native readers do. The `$dom_*`/`$tree_*` document
// double is untouched: it is what `ui/node`'s real `mount` and `ui/web` render
// into.

// The identity counter, the twin of the JavaScript `$dom.identities` for the
// scene arena: stamped once at creation, given to no other record, monotonic
// across every document a program builds.
let $scene_ids = 0;

function $scene_mint() {
  return $scene_ids++;
}

// A fresh, empty scene document. `records[0]` is the host the tree hangs off,
// never written to markup or counted. `ctx` is the render context, kept so the
// reactive re-walk (`rebuildRegion`) and the keyed reconcile drive the closures
// `renderInto` handed over with the context `render` was called with.
function $scene_open(ctx) {
  return {
    records: [$scene_record(0, "root", "", "")],
    cursor: [{ parent: 0, anchor: null }],
    regions: [],
    eachRegions: [],
    outside: [],
    // The file `pickFile` last offered, which a picker's handler reads back.
    offer: { name: "", type: "", content: [] },
    // The element holding the pointer since a `pointerDown`, or -1.
    capture: -1,
    // What the pointer handler being fired reads back: where the pointer is and
    // the row under it (undefined for none).
    pointer: { x: 0, y: 0, row: undefined },
    ctx,
  };
}

// kind: 0 element, 1 text, 2 marker.
function $scene_record(kind, name, body, text) {
  return {
    identity: $scene_mint(),
    kind,
    name,
    body,
    text,
    parent: null,
    children: [],
    // The press/submit handler thunk, the bound value signal, and whether a
    // button submits its form — the widget state `press`/`fill`/`flip`/`submit`
    // read, the scene twin of the JavaScript listener on an element.
    press: null,
    valueSignal: -1,
    // The signal a field's caret/selection is bound to, so `select` writes it an
    // `(anchor, focus)` pair. `-1` when the field carries none.
    selectionSignal: -1,
    submit: false,
    // A control's accessible name, for `press` — the `aria-label` twin. Empty
    // means the element's own text is its name.
    label: "",
    // A route link's plain-click handler thunk, or null — `follow` fires it,
    // `openInNewTab` leaves it to the browser.
    follow: null,
    // A file picker's handler thunk, or null — `pickFile` fires it.
    pick: null,
    // The pointer handler thunks, down/move/up, each null when unset.
    pointer: [null, null, null],
  };
}

function $scene_of(x) {
  return $t.h[Number(x[0])];
}

// Adds a record in the current frame — under its parent, before its anchor, or
// at the end of that parent when there is none — and answers its index. The
// native `Document::add`.
function $scene_add(doc, kind, name, body, text) {
  const frame = doc.cursor[doc.cursor.length - 1];
  const index = doc.records.length;
  const record = $scene_record(kind, name, body, text);
  record.parent = frame.parent;
  doc.records.push(record);
  const children = doc.records[frame.parent].children;
  let pos = children.length;
  if (frame.anchor !== null) {
    const at = children.indexOf(frame.anchor);
    if (at >= 0) pos = at;
  }
  children.splice(pos, 0, index);
  return index;
}

// The element the walk currently has open — where an event arm attaches.
function $scene_openElement(doc) {
  return doc.cursor[doc.cursor.length - 1].parent;
}

// The records reachable from the host, in document order, with the tree depth
// their scene line carries — the host's own children at depth 0.
function $scene_ordered(doc) {
  const out = [];
  $scene_visit(doc, 0, 0, out);
  return out;
}

function $scene_visit(doc, node, depth, out) {
  for (const child of doc.records[node].children) {
    out.push([child, depth]);
    $scene_visit(doc, child, depth + 1, out);
  }
}

// --- The builders `renderInto` emits to --------------------------------------

function $ui_node_emitElement(builder, name, body) {
  const doc = $scene_of(builder);
  const index = $scene_add(doc, 0, name, body, "");
  doc.cursor.push({ parent: index, anchor: null });
}

function $ui_node_exitElement(builder) {
  const doc = $scene_of(builder);
  if (doc.cursor.length > 1) doc.cursor.pop();
}

function $ui_node_emitText(builder, content) {
  $scene_add($scene_of(builder), 1, "", "", content);
}

function $ui_node_openText(builder) {
  return BigInt($scene_add($scene_of(builder), 1, "", "", ""));
}

function $ui_node_patchText(builder, at, content) {
  const record = $scene_of(builder).records[Number(at)];
  if (record !== undefined) record.text = content;
}

// Emits an element and enters it like `$ui_node_emitElement`, answering the
// index a reactive style patches its body by — its identity minted once.
function $ui_node_openElement(builder, name, body) {
  const doc = $scene_of(builder);
  const index = $scene_add(doc, 0, name, body, "");
  doc.cursor.push({ parent: index, anchor: null });
  return BigInt(index);
}

// Writes `body` over the element `openElement` answered, the element kept — a
// `$tree_styles` bind re-running for the scene.
function $ui_node_patchBody(builder, at, body) {
  const record = $scene_of(builder).records[Number(at)];
  if (record !== undefined) record.body = body;
}

// The renderer's own `watch`: registers `body` in the graph and runs it once,
// so a leaf follows its prop and a region rebuilds on every change to what its
// build read. The graph owns the watcher and disposes it with whatever created
// it — the native `$ui_node_reactive` / `buri_rt_ui_watch`.
function $ui_node_reactive(body) {
  $ui_run($ui_cell(2, undefined, body));
}

function $ui_node_enterDynamic(builder) {
  const doc = $scene_of(builder);
  const start = $scene_add(doc, 2, "", "", "");
  const end = $scene_add(doc, 2, "", "", "");
  doc.regions.push({ start, end });
  return BigInt(doc.regions.length - 1);
}

// Clears a region and points the builder at its gap (`$scene_beginRegion`
// pushes the frame), then `endRegion` pops it — `rebuildRegion` split open, for
// a reactive widget that emits inline under its own watcher and needs no ctx.
function $ui_node_beginRegion(builder, region) {
  $scene_beginRegion($scene_of(builder), Number(region));
}

function $ui_node_endRegion(builder) {
  const doc = $scene_of(builder);
  if (doc.cursor.length > 1) doc.cursor.pop();
}

// Clears a region and points the builder at the gap, the native
// `Document::begin_region`: every record between the two markers is unlinked
// from its parent, then a frame is pushed that inserts before the end marker so
// the re-walk lands where the last one did.
function $scene_beginRegion(doc, region) {
  const reg = doc.regions[region];
  if (reg === undefined) return;
  const start = reg.start;
  const end = reg.end;
  const parent = doc.records[start] !== undefined ? doc.records[start].parent : 0;
  const children = doc.records[parent].children;
  const si = children.indexOf(start);
  const ei = children.indexOf(end);
  if (si >= 0 && ei >= 0 && ei > si + 1) {
    const removed = children.splice(si + 1, ei - si - 1);
    for (const child of removed) {
      if (doc.records[child] !== undefined) doc.records[child].parent = null;
    }
  }
  doc.cursor.push({ parent, anchor: end });
}

function $ui_node_rebuildRegion(builder, region, node, walk) {
  const doc = $scene_of(builder);
  $scene_beginRegion(doc, Number(region));
  walk(doc.ctx, builder, node);
  if (doc.cursor.length > 1) doc.cursor.pop();
}

// A marker under `parent`, inserted before the child `anchor` — the positional
// twin of `$scene_add`, reached by the reconciler. The native
// `Document::marker_before`.
function $scene_markerBefore(doc, parent, anchor) {
  const index = doc.records.length;
  const record = $scene_record(2, "", "", "");
  record.parent = parent;
  doc.records.push(record);
  const children = doc.records[parent].children;
  let pos = children.indexOf(anchor);
  if (pos < 0) pos = children.length;
  children.splice(pos, 0, index);
  return index;
}

// Moves the contiguous run of children from `start` to `end` (a row's two
// markers and everything between them) to just before `anchor` — the native
// `Document::move_block` / `$tree_move`.
function $scene_moveBlock(doc, parent, start, end, anchor) {
  const children = doc.records[parent].children;
  const si = children.indexOf(start);
  const ei = children.indexOf(end);
  if (si < 0 || ei < 0 || ei < si) return;
  const block = children.splice(si, ei - si + 1);
  let pos = children.indexOf(anchor);
  if (pos < 0) pos = children.length;
  children.splice(pos, 0, ...block);
}

// Unlinks a row's run of children from `parent`, so no reader reaches it — the
// native `Document::detach_block` / `$tree_detach`.
function $scene_detachBlock(doc, parent, start, end) {
  const children = doc.records[parent].children;
  const si = children.indexOf(start);
  const ei = children.indexOf(end);
  if (si < 0 || ei < 0 || ei < si) return;
  const block = children.splice(si, ei - si + 1);
  for (const child of block) {
    if (doc.records[child] !== undefined) doc.records[child].parent = null;
  }
}

function $ui_node_enterEach(builder) {
  const doc = $scene_of(builder);
  // The rows hang off this owner rather than off the reconciling watcher, so a
  // row survives the watcher re-running — the native `$tree_each`'s owner.
  const listOwner = $ui_cell(3, undefined, null);
  const start = $scene_add(doc, 2, "", "", "");
  const end = $scene_add(doc, 2, "", "", "");
  const parent = doc.records[start].parent;
  doc.eachRegions.push({ start, end, parent, listOwner, rows: [] });
  return BigInt(doc.eachRegions.length - 1);
}

// The native `$tree_reconcile`: key the prior rows, walk the new keys
// backwards, move a surviving key's records (same records, same identities) and
// build only genuinely new keys; a departed key's row owner is disposed and
// forgotten, so the next write runs the computations of the rows that stayed
// and no others.
function $ui_node_reconcile(builder, region, keys, build) {
  const doc = $scene_of(builder);
  const reg = doc.eachRegions[Number(region)];
  if (reg === undefined) return;
  const end = reg.end;
  const parent = reg.parent;
  const listOwner = reg.listOwner;
  const byKey = new Map();
  for (const row of reg.rows) byKey.set(row.key, row);
  const next = new Array(keys.length);
  let anchor = end;
  for (let i = keys.length - 1; i >= 0; i--) {
    const key = keys[i];
    if (byKey.has(key)) {
      const row = byKey.get(key);
      byKey.delete(key);
      $scene_moveBlock(doc, parent, row.start, row.end, anchor);
      anchor = row.start;
      next[i] = row;
    } else {
      const rowOwner = $ui_under(listOwner, () => $ui_cell(3, undefined, null));
      const start = $scene_markerBefore(doc, parent, anchor);
      const rowEnd = $scene_markerBefore(doc, parent, anchor);
      doc.cursor.push({ parent, anchor: rowEnd });
      $ui_under(rowOwner, () => {
        build(doc.ctx, 0n, BigInt(i));
        return 0;
      });
      if (doc.cursor.length > 1) doc.cursor.pop();
      anchor = start;
      next[i] = { key, start, end: rowEnd, owner: rowOwner };
    }
  }
  for (const row of byKey.values()) {
    $scene_detachBlock(doc, parent, row.start, row.end);
    $ui_dispose(row.owner);
    $ui_forget(listOwner, row.owner);
  }
  reg.rows = next;
}

// A button's or a form's handler, kept as a thunk on the open element so a
// press or a submit fires it in one update transaction.
function $ui_node_registerPress(builder, onPress) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) {
    record.press = () => $ui_flush(() => onPress($taskApart(doc.ctx), [0n]));
  }
}

// Keeps an `onPressOutside`'s handler on the document, paired with the open
// element, so a press landing outside that element's subtree fires it. Tied to
// the graph through `$ui_dispose_with`, so disposing the subtree (a `choose`
// that shuts) marks the pair dead and a later press reaches nothing — the
// native `registerOutside` / the JavaScript `$dom_outside`.
function $ui_node_registerOutside(builder, handler) {
  const doc = $scene_of(builder);
  const elem = $scene_openElement(doc);
  const entry = {
    fire: () => $ui_flush(() => handler($taskApart(doc.ctx), [0n])),
    elem,
    alive: true,
  };
  doc.outside.push(entry);
  $ui_dispose_with(() => {
    entry.alive = false;
  });
}

// The signal a field's or a toggle's value is bound to, stored on the open
// element so `fill` writes it a string and `flip` its negation.
function $ui_node_registerValue(builder, signal) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) record.valueSignal = Number(signal);
}

// The signal a field's caret/selection is bound to, stored on the open element
// so `select` writes it an `(anchor, focus)` pair. Never touched while the scene
// is built — a caret is not in the paint.
function $ui_node_registerSelection(builder, signal) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) record.selectionSignal = Number(signal);
}

// Keeps a control's accessible name on the open element, so `press` addresses a
// button by the label a reader hears rather than its glyphs.
function $ui_node_registerLabel(builder, label) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) record.label = label;
}

// Keeps a route link's plain-click handler (its destination captured) on the
// open anchor, so `follow` fires it and `openInNewTab` does not.
function $ui_node_registerFollow(builder, dest, onFollow) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) {
    record.follow = () => $ui_flush(() => onFollow($taskApart(doc.ctx), dest));
  }
}

// Flags the open button as the one whose press submits its form.
function $ui_node_markSubmit(builder) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) record.submit = true;
}

// A file picker's handler, kept on the open element in a slot `press` never
// reads, so only `pickFile` fires it.
function $ui_node_registerPick(builder, onPick) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) {
    record.pick = () => $ui_flush(() => onPick($taskApart(doc.ctx), [0n]));
  }
}

// A pointer handler, kept on the open element under its phase (0 down, 1 move,
// 2 up) for the `pointerDown`/`pointerMove`/`pointerUp` dispatch to fire.
function $ui_node_registerPointer(builder, phase, handler) {
  const doc = $scene_of(builder);
  const record = doc.records[$scene_openElement(doc)];
  if (record !== undefined) {
    record.pointer[Number(phase)] = () => $ui_flush(() => handler($taskApart(doc.ctx), [0n]));
  }
}

// Where the pointer is for the handler being fired, read back by its `PointerAt`.
function $ui_node_pointerX(builder) {
  return $scene_of(builder).pointer.x;
}

function $ui_node_pointerY(builder) {
  return $scene_of(builder).pointer.y;
}

function $ui_node_pointerOverRow(builder) {
  return $scene_of(builder).pointer.row !== undefined;
}

function $ui_node_pointerRow(builder) {
  const row = $scene_of(builder).pointer.row;
  return row === undefined ? "" : row;
}

// The file `pickFile` offered, read back by the picker's handler.
function $ui_node_offeredName(builder) {
  return $scene_of(builder).offer.name;
}

function $ui_node_offeredType(builder) {
  return $scene_of(builder).offer.type;
}

function $ui_node_offeredBytes(builder) {
  return $scene_of(builder).offer.content;
}

// `mount(ctx, root, walk)` — open a scene document, walk `root` into it with the
// `renderInto` closure `render` passed, and answer the handle a `Rendered`
// carries. The builder is the same one-field handle a `Rendered` is, so a
// reader and the walk name the one document. The initial walk runs each leaf
// and region watcher once, so what comes back is already the resting tree.
function $host_testing_mount(ctx, root, walk) {
  const handle = $handle($scene_open(ctx));
  walk(ctx, handle, root);
  return handle[0];
}

// --- The readers a `Rendered` answers from the scene document ------------------

// The value of a `prop:value` declaration in a scene `e`-line body, or
// undefined — the native `decl_value`.
function $scene_declValue(body, prop) {
  for (const token of body.split(";")) {
    const at = token.indexOf(":");
    if (at >= 0 && token.slice(0, at) === prop) return token.slice(at + 1);
  }
  return undefined;
}

// The class names a body's `class:<names>` token carries — the native
// `classes_of`.
function $scene_classes(body) {
  for (const token of body.split(";")) {
    if (token.startsWith("class:")) {
      return token.slice("class:".length).split(" ").filter((s) => s !== "");
    }
  }
  return [];
}

// Whether a control's flag is set — the scene's `disabled:true`.
function $scene_isDisabled(body) {
  return $scene_declValue(body, "disabled") === "true";
}

// The input kinds that block implicit submission, read from `field:<kind>` —
// the HTML Standard's list, the native `blocks_submission`.
const $SCENE_BLOCKING = { text: true, password: true, email: true, number: true, search: true };

function $scene_blocksSubmission(body) {
  return $SCENE_BLOCKING[$scene_declValue(body, "field")] === true;
}

// The elements whose own text an accessible-name walk does not fold into an
// ancestor's name, so a field label's name stays its own text and not the value
// run inside its input. The native `is_control`.
const $SCENE_CONTROL = { input: true, textarea: true, select: true, button: true };

// The accessible name a reader hears for `node`: every run of text in its
// subtree, in document order, joined by a space — the same joining `text()`
// does for a whole tree, so a link wrapping many runs is addressed by the words
// it shows and not by the runs run together. It does not descend into a nested
// control, so a field label's name stays its own text. The native
// `Document::accessible_name`.
function $scene_accessibleName(doc, node) {
  const runs = [];
  $scene_nameRuns(doc, node, true, runs);
  return runs.join(" ");
}

// Gathers the text runs of `node`'s subtree into `out`, skipping the subtree of
// any nested control. `root` is the node the name is computed for, whose own
// control-ness never stops the walk.
function $scene_nameRuns(doc, node, root, out) {
  const r = doc.records[node];
  if (!root && r.kind === 0 && $SCENE_CONTROL[r.name] === true) return;
  if (r.kind === 1) out.push(r.text);
  for (const child of r.children) $scene_nameRuns(doc, child, false, out);
}

// The first element of `name`, in document order, whose own label is `label` —
// the native `labelled` / `$tree_labelled`, or `-1` where there is none.
function $scene_labelled(doc, name, label) {
  for (const [i] of $scene_ordered(doc)) {
    const r = doc.records[i];
    // A control's accessible name is its stored label where it has one, the
    // text of its descendants otherwise — a button's glyphs are not its name.
    const named = r.label === "" ? $scene_accessibleName(doc, i) : r.label;
    if (r.kind === 0 && r.name === name && named === label) return i;
  }
  return -1;
}

// The first descendant of `node` (itself included) whose name is one of
// `names`, or `-1` — the native `first_named`.
function $scene_firstNamed(doc, node, names) {
  const r = doc.records[node];
  if (r.kind === 0 && names.indexOf(r.name) >= 0) return node;
  for (const child of r.children) {
    const found = $scene_firstNamed(doc, child, names);
    if (found >= 0) return found;
  }
  return -1;
}

// Whether `node` is `ancestor` itself or a descendant of it — a press inside
// the subtree `onPressOutside` watches is not a press outside it.
function $scene_within(doc, ancestor, node) {
  let at = node;
  while (at !== null && at !== undefined) {
    if (at === ancestor) return true;
    at = doc.records[at].parent;
  }
  return false;
}

// The nearest element of `name` at or above `node`, or `-1` — the native
// `enclosing` / `$dom_enclosing`.
function $scene_enclosing(doc, node, name) {
  let at = node;
  while (at !== null && at !== undefined) {
    const r = doc.records[at];
    if (r.kind === 0 && r.name === name) return at;
    at = r.parent;
  }
  return -1;
}

// Every element of `name` reachable from the host, in document order — the
// native `elements`.
function $scene_elements(doc, name) {
  const out = [];
  for (const [i] of $scene_ordered(doc)) {
    if (doc.records[i].kind === 0 && doc.records[i].name === name) out.push(i);
  }
  return out;
}

// Every element of `name` at or under `node`, in document order — the native
// `descendants`.
function $scene_descendants(doc, node, name) {
  const out = [];
  $scene_gather(doc, node, name, out);
  return out;
}

function $scene_gather(doc, node, name, out) {
  const r = doc.records[node];
  if (r.kind === 0 && r.name === name) out.push(node);
  for (const child of r.children) $scene_gather(doc, child, name, out);
}

// What this element declares for `pointer-events`, or undefined — an inline
// declaration first, then a `pass-<value>` class. The native `pointer_events`:
// the resolved declaration the reachability walk reads, off the element's own
// scene body rather than the extracted sheet.
function $scene_pointerEvents(doc, node) {
  const body = doc.records[node].body;
  const v = $scene_declValue(body, "pointer-events");
  if (v !== undefined) return v !== "none";
  for (const cls of $scene_classes(body)) {
    if (cls.startsWith("pass-")) return cls.slice("pass-".length) !== "none";
  }
  return undefined;
}

// Whether the pointer reaches `node` — the native `reachable`. `pointer-events`
// inherits, so the nearest ancestor that declares one answers.
function $scene_reachable(doc, node) {
  let at = node;
  while (at !== null && at !== undefined) {
    const reaches = $scene_pointerEvents(doc, at);
    if (reaches !== undefined) return reaches;
    at = doc.records[at].parent;
  }
  return true;
}

// Whether a dialog has taken `node` out of the page — the native `inert`. A
// dialog is rendered only while open, so a `dialog` in the tree is an open one:
// what is inside it is reached, and what is outside every one is inert while
// any is present.
function $scene_inert(doc, node) {
  let at = node;
  while (at !== null && at !== undefined) {
    const r = doc.records[at];
    if (r.kind === 0 && r.name === "dialog") return false;
    at = r.parent;
  }
  return $scene_elements(doc, "dialog").length > 0;
}

// A run's content escaped for a `t` line — the native `escape_run` /
// `ui_node.buri`'s `textLine`: a backslash first (or the next two would
// double-escape), then a newline and a carriage return, and no fourth.
function $scene_escapeRun(content) {
  return content.split("\\").join("\\\\").split("\n").join("\\n").split("\r").join("\\r");
}

function $host_testing_Rendered_markup(self) {
  const doc = $scene_of(self);
  const lines = [];
  for (const pair of $scene_ordered(doc)) {
    const record = doc.records[pair[0]];
    if (record.kind === 0) lines.push("e " + pair[1] + " " + record.body);
    else if (record.kind === 1) lines.push("t " + pair[1] + " " + $scene_escapeRun(record.text));
  }
  return lines.join("\n");
}

function $host_testing_Rendered_text(self) {
  const doc = $scene_of(self);
  const runs = [];
  for (const pair of $scene_ordered(doc)) {
    if (doc.records[pair[0]].kind === 1) runs.push(doc.records[pair[0]].text);
  }
  return runs.join(" ");
}

function $host_testing_Rendered_press(self, label) {
  const doc = $scene_of(self);
  const button = $scene_labelled(doc, "button", label);
  if (button < 0) $abort('this tree has no button labelled "' + label + '"');
  // Out of reach when the pointer passes through it, or when a `dialog` has
  // taken it out of the page.
  if (!$scene_reachable(doc, button) || $scene_inert(doc, button)) return 0;
  // The press reaches the document before the button: an overlay watching for a
  // press outside itself sees this one and fires on it, unless it landed inside
  // its subtree. A pair whose subtree has gone is dead and fires nothing.
  for (const entry of doc.outside) {
    if (entry.alive && !$scene_within(doc, entry.elem, button)) entry.fire();
  }
  const record = doc.records[button];
  if ($scene_isDisabled(record.body)) return 0;
  if (record.press !== null) record.press();
  // A submit button has no handler of its own: reaching the form is the
  // browser's default action for the press.
  if (record.submit) {
    const form = $scene_enclosing(doc, button, "form");
    if (form >= 0 && doc.records[form].press !== null) doc.records[form].press();
  }
  return 0;
}

// A route link's follow handler is not in the scene document — the scene drops
// it, as `describe` does — so a headless scene does not follow one. The blocks
// that assert following render the HTML document double and live in the JS-only
// sibling; this resolves the anchor by its text the way a reader would and
// otherwise does nothing, the same observable nothing an ordinary link is.
function $host_testing_Rendered_follow(self, label) {
  const doc = $scene_of(self);
  const anchor = $scene_labelled(doc, "a", label);
  if (anchor < 0) $abort('this tree has no a labelled "' + label + '"');
  // A plain left-click the app answers in place: a route link's handler moves
  // the address and keeps the tree. An ordinary link carries none, so nothing
  // observable happens — which is what a test asserts by what did not change.
  if (!$scene_reachable(doc, anchor) || $scene_inert(doc, anchor)) return 0;
  const record = doc.records[anchor];
  if (record.follow !== null) record.follow();
  return 0;
}

function $host_testing_Rendered_openInNewTab(self, label) {
  const doc = $scene_of(self);
  const anchor = $scene_labelled(doc, "a", label);
  if (anchor < 0) $abort('this tree has no a labelled "' + label + '"');
  // A modified click — the middle button, or a held modifier — is the browser's
  // to follow as an ordinary anchor, so a route link leaves the address where
  // it is and its handler never runs.
  return 0;
}

function $host_testing_Rendered_fill(self, label, value) {
  const doc = $scene_of(self);
  const labelNode = $scene_labelled(doc, "label", label);
  const field = labelNode < 0 ? -1 : $scene_firstNamed(doc, labelNode, ["input", "textarea"]);
  if (field < 0) $abort('the label "' + label + '" is not a field');
  const record = doc.records[field];
  // Nothing is typed into a disabled or inert field.
  if ($scene_isDisabled(record.body) || $scene_inert(doc, field)) return 0;
  if (record.valueSignal >= 0) {
    const signal = record.valueSignal;
    // A slider binds a `Signal<Float>` and its `value` is a string on the
    // control, so what a reader types has to cross back the way the browser's
    // `input` listener crosses it — `Number` in — or the signal holds a `Str`
    // where a `Float` belongs, and `==` and `Show` then dispatch on the wrong
    // tag (`80` compares unequal to `80.0`, and to itself). A text field's
    // signal is a `Str`, so it takes the value as typed.
    const typed = $scene_declValue(record.body, "field") === "range" ? Number(value) : value;
    $ui_flush(() => $ui_write(signal, typed));
  }
  return 0;
}

function $host_testing_Rendered_select(self, label, start, end) {
  const doc = $scene_of(self);
  const labelNode = $scene_labelled(doc, "label", label);
  const field = labelNode < 0 ? -1 : $scene_firstNamed(doc, labelNode, ["input", "textarea"]);
  if (field < 0) $abort('the label "' + label + '" is not a field');
  const record = doc.records[field];
  // Nothing moves the caret of a disabled or inert field.
  if ($scene_isDisabled(record.body) || $scene_inert(doc, field)) return 0;
  // A field built without a `selection` signal keeps no caret, so there is
  // nowhere to write and this is the observable nothing the interface promises.
  if (record.selectionSignal >= 0) {
    const signal = record.selectionSignal;
    // An `(anchor, focus)` pair — two `Int`s, which cross as `BigInt`s in a
    // two-element array, the shape a `(Int, Int)` tuple has here.
    const pair = [BigInt(start), BigInt(end)];
    $ui_flush(() => $ui_write(signal, pair));
  }
  return 0;
}

function $host_testing_Rendered_flip(self, label) {
  const doc = $scene_of(self);
  const labelNode = $scene_labelled(doc, "label", label);
  const box = labelNode < 0 ? -1 : $scene_firstNamed(doc, labelNode, ["input"]);
  if (box < 0) $abort('the label "' + label + '" is not a toggle');
  const record = doc.records[box];
  if ($scene_isDisabled(record.body) || $scene_inert(doc, box)) return 0;
  if (record.valueSignal >= 0) {
    const signal = record.valueSignal;
    $ui_flush(() => $ui_write(signal, !$ui_read(signal)));
  }
  return 0;
}

// `pickFile`'s two halves: keep the file on the document, then fire the
// picker's handler, which reads it back. A disabled or unreachable picker opens
// no chooser, so nothing runs.
function $host_testing_offerFile(page, name, type, content) {
  $scene_of(page).offer = { name, type, content };
  return 0;
}

function $host_testing_deliverFile(page, label) {
  const doc = $scene_of(page);
  const button = $scene_labelled(doc, "button", label);
  if (button < 0 || doc.records[button].pick === null) {
    $abort('this tree has no file picker labelled "' + label + '"');
  }
  const record = doc.records[button];
  if (!$scene_reachable(doc, button) || $scene_inert(doc, button)) return 0;
  if ($scene_isDisabled(record.body)) return 0;
  record.pick();
  return 0;
}

// Pressing Enter in a field, which is implicit submission — the platform's
// rule, the native `submit`: an enabled submit button submits the form, and a
// form with none is submitted only while exactly one of its fields blocks
// implicit submission. A form a `dialog` has taken out of reach submits nothing.
function $host_testing_Rendered_submit(self, at) {
  const doc = $scene_of(self);
  const forms = $scene_elements(doc, "form");
  const index = Number(at);
  if (index < 0 || index >= forms.length) $abort("this tree has no form " + index);
  const form = forms[index];
  if ($scene_inert(doc, form)) return 0;
  let hasSubmit = false;
  for (const b of $scene_descendants(doc, form, "button")) {
    if (doc.records[b].submit && !$scene_isDisabled(doc.records[b].body)) hasSubmit = true;
  }
  let blocking = 0;
  for (const i of $scene_descendants(doc, form, "input")) {
    if ($scene_blocksSubmission(doc.records[i].body)) blocking++;
  }
  if ((hasSubmit || blocking === 1) && doc.records[form].press !== null) {
    doc.records[form].press();
  }
  return 0;
}

// The innermost element whose name — its label, or else its text — is `label`,
// or `-1`: the first such element in document order, then down through any
// child element that has the same name. The native `Document::named`.
function $scene_named(doc, label) {
  const matches = (i) => {
    const r = doc.records[i];
    if (r.kind !== 0) return false;
    return (r.label === "" ? $scene_accessibleName(doc, i) : r.label) === label;
  };
  let found = -1;
  for (const [i] of $scene_ordered(doc)) {
    if (matches(i)) {
      found = i;
      break;
    }
  }
  if (found < 0) return -1;
  for (;;) {
    const inner = doc.records[found].children.find(matches);
    if (inner === undefined) return found;
    found = inner;
  }
}

// The innermost keyed row holding `node`, as `{ list, key, start }`, or null —
// read off the row markers the reconciler keeps: at each level up, the row of
// an `each` under that parent whose two markers stand either side of the node.
// The native `Document::row_of`.
function $scene_rowOf(doc, node) {
  let at = node;
  for (;;) {
    const parent = doc.records[at].parent;
    if (parent === null || parent === undefined) return null;
    const children = doc.records[parent].children;
    const pos = children.indexOf(at);
    let best = null;
    let bestStart = -1;
    doc.eachRegions.forEach((region, list) => {
      if (region.parent !== parent) return;
      for (const row of region.rows) {
        const s = children.indexOf(row.start);
        const e = children.indexOf(row.end);
        if (s >= 0 && s < pos && pos < e && s > bestStart) {
          best = { list, key: row.key, start: row.start };
          bestStart = s;
        }
      }
    });
    if (best !== null) return best;
    at = parent;
  }
}

// The key of the row under the pointer, `over`, in the list the element `at`
// is a row of — undefined when `at` is in no row or `over` in none of that
// list's. The native `Document::over_row`.
function $scene_overRow(doc, at, over) {
  const own = $scene_rowOf(doc, at);
  if (own === null) return undefined;
  for (let row = $scene_rowOf(doc, over); row !== null; row = $scene_rowOf(doc, row.start)) {
    if (row.list === own.list) return row.key;
  }
  return undefined;
}

// Fires `phase`'s handler on `target` and on every element out to the root,
// the way a pointer event bubbles. The path is taken first, as a browser's is,
// so a handler that rebuilds the tree does not change who hears this event.
function $scene_firePointer(doc, phase, target, over, x, y) {
  const path = [];
  for (let at = target; at !== null && at !== undefined; at = doc.records[at].parent) {
    const handler = doc.records[at].pointer[phase];
    if (handler !== null) path.push([handler, $scene_overRow(doc, at, over)]);
  }
  for (const [handler, row] of path) {
    doc.pointer = { x, y, row };
    handler();
  }
}

// The element `label` names, or an abort — what every pointer method starts by.
function $scene_pointerTarget(doc, label) {
  const node = $scene_named(doc, label);
  if (node < 0) $abort('this tree has no element named "' + label + '"');
  return node;
}

function $host_testing_Rendered_pointerDown(self, label, x, y) {
  const doc = $scene_of(self);
  const target = $scene_pointerTarget(doc, label);
  if (!$scene_reachable(doc, target) || $scene_inert(doc, target)) return 0;
  // A press reaches an overlay watching for one outside itself, as `press` does.
  for (const entry of doc.outside) {
    if (entry.alive && !$scene_within(doc, entry.elem, target)) entry.fire();
  }
  // The nearest element with a pointer handler captures the pointer.
  doc.capture = -1;
  for (let at = target; at !== null && at !== undefined; at = doc.records[at].parent) {
    if (doc.records[at].pointer.some((h) => h !== null)) {
      doc.capture = at;
      break;
    }
  }
  $scene_firePointer(doc, 0, target, target, x, y);
  return 0;
}

// Where a move or a release goes: the element holding the capture while it is
// still in the tree, and otherwise what the pointer is over, if it is reached.
function $scene_pointerRoute(doc, over) {
  if (doc.capture >= 0 && $scene_within(doc, 0, doc.capture)) return doc.capture;
  if (!$scene_reachable(doc, over) || $scene_inert(doc, over)) return -1;
  return over;
}

function $host_testing_Rendered_pointerMove(self, label, x, y) {
  const doc = $scene_of(self);
  const over = $scene_pointerTarget(doc, label);
  const target = $scene_pointerRoute(doc, over);
  if (target >= 0) $scene_firePointer(doc, 1, target, over, x, y);
  return 0;
}

function $host_testing_Rendered_pointerUp(self, label, x, y) {
  const doc = $scene_of(self);
  const over = $scene_pointerTarget(doc, label);
  const target = $scene_pointerRoute(doc, over);
  doc.capture = -1;
  if (target >= 0) $scene_firePointer(doc, 2, target, over, x, y);
  return 0;
}

function $host_testing_Rendered_count(self, name) {
  return BigInt($scene_elements($scene_of(self), name).length);
}

function $host_testing_Rendered_identity(self, name, at) {
  const doc = $scene_of(self);
  const elements = $scene_elements(doc, name);
  const index = Number(at);
  if (index < 0 || index >= elements.length) {
    $abort("this tree has no " + name + " " + index);
  }
  return BigInt(doc.records[elements[index]].identity);
}

// --- The test platform ------------------------------------------------------------
//
// The doubles carry an I64 handle rather than their state, because Buri has no
// mutation. Each call to a constructor allocates a fresh one, which is why a
// named context is called rather than referred to.

// `from` is the table's length as the current `test` block started, which is
// what makes `$test_leave` a question about that block's doubles. `$run` sets
// it; `buri_rt_test_enter` marks the same watermark natively.
// `pass`, `total` and `note` are `everyOrder`'s: which run of the block's body
// this is, how many there are, and the order the report would name. `$run`
// resets all three as a block starts, which is what `buri_rt_test_enter` does
// natively.
// `seed` is the order `tasks().anyOrder()` schedules with, spliced in by
// `commands/test.rs` beside the fixed clock: a constant of the program, derived
// from the program's own action key (D-10). `null` is a run nothing spliced one
// into — a hand-written driver, or an artifact run directly — and the default is
// then the one D5 chose, the last rank.
const $t = { h: [], fail: null, from: 0, pass: 0n, total: 1n, note: null, seed: null };

function $handle(v) {
  $t.h.push(v);
  return [BigInt($t.h.length - 1)];
}

function $slot(x) {
  return $t.h[Number(x[0])];
}

// --- platform/effect/testing --------------------------------------------------------------
//
// The test implementations' names, called rather than referred to, over the `$t.h` table:
// one handle store, and the Buri type of the value carrying a handle says which
// slot shape made it.
//
// Configuration answers a *new* handle rather than editing the one it was
// called on, so `clock()` and `clock().at(1000)` are two clocks and a test
// holding both holds two. `TestFileSystem.readOnly` is the one that answers a new
// handle over the *same* two objects, because attenuating a filesystem is not
// copying it.

function $host_testing_alloc() {
  return $handle({});
}

// `Region` is a newtype over `I64`, so the charge stays a `BigInt`: the count
// is handed straight back, which is what both native backends open-code and
// what makes `alloc.allocate(ctx, 64) == Region(64)` true on every backend.
function $host_testing_TestAllocator_allocate(self, n) {
  return [n];
}

function $host_testing_stdout() {
  return $handle({ text: "" });
}

function $host_testing_stderr() {
  return $handle({ text: "" });
}

// The five captured writers answer `Result<(), IoError>` and always answer
// `.Ok(())`: a captured stream is a string this runner owns, so there is
// nothing to fail. The shape is the effect's rather than the implementation's —
// a test writes the same line a program does.
function $host_testing_TestStdout_print(self, t) {
  $slot(self).text += t;
  return $ok(0);
}

function $host_testing_TestStdout_println(self, t) {
  $slot(self).text += t + "\n";
  return $ok(0);
}

// Captured as the text the octets spell, so `captured` answers one question
// rather than two, which is what `cli/runtime/testing.rs` writes as well.
function $host_testing_TestStdout_writeBytes(self, b) {
  const r = $bytes_fromUtf8(null, b);
  $slot(self).text += r[0] === 0 ? r[1] : String.fromCharCode.apply(null, b);
  return $ok(0);
}

function $host_testing_TestStdout_captured(self) {
  return $slot(self).text;
}

function $host_testing_TestStderr_eprint(self, t) {
  $slot(self).text += t;
  return $ok(0);
}

function $host_testing_TestStderr_eprintln(self, t) {
  $slot(self).text += t + "\n";
  return $ok(0);
}

function $host_testing_TestStderr_captured(self) {
  return $slot(self).text;
}

// End of input until a test says otherwise: no lines and no octets, so
// `readLine` runs off the end and `readBytes` finds nothing.
function $host_testing_stdin() {
  return $handle({ lines: [], at: 0, calls: [] });
}

// A line stream and an octet stream are two streams and a test picks one, so
// these two builders replace each other rather than composing: the last one in
// a chain is the stream.
function $host_testing_TestStdin_lines(self, lines) {
  return $handle({ lines: lines.slice(), at: 0, calls: [] });
}

function $host_testing_TestStdin_bytes(self, b) {
  return $handle({ lines: [], at: 0, bytes: b.slice(), calls: [] });
}

function $host_testing_TestStdin_readLine(self) {
  const s = $slot(self);
  if (s.bytes) return $host_testing_logged(s, ["readLine", 0n], undefined);
  return $host_testing_logged(
    s,
    ["readLine", 0n],
    s.at < s.lines.length ? $some(s.lines[s.at++]) : undefined,
  );
}

function $host_testing_TestStdin_readBytes(self, want) {
  const s = $slot(self);
  const call = ["readBytes", want];
  const n = Number(want);
  const src = s.bytes || [];
  if (s.at >= src.length || n <= 0) return $host_testing_logged(s, call, undefined);
  const out = src.slice(s.at, s.at + n);
  s.at += out.length;
  return $host_testing_logged(s, call, out);
}

// Every read this stream was asked for, in the order they completed.
function $host_testing_TestStdin_calls(self) {
  return $slot(self).calls.map(function (c) {
    return c.slice();
  });
}

// A `TestFileSystem` handle is a *view*: the files and directories it reads and writes,
// and whether writes through this view are refused. `readOnly` answers a second
// view over the *same* two objects, which is what makes `readOnly` a method
// without turning it into a copy — an attenuating view holds the inner
// filesystem, so a read through it sees whatever that holds now.
//
// The slot holds octets per path and the directories `makeDir` has been asked
// for, because a flat map has no empty directory otherwise. `plan` names the fault plan this view fails through, or
// `-1` where nothing has called `faults`; it travels with a builder exactly as
// `ro` does.
//
// Every row below takes the **handle** rather than the `TestFileSystem`, because a
// `TestFileSystem` is a handle *and* a fault plan and an argument crosses as its leaves.
// That is `$host_testing_netCalls`'s reason, one slice later.
function $tslot(h) {
  return $t.h[Number(h)];
}

// A slot, and the bare handle that names it: `$handle` answers the newtype and
// these answer the `I64` inside it.
function $tmint(v) {
  return $handle(v)[0];
}

function $host_testing_newFs() {
  return $tmint({ files: {}, dirs: [], ro: false, plan: -1, calls: [] });
}

// This view's files with these written over them, in a map of its own, under
// this view's attenuation and plan — so `files` and `filesBytes` compose in
// either order, `fs().readOnly().files(..)` is still read-only, and
// `fs().faults(p).files(..)` still fails what `p` names.
function $host_testing_fsFiles(h, entries) {
  const s = $tslot(h);
  const files = Object.assign({}, s.files);
  for (const e of entries) files[e[0]] = $bytes_toUtf8(null, e[1]);
  return $tmint({ files, dirs: s.dirs.slice(), ro: s.ro, plan: s.plan, calls: [] });
}

function $host_testing_fsFilesBytes(h, entries) {
  const s = $tslot(h);
  const files = Object.assign({}, s.files);
  for (const e of entries) files[e[0]] = e[1].slice();
  return $tmint({ files, dirs: s.dirs.slice(), ro: s.ro, plan: s.plan, calls: [] });
}

// The same two objects, deliberately: a method that copied would be a snapshot
// wearing an attenuator's name.
function $host_testing_fsReadOnly(h) {
  const s = $tslot(h);
  return $tmint({ files: s.files, dirs: s.dirs, ro: true, plan: s.plan, calls: [] });
}

// A view onto the same files with a fresh, empty plan, retiring the plan this
// handle was using: `faults` replaces rather than composing, and a promise that
// has been replaced is not one `$test_leave` should report.
function $host_testing_fsWithPlan(h) {
  const s = $tslot(h);
  $tretire(s.plan);
  const plan = $tmint({ plan: [], retired: false });
  return $tmint({ files: s.files, dirs: s.dirs, ro: s.ro, plan, calls: [] });
}

// Records one call on a slot's log and answers what the method answered.
//
// The answer is an *argument*, so JavaScript has evaluated it by the time this
// runs: the call is recorded on the way out, in the order calls complete, which
// is what `calls()` promises and what `cli/runtime/testing.rs`'s `Recording`
// does with a `Drop`. Nothing suspends yet, so completion order is program
// order — recording it this way is what keeps that true when something does.
function $host_testing_logged(s, call, answer) {
  s.calls.push(call);
  return answer;
}

// Every call this view was asked for, in the order they completed. Through
// *this* handle: a builder and `readOnly` each answer a new one, with a log of
// its own.
function $host_testing_fsCalls(h) {
  return $tslot(h).calls.map(function (c) {
    return c.slice();
  });
}

// The text these octets spell, exactly as `readFile` reads back what
// `writeFileBytes` wrote. An `FsCall` constructor's decode: a test writing a
// call down performs no effect, so it has no context to reach `bytes` with.
function $host_testing_spelled(b) {
  return $utf8Lossy(b);
}

// --- the fault plan's promise -------------------------------------------------
//
// The plan itself never reaches this file. It is a list of Buri values holding
// an `IoError`, and `cli/runtime/lib.rs` §2.1 cannot name an error variant that
// carries a field, so matching is the `Equal` the `Call` records derive and happens
// in `host_testing.buri` — on both backends, from one implementation. What is
// here is the half a program cannot keep: which entries have fired, and what
// each of them would read like in a failure message.
//
// `cli/runtime/testing.rs`'s `Slot::Plan` is the same three fields for the same
// reason, and the two are held together by the conformance corpus.

// `IoError`'s and `NetError`'s variant names, in `platform/effect`'s declaration
// order — `ioCode`'s and `netCode`'s indices.
const $ioErrors = [
  ".NotFound",
  ".PermissionDenied",
  ".ReadOnly",
  ".AlreadyExists",
  ".NotADirectory",
  ".CrossDevice",
  ".Other",
];
const $netErrors = [".Timeout", ".Refused", ".BadUrl", ".Transport", ".Aborted"];

function $tretire(plan) {
  const s = $tslot(plan);
  if (s) s.retired = true;
}

// One error as a message names it: the variant, and the text it carries where
// it carries any.
function $terror(names, code, payload) {
  const name = names[Number(code)] || "?";
  return payload === "" ? name : name + '("' + payload + '")';
}

// Adds one entry to the plan the handle names, with only its call spelled. The
// other half arrives in a second call, and the split is the frame-threaded
// backend's: `stencil/abi.rs`'s `MAX_INT_ARGS` is ten and a `Str` is three of
// them, so one row carrying a call *and* an error would be fifteen. This file
// pays nothing for that and follows it anyway, because the two backends
// implement one module.
function $tfault(h, call) {
  const s = $tslot(h);
  const plan = s && $tslot(s.plan);
  if (plan) plan.plan.push({ shown: call, fired: false });
  return 0;
}

function $host_testing_addFsFault(h, name, path, body) {
  return $tfault(
    h,
    body === "" ? name + '("' + path + '")' : name + '("' + path + '", "' + body + '")',
  );
}

// The URL and not the whole request: matching is `NetCall`'s derived `Equal` and
// reads every field of it, and a message naming every header would be a
// paragraph where a reader wants a line.
function $host_testing_addNetFault(h, url) {
  return $tfault(h, 'fetch("' + url + '")');
}

// The failure half of the entry just added, for whichever double added it: a
// fault fails the same way whatever it names, and only the error enum differs —
// told apart by the slot the plan hangs from.
function $host_testing_faultFails(h, nth, code, payload) {
  const s = $tslot(h);
  const plan = s && $tslot(s.plan);
  const entry = plan && plan.plan[plan.plan.length - 1];
  if (!entry) return 0;
  entry.shown += " fails " + $terror("files" in s ? $ioErrors : $netErrors, code, payload);
  if (Number(nth) !== 0) entry.shown += " on call " + Number(nth);
  return 0;
}

// The entry at `i` has fired, so its promise is kept. Idempotent: a `fails`
// entry fires on every matching call and the first of them keeps the promise.
function $host_testing_noteFault(h, i) {
  const s = $tslot(h);
  const plan = s && $tslot(s.plan);
  const entry = plan && plan.plan[Number(i)];
  if (entry) entry.fired = true;
  return 0;
}

// One call on the path that never reached the row that would have recorded it:
// a call the plan failed is a call, because the code under test asked the
// filesystem for something and was answered.
function $host_testing_noteFsCall(h, name, path, body) {
  $tslot(h).calls.push([name, path, body]);
  return 0;
}

// The end of a `test` block: every fault the block planned has happened, or the
// block fails now with the ones that did not.
//
// `middle::monomorphize` emits this call after every test body, so all three
// backends get it from one place; `$run` marks `$t.from` as the block starts,
// which is what `buri_rt_test_enter` marks natively. The table grows for the
// life of the process, so without that watermark this would report the block
// before this one.
function $test_leave(index) {
  const unconsumed = [];
  for (let i = $t.from; i < $t.h.length; i++) {
    const s = $t.h[i];
    if (!s || !Array.isArray(s.plan) || s.retired) continue;
    for (const e of s.plan) if (!e.fired) unconsumed.push(e.shown);
  }
  if (unconsumed.length === 0) return 0;
  $abort("a fault was planned and never happened: " + unconsumed.join("; "));
  return 0;
}

// --- tasks(): the order the work happens in ---------------------------------------
//
// The one double whose subject is scheduling rather than state.
// `Tasks.parallel` promises its results in the items' order and promises
// nothing about the order the work runs in; `TestTasks` makes that order a
// value a test writes down, and this is where the choice is made.
//
// `cli/runtime/testing.rs` is the same design for the other backend and the two
// have to agree line for line: a seed names an order, and a seed that named two
// different orders on two backends would be a replay line that only works where
// it was printed.

// Program order, one seeded order, every order. The numbers cross from the
// builders, and this is one of the two places that spell them out.
const $TASKS_PROGRAM = 0;
const $TASKS_SEEDED = 1;
const $TASKS_EVERY = 2;

// Six items are 720 runs of a block and seven are 5040.
const $TASKS_CEILING = 6;

function $host_testing_tasks() {
  return $handle({ mode: $TASKS_PROGRAM, seed: -1n, plan: -1, log: [], faults: [] });
}

// A new scheduler at that mode and seed, carrying this one's plan and its
// faults, and a fresh log — every builder in this module answers a new handle
// and a new log, and a builder is configuration rather than a write.
function $tsched(self, mode, seed) {
  const s = $slot(self);
  return $handle({ mode, seed, plan: s.plan, log: [], faults: s.faults.slice() });
}

function $host_testing_TestTasks_anyOrder(self) {
  return $tsched(self, $TASKS_SEEDED, -1n);
}

function $host_testing_TestTasks_everyOrder(self) {
  return $tsched(self, $TASKS_EVERY, -1n);
}

function $host_testing_TestTasks_seed(self, n) {
  return $tsched(self, $TASKS_SEEDED, n);
}

// A new scheduler with a fresh, empty plan and this one's ordering, retiring the
// plan it was using: `faults` replaces rather than composing.
function $host_testing_TestTasks_replan(self) {
  const s = $slot(self);
  $tretire(s.plan);
  const plan = $tmint({ plan: [], retired: false });
  return $handle({ mode: s.mode, seed: s.seed, plan, log: [], faults: [] });
}

// One entry of a plan, in the two places it belongs: the promise `$test_leave`
// reports on, and the three fields the walk matches on — because for tasks the
// matching is here rather than in the program. The two lists are appended
// together and read by the same index.
function $host_testing_TestTasks_addFault(self, index, nth, reason) {
  const shown =
    "task(" + index + ') fails "' + reason + '"' + (nth === 0n ? "" : " on call " + nth);
  $tfault(self[0], shown);
  $slot(self).faults.push({ index: Number(index), nth: Number(nth), reason });
  return 0;
}

function $host_testing_TestTasks_calls(self) {
  return $slot(self).log.map(function (index) {
    return [index];
  });
}

// The runs are the *block*'s: a block with two schedulers in it is still one
// block being run again, so the receiver is read for nothing.
function $host_testing_TestTasks_runs(self) {
  return $t.pass + 1n;
}

function $host_testing_TestTasks_orders(self) {
  return $t.total;
}

// `n!`, as a BigInt, which has no ceiling to saturate at.
function $tfactorial(n) {
  let out = 1n;
  for (let i = 2n; i <= n; i++) out *= i;
  return out;
}

// The `rank`th permutation of `0..n`, counted from zero in lexicographic order,
// wrapping past the last — the factorial number system, read out digit by
// digit. Written this way rather than as a shuffle because a shuffle is not
// invertible, and a report that names a seed has to be one a reader can replay.
function $tpermutation(n, rank) {
  const pool = [];
  for (let i = 0; i < n; i++) pool.push(i);
  let left = n === 0 ? 0n : rank % $tfactorial(BigInt(n));
  const out = [];
  for (let taken = 0; taken < n; taken++) {
    const remaining = $tfactorial(BigInt(n - taken - 1));
    let digit = Number(left / remaining);
    if (digit >= pool.length) digit = pool.length - 1;
    left %= remaining;
    out.push(pool.splice(digit, 1)[0]);
  }
  return out;
}

// The order this run schedules `n` tasks in, and the call where `everyOrder`
// learns how many runs there are to make: the count is not known until a
// `parallel` says it.
function $torder(self, n) {
  const s = $slot(self);
  const orders = $tfactorial(BigInt(n));
  if (s.mode === $TASKS_EVERY && n > $TASKS_CEILING) {
    $abort(
      "everyOrder over " +
        n +
        " tasks is more runs of this block than a suite can finish: " +
        "a fan-out that wide is anyOrder's question",
    );
  }
  let rank = 0n;
  if (s.mode === $TASKS_EVERY) {
    if ($t.total <= 1n) $t.total = orders;
    rank = $t.pass;
  } else if (s.mode === $TASKS_SEEDED) {
    const wrap = orders > 0n ? orders : 1n;
    // `anyOrder()` with no seed of its own: the program's content names the
    // order, and the last rank — the reverse of program order — where nothing
    // named a program. The rank wraps, which is what makes a content-derived
    // number legal at every length.
    const fallback = $t.seed === null ? (orders > 0n ? orders - 1n : 0n) : $t.seed % wrap;
    rank = s.seed < 0n ? fallback : s.seed % wrap;
  }
  const order = $tpermutation(n, rank);
  // The first fan-out of the run, not the last: `everyOrder` enumerates the
  // orders of the first, so that is the one whose number a replay line carries.
  if ($t.note === null) $t.note = { mode: s.mode, rank, order: order.slice() };
  return order;
}

// The plan's answer for the task about to run — and the end of the block where
// there is one. A task answers `B` and every `B` comes from the closure, so a
// double cannot fail one and carry on: what a task that died would do to the run
// is end it.
function $tplanned(self, index) {
  const s = $slot(self);
  if (s.faults.length === 0) return;
  const nth = s.log.filter(function (entry) {
    return Number(entry) === index;
  }).length + 1;
  for (let at = 0; at < s.faults.length; at++) {
    const fault = s.faults[at];
    if (fault.index !== index) continue;
    if (fault.nth !== 0 && fault.nth !== nth) continue;
    $host_testing_noteFault(self[0], BigInt(at));
    $abort("a task was failed by the plan: task(" + index + "): " + fault.reason);
  }
}

// `parallel(self, ctx, items, f)` — every task once, in the order this
// scheduler chose, and the results in the items' order.
//
// **Not** `Promise.all`, which is what the real `HostTasks.parallel` is: a
// double that raced would be the thing it exists to remove, so a task runs to
// completion before the next one starts and the order is the whole of what this
// decides. The result is written at each item's own index rather than appended,
// which is `Tasks.parallel`'s order promise and is why the scheduler is free to
// hand the work out in any order at all.
//
// `self` is the handle this reads the order and the fault plan off; `ctx` is
// the caller's whole context and is what the step is handed. Two values, and
// this double is where the difference bites hardest: `self` is a `TestTasks`
// slot index, so a step handed it in place of a context read a scheduler
// handle as whatever effect it asked for.
// Each step is **awaited**, which is what "runs to completion before the next
// one starts" means for a step that waits. A spawned task is run through here
// (`core/tasks::running`), and a task that sleeps, dials a socket or asks an
// actor suspends part-way; without the await this returned a list of promises
// and the rest of every such task ran after the test had finished asserting.
// `middle::rc::suspends` carries the key so that a caller waits for this too.
async function $host_testing_TestTasks_parallel(self, ctx, xs, f) {
  const out = new Array(xs.length);
  for (const index of $torder(self, xs.length)) {
    $tplanned(self, index);
    out[index] = await f(ctx, BigInt(index), $share(xs[index]));
    $slot(self).log.push(BigInt(index));
  }
  return $own(out);
}

// What a failure report says about the order this run used, or null where
// nothing ordered anything. **The datum D6 renders**: the report is one wire
// format shared by three backends, and widening it is a slice of its own.
function $taskOrderNote() {
  const n = $t.note;
  if (!n || n.mode === $TASKS_PROGRAM) return null;
  const run = $t.total > 1n ? ", order " + ($t.pass + 1n) + " of " + $t.total : "";
  return (
    "the tasks completed in the order " +
    n.order.join(", ") +
    run +
    " — replay it with `tasks().seed(" +
    n.rank +
    ")`"
  );
}

// The end of one run of a `test` body: whether to run it again.
//
// `middle::monomorphize` emits this after `$test_leave`, so the two questions
// are asked in the order a reader wants — did this run keep what it promised,
// and only then is there another order to try. Answering true makes the body
// call itself, which is what "reruns the body" means on all three backends from
// one lowering.
//
// It moves the watermark up, which is the question `$test_leave` leaves open:
// every run installs its own doubles, so a plan the *next* run declares is the
// only plan the next `$test_leave` is asking about.
function $test_replay(index) {
  if ($t.pass + 1n >= $t.total) return false;
  $t.pass += 1n;
  $t.note = null;
  $t.from = $t.h.length;
  return true;
}

// A socket with no network behind it: `open` mints one, the three methods of
// `Sockets` act on it, and `sent` reads back what was pushed.
//
// `cli/runtime/testing.rs` is the same design for the other backend, and the
// two agree on the thing a program can see: a socket is a **slot of its own**,
// so its number is unique across the runner and a socket one double minted is
// not one another double has. That is what makes the three cases `effect
// Sockets` calls alike alike here too — a handle a program invented, a socket
// this double closed, and another double's socket all drop the message.
//
// The close's code and phrase are not kept, for `proc()`'s reason: they are
// what the far side would be told, there is no far side, and a number nothing
// can read back is state held for its own sake.

// The framings are `$FRAME_TEXT` and `$FRAME_BINARY`, the same two indices the
// client above writes. `$FRAME_CLOSED` never reaches this log: only the two
// sends write it.

function $host_testing_sockets() {
  return $handle({ sent: [] });
}

function $host_testing_socketsOpen(h) {
  return $tmint({ owner: Number(h), open: true, closed: null });
}

// Whether a push through this double onto that socket goes anywhere — which is
// `isOpen`'s answer and the send path's test, one function so the two cannot
// disagree.
function $tsockwritable(h, socket) {
  const s = $tslot(socket);
  return !!s && s.owner === Number(h) && s.open === true;
}

function $host_testing_socketsIsOpen(h, socket) {
  return $tsockwritable(h, socket);
}

// Every push through this double, oldest first. A `Sent` is
// `{ socket, frame, text, data }`, which is a struct and so an array of its
// fields in declaration order — the flat record the program builds its
// `Message` out of.
function $host_testing_socketsSent(h) {
  return $tslot(h).sent.map(function (one) {
    return [one[0], one[1], one[2], one[3].slice()];
  });
}

// One push, dropped where the socket is not an open one of this double's.
function $tsockpush(self, socket, frame, text, data) {
  if (!$tsockwritable(self[0], socket)) return 0;
  $slot(self).sent.push([socket, frame, text, data]);
  return 0;
}

function $host_testing_TestSockets_socketSendText(self, socket, text) {
  return $tsockpush(self, socket, $FRAME_TEXT, text, []);
}

function $host_testing_TestSockets_socketSendBytes(self, socket, body) {
  return $tsockpush(self, socket, $FRAME_BINARY, "", body.slice());
}

function $host_testing_TestSockets_socketClose(self, socket, code, reason) {
  if (!$tsockwritable(self[0], socket)) return 0;
  // The code the program chose is kept, so a socket it closed reads that
  // reason back through a `TestWebSocketClient` — a `close` with `.GoingAway`
  // reaches `onClose` as `.GoingAway`, which is what a real client does with a
  // close it started (`net.rs`'s `told`). Clamped to the wire's range, with
  // 1000 (`.Normal`) for anything outside it. The phrase has no far side to
  // reach and is dropped.
  const told = code < 1000n ? 1000n : code > 4999n ? 4999n : code;
  const slot = $tslot(socket);
  slot.open = false;
  slot.closed = told;
  return 0;
}

// A WebSocket client with a script instead of a network: every time it dials
// it delivers the messages it was handed, in order, and then closes normally.
//
// **It writes through the `sockets()` double it was built on** rather than
// minting a socket of its own kind, and that is why it is that double's own
// method. The socket it answers is a socket of that double's, so the program's
// `socket.send(...)` lands in that double's `sent()` and its `socket.close(...)`
// shows up in that double's `isOpen()`. One log, one test, no network.
//
// It takes the **handle** and answers a bare `I64`, like `socketsOpen` and the
// two readers beside it. `dialling` is a Buri body that wraps the answer in a
// `TestWebSocketClient`, so the crossing carries the number and nothing else.
//
// **Nothing here waits.** There is no timer, no promise and no deadline in any
// of it — the script is already in hand, so answering the next message is a
// read of an array. Both bodies are therefore plain functions, which is what
// `platform/effect/testing` declares. And two `dialling(...)` calls are two
// independent worlds, exactly as two `sockets()` calls are.
function $host_testing_socketsDialling(handle, messages) {
  return $tmint({
    owner: Number(handle),
    script: messages.slice(),
    at: 0,
    socket: -1,
    gone: false,
  });
}

function $host_testing_TestWebSocketClient_connectSocket(self, url) {
  const s = $slot(self);
  if (!$wsDials(url)) {
    // The double's one signature failure, so a test can check the refusal path
    // with no network behind it — and it is the real client's answer for the
    // real client's reason.
    const said = "this double dials ws:// and wss://, and not " + $wsScheme(url);
    return $err([$SERVE_UNSUPPORTED, said]);
  }
  // The same mint `socketsOpen` uses, on the double this client was handed.
  // Every dial starts the script again: `connect` returns when a socket closes
  // and reconnecting is a loop around it, so a double that delivered its
  // messages once would answer the second dial with a socket that was already
  // spent.
  const socket = $tmint({ owner: s.owner, open: true, closed: null });
  s.socket = Number(socket);
  s.at = 0;
  s.gone = false;
  return $ok([socket, 101n, [], []]);
}

function $host_testing_TestWebSocketClient_connectReceive(self, socket) {
  const s = $slot(self);
  const key = Number(socket);
  if (s.socket !== key || s.gone) {
    return $err([$SERVE_CLOSED, "this socket has already gone"]);
  }
  // Two ways for the stream to end: the script runs out, or the program closed
  // the socket and the `TestSockets` double no longer reports it open. A script
  // that merely ran out is a close with no program behind it, which is code
  // 1000 — `core/net/server`'s `reasonOf` reads it as `.Normal`. A socket the
  // program closed carries the code it chose, so the reason it closed with is
  // the reason `connect` reports, exactly as a real client reads back a close
  // it started.
  const open = $tsockwritable(s.owner, socket);
  if (s.at >= s.script.length || !open) {
    s.gone = true;
    // The reason the program chose where it closed the socket itself, read
    // before the shut below; 1000 for a script that ran out on its own.
    const code = open ? 1000n : $tslot(socket).closed ?? 1000n;
    // The socket is shut on the double that owns it before the `.Closed` goes
    // out, exactly as `socketClose` would shut it. So it is already closed
    // while `onClose` runs, and a push from that hook is dropped rather than
    // recorded — which is the promise `core/net/websocket` makes.
    if (open) $tslot(socket).open = false;
    return $ok([$FRAME_CLOSED, "", [], code]);
  }
  // A `Message` is `[tag, payload]`, and its two variants are declared in
  // `Frame`'s order: 0 is `.Text(Str)` and 1 is `.Binary([U8])`.
  const message = s.script[s.at];
  s.at += 1;
  if (message[0] === $FRAME_TEXT) return $ok([$FRAME_TEXT, message[1], [], 0n]);
  return $ok([$FRAME_BINARY, "", message[1].slice(), 0n]);
}

// A fresh, empty log, and the handle that names it. A bare `I64` rather than a
// handle-carrying value, because `net()` is a Buri body that builds the
// `TestNetwork` around it — the responder in the other field is a value this file
// cannot make.
function $host_testing_newNet() {
  return $tmint({ calls: [], plan: -1 });
}

// A fresh, empty log carrying this one's plan. `respond` answers a new network
// and a new log, because a network that shared its receiver's log would report
// calls made to a different one; what it does not change is what the test said
// would fail.
function $host_testing_netRebind(h) {
  return $tmint({ calls: [], plan: $tslot(h).plan });
}

// `$host_testing_fsWithPlan` for the network, retiring the replaced plan for
// the same reason.
function $host_testing_netWithPlan(h) {
  $tretire($tslot(h).plan);
  const plan = $tmint({ plan: [], retired: false });
  return $tmint({ calls: [], plan });
}

// One request, recorded once the responder has answered it. `Request`'s five
// fields arrive separately because a `Method` crosses as its variant index;
// they go back together as the `NetCall` the constructor `fetch` builds.
function $host_testing_recordFetch(h, method, url, headers, body, timeout) {
  $t.h[Number(h)].calls.push([
    Number(method),
    url,
    headers.map(function (e) {
      return e.slice();
    }),
    body.slice(),
    timeout,
  ]);
  return 0;
}

// Every request this network answered, in that order. A `NetCall` is a newtype
// over `Request`, so each one is its request in a one-element array.
//
// By the handle and not by the `TestNetwork`: that value carries a responder as
// well, and `TestNetwork.calls` is the Buri body that unwraps it — the one `calls()`
// in this module that is not a row of its own.
function $host_testing_netCalls(h) {
  return $t.h[Number(h)].calls.map(function (r) {
    return [r.slice()];
  });
}

// `tcp()` — a connection with nothing behind it.
//
// A slot is the octets a read draws from, how far through them it has got, the
// streams this double has minted and not closed, and the log. A `TcpCall` is
// its six fields in order, and a stream handle is an `Int`, so it crosses as a
// `BigInt` — which is why `open` is compared with `includes` on `BigInt`s
// rather than with numbers.
function $host_testing_newTcp() {
  return $tmint({ stream: [], taken: 0, open: [], calls: [] });
}

// A **new** double answering reads from those octets, on `TestStdin.bytes`'s
// rule: a builder answers a value, so a test that kept the receiver kept what
// it had.
function $host_testing_tcpStream(h, b) {
  return $tmint({ stream: b.slice(), taken: 0, open: [], calls: [] });
}

// Handles start at one and only go up: a spent one that came back would let a
// test write to a stream it had closed and see it recorded against a live one.
function $host_testing_recordTcpConnect(h, host, port) {
  const slot = $tslot(h);
  const stream = BigInt(slot.calls.filter((c) => c[0] === "connect").length + 1);
  slot.calls.push(["connect", host, port, stream, 0n, []]);
  slot.open.push(stream);
  return stream;
}

// `.None` — `undefined` — is a stream this double does not hold open, which the
// Buri body turns into `.Err(.NotFound)`. An empty answer is the script having
// run out, which is the far side closing.
function $host_testing_recordTcpRead(h, stream, limit) {
  const slot = $tslot(h);
  if (!slot.open.includes(stream)) return undefined;
  slot.calls.push(["read", "", 0n, stream, limit, []]);
  const want = Number(limit) > 0 ? Number(limit) : 0;
  const end = Math.min(slot.stream.length, slot.taken + want);
  const piece = slot.stream.slice(slot.taken, end);
  slot.taken = end;
  return $some(piece);
}

function $host_testing_recordTcpWrite(h, stream, body) {
  const slot = $tslot(h);
  if (!slot.open.includes(stream)) return false;
  slot.calls.push(["write", "", 0n, stream, 0n, body.slice()]);
  return true;
}

// Recorded whether or not the stream was open: what a test asserts is what the
// code under test did, and closing something twice is a thing a program can do.
function $host_testing_recordTcpClose(h, stream) {
  const slot = $tslot(h);
  slot.calls.push(["close", "", 0n, stream, 0n, []]);
  slot.open = slot.open.filter((s) => s !== stream);
  return 0;
}

function $host_testing_tcpCalls(h) {
  return $tslot(h).calls.map((c) => c.slice());
}

// The read-back, without the effect: the same answer `readFile` gives, and no
// `FileSystem` bound needed to ask it.
function $host_testing_fsRead(h, p) {
  const f = $tslot(h).files;
  return p in f ? $ok($utf8Lossy(f[p])) : $err([0]);
}

// Sorted by path, which is `sort()`'s UTF-16 code-unit order and the one
// `readDir` already uses. Files only: a directory holds no octets.
function $host_testing_fsSnapshot(h) {
  const f = $tslot(h).files;
  return Object.keys(f)
    .sort()
    .map(function (k) {
      return [k, $utf8Lossy(f[k])];
    });
}

// Recorded here and not in `read`, which the two share: `read` is the read-back
// and a read-back is not a call.
function $host_testing_fsReadFile(h, p) {
  return $host_testing_logged(
    $tslot(h),
    ["readFile", p, ""],
    $host_testing_fsRead(h, p),
  );
}

// `.ReadOnly` is `IoError`'s third variant, and the six that write are the six
// `ReadOnly<C>` refuses.
function $host_testing_fsWriteFile(h, p, b) {
  const s = $tslot(h);
  const call = ["writeFile", p, b];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  s.files[p] = $bytes_toUtf8(null, b);
  return $host_testing_logged(s, call, $ok(0));
}

function $host_testing_fsFileExists(h, p) {
  const s = $tslot(h);
  return $host_testing_logged(s, ["fileExists", p, ""], p in s.files || s.dirs.includes(p));
}

function $host_testing_fsReadDir(h, p) {
  // A directory that holds nothing is still not an error; only a path that
  // names nothing at all is.
  const prefix = p === "" || p === "." ? "" : p.replace(/\/$/, "") + "/";
  const s = $tslot(h);
  const out = [];
  for (const k of Object.keys(s.files).concat(s.dirs)) {
    if (k.startsWith(prefix)) {
      const rest = k.slice(prefix.length);
      if (rest && !out.includes(rest.split("/")[0])) out.push(rest.split("/")[0]);
    }
  }
  return $host_testing_logged(s, ["readDir", p, ""], $ok(out.sort()));
}

function $host_testing_fsReadFileBytes(h, p) {
  const s = $tslot(h);
  const f = s.files;
  return $host_testing_logged(
    s,
    ["readFileBytes", p, ""],
    p in f ? $ok(f[p].slice()) : $err([0]),
  );
}

function $host_testing_fsWriteFileBytes(h, p, b) {
  const s = $tslot(h);
  const call = ["writeFileBytes", p, $utf8Lossy(b)];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  s.files[p] = b.slice();
  return $host_testing_logged(s, call, $ok(0));
}

function $host_testing_fsAppendFile(h, p, b) {
  const s = $tslot(h);
  const call = ["appendFile", p, $utf8Lossy(b)];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  const f = s.files;
  f[p] = (p in f ? f[p] : []).concat(b);
  return $host_testing_logged(s, call, $ok(0));
}

function $host_testing_fsRenameFile(h, from, to) {
  const s = $tslot(h);
  const call = ["renameFile", from, to];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  const f = s.files;
  if (!(from in f)) return $host_testing_logged(s, call, $err([0]));
  f[to] = f[from];
  delete f[from];
  return $host_testing_logged(s, call, $ok(0));
}

function $host_testing_fsRemoveFile(h, p) {
  const s = $tslot(h);
  const call = ["removeFile", p, ""];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  const f = s.files;
  if (!(p in f)) return $host_testing_logged(s, call, $err([0]));
  delete f[p];
  return $host_testing_logged(s, call, $ok(0));
}

// `rmdir`: the directory must be there and must be empty. A flat map has no
// containment, so "empty" is "no file and no recorded directory has it as a
// prefix" — which is the same question `readDir` answers and the same answer.
// `ENOTEMPTY` has no classified `IoError` variant, so a directory that still
// holds something is `.Other`, as it is on a real filesystem.
function $host_testing_fsRemoveDir(h, p) {
  const s = $tslot(h);
  const call = ["removeDir", p, ""];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  const clean = p.replace(/\/+$/, "");
  const root = clean === "" || clean === ".";
  if (root) return $host_testing_logged(s, call, $err([6, "cannot remove the root"]));
  // The order is the one `cli/runtime/testing.rs` uses, and the two are written
  // to agree: a path naming a file is `.NotADirectory`, one naming nothing is
  // `.NotFound`, and only a directory that is really there can be reported as
  // still holding something.
  if (clean in s.files) return $host_testing_logged(s, call, $err([4]));
  const at = s.dirs.indexOf(clean);
  if (at < 0) return $host_testing_logged(s, call, $err([0]));
  const prefix = clean + "/";
  const held = Object.keys(s.files)
    .concat(s.dirs)
    .some(function (k) {
      return k.startsWith(prefix);
    });
  if (held) return $host_testing_logged(s, call, $err([6, "directory not empty"]));
  s.dirs.splice(at, 1);
  return $host_testing_logged(s, call, $ok(0));
}

// Parents included, an existing directory is `.Ok`, and a path already naming
// a file is `.AlreadyExists` — the three answers `mkdir -p` gives.
function $host_testing_fsMakeDir(h, p) {
  const s = $tslot(h);
  const call = ["makeDir", p, ""];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  const clean = p.replace(/\/+$/, "");
  if (clean === "" || clean === ".") return $host_testing_logged(s, call, $ok(0));
  if (clean in s.files) return $host_testing_logged(s, call, $err([3]));
  const parts = clean.split("/");
  for (let i = 0; i < parts.length; i++) {
    const at = parts.slice(0, i + 1).join("/");
    if (at !== "" && !s.dirs.includes(at)) s.dirs.push(at);
  }
  return $host_testing_logged(s, call, $ok(0));
}

// Nothing to flush, so this answers whether there is anything to have flushed.
// Not refused through an attenuated view: `sync` is not a write, and whatever
// the filesystem already holds is what gets flushed.
function $host_testing_fsSyncFile(h, p) {
  const s = $tslot(h);
  const call = ["syncFile", p, ""];
  const clean = p.replace(/\/+$/, "");
  if (clean === "" || clean === ".") return $host_testing_logged(s, call, $ok(0));
  return $host_testing_logged(
    s,
    call,
    p in s.files || s.dirs.includes(clean) ? $ok(0) : $err([0]),
  );
}

// A flat map holds no links, so `kind` is `.File` for a file, `.Directory` for
// a `makeDir` path, and never `.Symlink`. `modified` is the epoch: a hermetic
// double has no clock behind it.
function $host_testing_fsMetadata(h, p) {
  const s = $tslot(h);
  const call = ["metadata", p, ""];
  const clean = p.replace(/\/+$/, "");
  if (p in s.files) {
    const size = BigInt(s.files[p].length);
    return $host_testing_logged(s, call, $ok([0, size, [0n]]));
  }
  if (clean === "" || clean === "." || s.dirs.includes(clean)) {
    return $host_testing_logged(s, call, $ok([1, 0n, [0n]]));
  }
  return $host_testing_logged(s, call, $err([0]));
}

// The window the file holds, clamped at both ends: past the end is empty, and
// a short file gives back what it has.
function $host_testing_fsReadRange(h, p, from, count) {
  const s = $tslot(h);
  const call = ["readRange", p, ""];
  const start = Number(from);
  const want = Number(count);
  if (start < 0 || want < 0) {
    return $host_testing_logged(s, call, $err([6, "a negative offset or count"]));
  }
  if (!(p in s.files)) return $host_testing_logged(s, call, $err([0]));
  return $host_testing_logged(s, call, $ok(s.files[p].slice(start, start + want)));
}

// The path back as it was given: a flat map has no links and no `..` to
// resolve. A path naming nothing is still `.NotFound`.
function $host_testing_fsRealPath(h, p) {
  const s = $tslot(h);
  const call = ["realPath", p, ""];
  const clean = p.replace(/\/+$/, "");
  const there = p in s.files || s.dirs.includes(clean) || clean === "" || clean === ".";
  return $host_testing_logged(s, call, there ? $ok(p) : $err([0]));
}

function $host_testing_fsCopyFile(h, from, to) {
  const s = $tslot(h);
  const call = ["copyFile", from, to];
  if (s.ro) return $host_testing_logged(s, call, $err([2]));
  const f = s.files;
  if (!(from in f)) return $host_testing_logged(s, call, $err([0]));
  f[to] = f[from].slice();
  return $host_testing_logged(s, call, $ok(0));
}

// --- core/platforms/testing/state ---------------------------------------------
//
// One value per handle, as `cli/runtime/testing.rs` keeps it. A value going in
// is `$share`d, because the table holding it is a second reference. One thread,
// and `update` never awaits, so `take` and `put` need no lock: the `out` mark is
// only for a state used inside its own update.
const $states = [];

function $platforms_testing_state_kept(handle) {
  const s = $states[Number(handle)];
  if (s === undefined) $abort("this state does not exist");
  if (s.out) $abort("a state was used inside its own update");
  return s;
}

function $platforms_testing_state_stateNew(value) {
  $states.push({ value: $share(value), out: false });
  return BigInt($states.length - 1);
}

function $platforms_testing_state_stateRead(handle) {
  return $share($platforms_testing_state_kept(handle).value);
}

function $platforms_testing_state_stateTake(handle) {
  const s = $platforms_testing_state_kept(handle);
  s.out = true;
  const value = s.value;
  s.value = undefined;
  return value;
}

function $platforms_testing_state_statePut(handle, value) {
  const s = $states[Number(handle)];
  s.value = $share(value);
  s.out = false;
  return 0;
}

// Millis in and milliseconds out are both `I64`, so this one counts in `BigInt`.
function $host_testing_clock() {
  return $handle({ now: 0n });
}

function $host_testing_TestClock_at(self, ms) {
  return $handle({ now: ms });
}

function $host_testing_TestClock_nowMilliseconds(self) {
  return $slot(self).now;
}

// Moves the clock without sleeping, which is the whole point of a test clock.
function $host_testing_TestClock_sleepMilliseconds(self, ms) {
  $slot(self).now += ms;
  return 0;
}

// One reading, two clocks: the monotonic side is the millisecond side in
// nanoseconds, so `sleepMilliseconds` moves both together and a test can assert an
// elapsed measurement without waiting for one.
function $host_testing_TestClock_monotonicNanoseconds(self) {
  return $slot(self).now * 1000000n;
}

// The same xorshift32 steps as `cli/runtime/testing.rs`'s `next`, so a seeded
// sequence is the *same* sequence on both backends and not merely a
// reproducible one on each.
function $nextRand(s) {
  // xorshift32, which is enough for a test fixture and is exactly
  // reproducible across engines.
  let x = s.s;
  x = (x ^ (x << 13)) >>> 0;
  x = (x ^ (x >>> 17)) >>> 0;
  x = (x ^ (x << 5)) >>> 0;
  s.s = x;
  return x;
}

function $host_testing_rand() {
  return $handle({ s: 1 });
}

function $host_testing_TestRandom_seed(self, n) {
  return $handle({ s: Number(BigInt.asUintN(32, n)) || 1 });
}

function $host_testing_TestRandom_nextInt(self, lo, hi) {
  if (hi <= lo) $abort("random range is empty");
  return lo + (BigInt($nextRand($slot(self))) % (hi - lo));
}

function $host_testing_TestRandom_nextFloat(self) {
  return $nextRand($slot(self)) / 4294967296;
}

// The seeded `Entropy`, on `TestRandom`'s own generator and at its own seeds, so
// `entropy().seed(7)` and `rand().seed(7)` draw the same sequence — one octet
// per step, which is the low byte `nextInt(0, 256)` would have taken. A sealed
// value written into an assertion therefore holds on both backends.
function $host_testing_entropy() {
  return $handle({ s: 1 });
}

function $host_testing_TestEntropy_seed(self, n) {
  return $handle({ s: Number(BigInt.asUintN(32, n)) || 1 });
}

function $host_testing_TestEntropy_bytes(self, count) {
  const n = Number(count);
  if (n < 0) $abort("entropy count is negative");
  const slot = $slot(self);
  const out = [];
  for (let i = 0; i < n; i += 1) out.push($nextRand(slot) & 255);
  return out;
}

function $host_testing_env() {
  return $handle({ vars: {}, args: [] });
}

// Each builder keeps the other half, so the two compose in either order. The
// last binding of a name wins, because each assignment overwrites the one
// before it.
function $host_testing_TestEnvironment_variables(self, vars) {
  const v = {};
  for (const e of vars) v[e[0]] = e[1];
  return $handle({ vars: v, args: $slot(self).args.slice() });
}

function $host_testing_TestEnvironment_withArguments(self, args) {
  return $handle({ vars: Object.assign({}, $slot(self).vars), args: args.slice() });
}

function $host_testing_TestEnvironment_variable(self, name) {
  const v = $slot(self).vars;
  return name in v ? $some(v[name]) : undefined;
}

function $host_testing_TestEnvironment_arguments(self) {
  return $slot(self).args.slice();
}

// `/` and `test`, whatever the machine running the suite is. A double that
// answered the runner's own directory or platform would give one test two
// answers on two machines.
function $host_testing_TestEnvironment_currentDirectory(self) {
  return "/";
}

// Sorted, unlike the real thing: a hermetic double owes a test one order.
function $host_testing_TestEnvironment_allVariables(self) {
  const v = $slot(self).vars;
  return Object.keys(v)
    .sort()
    .map(function (k) {
      return [k, v[k]];
    });
}

function $host_testing_TestEnvironment_operatingSystemName(self) {
  return "test";
}

// --- Spawn --------------------------------------------------------------------
//
// The log, and nothing else: the scripted answer stays in the program, because
// `IoError.Other` carries a `Str` and §2.1 cannot hand one back across a row.

function $host_testing_newSpawn() {
  return $tmint({ calls: [] });
}

// The plan is the program, the working directory and then the arguments, and
// the split happens here for the reason `host_testing.buri` gives: a Buri body
// would need an `Allocator` and an effect method takes only `self`.
function $host_testing_recordSpawn(h, plan) {
  $tslot(h).calls.push([plan.length > 0 ? plan[0] : "", plan.slice(2)]);
  return 0;
}

function $host_testing_spawnCalls(h) {
  return $tslot(h).calls.map(function (c) {
    return [c[0], c[1].slice()];
  });
}

// `proc()` has no function here and no slot: `TestProcess` records nothing,
// because nothing can read it back. `proc()` is `TestProcess(0)` and `exitWith`
// is an empty body, both written in `host_testing.buri` — the same shape
// `TestNetwork` has, reached for the plainer reason.

// --- core/testing/assert ------------------------------------------------------------
//
// A failure ends that test and no other, the way `crash` ends a program.

function $fail(message, actual, expected) {
  const e = new Error(message);
  e.$assert = { message, actual, expected };
  throw e;
}

function $testing_assert_report(passed, kind, actual, expected, d) {
  if (!passed) {
    $fail("assert." + kind + " failed", $show(actual, d), $show(expected, d));
  }
  return 0;
}

function $testing_assert_failWith(m) {
  $fail(m, null, null);
}

function $testing_assert_failExpected(kind, got, d) {
  $fail("assert." + kind + " failed", $show(got, d), "." + kind[0].toUpperCase() + kind.slice(1));
}

// --- Checked and saturating arithmetic -----------------------------------------

function $checkedIn(v, lo, hi) {
  if (!Number.isFinite(v) || v < lo || v > hi) return undefined;
  return $some(v);
}

// The same at a `BigInt` width, where the value is finite by construction and
// the bounds are the type's own — so `.Some` is the answer whenever the answer
// fits, which is what a native backend says too.
function $checkedInBig(v, lo, hi) {
  if (v < lo || v > hi) return undefined;
  return $some(v);
}

function $sat(v, lo, hi) {
  return v < lo ? lo : v > hi ? hi : v;
}

// `x` to the `e`th, inside `[lo, hi]`, or `undefined`. The one member of
// `Checked` that is a loop rather than an expression.
//
// Exponentiation by squaring, with the range tested after every multiplication,
// so a `.Some` is the value the answer really is. A negative exponent is
// `undefined`: the answer is a fraction, and an integer type holds none.
//
// The squaring is skipped on the last round, so `x.checkedPower(1)` never asks
// whether `x * x` fits. Where a squaring *does* leave the range, the answer has
// left it too — the exponent still has a bit above the one just folded in, and
// the base is at least two in magnitude.
function $checkedPow(x, e, lo, hi, big) {
  // The exponent is an `Int`, which is a BigInt here, and its parity has to be
  // read exactly: past 2^53 a double no longer knows whether it is even.
  let n = $toBig(e);
  if (n < 0n) return undefined;
  const inside = (v) => v >= lo && v <= hi;
  let acc = big ? 1n : 1;
  let base = x;
  while (n > 0n) {
    if (n % 2n === 1n) {
      acc = acc * base;
      if (!inside(acc)) return undefined;
    }
    n = n / 2n;
    if (n === 0n) break;
    // A squaring that leaves the range takes the answer with it: there is a
    // bit of the exponent left, and a base of 0, 1 or -1 never grows.
    base = base * base;
    if (!inside(base)) return undefined;
  }
  return big ? $checkedInBig(acc, lo, hi) : $checkedIn(acc, lo, hi);
}

// Turning a Template into a Str is the point at which interpolation
// allocates; constructing the Template itself does not.
function $str_format(c, t) {
  return t;
}

// --- Lazily loaded chunks ------------------------------------------------------

// One promise per chunk, so a second `load` of the same one is a hit rather
// than a second request.
let $lazyChunks = [];

// Chunk `n` of this artifact, which sits beside it as `<artifact>.<n>.mjs`.
// The name is derived from `import.meta.url` rather than written into the
// artifact, so nothing here records where the build ran or what the output
// directory was called.
//
// `env` is a thunk answering everything the chunk borrows from this module.
// The chunk is handed them rather than importing them back, because this
// module is still evaluating — a chunk is fetched from inside a call this
// module's own top-level `await` is waiting on, and a cycle there is a program
// that never finishes starting.
function $lazy(n, env) {
  if (!$lazyChunks[n]) {
    const here = import.meta.url;
    $lazyChunks[n] = import(here.slice(0, here.length - 4) + "." + n + ".mjs").then(function (m) {
      m.$bind(env());
      return m;
    });
  }
  return $lazyChunks[n];
}

// --- A website ----------------------------------------------------------------
//
// `ui/web`: the tree rendered to HTML on a worker, and what the page does with
// the document that arrives. There is no second renderer here — `$tree_render`
// is the one `mount` uses, pointed at the substitute document — so what a
// worker writes is what the page would have built.

function $ui_web_stylesheet() {
  return $ui_sheet;
}

function $ui_web_render(root) {
  // What a render registers is per-request bookkeeping: the tree is thrown away
  // with the answer, and a worker's module state outlives the request. So the
  // cells this made go with it.
  const before = $ui.nodes.length;
  // The document this request answers with numbers its radio groups from zero,
  // the way a fresh page does — so the client, resuming into a fresh page,
  // names them the same.
  $tree_radio_groups = 0;
  const host = $dom_make(0, "root");
  // No context. A handler is never called here — what this answers is text —
  // and every constructor that receives one is unbounded in it, so nothing on
  // this path can do anything with a context at all.
  $tree_render(null, root, host, null);
  let out = "";
  for (const child of host.children) out += $dom_markup(child);
  $ui.nodes.length = before;
  return out;
}

// The script `shell` wrote, or `null` where there is no document — which is
// every JavaScript host that is not a browser — and where the server sent none.
function $ui_web_holder() {
  if (typeof document === "undefined" || !document.getElementById) return null;
  const holder = document.getElementById("buri-state");
  return holder === null || holder === undefined ? null : holder;
}

// The state the server embedded, as `undefined` — `None` — where there is none.
function $ui_web_embedded() {
  const holder = $ui_web_holder();
  return holder === null ? undefined : holder.textContent;
}

// The address `shell` rendered this document for, or `null` where the markup
// came from something that is not `shell` and has none to compare against.
function $ui_web_sent_path() {
  const holder = $ui_web_holder();
  if (holder === null || !holder.getAttribute) return null;
  const at = holder.getAttribute("data-path");
  return at === undefined ? null : at;
}

function $ui_web_resume(ctx, root) {
  const body = $dom_body();
  if (!body) return $err("there is nowhere to resume: this platform has no document");
  // Where before what. Two routes that render the same shape adopt each other's
  // markup happily, and the reader is then looking at one page with another
  // page's handlers on it — which the walk below cannot see, because as far as
  // it is concerned everything matched. A query string and a fragment are not
  // part of a path: `location.pathname` and `Request.path` both leave them out,
  // so an address that differs only there is the same page.
  const rendered = $ui_web_sent_path();
  if (rendered !== null && rendered !== $ui_web_path()) {
    return $err(
      "this page is not the page the server sent: it was rendered at " +
        rendered +
        " and the address is " +
        $ui_web_path(),
    );
  }
  // The tree is rendered against the document the server sent: every element
  // and every run of text is the one already there, and what the walk adds is
  // the listeners and the computations. Nothing is created and nothing is
  // removed, so the reader keeps looking at the markup that arrived.
  $adopt.ops = { claim: $adopt_claim, at: $adopt_at, split: $adopt_split };
  $adopt.at = new Map();
  try {
    $tree_render(ctx, root, body, null);
    const over = $adopt_leftovers(body);
    if (over !== null) return $err(over);
  } catch (e) {
    // A tree that does not match the markup, and nothing else: anything this
    // did not throw itself belongs to the program.
    if (e === null || typeof e !== "object" || e.$resume === undefined) throw e;
    return $err(e.$resume);
  } finally {
    $adopt.ops = null;
    $adopt.at = new Map();
  }
  return $ok(0);
}

function $ui_web_state(ctx) {
  // Read off the document rather than remembered from a resume, because the
  // state is what a page builds its tree out of and the tree is what it
  // resumes with.
  return $ui_web_embedded();
}

// The tab's name. `ui/web`'s `title` runs this inside a watch, so a title built
// out of the route is rewritten every time the reader navigates. Nowhere to
// write it is not a failure: a JavaScript host that is not a browser has no
// document, and a page's name is not something a program reads back.
function $ui_web_setTitle(text) {
  if (typeof document === "undefined" || document === null) return 0;
  document.title = text;
  return 0;
}

// The address the document is at, for `resume`'s check against the address
// the server rendered. `web`'s `main.mjs` reads the same thing for
// `HostLocation`.
function $ui_web_path() {
  if (typeof location === "undefined" || location === null) return "/";
  return location.pathname || "/";
}
