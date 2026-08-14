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
//   Template        an array of parts, rendered by $fmt
//   struct          an array of fields, in declaration order
//   enum            a number (the tag) when no variant has a payload,
//                   otherwise [tag, ...payload]
//   tuple, [T]      an array
//   fn              a function
//   context         an array of implementations, in binding order

// --- Failure ----------------------------------------------------------------

// The program has no way to say "this cannot happen" — every case is handled —
// so this is only ever reached from a runtime failure the language does define.
function $abort(m) {
  const e = new Error(typeof m === "string" ? m : $fmt(m));
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

// --- Rendering ---------------------------------------------------------------

// Constructing a Template allocates nothing; this is where it is rendered.
function $fmt(parts) {
  let out = "";
  for (let i = 0; i < parts.length; i++) out += $str(parts[i]);
  return out;
}

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

function $eq(a, b) {
  if (a === b) return true;
  // NaN !== NaN, so a struct with an F64 field holding NaN is not equal to
  // itself. That is structural equality being honest about its components.
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!$eq(a[i], b[i])) return false;
    return true;
  }
  return false;
}

// Returns the index of an `Order` variant: 0 Less, 1 Equal, 2 Greater.
function $cmp(a, b) {
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
function $hash(v) {
  let h = 0x811c9dc5;
  const mix = (x) => {
    h = (h ^ (x >>> 0)) >>> 0;
    h = Math.imul(h, 0x01000193) >>> 0;
  };
  const go = (x) => {
    if (Array.isArray(x)) {
      mix(x.length);
      for (const y of x) go(y);
    } else if (typeof x === "string") {
      for (let i = 0; i < x.length; i++) mix(x.charCodeAt(i));
    } else if (typeof x === "boolean") mix(x ? 1 : 0);
    else mix(Math.trunc(x) || 0);
  };
  go(v);
  return h;
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
    if (!fields.length) return name;
    if (record) {
      return (
        name +
        " { " +
        fields.map((f, i) => f + ": " + $show(v[i], types[i])).join(", ") +
        " }"
      );
    }
    return name + "(" + v.map((x, i) => $show(x, types[i])).join(", ") + ")";
  }
  if (k === 3) {
    // [3, name, variants, payloadless]
    const [, , variants, flat] = d;
    const tag = flat ? v : v[0];
    const [vname, record, fields, types] = variants[tag];
    if (!fields.length) return "." + vname;
    const args = flat ? [] : v.slice(1);
    if (record) {
      return (
        "." +
        vname +
        " { " +
        fields.map((f, i) => f + ": " + $show(args[i], types[i])).join(", ") +
        " }"
      );
    }
    return "." + vname + "(" + args.map((x, i) => $show(x, types[i])).join(", ") + ")";
  }
  if (k === 4) return "[" + v.map((x) => $show(x, d[1])).join(", ") + "]";
  if (k === 5) return "(" + v.map((x, i) => $show(x, d[1][i])).join(", ") + ")";
  return $str(v);
}

// --- core/list ----------------------------------------------------------------
//
// Indexing yields Option<T>, so `get` is where the absence shows up. Option is
// [0, x] for Some and 1 for None only if it were payloadless, which it is not:
// Some carries a value, so None is [1].

function $some(x) {
  return [0, x];
}
const $none = [1];

function $list_len(xs) {
  return xs.length;
}

function $list_get(xs, i) {
  const n = Number(i);
  return n >= 0 && n < xs.length ? $some(xs[n]) : $none;
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
  return $none;
}

function $list_findIndex(xs, p) {
  for (let i = 0; i < xs.length; i++) if (p(xs[i])) return $some(i);
  return $none;
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

function $str_len(s) {
  return $chars(s).length;
}

function $str_charAt(s, i) {
  const cs = $chars(s);
  const n = Number(i);
  return n >= 0 && n < cs.length ? $some(cs[n]) : $none;
}

function $str_slice(s, a, b) {
  return $chars(s)
    .slice(Math.max(0, Number(a)), Math.max(0, Number(b)))
    .join("");
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
  return i < 0 ? $none : $some($chars(s.slice(0, i)).length);
}

// Two slices, or .None when the separator does not occur. Pure, because
// neither half is a copy.
function $str_splitOnce(s, sep) {
  const i = s.indexOf(sep);
  return i < 0 ? $none : $some([s.slice(0, i), s.slice(i + sep.length)]);
}

function $str_compare(a, b) {
  return a < b ? 0 : a > b ? 2 : 1;
}

function $str_toInt(s) {
  const t = s.trim();
  if (!/^[+-]?\d+$/.test(t)) return $none;
  try {
    const v = Number(t);
    // `Int` is `I64`, and a double represents integers exactly only to 2^53.
    // Past that there is no `Int` to parse to, which is what the `Option` is
    // for — rather than handing back a value that is quietly not the one
    // written.
    if (!Number.isSafeInteger(v)) return $none;
    return $some(v);
  } catch {
    return $none;
  }
}

function $str_toFloat(s) {
  const t = s.trim();
  if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t)) return $none;
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
  const n = Number(w) - $chars(s).length;
  return n > 0 ? fill.repeat(n) + s : s;
}

function $str_padEnd(s, c, w, fill) {
  const n = Number(w) - $chars(s).length;
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
  return Number.isNaN(n) ? $none : $some(n);
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

function $host_HostAlloc_allocate(self, n) {
  return [Number(n)];
}

function $host_HostStdout_print(self, t) {
  $host.out.push($fmt(t));
  if ($host.out.length > 64) $host.flush();
  return 0;
}

function $host_HostStdout_println(self, t) {
  $host.out.push($fmt(t) + "\n");
  if ($host.out.length > 64) $host.flush();
  return 0;
}

function $host_HostStderr_eprint(self, t) {
  $host.err.push($fmt(t));
  return 0;
}

function $host_HostStderr_eprintln(self, t) {
  $host.err.push($fmt(t) + "\n");
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
  return $stdinAt < $stdinLines.length ? $some($stdinLines[$stdinAt++]) : $none;
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
  return v === undefined ? $none : $some(v);
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
  $slot(self).text += $fmt(t);
  return 0;
}

function $testing_context_CaptureOut_println(self, t) {
  $slot(self).text += $fmt(t) + "\n";
  return 0;
}

function $testing_context_CaptureErr_eprint(self, t) {
  $slot(self).text += $fmt(t);
  return 0;
}

function $testing_context_CaptureErr_eprintln(self, t) {
  $slot(self).text += $fmt(t) + "\n";
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
  return s.at < s.lines.length ? $some(s.lines[s.at++]) : $none;
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
  return name in v ? $some(v[name]) : $none;
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
  if (!Number.isFinite(v) || v < lo || v > hi) return $none;
  return $some(v);
}

function $sat(v, lo, hi) {
  return v < lo ? lo : v > hi ? hi : v;
}

// Turning a Template into a Str is the point at which interpolation
// allocates; constructing the Template itself does not.
function $str_format(c, t) {
  return $fmt(t);
}
