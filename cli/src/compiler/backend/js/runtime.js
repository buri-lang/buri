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
// answer — `U32.wrappingMul(0xffffffff, 0xffffffff)` is 1, its exact product
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
// A set rather than a property so that marking a value writes nothing on it:
// a host array handed to `$share` comes back exactly as it went in, and an
// aggregate this backend allocated does not change shape when it is marked.

// A fresh list this runtime allocated. Called on the way out of everything in
// `core/list` that builds one.
function $own(a) {
  a.$u = true;
  return a;
}

const $shared = new WeakSet();

// A second reference to a value has come into existence. Sticky: nothing ever
// puts a value back, because the cost of an over-set mark is one copy and the
// cost of a cleared one is an aliasing bug.
function $share(v) {
  if (v !== null && typeof v === "object") {
    if (v.$u === true) v.$u = false;
    else if (v.$u === undefined) $shared.add(v);
  }
  return v;
}

// A field read out of a parent this expression is the last use of: a second
// reference only if the parent was one. Perceus's drop specialisation with the
// answer left until run time, because a garbage collector hides the count that
// would have decided it statically.
function $fromShared(p, v) {
  if (p !== null && typeof p === "object" && (p.$u === false || $shared.has(p))) {
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

function $list_len(xs) {
  return BigInt(xs.length);
}

function $list_get(xs, i) {
  const n = Number(i);
  return n >= 0 && n < xs.length ? $some(xs[n]) : undefined;
}

// A higher-order runtime function marks what it hands to a callback: the
// element belongs to `xs`, and the seed still belongs to whoever passed it.
// Everything after the first iteration is the callback's own fresh result, so
// an accumulator threaded through a fold is marked once and never again.
function $list_fold(xs, f, acc) {
  acc = $share(acc);
  for (let i = 0; i < xs.length; i++) acc = f(acc, $share(xs[i]));
  return acc;
}

function $list_foldCtx(xs, c, f, acc) {
  acc = $share(acc);
  for (let i = 0; i < xs.length; i++) acc = f(c, acc, $share(xs[i]));
  return acc;
}

// Stops at the first .Err, which is how a fallible fold is written without an
// early exit.
function $list_foldResult(xs, f, acc) {
  let cur = [0, $share(acc)];
  for (let i = 0; i < xs.length; i++) {
    cur = f(cur[1], $share(xs[i]));
    if (cur[0] !== 0) return cur;
  }
  return cur;
}

function $list_foldResultCtx(xs, c, f, acc) {
  let cur = [0, $share(acc)];
  for (let i = 0; i < xs.length; i++) {
    cur = f(c, cur[1], $share(xs[i]));
    if (cur[0] !== 0) return cur;
  }
  return cur;
}

function $list_any(xs, p) {
  for (let i = 0; i < xs.length; i++) if (p($share(xs[i]))) return true;
  return false;
}

function $list_all(xs, p) {
  for (let i = 0; i < xs.length; i++) if (!p($share(xs[i]))) return false;
  return true;
}

function $list_find(xs, p) {
  for (let i = 0; i < xs.length; i++) if (p($share(xs[i]))) return $some(xs[i]);
  return undefined;
}

function $list_findIndex(xs, p) {
  for (let i = 0; i < xs.length; i++) if (p($share(xs[i]))) return $some(BigInt(i));
  return undefined;
}

function $list_count(xs, p) {
  let n = 0;
  for (let i = 0; i < xs.length; i++) if (p($share(xs[i]))) n++;
  return BigInt(n);
}

function $list_map(xs, c, f) {
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = f($share(xs[i]));
  return $own(out);
}

function $list_mapCtx(xs, c, f) {
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = f(c, $share(xs[i]));
  return $own(out);
}

// `list.mapCtxStep` is `mapCtx` with a runtime-driven step on the native
// backends, and here it is the same loop as `$list_mapCtx` — deliberately.
// JavaScript is the reference implementation the two natives are compared
// against (`cli/tests/native/agreement.rs`), and a reference that shared the
// mechanism under test would prove nothing about it. This is the same argument
// `middle/fuse.rs` makes for running the fusion pass on the native branch only.
function $list_mapCtxStep(xs, c, f) {
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = f(c, $share(xs[i]));
  return $own(out);
}

function $list_filter(xs, c, p) {
  const out = [];
  for (let i = 0; i < xs.length; i++) if (p($share(xs[i]))) out.push(xs[i]);
  return $own(out);
}

function $list_filterCtx(xs, c, p) {
  const out = [];
  for (let i = 0; i < xs.length; i++) if (p(c, $share(xs[i]))) out.push(xs[i]);
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
    for (let i = 0; i < n; i++) xs.push(ys[i]);
    return xs;
  }
  return $own(xs.concat(ys));
}

function $list_push(xs, c, x) {
  if (xs.$u === true) {
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
function $list_sortBy(xs, c, order) {
  return $own(
    xs
      .map((v, i) => [$share(v), i])
      .sort((a, b) => {
        const o = order(a[0], b[0]);
        return o === 1 ? a[1] - b[1] : o === 0 ? -1 : 1;
      })
      .map((p) => p[0]),
  );
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
function $list_range(c, a, b) {
  const out = [];
  for (let i = a; i < b; i++) out.push(i);
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
// That is worth testing for, because `$chars` allocates an array as long as the
// string: without this, `len` was O(n) *with an allocation*, and the ordinary
// `for i in 0..s.len() { s.charAt(i) }` scan was O(n²) with n allocations. The
// scan is still quadratic here — the fix for that is to iterate `chars()`
// rather than to index — but the constant is about a hundred times smaller.
//
// A lone unpaired surrogate takes the slow path, where `Array.from` yields it
// as one element, which is the same answer as before.
const $surrogate = /[\uD800-\uDFFF]/;

function $wide(s) {
  return $surrogate.test(s);
}

function $str_len(s) {
  return BigInt($wide(s) ? $chars(s).length : s.length);
}

function $str_charAt(s, i) {
  const n = Number(i);
  if (!$wide(s)) return n >= 0 && n < s.length ? $some(s[n]) : undefined;
  const cs = $chars(s);
  return n >= 0 && n < cs.length ? $some(cs[n]) : undefined;
}

function $str_slice(s, a, b) {
  const lo = Math.max(0, Number(a));
  const hi = Math.max(0, Number(b));
  // `String.prototype.slice` clamps past the end and answers "" when the end
  // is at or before the start, which is what the array path does too.
  if (!$wide(s)) return s.slice(lo, hi);
  return $chars(s).slice(lo, hi).join("");
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
  // what has to be counted is the prefix — and only when it holds a surrogate.
  const prefix = s.slice(0, i);
  return $some(BigInt($wide(prefix) ? $chars(prefix).length : i));
}

// Two slices, or .None when the separator does not occur. Pure, because
// neither half is a copy.
function $str_splitOnce(s, sep) {
  const i = s.indexOf(sep);
  return i < 0 ? undefined : $some([s.slice(0, i), s.slice(i + sep.length)]);
}

function $str_compare(a, b) {
  return a < b ? 0 : a > b ? 2 : 1;
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
  const n = Number(w) - Number($str_len(s));
  return n > 0 ? fill.repeat(n) + s : s;
}

function $str_padEnd(s, c, w, fill) {
  const n = Number(w) - Number($str_len(s));
  return n > 0 ? s + fill.repeat(n) : s;
}

// --- core/char ------------------------------------------------------------------

function $char_isDigit(c) {
  return c >= "0" && c <= "9";
}

function $char_isAlpha(c) {
  return /^\p{L}$/u.test(c);
}

function $char_isSpace(c) {
  return /^\s$/u.test(c);
}

function $char_isUpper(c) {
  return c !== c.toLowerCase() && c === c.toUpperCase();
}

function $char_isLower(c) {
  return c !== c.toUpperCase() && c === c.toLowerCase();
}

function $char_toLower(c) {
  return c.toLowerCase();
}

function $char_toUpper(c) {
  return c.toUpperCase();
}

function $char_toU32(c) {
  return c.codePointAt(0);
}

function $char_toDigit(c, radix) {
  const n = parseInt(c, Number(radix));
  return Number.isNaN(n) ? undefined : $some(BigInt(n));
}

// --- core/math --------------------------------------------------------------------

const $math_sqrt = Math.sqrt;
const $math_cbrt = Math.cbrt;
const $math_pow = Math.pow;
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
const $math_ceil = Math.ceil;
const $math_round = Math.round;
const $math_trunc = Math.trunc;
const $math_absFloat = Math.abs;
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
  return BigInt(k);
}

function $big(x) {
  return BigInt(Math.trunc(x));
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

function $bits_shl(x, n) {
  return BigInt.asIntN(64, x << $shiftCount(n, 64));
}

function $bits_shr(x, n) {
  // Logical: reinterpret as unsigned, shift, then narrow back, so a shift by
  // zero is the identity rather than the unsigned reinterpretation.
  return BigInt.asIntN(64, BigInt.asUintN(64, x) >> $shiftCount(n, 64));
}

function $bits_sar(x, n) {
  return x >> $shiftCount(n, 64);
}

function $bits_popCount(x) {
  let v = BigInt.asUintN(64, x);
  let n = 0n;
  while (v) {
    n += v & 1n;
    v >>= 1n;
  }
  return n;
}

function $bits_leadingZeros(x) {
  const v = BigInt.asUintN(64, x);
  let n = 0n;
  for (let i = 63n; i >= 0n; i--) {
    if ((v >> i) & 1n) break;
    n++;
  }
  return n;
}

function $bits_trailingZeros(x) {
  const v = BigInt.asUintN(64, x);
  if (v === 0n) return 64n;
  let n = 0n;
  while (!((v >> n) & 1n)) n++;
  return n;
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
function $bits_shlU8(x, n) {
  return Number(BigInt.asUintN(8, $big(x) << $shiftCount(n, 8)));
}
function $bits_shrU8(x, n) {
  return Number($big(x) >> $shiftCount(n, 8));
}
function $bits_shlU32(x, n) {
  return Number(BigInt.asUintN(32, $big(x) << $shiftCount(n, 32)));
}
function $bits_shrU32(x, n) {
  return Number($big(x) >> $shiftCount(n, 32));
}
function $bits_shlU64(x, n) {
  return BigInt.asUintN(64, x << $shiftCount(n, 64));
}
function $bits_shrU64(x, n) {
  return x >> $shiftCount(n, 64);
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
const $f64buf = new DataView(new ArrayBuffer(8));

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
  return $f64buf.getFloat64(0, true);
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
  return $f64buf.getFloat32(0, true);
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

function $write(fd, s) {
  if (typeof Bun !== "undefined") {
    (fd === 1 ? Bun.stdout : Bun.stderr).write(s);
  } else if (typeof process !== "undefined") {
    (fd === 1 ? process.stdout : process.stderr).write(s);
  } else {
    (fd === 1 ? console.log : console.error)(s);
  }
}

// The platform's allocator: unbounded, and it counts nothing. `core/alloc`'s
// three count; this one is what a program gets when it asks for none of them,
// and it is a `Region` of the bytes requested (`effect.buri`'s cost model, last
// row).
function $host_HostAlloc_allocate(self, n) {
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
// to report the failure with (SPEC 6.10, MEMORY.md §7.2). The message is
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
// `Scoped<C>` forwards every effect but `Alloc`, and its `Alloc` charges the
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
  const buf = typeof Buffer !== "undefined" ? Buffer.from(bytes) : Uint8Array.from(bytes);
  if (typeof Bun !== "undefined") {
    // Bun's stdout writer is async; `writeSync` on the file descriptor is not,
    // and a protocol that answers a request has to have answered before it
    // reads the next one.
    $fs().writeSync(fd, buf);
  } else if (typeof process !== "undefined") {
    $fs().writeSync(fd, buf);
  } else {
    throw new Error("no way to write bytes on this platform");
  }
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
// one of the two modules below. A program whose `main` binds neither `Fs` nor
// `Stdout.writeBytes` never gets one.
//
// The synchronous half survives for exactly one caller: `$writeRaw`, which
// answers `Stdout.writeBytes` and does not wait, because a protocol that
// answers a request has to have answered before it reads the next one.
function $fs() {
  if (typeof $require === "function") return $require("fs");
  if (typeof require === "function") return require("fs");
  $abort("this platform grants no filesystem");
}

// The filesystem every `Fs` method reaches: `node:fs/promises`, so that a read
// is a wait rather than a stall. **Node and Bun only.** A browser has no such
// module and no browser platform grants `Fs` (`standard_library`'s grant
// table), so the abort below is unreachable from a `WEB` artifact and is what
// a mis-grant would say out loud rather than silently.
function $fsp() {
  if (typeof $require === "function") return $require("node:fs/promises");
  if (typeof require === "function") return require("node:fs/promises");
  $abort("this platform grants no filesystem");
}

async function $host_HostFs_readFile(self, p) {
  try {
    return $ok(await $fsp().readFile(p, "utf8"));
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFs_writeFile(self, p, b) {
  try {
    await $fsp().writeFile(p, b);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `access` rather than a `stat`: the question is whether the name resolves,
// and the answer to every failure is the same `false`.
async function $host_HostFs_fileExists(self, p) {
  try {
    await $fsp().access(p);
    return true;
  } catch {
    return false;
  }
}

async function $host_HostFs_readDir(self, p) {
  try {
    return $ok(await $fsp().readdir(p));
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFs_readFileBytes(self, p) {
  try {
    return $ok(Array.from(await $fsp().readFile(p)));
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFs_writeFileBytes(self, p, b) {
  try {
    await $fsp().writeFile(p, Uint8Array.from(b));
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `"a"` is `O_APPEND | O_CREAT`, so the position is taken and the octets
// written as one operation and the file appears when it was absent.
async function $host_HostFs_appendFile(self, p, b) {
  try {
    await $fsp().appendFile(p, Uint8Array.from(b));
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFs_renameFile(self, from, to) {
  try {
    await $fsp().rename(from, to);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

async function $host_HostFs_removeFile(self, p) {
  try {
    await $fsp().unlink(p);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

// `recursive` is what makes the parents and the already-there case both work;
// a path naming a file is still `EEXIST`, which is `.AlreadyExists`.
async function $host_HostFs_makeDir(self, p) {
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
async function $host_HostFs_syncFile(self, p) {
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
// `Request` is `[method, url, headers, body]` and `Response` is
// `[status, headers, body]`: a struct is its fields in order, a `Header` is
// `[name, value]`, a payloadless enum is its variant index, and a `[U8]` is an
// ordinary array of numbers.
async function $host_HostNet_fetch(self, request) {
  const method = $HTTP_METHOD[Number(request[0])] || "GET";
  const url = request[1];
  const headers = request[2];
  const body = request[3];
  try {
    // A `GET` or a `HEAD` may carry no body at all, which `fetch` enforces
    // rather than ignores, so an empty one is left off entirely.
    const sends = body.length !== 0 && method !== "GET" && method !== "HEAD";
    const init = { method, headers: Array.from(headers, (h) => [h[0], h[1]]) };
    if (sends) init.body = new Uint8Array(body);
    const r = await fetch(url, init);
    // Every byte back unchanged, which is what the `overrideMimeType` trick was
    // standing in for: a response that is not text used to arrive as
    // replacement characters, and a `[U8]` body exists to avoid exactly that.
    const octets = new Uint8Array(await r.arrayBuffer());
    return $ok([BigInt(r.status), $httpResponseHeaders(r), Array.from(octets)]);
  } catch (e) {
    // `.Transport(Str)`, the fourth variant of `NetError` — a request that did
    // not reach an answer, whatever stopped it.
    return $err([3, String((e && e.message) || e)]);
  }
}

function $host_HostClock_nowMillis(self) {
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
async function $host_HostClock_sleepMillis(self, ms) {
  const n = Number(ms);
  await new Promise((wake) => setTimeout(wake, n > 0 ? n : 0));
  return 0;
}

function $host_HostRand_nextInt(self, lo, hi) {
  if (hi <= lo) $abort("random range is empty");
  const span = Number(hi - lo);
  return lo + BigInt(Math.floor(Math.random() * span));
}

function $host_HostRand_nextFloat(self) {
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

function $host_HostEnv_variable(self, name) {
  const env = typeof process !== "undefined" ? process.env : {};
  const v = env[name];
  return v === undefined ? undefined : $some(v);
}

function $host_HostEnv_args(self) {
  if (typeof Bun !== "undefined") return Bun.argv.slice(2);
  if (typeof process !== "undefined") return process.argv.slice(2);
  return [];
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
  const out = await Promise.all(xs.map((x, i) => f(ctx, BigInt(i), $share(x))));
  return $own(out);
}

function $host_HostProc_exitWith(self, code) {
  $host.flush();
  if (typeof process !== "undefined") process.exit(Number(code));
  return 0;
}

// --- core/actor -------------------------------------------------------------
//
// The mailbox, the state and the reply slots. Nine functions, and between them
// they are the same table `cli/runtime/rt.rs` holds — one queue per actor, one
// state beside it, one baton that says who may step it, and a slot per `ask`.
//
// Everything crosses as a one-element list, which is what makes the native
// half possible at all (a `[T]` is two words whatever `T` is) and costs nothing
// here, where a list is an array. `$share` on the way in, because a value the
// runtime holds onto is a second reference by definition and a list this
// backend allocated must stop being writable in place the moment there are two
// names for it.
//
// The two waits are `async` because `middle/rc.rs`'s `suspends` says they are,
// and the two lists must agree: `mailboxPush` waits for room, `mailboxClose`
// waits for the step in flight to give the state back. Nothing else here waits.
const $actors = [];

function $actor_mailboxOpen(c, state, bound) {
  const room = Number(bound) > 0 ? Number(bound) : 1;
  $actors.push({
    queue: [],
    state: $share(state),
    closed: false,
    bound: room,
    room: [],
    baton: true,
    free: [],
  });
  return BigInt($actors.length - 1);
}

function $actorAt(handle) {
  const i = Number(handle);
  return i >= 0 && i < $actors.length ? $actors[i] : undefined;
}

async function $actor_mailboxPush(c, handle, message) {
  const a = $actorAt(handle);
  if (a === undefined || a.closed) return undefined;
  while (a.queue.length >= a.bound) {
    await new Promise((resolve) => a.room.push(resolve));
    if (a.closed) return undefined;
  }
  a.queue.push($share(message));
  return $some(BigInt(a.queue.length));
}

function $actor_mailboxPop(c, handle) {
  const a = $actorAt(handle);
  if (a === undefined || a.queue.length === 0) return undefined;
  const held = a.queue.shift();
  const wake = a.room.shift();
  if (wake !== undefined) wake();
  return $some(held);
}

async function $actor_mailboxClose(c, handle) {
  const a = $actorAt(handle);
  if (a === undefined || a.closed) return undefined;
  a.closed = true;
  for (const wake of a.room.splice(0)) wake();
  while (!a.baton) {
    await new Promise((resolve) => a.free.push(resolve));
  }
  // Kept, never given back: the baton is what a `stateTake` needs, so holding
  // it forever is what makes a stopped actor unsteppable.
  a.baton = false;
  const held = a.state;
  a.state = undefined;
  return held === undefined ? undefined : $some(held);
}

function $actor_stateTake(c, handle) {
  const a = $actorAt(handle);
  if (a === undefined || a.closed || !a.baton || a.state === undefined) return undefined;
  a.baton = false;
  const held = a.state;
  a.state = undefined;
  return $some(held);
}

function $actor_statePut(c, handle, state) {
  const a = $actorAt(handle);
  if (a === undefined) return undefined;
  a.state = $share(state);
  a.baton = true;
  for (const wake of a.free.splice(0)) wake();
  return $some(BigInt(a.queue.length));
}

// The reply slots, reused behind a generation, exactly as the native table
// reuses them: a server answering a million `ask`s would otherwise grow a
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

function $actor_replyTake(c, handle) {
  const slot = $replyAt(handle);
  if (slot === undefined || slot.state !== 1) return undefined;
  const value = slot.value;
  slot.state = 2;
  slot.value = undefined;
  slot.generation = slot.generation + 1n;
  $replies.free.push(Number(handle & $REPLY_INDEX_MASK));
  return $some(value);
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
};

// A runaway is a program whose watchers write what they read. The limit is not
// a policy, it is the difference between a diagnosis and a hung tab.
const $UI_STEPS = 100000;

function $ui_cell(kind, value, compute) {
  const owner = $ui.current;
  $ui.nodes.push({
    kind,
    value,
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

function $ui_dispose(id) {
  const n = $ui.nodes[id];
  if (n === undefined || n.disposed) return;
  n.disposed = true;
  for (const c of n.children) $ui_dispose(c);
  n.children = [];
  $ui_unsubscribe(id, n);
  n.subs = [];
  n.compute = null;
}

function $ui_run(id) {
  const n = $ui_at(id);
  if (n.disposed) return;
  // Everything the previous run created belongs to the previous run.
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
    if (n.kind === 1) n.value = v;
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
  $ui.queue = [];
}

function $ui_write(cell, v) {
  const id = Number(cell);
  const n = $ui_at(id);
  // Identical is not a change. This is what makes "wrote the same value, so
  // nothing re-ran" a thing a test can assert.
  if (n.value === v) return 0;
  n.value = v;
  $ui_notify(n);
  if ($ui.depth === 0) $ui_drain();
  return 0;
}

// One update transaction: N writes cause one pass over the watchers rather
// than N.
//
// The transaction is the handler's *synchronous* run, and that is the whole of
// the decision now that a handler can wait. A page grants `Net`, so a press
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

function $host_HostWatch_read(self, id) {
  return $ui_read(id);
}

function $ui_effect_Scope_read(self, id) {
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
  };
}

// The three constructors take the parent they are destined for, because which
// document a node belongs to is decided by what it will hang off rather than
// by what the platform happens to have.
function $dom_element(parent, name) {
  return parent.$shim ? $dom_make(0, name) : document.createElement(name);
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

// The classes an element has, all of them at once. Replacing rather than
// adding is what makes re-applying a style list idempotent: a `When` that
// switched back has to lose the class it gained.
function $dom_classes(element, value) {
  if (element.$shim) {
    element.classes = value;
    return;
  }
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
// Only the substitute: these are what `ui/testing` is, and a test holds one of
// its trees. Markers are left out of the markup deliberately — they are the
// runtime's own bookkeeping, not something a reader sees, and pinning a test to
// them would pin it to how a region is anchored.

function $dom_escape(text, quotes) {
  let out = text.split("&").join("&amp;").split("<").join("&lt;").split(">").join("&gt;");
  if (quotes) out = out.split('"').join("&quot;");
  return out;
}

function $dom_markup(node) {
  if (node.kind === 2) return "";
  if (node.kind === 1) return $dom_escape(node.data, false);
  let out = "<" + node.name;
  for (const name of Object.keys(node.attributes)) {
    out += " " + name + '="' + $dom_escape(node.attributes[name], true) + '"';
  }
  if (node.classes !== "") out += ' class="' + $dom_escape(node.classes, true) + '"';
  if (node.value !== "") out += ' value="' + $dom_escape(node.value, true) + '"';
  if (node.checked) out += " checked";
  const styles = Object.keys(node.styles);
  if (styles.length > 0) {
    const parts = [];
    for (const property of styles) parts.push(property + ": " + node.styles[property]);
    out += ' style="' + $dom_escape(parts.join("; "), true) + '"';
  }
  let inner = "";
  for (const child of node.children) inner += $dom_markup(child);
  return inner === "" ? out + " />" : out + ">" + inner + "</" + node.name + ">";
}

// Every run of text, in order. Separate runs stay separate, because two runs
// are what a reader is shown as two things.
function $dom_runs(node, out) {
  if (node.kind === 1) {
    if (node.data !== "") out.push(node.data);
  } else if (node.kind === 0) {
    for (const child of node.children) $dom_runs(child, out);
  }
  return out;
}

// The text of one element, run together — what a reader would call the name of
// a button.
function $dom_label(node) {
  return $dom_runs(node, []).join("");
}

function $dom_elements(node, name, out) {
  if (node.kind === 0) {
    if (node.name === name) out.push(node);
    for (const child of node.children) $dom_elements(child, name, out);
  }
  return out;
}

// The first element of any of these kinds, in document order. A field is an
// `input` or a `textarea` depending on its kind, and the test that fills one
// should not have to know which.
function $dom_first(node, names) {
  if (node.kind === 0) {
    if (names.indexOf(node.name) >= 0) return node;
    for (const child of node.children) {
      const found = $dom_first(child, names);
      if (found !== null) return found;
    }
  }
  return null;
}

function $dom_fire(node, type) {
  const handler = node.listeners[type];
  if (handler !== undefined) handler({ preventDefault() {}, target: node });
}

// --- The tree ---------------------------------------------------------------
//
// `ui/node`'s vocabulary, lowered. A `Node` is the one-field struct that keeps
// the tree opaque, so it is `[kind]`; the kind inside is `[tag, ...payload]`
// and the tags are the order `ui/node` declares `NodeKind`'s variants in:
//
//   0 Nothing   1 Text     2 Heading  3 Stack   4 Region  5 Button  6 Link
//   7 Image     8 Field    9 Toggle  10 Form   11 When   12 Computed  13 Each
//
// A component runs once. What re-runs is what the last three tags stand for,
// and each re-runs the smallest thing it can: a `Prop` on a leaf changes one
// run of text or one attribute; `When` and `Computed` rebuild one subtree; and
// `Each` moves the rows that are still there and builds only the rows that are
// not.

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
// keeps the arrays the same shape.
const $TREE_FIELD_KINDS = ["text", "text", "password", "email", "number", "search"];

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

// Logical edges, so a right-to-left page is right by construction.
const $TREE_EDGES = ["block-start", "block-end", "inline-start", "inline-end"];

const $TREE_POSITIONS = ["relative", "sticky", "fixed"];

const $TREE_BORDER_STYLES = ["none", "solid", "dashed"];

const $TREE_TEXT_CASES = ["none", "uppercase", "lowercase", "capitalize"];

const $TREE_TEXT_LINES = ["none", "underline", "line-through"];

const $TREE_TEXT_WRAPS = ["wrap", "nowrap", "balance"];

const $TREE_FONTS = [
  "ui-sans-serif,system-ui,sans-serif",
  "ui-serif,Georgia,serif",
  "ui-monospace,SFMono-Regular,monospace",
];

// The stylesheet the compiler extracted, assigned by one statement the backend
// emits ahead of the program and empty in a program that styles nothing.
// Nothing here ever writes to it: every rule in it was written at compile time,
// which is what "nothing is generated at run time" means.
let $ui_sheet = "";

// The inline tier's lowering, reached through a hole rather than by name.
//
// `$tree_declare` below is the run-time lowering of all forty-five properties
// and is 3.5 KB of an artifact. `$tree_style_collect` is the only thing that
// needs it, and a call by name is a reference dead-code elimination cannot
// argue with — so every user interface carried the whole tier, including one
// whose styles are all static and are therefore all classes before the artifact
// is written. The backend assigns this when `Program::inline_styles` says some
// style in the program can reach the tier, and emits nothing when it cannot,
// which is the same mechanism `$ui_sheet` above uses.
let $tree_declare_hook = null;

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
  if (tag === 2) return length[1] + "%";
  if (tag === 3) return "auto";
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
  return "inherit";
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
      $tree_declare_hook(style, inline);
    } else {
      // The compiler said no style here could reach the inline tier, so it
      // left the lowering out of the artifact. Reaching this is that decision
      // being wrong, and saying so beats a `TypeError` about `null`.
      $abort("a style reached the inline tier in a program that was said to have none");
    }
  }
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
    if (value[0] === 2) {
      out.set("display", "grid");
      out.set("grid-template-columns", value[1].map($tree_track).join(" "));
    } else if (value[0] === 3) {
      // The children's half of `Layers` — every child in one cell — is a rule
      // about descendants, which an element's own style attribute cannot say.
      // A `Layers` that reached this tier stacks nothing.
      out.set("display", "grid");
    } else {
      out.set("display", "flex");
      out.set("flex-direction", value[0] === 1 ? "row" : "column");
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
    out.set("border-color", $tree_color(value));
  } else if (tag === 35) {
    out.set("border-style", $TREE_BORDER_STYLES[value]);
  } else if (tag === 36) {
    out.set("border-radius", $tree_length(value));
  } else if (tag === 37) {
    out.set("opacity", String(value));
  } else if (tag === 38) {
    out.set(
      "box-shadow",
      $tree_length(value[0]) +
        " " +
        $tree_length(value[1]) +
        " " +
        $tree_length(value[2]) +
        " " +
        $tree_length(value[3]) +
        " " +
        $tree_color(value[4]),
    );
  } else if (tag === 39) {
    out.set("font-family", $tree_font(value));
  } else if (tag === 40) {
    out.set("font-size", $tree_length(value));
  } else if (tag === 41) {
    out.set("font-weight", $TREE_WEIGHTS[value]);
  } else if (tag === 42) {
    out.set("font-style", value ? "italic" : "normal");
  } else if (tag === 43) {
    out.set("line-height", String(value));
  } else if (tag === 44) {
    out.set("letter-spacing", $tree_length(value));
  } else if (tag === 45) {
    out.set("text-align", $TREE_TEXT_ALIGNMENTS[value]);
  } else if (tag === 46) {
    out.set("text-transform", $TREE_TEXT_CASES[value]);
  } else if (tag === 47) {
    out.set("text-decoration-line", $TREE_TEXT_LINES[value]);
  } else if (tag === 48) {
    out.set("text-wrap", $TREE_TEXT_WRAPS[value]);
  } else if (tag === 49) {
    if (value > 0) {
      out.set("display", "-webkit-box");
      out.set("-webkit-box-orient", "vertical");
      out.set("-webkit-line-clamp", String(value));
      out.set("overflow", "hidden");
    } else {
      out.set("-webkit-line-clamp", "none");
      out.set("overflow", "visible");
    }
  } else {
    out.set("cursor", $TREE_CURSORS[value]);
  }
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

function $tree_element(parent, name, anchor) {
  const element = $dom_element(parent, name);
  $dom_insert(parent, element, anchor);
  return element;
}

function $tree_text(prop, parent, anchor) {
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
  const start = $dom_marker(parent);
  $dom_insert(parent, start, anchor);
  const end = $dom_marker(parent);
  $dom_insert(parent, end, anchor);
  $ui_run(
    $ui_cell(2, undefined, (scope) => {
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
  const start = $dom_marker(parent);
  $dom_insert(parent, start, anchor);
  const end = $dom_marker(parent);
  $dom_insert(parent, end, anchor);
  const rowOwner = $ui_under(owner, () => $ui_cell(3, undefined, null));
  $ui_under(rowOwner, () => {
    $tree_render(ctx, rowAt(ctx, [rowOwner], index), parent, end);
    return 0;
  });
  return { key, start, end, owner: rowOwner };
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
  const start = $dom_marker(parent);
  $dom_insert(parent, start, anchor);
  const end = $dom_marker(parent);
  $dom_insert(parent, end, anchor);
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
      return 0;
    }),
  );
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
    // Nothing: no element, no text, no place held. A `when` that answers this
    // is a `when` whose region is empty, and its own markers hold the place.
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
    $tree_text(node[2], $tree_element(parent, "h" + level, anchor), null);
    return;
  }
  if (tag === 3) {
    $tree_children(ctx, $tree_element(parent, "div", anchor), node[1], node[2]);
    return;
  }
  if (tag === 4) {
    const role = $TREE_ROLES[node[1]];
    const element = $tree_element(parent, role[0], anchor);
    for (let i = 1; i + 1 < role.length; i += 2) $dom_attribute(element, role[i], role[i + 1]);
    $tree_children(ctx, element, node[2], node[3]);
    return;
  }
  if (tag === 5) {
    const element = $tree_element(parent, "button", anchor);
    // Not a submit button: a form's submission is its own handler, and a
    // button that submits the form it happens to be inside is the surprise
    // this vocabulary exists to remove.
    $dom_attribute(element, "type", "button");
    $tree_text(node[1], element, null);
    const onPress = node[2];
    $dom_listen(element, "click", () =>
      // One transaction, so that a handler which writes three signals causes
      // one pass over the watchers rather than three.
      $ui_flush(() => onPress(ctx, [0])),
    );
    return;
  }
  if (tag === 6) {
    const element = $tree_element(parent, "a", anchor);
    $tree_bind(node[1], (dest) => $dom_attribute(element, "href", dest));
    $tree_children(ctx, element, [], node[2]);
    return;
  }
  if (tag === 7) {
    const element = $tree_element(parent, "img", anchor);
    $tree_bind(node[1], (source) => $dom_attribute(element, "src", source));
    $tree_bind(node[2], (alt) => $dom_attribute(element, "alt", alt));
    return;
  }
  if (tag === 8) {
    // The label wraps the field rather than pointing at it by an identifier,
    // which is what makes the pair correct with nothing generated: there is no
    // identifier to collide, and no way to render a field whose label is
    // attached to something else.
    const wrapper = $tree_element(parent, "label", anchor);
    $tree_text(node[1], $tree_element(wrapper, "span", null), null);
    const kind = node[2];
    const element = $tree_element(wrapper, kind === 1 ? "textarea" : "input", null);
    if (kind !== 1) $dom_attribute(element, "type", $TREE_FIELD_KINDS[kind]);
    const cell = node[3][0];
    $tree_bind([1, node[3]], (value) => {
      // Writing what is already there moves the caret in a real browser.
      if (element.value !== value) element.value = value;
    });
    $dom_listen(element, "input", () => $ui_flush(() => $ui_write(cell, element.value)));
    return;
  }
  if (tag === 9) {
    const wrapper = $tree_element(parent, "label", anchor);
    const element = $tree_element(wrapper, "input", null);
    $dom_attribute(element, "type", "checkbox");
    $tree_text(node[1], $tree_element(wrapper, "span", null), null);
    const cell = node[2][0];
    $tree_bind([1, node[2]], (value) => {
      element.checked = value;
    });
    $dom_listen(element, "change", () => $ui_flush(() => $ui_write(cell, element.checked)));
    return;
  }
  if (tag === 10) {
    const element = $tree_element(parent, "form", anchor);
    const onSubmit = node[1];
    $dom_listen(element, "submit", (event) => {
      // The page must not navigate: submission is the handler, and there is
      // nowhere for a browser to post to.
      if (event && event.preventDefault) event.preventDefault();
      $ui_flush(() => onSubmit(ctx, [0]));
    });
    $tree_children(ctx, element, node[2], node[3]);
    return;
  }
  if (tag === 11) {
    const cond = node[1];
    const then = node[2];
    const otherwise = node[3];
    $tree_dynamic(ctx, parent, anchor, (scope) => ($tree_value(cond, scope) ? then : otherwise));
    return;
  }
  if (tag === 12) {
    const build = node[1];
    $tree_dynamic(ctx, parent, anchor, (scope) => build(scope));
    return;
  }
  $tree_each(ctx, parent, anchor, node[1], node[2], node[3]);
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
// nothing to look at it, which is why `ui/testing` reads `$ui_sheet` instead.
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
// around it. Off a browser this is all there is, which is what `ui/testing`
// reads.
let $ui_theme_text = "";

// `namespace-name` — the custom property's name, without the leading dashes.
function $ui_theme_name(reference) {
  return reference[0] + "-" + reference[1];
}

// Which `Values` themes currently apply, in the order they were passed: a
// switch is followed to whichever branch its condition picks, and what comes
// back is a list of binding lists. Deciding this in one place is what keeps the
// map used for resolution and the blocks that are written from ever disagreeing
// about which side of a switch the page is on.
function $ui_theme_applied(themes, scope, out) {
  for (const wrapper of themes) {
    const theme = wrapper[0];
    if (theme[0] === 0) {
      out.push(theme[1]);
    } else {
      $ui_theme_applied($tree_value(theme[1], scope) ? theme[2] : theme[3], scope, out);
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
  for (const values of applied) {
    for (const binding of values) {
      if (binding[0][0] === $UI_COLOR_TOKEN) {
        bindings.set($ui_theme_name(binding[0][1]), binding[1]);
      }
    }
  }

  let out = "";
  for (const values of applied) {
    const body = [];
    for (const binding of values) {
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
    if (wrapper[0][0] !== 0) return false;
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

// --- The headless user-interface platform ------------------------------------
//
// The same graph, with no document attached: `ui/testing` is about what the
// runtime does, and a second implementation of it would be a second thing to
// be right. The handles are unused — the graph is the state — but the structs
// carry one so that the shape matches every other test double.

const $ui_testing_Headless_signal = $host_HostUi_signal;
const $ui_testing_Headless_read = $host_HostUi_read;
const $ui_testing_Headless_write = $host_HostUi_write;
const $ui_testing_Headless_memo = $host_HostUi_memo;
const $ui_testing_Headless_watch = $host_HostUi_watch;
const $ui_testing_Observer_read = $host_HostWatch_read;

function $ui_testing_headless() {
  return $handle(0);
}

function $ui_testing_observer() {
  return $handle(0);
}

// The stylesheet the compiler extracted for this artifact, as text. Reading it
// is how a test asserts what a class *means* rather than only what it is
// called, and how it sees that two modules asking for one padding produced one
// rule.
function $ui_testing_stylesheet() {
  return $ui_sheet;
}

// Installs a theme list the way `mount` does, and answers the custom-property
// block it resolved to. A switching theme registers its computation here too,
// so reading `variables` again after a signal write is exactly what a page
// would show.
function $ui_testing_install(themes) {
  if ($ui_theme_hook !== null) $ui_theme_hook(themes);
  return $ui_theme_text;
}

function $ui_testing_variables() {
  return $ui_theme_text;
}

function $ui_testing_recorder() {
  return $handle({ tags: [], values: [] });
}

function $ui_testing_Recorder_record(self, tag) {
  $slot(self).tags.push(tag);
  return 0;
}

function $ui_testing_Recorder_recorded(self) {
  return $slot(self).tags.slice();
}

function $ui_testing_Recorder_note(self, value) {
  $slot(self).values.push(value);
  return value;
}

function $ui_testing_Recorder_noted(self) {
  return $slot(self).values.slice();
}

// A tree rendered into a document of its own, by the renderer `mount` uses.
// The handle holds the root, which is a substitute element and never a real
// one: a test asks what was rendered, and only the substitute can answer.

function $ui_testing_render(ctx, root) {
  const host = $dom_make(0, "root");
  $tree_render(ctx, root, host, null);
  return $handle(host);
}

function $ui_testing_Rendered_markup(self) {
  let out = "";
  for (const child of $slot(self).children) out += $dom_markup(child);
  return out;
}

function $ui_testing_Rendered_text(self) {
  return $dom_runs($slot(self), []).join(" ");
}

// Addressed by label, because that is what a reader addresses them by: a test
// that says which control it meant does not quietly start pressing another one
// when the tree changes. Not finding it is a failed test rather than a silent
// no-op, which is the whole reason these abort.
function $tree_labelled(self, name, label) {
  for (const element of $dom_elements($slot(self), name, [])) {
    if ($dom_label(element) === label) return element;
  }
  $abort("this tree has no " + name + ' labelled "' + label + '"');
  return null;
}

function $ui_testing_Rendered_press(self, label) {
  $dom_fire($tree_labelled(self, "button", label), "click");
  return 0;
}

function $ui_testing_Rendered_fill(self, label, value) {
  const field = $dom_first($tree_labelled(self, "label", label), ["input", "textarea"]);
  if (field === null) $abort('the label "' + label + '" is not a field');
  field.value = value;
  $dom_fire(field, "input");
  return 0;
}

function $ui_testing_Rendered_flip(self, label) {
  const box = $dom_first($tree_labelled(self, "label", label), ["input"]);
  if (box === null) $abort('the label "' + label + '" is not a toggle');
  box.checked = !box.checked;
  $dom_fire(box, "change");
  return 0;
}

function $ui_testing_Rendered_submit(self, at) {
  const forms = $dom_elements($slot(self), "form", []);
  const index = Number(at);
  if (index < 0 || index >= forms.length) $abort("this tree has no form " + index);
  $dom_fire(forms[index], "submit");
  return 0;
}

function $ui_testing_Rendered_count(self, name) {
  return BigInt($dom_elements($slot(self), name, []).length);
}

function $ui_testing_Rendered_identity(self, name, at) {
  const elements = $dom_elements($slot(self), name, []);
  const index = Number(at);
  if (index < 0 || index >= elements.length) {
    $abort("this tree has no " + name + " " + index);
  }
  return elements[index].identity;
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

// --- core/host/testing --------------------------------------------------------------
//
// `core/host`'s names, called rather than referred to, over the `$t.h` table:
// one handle store, and the Buri type of the value carrying a handle says which
// slot shape made it.
//
// Configuration answers a *new* handle rather than editing the one it was
// called on, so `clock()` and `clock().at(1000)` are two clocks and a test
// holding both holds two. `TestFs.readOnly` is the one that answers a new
// handle over the *same* two objects, because attenuating a filesystem is not
// copying it.

function $host_testing_alloc() {
  return $handle({});
}

// `Region` is a newtype over `I64`, so the charge stays a `BigInt`: the count
// is handed straight back, which is what both native backends open-code and
// what makes `alloc.allocate(ctx, 64) == Region(64)` true on every backend.
function $host_testing_TestAlloc_allocate(self, n) {
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

// A `TestFs` handle is a *view*: the files and directories it reads and writes,
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
// Every row below takes the **handle** rather than the `TestFs`, because a
// `TestFs` is a handle *and* a fault plan and an argument crosses as its leaves.
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
// carries a field, so matching is the `Eq` the `Call` records derive and happens
// in `host_testing.buri` — on both backends, from one implementation. What is
// here is the half a program cannot keep: which entries have fired, and what
// each of them would read like in a failure message.
//
// `cli/runtime/testing.rs`'s `Slot::Plan` is the same three fields for the same
// reason, and the two are held together by the conformance corpus.

// `IoError`'s and `NetError`'s variant names, in `core/effect`'s declaration
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

// The URL and not the whole request: matching is `NetCall`'s derived `Eq` and
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
function $host_testing_TestTasks_parallel(self, ctx, xs, f) {
  const out = new Array(xs.length);
  for (const index of $torder(self, xs.length)) {
    $tplanned(self, index);
    out[index] = f(ctx, BigInt(index), $share(xs[index]));
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

// `Frame`'s two framings, as the variant indices `core/effect` declares. The
// third, `.Closed`, is never written here: only the two sends write this log.
const $SOCKET_TEXT = 0;
const $SOCKET_BINARY = 1;

function $host_testing_sockets() {
  return $handle({ sent: [] });
}

function $host_testing_socketsOpen(h) {
  return $tmint({ owner: Number(h), open: true });
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
  return $tsockpush(self, socket, $SOCKET_TEXT, text, []);
}

function $host_testing_TestSockets_socketSendBytes(self, socket, body) {
  return $tsockpush(self, socket, $SOCKET_BINARY, "", body.slice());
}

function $host_testing_TestSockets_socketClose(self, socket, code, reason) {
  if (!$tsockwritable(self[0], socket)) return 0;
  $tslot(socket).open = false;
  return 0;
}

// A fresh, empty log, and the handle that names it. A bare `I64` rather than a
// handle-carrying value, because `net()` is a Buri body that builds the
// `TestNet` around it — the responder in the other field is a value this file
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

// One request, recorded once the responder has answered it. `Request`'s four
// fields arrive separately because a `Method` crosses as its variant index;
// they go back together as the `NetCall` the constructor `fetch` builds.
function $host_testing_recordFetch(h, method, url, headers, body) {
  $t.h[Number(h)].calls.push([
    Number(method),
    url,
    headers.map(function (e) {
      return e.slice();
    }),
    body.slice(),
  ]);
  return 0;
}

// Every request this network answered, in that order. A `NetCall` is a newtype
// over `Request`, so each one is its request in a one-element array.
//
// By the handle and not by the `TestNet`: that value carries a responder as
// well, and `TestNet.calls` is the Buri body that unwraps it — the one `calls()`
// in this module that is not a row of its own.
function $host_testing_netCalls(h) {
  return $t.h[Number(h)].calls.map(function (r) {
    return [r.slice()];
  });
}

// The read-back, without the effect: the same answer `readFile` gives, and no
// `Fs` bound needed to ask it.
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

// Millis in and millis out are both `I64`, so this one counts in `BigInt`.
function $host_testing_clock() {
  return $handle({ now: 0n });
}

function $host_testing_TestClock_at(self, ms) {
  return $handle({ now: ms });
}

function $host_testing_TestClock_nowMillis(self) {
  return $slot(self).now;
}

// Moves the clock without sleeping, which is the whole point of a test clock.
function $host_testing_TestClock_sleepMillis(self, ms) {
  $slot(self).now += ms;
  return 0;
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

function $host_testing_TestRand_seed(self, n) {
  return $handle({ s: Number(BigInt.asUintN(32, n)) || 1 });
}

function $host_testing_TestRand_nextInt(self, lo, hi) {
  if (hi <= lo) $abort("random range is empty");
  return lo + (BigInt($nextRand($slot(self))) % (hi - lo));
}

function $host_testing_TestRand_nextFloat(self) {
  return $nextRand($slot(self)) / 4294967296;
}

// The seeded `Entropy`, on `TestRand`'s own generator and at its own seeds, so
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
function $host_testing_TestEnv_variables(self, vars) {
  const v = {};
  for (const e of vars) v[e[0]] = e[1];
  return $handle({ vars: v, args: $slot(self).args.slice() });
}

function $host_testing_TestEnv_arguments(self, args) {
  return $handle({ vars: Object.assign({}, $slot(self).vars), args: args.slice() });
}

function $host_testing_TestEnv_variable(self, name) {
  const v = $slot(self).vars;
  return name in v ? $some(v[name]) : undefined;
}

function $host_testing_TestEnv_args(self) {
  return $slot(self).args.slice();
}

// `proc()` has no function here and no slot: `TestProc` records nothing,
// because nothing can read it back. `proc()` is `TestProc(0)` and `exitWith`
// is an empty body, both written in `host_testing.buri` — the same shape
// `TestNet` has, reached for the plainer reason.

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

// Turning a Template into a Str is the point at which interpolation
// allocates; constructing the Template itself does not.
function $str_format(c, t) {
  return t;
}
