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
//   every integer   number   -- `Int` is `I64`, and a double holds every
//                               integer to 2^53 - 1 exactly; past that,
//                               overflow is undefined behaviour, and lost
//                               precision is the form it takes here
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

// Taking the low `bits` of a value, for checksums and wire formats where
// wrapping is the intent. A BigInt is used because a double cannot hold the
// intermediate exactly at 64 bits and above.
function $wrapTo(v, bits, signed) {
  const b = BigInt(Math.trunc(v));
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
// Emitted only where the intermediate can leave the exact range *and* the
// type's whole range is inside it (`js/intrinsics.rs`), which is a product at
// 32 bits and nothing else today. At 64 and above the operands themselves may
// already be rounded, so computing exactly on them is not a repair; that is the
// precision ceiling of `VALUE-MODEL.md` §12 row 1.
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
  // float.
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
  return $mix(h, Math.trunc(x) || 0);
}

function $hash(v) {
  return $hashInto(0x811c9dc5, v);
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
    // "i" is an integer, and a number is also how a float is stored, so the
    // tag is what tells them apart.
    if (p === "i") return String(v);
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
    return [2, v];
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
    return j[1];
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
  return xs.length;
}

function $list_get(xs, i) {
  const n = Number(i);
  return n >= 0 && n < xs.length ? $some(xs[n]) : undefined;
}

function $list_fold(xs, f, acc) {
  for (let i = 0; i < xs.length; i++) acc = f(acc, xs[i]);
  return acc;
}

function $list_foldCtx(xs, c, f, acc) {
  for (let i = 0; i < xs.length; i++) acc = f(c, acc, xs[i]);
  return acc;
}

// Stops at the first .Err, which is how a fallible fold is written without an
// early exit.
function $list_foldResult(xs, f, acc) {
  let cur = [0, acc];
  for (let i = 0; i < xs.length; i++) {
    cur = f(cur[1], xs[i]);
    if (cur[0] !== 0) return cur;
  }
  return cur;
}

function $list_foldResultCtx(xs, c, f, acc) {
  let cur = [0, acc];
  for (let i = 0; i < xs.length; i++) {
    cur = f(c, cur[1], xs[i]);
    if (cur[0] !== 0) return cur;
  }
  return cur;
}

function $list_any(xs, p) {
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return true;
  return false;
}

function $list_all(xs, p) {
  for (let i = 0; i < xs.length; i++) if (!p(xs[i])) return false;
  return true;
}

function $list_find(xs, p) {
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return $some(xs[i]);
  return undefined;
}

function $list_findIndex(xs, p) {
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return $some(i);
  return undefined;
}

function $list_count(xs, p) {
  let n = 0;
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) n++;
  return n;
}

function $list_map(xs, c, f) {
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = f(xs[i]);
  return out;
}

function $list_mapCtx(xs, c, f) {
  const out = new Array(xs.length);
  for (let i = 0; i < xs.length; i++) out[i] = f(c, xs[i]);
  return out;
}

function $list_filter(xs, c, p) {
  const out = [];
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) out.push(xs[i]);
  return out;
}

function $list_filterCtx(xs, c, p) {
  const out = [];
  for (let i = 0; i < xs.length; i++) if (p(c, xs[i])) out.push(xs[i]);
  return out;
}

function $list_concat(xs, c, ys) {
  return xs.concat(ys);
}

function $list_push(xs, c, x) {
  const out = xs.slice();
  out.push(x);
  return out;
}

function $list_reverse(xs, c) {
  return xs.slice().reverse();
}

// Stable, so a tie-break the comparator does not decide keeps source order.
function $list_sortBy(xs, c, order) {
  return xs
    .map((v, i) => [v, i])
    .sort((a, b) => {
      const o = order(a[0], b[0]);
      return o === 1 ? a[1] - b[1] : o === 0 ? -1 : 1;
    })
    .map((p) => p[0]);
}

function $list_take(xs, c, n) {
  return xs.slice(0, Math.max(0, Number(n)));
}

function $list_drop(xs, c, n) {
  return xs.slice(Math.max(0, Number(n)));
}

function $list_slice(xs, c, a, b) {
  return xs.slice(Math.max(0, Number(a)), Math.max(0, Number(b)));
}

function $list_zip(xs, c, ys) {
  const n = Math.min(xs.length, ys.length);
  const out = new Array(n);
  for (let i = 0; i < n; i++) out[i] = [xs[i], ys[i]];
  return out;
}

function $list_flatten(xs, c) {
  const out = [];
  for (const x of xs) for (const y of x) out.push(y);
  return out;
}

function $list_empty() {
  return [];
}

function $list_range(c, a, b) {
  const out = [];
  for (let i = a; i < b; i++) out.push(i);
  return out;
}

function $list_repeat(c, x, n) {
  const out = [];
  for (let i = 0; i < n; i++) out.push(x);
  return out;
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
  return $wide(s) ? $chars(s).length : s.length;
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
  return $some($wide(prefix) ? $chars(prefix).length : i);
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

function $str_toInt(s) {
  const t = s.trim();
  if (!/^[+-]?\d+$/.test(t)) return undefined;
  try {
    const v = Number(t);
    // `Int` is `I64`, and a double represents integers exactly only to 2^53.
    // Past that there is no `Int` to parse to, which is what the `Option` is
    // for — rather than handing back a value that is quietly not the one
    // written.
    if (!Number.isSafeInteger(v)) return undefined;
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
  const n = Number(w) - $str_len(s);
  return n > 0 ? fill.repeat(n) + s : s;
}

function $str_padEnd(s, c, w, fill) {
  const n = Number(w) - $str_len(s);
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
  return Number.isNaN(n) ? undefined : $some(n);
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

// Bit operations are exact where a double is not, so each one goes through a
// BigInt and comes back as a number. Shifting by a count at or beyond the
// width of the type aborts.
function $shiftCount(n, bits) {
  const k = Number(n);
  if (k < 0 || k >= bits) $abort("shift out of range");
  return BigInt(k);
}

function $big(x) {
  return BigInt(Math.trunc(x));
}

// --- 64-bit bitwise ------------------------------------------------------------------
//
// JavaScript's `&`, `|`, `^` and `~` coerce to *32-bit signed* integers, so
// `1 << 31` comes back negative and everything above bit 31 is discarded
// outright. `Int` is `I64`, so using them directly made `a & b` silently wrong
// for half the range of the type — `(1<<40) & (1<<40)` was `0`.
//
// Only the 64-bit types route through here. At 32 bits and below the native
// operators are exact, and staying on them keeps ordinary integer code as fast
// as ordinary JavaScript.
//
// A BigInt is a heap object in every engine — no small-integer form — so each
// of these otherwise allocates two of them and pays a `Number(bigint)`
// conversion on the way out, which alone costs about twenty-five times an
// ordinary addition. Almost every operand is small, and where both already fit
// in a signed 32-bit integer JavaScript's own operator is *exact*: AND, OR, XOR
// and NOT distribute over sign extension, so the 32-bit answer sign-extends to
// the 64-bit one. The BigInt is built only when that is not true.
function $and64(a, b) {
  if ((a | 0) === a && (b | 0) === b) return a & b;
  return Number(BigInt.asIntN(64, $big(a) & $big(b)));
}
function $or64(a, b) {
  if ((a | 0) === a && (b | 0) === b) return a | b;
  return Number(BigInt.asIntN(64, $big(a) | $big(b)));
}
function $xor64(a, b) {
  if ((a | 0) === a && (b | 0) === b) return a ^ b;
  return Number(BigInt.asIntN(64, $big(a) ^ $big(b)));
}
function $not64(a) {
  if ((a | 0) === a) return ~a;
  return Number(BigInt.asIntN(64, ~$big(a)));
}

// Unsigned, 32 bits and below. JavaScript's bitwise operators produce a
// *signed* 32-bit result, so `0x80000000 | 0` came back as `-2147483648` and
// `~0` on a `U8` came back as `-1` instead of `255`. The operands are in range
// and the answer is in range; only the representation was wrong, so narrowing
// the result to the type's own width is the whole fix.
function $umask(v, bits) {
  return bits >= 32 ? v >>> 0 : v & ((1 << bits) - 1);
}

// The unsigned forms differ only in how the result is narrowed, which is the
// difference between `~0` being `-1` and being `2^64 - 1`.
//
// That difference is also what makes their fast path narrower than the signed
// one: it holds only where the two narrowings agree, which is both operands
// non-negative and below 2^31, where the result is non-negative too. `$notU64`
// has no fast path at all — `~a` for a non-negative `a` is negative, and
// `asUintN` maps it above 2^53, so there is nothing a 32-bit operator could
// answer.
function $andU64(a, b) {
  if (a >= 0 && b >= 0 && (a | 0) === a && (b | 0) === b) return a & b;
  return Number(BigInt.asUintN(64, $big(a) & $big(b)));
}
function $orU64(a, b) {
  if (a >= 0 && b >= 0 && (a | 0) === a && (b | 0) === b) return a | b;
  return Number(BigInt.asUintN(64, $big(a) | $big(b)));
}
function $xorU64(a, b) {
  if (a >= 0 && b >= 0 && (a | 0) === a && (b | 0) === b) return a ^ b;
  return Number(BigInt.asUintN(64, $big(a) ^ $big(b)));
}
function $notU64(a) {
  return Number(BigInt.asUintN(64, ~$big(a)));
}

function $bits_shl(x, n) {
  return Number(BigInt.asIntN(64, $big(x) << $shiftCount(n, 64)));
}

function $bits_shr(x, n) {
  // Logical: reinterpret as unsigned, shift, then narrow back, so a shift by
  // zero is the identity rather than the unsigned reinterpretation.
  return Number(BigInt.asIntN(64, BigInt.asUintN(64, $big(x)) >> $shiftCount(n, 64)));
}

function $bits_sar(x, n) {
  return Number($big(x) >> $shiftCount(n, 64));
}

function $bits_popCount(x) {
  let v = BigInt.asUintN(64, $big(x));
  let n = 0;
  while (v) {
    n += Number(v & 1n);
    v >>= 1n;
  }
  return n;
}

function $bits_leadingZeros(x) {
  const v = BigInt.asUintN(64, $big(x));
  let n = 0;
  for (let i = 63n; i >= 0n; i--) {
    if ((v >> i) & 1n) break;
    n++;
  }
  return n;
}

function $bits_trailingZeros(x) {
  const v = BigInt.asUintN(64, $big(x));
  if (v === 0n) return 64;
  let n = 0n;
  while (!((v >> n) & 1n)) n++;
  return Number(n);
}

function $bits_rotateLeft(x, n) {
  const k = $shiftCount(n, 64);
  const v = BigInt.asUintN(64, $big(x));
  return Number(BigInt.asIntN(64, ((v << k) | (v >> (64n - k))) & 0xffffffffffffffffn));
}

function $bits_rotateRight(x, n) {
  const k = $shiftCount(n, 64);
  const v = BigInt.asUintN(64, $big(x));
  return Number(BigInt.asIntN(64, ((v >> k) | (v << (64n - k))) & 0xffffffffffffffffn));
}

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
  return Number(BigInt.asUintN(64, $big(x) << $shiftCount(n, 64)));
}
function $bits_shrU64(x, n) {
  return Number(BigInt.asUintN(64, $big(x)) >> $shiftCount(n, 64));
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
      return $err([i]);
    }
    // The continuation bytes have to be there. A tuple struct is an array
    // of its fields, so a `Utf8Error(i)` is `[i]`.
    if (n > 0 && i + n >= b.length) return $err([i]);
    for (let k = 1; k <= n; k++) {
      const cc = b[i + k] & 0xff;
      if ((cc & 0xc0) !== 0x80) return $err([i]);
      cp = (cp << 6) | (cc & 0x3f);
    }
    const min = n === 0 ? 0 : n === 1 ? 0x80 : n === 2 ? 0x800 : 0x10000;
    if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return $err([i]);
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

function $bytes_f64FromBytes(b, at) {
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

function $bytes_f32FromBytes(b, at) {
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

// `lo` and `hi` are the target's range narrowed to what a `number` still holds
// exactly, so an `.Ok` is always the value that was converted — never one that
// merely rounded into range.
function $convChecked(v, lo, hi, target, flt) {
  if (!Number.isFinite(v)) return $rangeErr(v, target, flt);
  if (!Number.isInteger(v)) return $rangeErr(v, target, flt);
  if (v < lo || v > hi) return $rangeErr(v, target, flt);
  return $ok(v);
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

const $host = {
  out: [],
  err: [],
  flush() {
    if (this.out.length) {
      $write(1, this.out.join(""));
      this.out = [];
    }
    if (this.err.length) {
      $write(2, this.err.join(""));
      this.err = [];
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
  return $alloc.c.length - 1;
}

// A budget is checked *before* the charge lands, and exceeding it ends the
// process: `allocate` answers `Region` and not `Result`, so there is no value
// to report the failure with (SPEC 6.10, MEMORY.md §7.2). The message is
// `cli/runtime/abort.rs`'s, word for word.
function $alloc_charge(h, bytes) {
  const c = $alloc.c[h];
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
  return n;
}

function $alloc_count(h) {
  return $alloc.c[h].n;
}

function $alloc_total(h) {
  return $alloc.c[h].bytes;
}

function $host_HostStdout_print(self, t) {
  $host.out.push(t);
  if ($host.out.length > 64) $host.flush();
  return 0;
}

function $host_HostStdout_println(self, t) {
  $host.out.push(t + "\n");
  if ($host.out.length > 64) $host.flush();
  return 0;
}

// Octets, written through unchanged. The buffered text stream is flushed
// first, so the two orderings a program can see are the one it wrote.
function $host_HostStdout_writeBytes(self, b) {
  $host.flush();
  $writeRaw(1, b);
  return 0;
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
  return 0;
}

function $host_HostStderr_eprintln(self, t) {
  $host.err.push(t + "\n");
  return 0;
}

let $stdinLines = null;
let $stdinAt = 0;

function $host_HostStdin_readLine(self) {
  if ($stdinLines === null) {
    let text = "";
    try {
      text = $fs().readFileSync(0, "utf8");
    } catch {
      text = "";
    }
    $stdinLines = text.length ? text.split("\n") : [];
    if ($stdinLines.length && $stdinLines[$stdinLines.length - 1] === "") $stdinLines.pop();
  }
  return $stdinAt < $stdinLines.length ? $some($stdinLines[$stdinAt++]) : undefined;
}

// Exactly `n` octets, blocking until they arrive. `read` on a pipe returns
// what is available rather than what was asked for, so this loops — and a
// short read at end of input yields what it got, or nothing at all.
function $host_HostStdin_readBytes(self, n) {
  if (n <= 0) return [];
  const buf = Buffer.alloc(n);
  let got = 0;
  while (got < n) {
    let r;
    try {
      r = $fs().readSync(0, buf, got, n - got, null);
    } catch (e) {
      if (e && e.code === "EAGAIN") continue;
      if (e && e.code === "EOF") break;
      throw e;
    }
    if (r === 0) break;
    got += r;
  }
  if (got === 0) return undefined;
  return Array.from(buf.subarray(0, got));
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
  return [5, String((e && e.message) || e)];
}

// `require` does not exist in an ES module on node, so the backend emits a
// `createRequire` prologue when — and only when — a program actually reaches
// the filesystem. A program whose `main` never binds `Fs` never gets one.
function $fs() {
  if (typeof $require === "function") return $require("fs");
  if (typeof require === "function") return require("fs");
  $abort("this platform grants no filesystem");
}

function $host_HostFs_readFile(self, p) {
  try {
    return $ok($fs().readFileSync(p, "utf8"));
  } catch (e) {
    return $err($ioErr(e));
  }
}

function $host_HostFs_writeFile(self, p, b) {
  try {
    $fs().writeFileSync(p, b);
    return $ok(0);
  } catch (e) {
    return $err($ioErr(e));
  }
}

function $host_HostFs_fileExists(self, p) {
  try {
    return $fs().existsSync(p);
  } catch {
    return false;
  }
}

function $host_HostFs_readDir(self, p) {
  try {
    return $ok($fs().readdirSync(p));
  } catch (e) {
    return $err($ioErr(e));
  }
}

function $host_HostNet_fetch(self, method, url, body) {
  // Synchronous by necessity: Buri has no `async` in v0.3, so a request
  // blocks. Bun exposes a blocking XHR-shaped path; elsewhere this refuses
  // rather than pretending.
  try {
    const req = new XMLHttpRequest();
    req.open(method, url, false);
    req.send(body === "" ? null : body);
    return $ok([req.status, req.responseText]);
  } catch (e) {
    return $err([3, String((e && e.message) || e)]);
  }
}

function $host_HostClock_nowMillis(self) {
  return Date.now();
}

function $host_HostClock_sleepMillis(self, ms) {
  const end = Date.now() + Number(ms);
  if (typeof Bun !== "undefined" && Bun.sleepSync) Bun.sleepSync(Number(ms));
  else while (Date.now() < end);
  return 0;
}

function $host_HostRand_nextInt(self, lo, hi) {
  if (hi <= lo) $abort("random range is empty");
  return lo + Math.floor(Math.random() * (hi - lo));
}

function $host_HostRand_nextFloat(self) {
  return Math.random();
}

function $host_HostEnv_variable(self, name) {
  const env = typeof process !== "undefined" ? process.env : {};
  const v = env[name];
  return v === undefined ? undefined : $some(v);
}

function $host_HostEnv_arguments(self) {
  if (typeof Bun !== "undefined") return Bun.argv.slice(2);
  if (typeof process !== "undefined") return process.argv.slice(2);
  return [];
}

function $host_HostProc_exitWith(self, code) {
  $host.flush();
  if (typeof process !== "undefined") process.exit(code);
  return 0;
}

// --- The reactive graph -----------------------------------------------------
//
// Auto-tracking, in the shape design/ui-reactivity.md commits to: the runtime
// holds a pointer to the computation that is running, `read` records a
// source -> computation edge, `write` marks dependents out of date and
// schedules them, and dependencies are collected afresh on every run, so a
// read behind an `if` is tracked exactly.
//
// One array of nodes, indexed by the `Int` a Buri `Signal<T>` carries. Three
// kinds, told apart by `kind`:
//
//   0  cell        a value, written from outside
//   1  memo        a value, computed from other nodes, lazily
//   2  watcher     run for its effect on the world, eagerly
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
  // The computation whose body is running, or -1. This is what makes tracking
  // automatic: nothing is declared, `read` simply looks here.
  current: -1,
  queue: [],
  // Open batches. A write inside one defers the drain, so N writes cause one
  // pass rather than N.
  depth: 0,
};

// A runaway is a program whose watchers write what they read. The limit is not
// a policy, it is the difference between a diagnosis and a hung tab.
const $UI_STEPS = 100000;

function $ui_node(kind, value, compute) {
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

function $ui_read(id) {
  const n = $ui_at(id);
  // Reading is what makes a memo run: until then it has computed nothing, and
  // a memo nothing reads never runs at all.
  if (n.kind === 1 && n.dirty && !n.disposed) $ui_run(id);
  const c = $ui.current;
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
  const outer = $ui.current;
  $ui.current = id;
  try {
    // The `Scope` a Buri closure receives: a one-field struct naming the
    // computation it belongs to.
    const v = n.compute([id]);
    if (n.kind === 1) n.value = v;
  } finally {
    $ui.current = outer;
    n.dirty = false;
  }
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
    } else if (!c.queued) {
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

function $ui_write(id, v) {
  const n = $ui_at(id);
  // Identical is not a change. This is what makes "wrote the same value, so
  // nothing re-ran" a thing a test can assert.
  if (n.value === v) return 0;
  n.value = v;
  $ui_notify(n);
  if ($ui.depth === 0) $ui_drain();
  return 0;
}

// One update transaction. Event handlers and fetch callbacks land inside one,
// so N writes cause one pass over the watchers.
function $ui_flush(f) {
  $ui.depth++;
  try {
    f();
  } finally {
    $ui.depth--;
  }
  if ($ui.depth === 0) $ui_drain();
  return 0;
}

function $host_HostUi_signal(self, initial) {
  return $ui_node(0, initial, null);
}

function $host_HostUi_read(self, id) {
  return $ui_read(id);
}

function $host_HostUi_write(self, id, value) {
  return $ui_write(id, value);
}

function $host_HostUi_memo(self, compute) {
  return $ui_node(1, undefined, compute);
}

function $host_HostUi_watch(self, run) {
  // Eager, and that is not an optimization: a watcher learns what it depends
  // on by running, so one that has never run is subscribed to nothing and
  // would never run again.
  $ui_run($ui_node(2, undefined, run));
  return 0;
}

function $host_HostWatch_read(self, id) {
  return $ui_read(id);
}

function $ui_effect_Scope_read(self, id) {
  return $ui_read(id);
}

// A `Request` is `[method, url, headers, body]` and a `FetchError` is
// `[tag, ...payload]` with the tag order `ui/effect` declares:
// Timeout, Refused, BadUrl, Transport, Aborted.
function $host_HostFetch_fetch(self, request, done) {
  const settle = (r) => $ui_flush(() => done(self, r));
  try {
    const req = new XMLHttpRequest();
    req.open(request[0], request[1], true);
    for (const h of request[2]) req.setRequestHeader(h[0], h[1]);
    req.onload = () => settle($ok([req.status, req.responseText]));
    req.onerror = () => settle($err([3, "transport"]));
    req.ontimeout = () => settle($err([0]));
    req.onabort = () => settle($err([4]));
    req.send(request[3] === "" ? null : request[3]);
  } catch (e) {
    settle($err([3, String((e && e.message) || e)]));
  }
  return 0;
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

// --- The test platform ------------------------------------------------------------
//
// The structs carry an I64 handle rather than their state, because Buri has no
// mutation. Each call to a constructor allocates a fresh one, which is why a
// named context is called rather than referred to.

const $t = { h: [], data: {}, fail: null };

function $handle(v) {
  $t.h.push(v);
  return [$t.h.length - 1];
}

function $slot(x) {
  return $t.h[x[0]];
}

function $testing_context_alloc() {
  return $handle({});
}

function $testing_context_TestAlloc_allocate(self, n) {
  return [Number(n)];
}

function $testing_context_captureOut() {
  return $handle({ text: "" });
}

function $testing_context_captureErr() {
  return $handle({ text: "" });
}

function $testing_context_CaptureOut_print(self, t) {
  $slot(self).text += t;
  return 0;
}

function $testing_context_CaptureOut_println(self, t) {
  $slot(self).text += t + "\n";
  return 0;
}

function $testing_context_CaptureErr_eprint(self, t) {
  $slot(self).text += t;
  return 0;
}

function $testing_context_CaptureErr_eprintln(self, t) {
  $slot(self).text += t + "\n";
  return 0;
}

function $testing_context_CaptureOut_captured(self) {
  return $slot(self).text;
}

function $testing_context_CaptureErr_capturedErr(self) {
  return $slot(self).text;
}

function $testing_context_stdin(lines) {
  return $handle({ lines: lines.slice(), at: 0 });
}

function $testing_context_TestStdin_readLine(self) {
  const s = $slot(self);
  if (s.bytes) return undefined;
  return s.at < s.lines.length ? $some(s.lines[s.at++]) : undefined;
}

function $testing_context_stdinBytes(b) {
  return $handle({ lines: [], at: 0, bytes: b.slice() });
}

function $testing_context_TestStdin_readBytes(self, n) {
  const s = $slot(self);
  const src = s.bytes || [];
  if (s.at >= src.length || n <= 0) return undefined;
  const out = src.slice(s.at, s.at + n);
  s.at += out.length;
  return out;
}

// The captured stream is text, so octets are captured as the text they spell:
// `captured` answers one question rather than two.
function $testing_context_CaptureOut_writeBytes(self, b) {
  const r = $bytes_fromUtf8(null, b);
  $slot(self).text += r[0] === 0 ? r[1] : String.fromCharCode.apply(null, b);
  return 0;
}

// In-memory, rooted at the package directory, containing exactly test.data.
function $testing_context_data() {
  return $handle({ files: Object.assign({}, $t.data) });
}

function $testing_context_files(entries) {
  const files = {};
  for (const e of entries) files[e[0]] = e[1];
  return $handle({ files });
}

function $testing_context_MemFs_readFile(self, p) {
  const f = $slot(self).files;
  return p in f ? $ok(f[p]) : $err([0]);
}

function $testing_context_MemFs_writeFile(self, p, b) {
  $slot(self).files[p] = b;
  return $ok(0);
}

function $testing_context_MemFs_fileExists(self, p) {
  return p in $slot(self).files;
}

function $testing_context_MemFs_readDir(self, p) {
  // A directory that holds nothing is still not an error; only a path that
  // names nothing at all is.
  const prefix = p === "" || p === "." ? "" : p.replace(/\/$/, "") + "/";
  const out = [];
  for (const k of Object.keys($slot(self).files)) {
    if (k.startsWith(prefix)) {
      const rest = k.slice(prefix.length);
      if (rest && !out.includes(rest.split("/")[0])) out.push(rest.split("/")[0]);
    }
  }
  return $ok(out.sort());
}

function $testing_context_clockAt(ms) {
  return $handle({ now: ms });
}

function $testing_context_TestClock_nowMillis(self) {
  return $slot(self).now;
}

function $testing_context_TestClock_sleepMillis(self, ms) {
  $slot(self).now += ms;
  return 0;
}

function $testing_context_TestClock_advance(self, ms) {
  $slot(self).now += ms;
  return 0;
}

// Seeded, so a failure reproduces.
function $testing_context_randSeed(seed) {
  return $handle({ s: (Math.trunc(seed) >>> 0) || 1 });
}

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

function $testing_context_TestRand_nextInt(self, lo, hi) {
  if (hi <= lo) $abort("random range is empty");
  return lo + ($nextRand($slot(self)) % (hi - lo));
}

function $testing_context_TestRand_nextFloat(self) {
  return $nextRand($slot(self)) / 4294967296;
}

function $testing_context_envOf(vars, args) {
  const v = {};
  for (const e of vars) v[e[0]] = e[1];
  return $handle({ vars: v, args: args.slice() });
}

function $testing_context_TestEnv_variable(self, name) {
  const v = $slot(self).vars;
  return name in v ? $some(v[name]) : undefined;
}

function $testing_context_TestEnv_arguments(self) {
  return $slot(self).args.slice();
}

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

function $sat(v, lo, hi) {
  return v < lo ? lo : v > hi ? hi : v;
}

// Turning a Template into a Str is the point at which interpolation
// allocates; constructing the Template itself does not.
function $str_format(c, t) {
  return t;
}
