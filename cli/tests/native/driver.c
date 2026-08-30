/* A C program that knows only `cli/runtime/lib.rs`'s ABI contract.
 *
 * It is written in C on purpose. The runtime is Rust, and a Rust driver would
 * agree with it about `#[repr(C)]` by construction rather than by contract —
 * which is precisely the thing under test, since the callers that will exist in
 * production are the two native backends, and neither has ever heard of Rust.
 *
 * Every declaration below is transcribed from the contract by hand. If the
 * contract and the runtime disagree, this file fails to link or prints the
 * wrong answer, and both are what the suite is for.
 *
 * `argv[1]` selects a mode; `cli/tests/native/runtime.rs` owns the expected
 * output of each one. */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* --- The contract ------------------------------------------------------- */

typedef struct {
  uint8_t *base;
  const uint8_t *ptr;
  uint64_t len;
} BuriStr;

typedef struct {
  uint8_t *ptr;
  uint64_t len;
} BuriList;

typedef struct {
  uint64_t live_blocks;
  uint64_t live_bytes;
  uint64_t total_blocks;
  uint64_t total_bytes;
} BuriHeapStats;

#define BURI_OK (-1)
#define BURI_STR_ASCII (1ull << 63)
#define BURI_STR_MASK (BURI_STR_ASCII - 1)

/* memory */
extern uint8_t *buri_rt_alloc(uint64_t payload);
extern uint8_t *buri_rt_alloc_zeroed(uint64_t payload);
extern uint8_t *buri_rt_realloc(uint8_t *p, uint64_t payload);
extern void buri_rt_free(uint8_t *p);
extern void buri_rt_incref(uint8_t *p);
extern void buri_rt_decref(uint8_t *p, void (*drop_glue)(uint8_t *));
extern void buri_rt_make_immortal(uint8_t *p);
extern uint64_t buri_rt_rc(uint8_t *p);
extern uint64_t buri_rt_cap(uint8_t *p);
extern void buri_rt_heap_stats(BuriHeapStats *out);
extern uint64_t buri_rt_live_blocks(void);
extern int64_t buri_rt_host_alloc_allocate(int64_t bytes);

/* values */
extern void buri_rt_str_from_utf8(const uint8_t *bytes, uint64_t len, BuriStr *out);
extern void buri_rt_str_empty(BuriStr *out);
extern uint64_t buri_rt_str_ascii_flag(const uint8_t *bytes, uint64_t len);
extern uint64_t buri_rt_str_scalar_len(const uint8_t *bytes, uint64_t len);
extern uint8_t *buri_rt_list_new(uint64_t count, uint64_t stride, BuriList *out);
extern void buri_rt_i128_divmod(uint64_t a_lo, uint64_t a_hi, uint64_t b_lo, uint64_t b_hi,
                                uint8_t is_signed, uint64_t *quot, uint64_t *rem);

/* aborts */
extern void buri_rt_abort(const uint8_t *msg, uint64_t len);
extern void buri_rt_abort_div_zero(void);
extern void buri_rt_abort_shift(void);
extern void buri_rt_abort_random_range(void);
extern void buri_rt_abort_bounds(int64_t index, int64_t len);
extern void buri_rt_abort_unreachable(void);
extern void buri_rt_abort_alloc_budget(int64_t requested, int64_t budget);
extern void buri_rt_abort_oom(uint64_t bytes);
extern int64_t buri_rt_alloc_budget_check(int64_t requested, int64_t used, int64_t budget);

/* host */
extern void buri_rt_argv_init(int32_t argc, const uint8_t **argv);
extern void buri_rt_flush(void);
extern void buri_rt_host_stdout_print(uint8_t *base, const uint8_t *ptr, uint64_t len);
extern void buri_rt_host_stdout_println(uint8_t *base, const uint8_t *ptr, uint64_t len);
extern void buri_rt_host_stdout_write_bytes(const uint8_t *ptr, uint64_t len);
extern void buri_rt_host_stderr_eprint(uint8_t *base, const uint8_t *ptr, uint64_t len);
extern void buri_rt_host_stderr_eprintln(uint8_t *base, const uint8_t *ptr, uint64_t len);
extern int32_t buri_rt_host_stdin_read_line(BuriStr *out);
extern int32_t buri_rt_host_stdin_read_bytes(int64_t n, BuriList *out);
extern int32_t buri_rt_host_fs_read_file(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                         BuriStr *out_ok, BuriStr *out_err);
extern int32_t buri_rt_host_fs_write_file(uint8_t *pbase, const uint8_t *pptr, uint64_t plen,
                                          uint8_t *bbase, const uint8_t *bptr, uint64_t blen,
                                          BuriStr *out_err);
extern uint8_t buri_rt_host_fs_file_exists(uint8_t *base, const uint8_t *ptr, uint64_t len);
extern int32_t buri_rt_host_fs_read_dir(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                        BuriList *out_ok, BuriStr *out_err);
extern int32_t buri_rt_host_net_fetch(uint8_t *mbase, const uint8_t *mptr, uint64_t mlen,
                                      uint8_t *ubase, const uint8_t *uptr, uint64_t ulen,
                                      uint8_t *bbase, const uint8_t *bptr, uint64_t blen,
                                      int64_t *out_status, BuriStr *out_body);
extern int64_t buri_rt_host_clock_now_millis(void);
extern void buri_rt_host_clock_sleep_millis(int64_t millis);
extern int64_t buri_rt_host_rand_next_int(int64_t lo, int64_t hi);
extern double buri_rt_host_rand_next_float(void);
extern int32_t buri_rt_host_env_variable(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                         BuriStr *out);
extern void buri_rt_host_env_arguments(BuriList *out);
extern void buri_rt_host_proc_exit_with(int64_t code);

/* rendering — `cli/runtime/fmt.rs`. Every one writes an owned `Str` through an
 * out-pointer, which is §2 rule 2. */
extern void buri_rt_show_f64(double x, BuriStr *out);
extern void buri_rt_show_f32(float x, BuriStr *out);
extern void buri_rt_show_i128(uint64_t lo, uint64_t hi, BuriStr *out);
extern void buri_rt_show_u128(uint64_t lo, uint64_t hi, BuriStr *out);
extern void buri_rt_char_to_str(uint32_t c, BuriStr *out);
extern void buri_rt_show_char(uint32_t c, BuriStr *out);
extern void buri_rt_show_str(const uint8_t *ptr, uint64_t len, BuriStr *out);

/* hashing — `cli/runtime/hash.rs`. 32 bits wide, because `$mix` is. */
extern uint64_t buri_rt_mix(uint64_t h, uint32_t x);
extern uint64_t buri_rt_hash_f64(uint64_t h, double x);
extern uint64_t buri_rt_hash_char(uint64_t h, uint32_t c);
extern uint64_t buri_rt_hash_str(uint64_t h, uint8_t *base, const uint8_t *ptr, uint64_t len);

/* core/str — `cli/runtime/text.rs`. The pure entries answer *views* and take a
 * count on the receiver's base; the `Alloc`-bounded ones answer fresh blocks.
 * An `Option` is `BURI_OK` or `0` with the out-pointer untouched (§2 rule 3). */
extern int32_t buri_rt_str_char_at(uint8_t *base, const uint8_t *ptr, uint64_t len, int64_t index,
                                   uint32_t *out);
extern void buri_rt_str_slice(uint8_t *base, const uint8_t *ptr, uint64_t len, int64_t start,
                              int64_t end, BuriStr *out);
extern void buri_rt_str_trim(uint8_t *base, const uint8_t *ptr, uint64_t len, BuriStr *out);
extern void buri_rt_str_trim_start(uint8_t *base, const uint8_t *ptr, uint64_t len, BuriStr *out);
extern void buri_rt_str_trim_end(uint8_t *base, const uint8_t *ptr, uint64_t len, BuriStr *out);
extern uint8_t buri_rt_str_starts_with(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                       uint8_t *pbase, const uint8_t *pptr, uint64_t plen);
extern uint8_t buri_rt_str_ends_with(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                     uint8_t *sbase, const uint8_t *sptr, uint64_t slen);
extern uint8_t buri_rt_str_contains(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                    uint8_t *nbase, const uint8_t *nptr, uint64_t nlen);
extern int32_t buri_rt_str_index_of(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                    uint8_t *nbase, const uint8_t *nptr, uint64_t nlen,
                                    int64_t *out);
extern int32_t buri_rt_str_split_once(uint8_t *base, const uint8_t *ptr, uint64_t len,
                                      uint8_t *sbase, const uint8_t *sptr, uint64_t slen,
                                      BuriStr *out);
extern int32_t buri_rt_str_compare(uint8_t *base, const uint8_t *ptr, uint64_t len, uint8_t *obase,
                                   const uint8_t *optr, uint64_t olen);
extern uint8_t buri_rt_str_eq(uint8_t *base, const uint8_t *ptr, uint64_t len, uint8_t *obase,
                              const uint8_t *optr, uint64_t olen);
extern uint64_t buri_rt_str_hash(uint8_t *base, const uint8_t *ptr, uint64_t len);
extern int32_t buri_rt_str_to_int(uint8_t *base, const uint8_t *ptr, uint64_t len, int64_t *out);
extern int32_t buri_rt_str_to_float(uint8_t *base, const uint8_t *ptr, uint64_t len, double *out);
extern void buri_rt_str_split(uint8_t *base, const uint8_t *ptr, uint64_t len, uint8_t *sbase,
                              const uint8_t *sptr, uint64_t slen, BuriList *out);
extern void buri_rt_str_split_any(uint8_t *base, const uint8_t *ptr, uint64_t len, uint8_t *sbase,
                                  const uint8_t *sptr, uint64_t slen, BuriList *out);
extern void buri_rt_str_lines(uint8_t *base, const uint8_t *ptr, uint64_t len, BuriList *out);
extern void buri_rt_str_replace(uint8_t *base, const uint8_t *ptr, uint64_t len, uint8_t *nbase,
                                const uint8_t *nptr, uint64_t nlen, uint8_t *rbase,
                                const uint8_t *rptr, uint64_t rlen, BuriStr *out);
extern void buri_rt_str_repeat(uint8_t *base, const uint8_t *ptr, uint64_t len, int64_t times,
                               BuriStr *out);
extern void buri_rt_str_to_upper(uint8_t *base, const uint8_t *ptr, uint64_t len, BuriStr *out);
extern void buri_rt_str_to_lower(uint8_t *base, const uint8_t *ptr, uint64_t len, BuriStr *out);
extern void buri_rt_str_chars(uint8_t *base, const uint8_t *ptr, uint64_t len, BuriList *out);
extern void buri_rt_str_from_chars(const uint8_t *ptr, uint64_t count, BuriStr *out);
extern void buri_rt_str_from_int(int64_t n, BuriStr *out);
extern void buri_rt_str_from_float(double x, BuriStr *out);
extern void buri_rt_str_pad_start(uint8_t *base, const uint8_t *ptr, uint64_t len, int64_t width,
                                  uint32_t fill, BuriStr *out);
extern void buri_rt_str_pad_end(uint8_t *base, const uint8_t *ptr, uint64_t len, int64_t width,
                                uint32_t fill, BuriStr *out);
extern void buri_rt_list_join(const uint8_t *xs, uint64_t count, uint8_t *sbase,
                              const uint8_t *sptr, uint64_t slen, BuriStr *out);

/* core/list — `cli/runtime/list.rs`. `stride` and `retain` come after the Buri
 * arguments and before the out-pointer, uniformly (§2 rule 4). */
extern int32_t buri_rt_list_get(const uint8_t *ptr, uint64_t len, int64_t index, uint64_t stride,
                                void (*retain)(uint8_t *), uint8_t *out);
extern void buri_rt_list_concat(const uint8_t *ptr, uint64_t len, const uint8_t *optr,
                                uint64_t olen, uint64_t stride, void (*retain)(uint8_t *),
                                BuriList *out);
extern void buri_rt_list_push(const uint8_t *ptr, uint64_t len, const uint8_t *item,
                              uint64_t stride, void (*retain)(uint8_t *), BuriList *out);
extern void buri_rt_list_reverse(const uint8_t *ptr, uint64_t len, uint64_t stride,
                                 void (*retain)(uint8_t *), BuriList *out);
extern void buri_rt_list_slice(const uint8_t *ptr, uint64_t len, int64_t start, int64_t end,
                               uint64_t stride, void (*retain)(uint8_t *), BuriList *out);
extern void buri_rt_list_repeat(const uint8_t *item, int64_t times, uint64_t stride,
                                void (*retain)(uint8_t *), BuriList *out);
extern void buri_rt_list_range(int64_t start, int64_t end, BuriList *out);

/* --- Helpers ------------------------------------------------------------ */

/* A borrowed `Str` argument, flattened to the three parameters the contract
 * asks for. `base` is null because a C literal is not on the Buri heap, and
 * §3 says a parameter is borrowed, so nothing here is ever counted. */
#define S(cstr)                                                                                    \
  NULL, (const uint8_t *)(cstr),                                                                   \
      (uint64_t)strlen(cstr) |                                                                     \
          buri_rt_str_ascii_flag((const uint8_t *)(cstr), (uint64_t)strlen(cstr))

static int bytes_of(BuriStr s) { return (int)(s.len & BURI_STR_MASK); }

static const char *chars_of(BuriStr s) { return (const char *)s.ptr; }

static int drops = 0;
static void count_drop(uint8_t *p) {
  (void)p;
  drops++;
}

/* --- Modes -------------------------------------------------------------- */

static int mode_memory(void) {
  uint64_t base_live = buri_rt_live_blocks();

  uint8_t *p = buri_rt_alloc(100);
  uint64_t rc = buri_rt_rc(p);
  uint64_t cap = buri_rt_cap(p);
  int aligned = ((uintptr_t)p % 16) == 0;

  buri_rt_incref(p);
  uint64_t after_incref = buri_rt_rc(p);
  buri_rt_decref(p, count_drop);
  uint64_t after_decref = buri_rt_rc(p);
  buri_rt_decref(p, count_drop);
  int freed = buri_rt_live_blocks() == base_live;

  /* An immortal block survives a decref that would otherwise free it, and is
   * not counted as live, so a leak check does not report every literal. */
  uint8_t *q = buri_rt_alloc(8);
  buri_rt_make_immortal(q);
  buri_rt_decref(q, count_drop);
  int immortal_survives = buri_rt_rc(q) == UINT64_MAX;

  uint8_t *r = buri_rt_alloc(100);
  r = buri_rt_realloc(r, 200);
  int realloc_keeps_rc = buri_rt_rc(r) == 1;
  uint64_t realloc_cap = buri_rt_cap(r);
  buri_rt_decref(r, NULL);

  /* Zeroed allocation, so the flag on that path is exercised too. */
  uint8_t *z = buri_rt_alloc_zeroed(32);
  for (int i = 0; i < 32; i++) {
    if (z[i] != 0) {
      fprintf(stderr, "alloc_zeroed left byte %d set\n", i);
      return 1;
    }
  }
  buri_rt_free(z);

  BuriHeapStats stats;
  buri_rt_heap_stats(&stats);
  uint64_t leaked = stats.live_blocks - base_live;

  if (buri_rt_host_alloc_allocate(4096) != 4096) {
    fprintf(stderr, "Alloc::allocate did not report its own charge\n");
    return 1;
  }

  printf("rc=%llu cap=%llu aligned=%d after-incref=%llu after-decref=%llu "
         "dropped=%d freed=%d immortal-survives=%d realloc-keeps-rc=%d "
         "realloc-cap=%llu leaked=%llu\n",
         (unsigned long long)rc, (unsigned long long)cap, aligned,
         (unsigned long long)after_incref, (unsigned long long)after_decref, drops, freed,
         immortal_survives, realloc_keeps_rc, (unsigned long long)realloc_cap,
         (unsigned long long)leaked);
  return 0;
}

static int mode_values(void) {
  BuriStr ascii, utf8, empty;
  buri_rt_str_from_utf8((const uint8_t *)"hello", 5, &ascii);
  /* "héllo": six bytes, five scalars, so the flag is clear and `str.len()`
   * costs a scan (VALUE-MODEL.md §3.1). */
  buri_rt_str_from_utf8((const uint8_t *)"h\xc3\xa9llo", 6, &utf8);
  buri_rt_str_empty(&empty);

  BuriList list;
  uint8_t *elements = buri_rt_list_new(4, 8, &list);

  uint64_t q[2], r[2];
  buri_rt_i128_divmod(1000000, 0, 7, 0, 1, q, r);
  long long sq = (long long)q[0];
  long long sr = (long long)r[0];

  unsigned __int128 neg = (unsigned __int128)(__int128)(-1000000);
  uint64_t nq[2], nr[2];
  buri_rt_i128_divmod((uint64_t)neg, (uint64_t)(neg >> 64), 7, 0, 1, nq, nr);
  long long snq = (long long)nq[0];
  long long snr = (long long)nr[0];

  /* Unsigned, and genuinely 128-bit: 2^70 / 3. */
  unsigned __int128 big = (unsigned __int128)1 << 70;
  uint64_t uq[2], ur[2];
  buri_rt_i128_divmod((uint64_t)big, (uint64_t)(big >> 64), 3, 0, 0, uq, ur);

  printf("ascii bytes=%d flag=%d scalars=%llu "
         "utf8 bytes=%d flag=%d scalars=%llu "
         "empty bytes=%d flag=%d "
         "list len=%llu cap=%llu "
         "divmod %lld %lld %lld %lld "
         "udivmod-high %llu %llu %llu\n",
         bytes_of(ascii), (ascii.len & BURI_STR_ASCII) != 0,
         (unsigned long long)buri_rt_str_scalar_len(ascii.ptr, ascii.len), bytes_of(utf8),
         (utf8.len & BURI_STR_ASCII) != 0,
         (unsigned long long)buri_rt_str_scalar_len(utf8.ptr, utf8.len), bytes_of(empty),
         (empty.len & BURI_STR_ASCII) != 0, (unsigned long long)list.len,
         (unsigned long long)buri_rt_cap(elements), sq, sr, snq, snr, (unsigned long long)uq[0],
         (unsigned long long)uq[1], (unsigned long long)ur[0]);
  return 0;
}

static int mode_streams(void) {
  buri_rt_host_stdout_print(S("one "));
  buri_rt_host_stdout_println(S("two"));
  buri_rt_host_stderr_eprintln(S("err one"));
  /* Flushes the buffered text first, so the two orderings a program can see
   * are the one it wrote. */
  buri_rt_host_stdout_write_bytes((const uint8_t *)"three\n", 6);
  buri_rt_host_stdout_print(S("four"));
  buri_rt_host_stderr_eprint(S("err two"));
  buri_rt_flush();
  return 0;
}

static int mode_fs(const char *dir) {
  char path[4096], utf8path[4096], missing[4096], notdir[4096];
  snprintf(path, sizeof path, "%s/f.txt", dir);
  snprintf(utf8path, sizeof utf8path, "%s/u.txt", dir);
  snprintf(missing, sizeof missing, "%s/absent.txt", dir);
  snprintf(notdir, sizeof notdir, "%s/f.txt/under-a-file", dir);

  BuriStr err, ok, utf8;
  int32_t wrote = buri_rt_host_fs_write_file(S(path), S("hello"), &err);
  int32_t wrote_utf8 = buri_rt_host_fs_write_file(S(utf8path), S("h\xc3\xa9llo"), &err);
  if (wrote_utf8 != BURI_OK) {
    fprintf(stderr, "writing the UTF-8 fixture failed with %d\n", wrote_utf8);
    return 1;
  }

  uint8_t exists = buri_rt_host_fs_file_exists(S(path));
  int32_t read = buri_rt_host_fs_read_file(S(path), &ok, &err);
  int32_t read_utf8 = buri_rt_host_fs_read_file(S(utf8path), &utf8, &err);
  if (read != BURI_OK || read_utf8 != BURI_OK) {
    fprintf(stderr, "reading back failed with %d / %d\n", read, read_utf8);
    return 1;
  }

  BuriList entries;
  int32_t listed = buri_rt_host_fs_read_dir(S(dir), &entries, &err);
  if (listed != BURI_OK) {
    fprintf(stderr, "readDir failed with %d\n", listed);
    return 1;
  }

  BuriStr ignored;
  int32_t not_found = buri_rt_host_fs_read_file(S(missing), &ignored, &err);
  int32_t not_a_dir = buri_rt_host_fs_read_file(S(notdir), &ignored, &err);
  uint8_t exists_missing = buri_rt_host_fs_file_exists(S(missing));

  printf("write=%s exists=%d read=%.*s utf8=%.*s readdir=%llu missing=%d notdir=%d "
         "exists-missing=%d\n",
         wrote == BURI_OK ? "ok" : "err", exists, bytes_of(ok), chars_of(ok), bytes_of(utf8),
         chars_of(utf8), (unsigned long long)entries.len, not_found, not_a_dir, exists_missing);
  return 0;
}

static int mode_env(void) {
  BuriStr value;
  int32_t present = buri_rt_host_env_variable(S("BURI_RT_TEST"), &value);
  BuriStr absent;
  int32_t missing = buri_rt_host_env_variable(S("BURI_RT_DEFINITELY_NOT_SET"), &absent);

  BuriList args;
  buri_rt_host_env_arguments(&args);

  printf("var=%.*s missing=%s args=%llu:", present == BURI_OK ? bytes_of(value) : 0,
         present == BURI_OK ? chars_of(value) : "", missing == BURI_OK ? "some" : "none",
         (unsigned long long)args.len);
  for (uint64_t i = 0; i < args.len; i++) {
    BuriStr arg;
    memcpy(&arg, args.ptr + i * sizeof(BuriStr), sizeof(BuriStr));
    printf("%s%.*s", i == 0 ? "" : ",", bytes_of(arg), chars_of(arg));
  }
  printf("\n");
  return 0;
}

static int mode_clock_rand(void) {
  int64_t start = buri_rt_host_clock_now_millis();
  /* 2020-01-01T00:00:00Z, which any working clock is past. */
  int after_2020 = start > 1577836800000LL;
  buri_rt_host_clock_sleep_millis(5);
  int slept = buri_rt_host_clock_now_millis() - start >= 1;

  int in_range = 0;
  int varies = 0;
  int64_t first = buri_rt_host_rand_next_int(5, 10);
  for (int i = 0; i < 1000; i++) {
    int64_t v = buri_rt_host_rand_next_int(5, 10);
    if (v >= 5 && v < 10) {
      in_range++;
    }
    if (v != first) {
      varies = 1;
    }
  }

  int floats_in_range = 0;
  for (int i = 0; i < 1000; i++) {
    double f = buri_rt_host_rand_next_float();
    if (f >= 0.0 && f < 1.0) {
      floats_in_range++;
    }
  }

  printf("now-after-2020=%d slept=%d int-in-range=%d float-in-range=%d varies=%d\n", after_2020,
         slept, in_range, floats_in_range, varies);
  return 0;
}

static int mode_stdin_lines(void) {
  for (;;) {
    BuriStr line;
    if (buri_rt_host_stdin_read_line(&line) != BURI_OK) {
      printf("end\n");
      return 0;
    }
    printf("line=%.*s ", bytes_of(line), chars_of(line));
  }
}

static int mode_stdin_bytes(void) {
  BuriList first, second, third;
  int32_t a = buri_rt_host_stdin_read_bytes(4, &first);
  int32_t b = buri_rt_host_stdin_read_bytes(2, &second);
  int32_t c = buri_rt_host_stdin_read_bytes(2, &third);
  printf("got=%llu:%.*s ", a == BURI_OK ? (unsigned long long)first.len : 0,
         a == BURI_OK ? (int)first.len : 0, a == BURI_OK ? (const char *)first.ptr : "");
  printf("then=%llu:%.*s ", b == BURI_OK ? (unsigned long long)second.len : 0,
         b == BURI_OK ? (int)second.len : 0, b == BURI_OK ? (const char *)second.ptr : "");
  printf("then=%s\n", c == BURI_OK ? "some" : "none");
  return 0;
}

static int mode_net(const char *url) {
  int64_t status = 0;
  BuriStr body;
  int32_t result = buri_rt_host_net_fetch(S("GET"), S(url), S(""), &status, &body);
  if (result == BURI_OK) {
    printf("status=%lld body=%.*s\n", (long long)status, bytes_of(body), chars_of(body));
  } else {
    printf("err=%d message=%.*s\n", result, bytes_of(body), chars_of(body));
  }
  return 0;
}

/* --- Entry -------------------------------------------------------------- */

/* Rendering and hashing: the two places VALUE-MODEL.md §12 asks for the *same
 * bytes* as JavaScript rather than for a defensible answer. The float corpus
 * lives in `cli/tests/native/float_parity.rs`, which checks four million values
 * against a JavaScript engine; this checks that the symbols exist, have the
 * arity the contract states, and answer the handful of cases a reader would
 * look up. */
static int mode_render(void) {
  BuriStr s;
  buri_rt_show_f64(0.1, &s);
  printf("f64 %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_f64(1.0, &s);
  printf("int %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_f64(-0.0, &s);
  printf("negzero %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_f64(1e21, &s);
  printf("big %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_f64(5e-324, &s);
  printf("denormal %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_f32(0.1f, &s);
  printf("f32 %.*s\n", bytes_of(s), chars_of(s));
  /* -1 as a 128-bit value: all ones in both halves. */
  buri_rt_show_i128(~0ull, ~0ull, &s);
  printf("i128 %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_u128(0, 1, &s);
  printf("u128 %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_char('a', &s);
  printf("char %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_char_to_str('a', &s);
  printf("charstr %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_show_str((const uint8_t *)"a\"b\n", 4, &s);
  printf("quoted %.*s\n", bytes_of(s), chars_of(s));
  buri_rt_str_from_int(-42, &s);
  printf("fromint %.*s\n", bytes_of(s), chars_of(s));

  /* `$hash(7)`, `$hash("ab")` and `$hash('a')` under the JavaScript runtime. */
  printf("hash-int %llu\n", (unsigned long long)buri_rt_mix(0x811c9dc5ull, 7));
  printf("hash-str %llu\n", (unsigned long long)buri_rt_str_hash(S("ab")));
  printf("hash-char %llu\n", (unsigned long long)buri_rt_hash_char(0x811c9dc5ull, 'a'));
  printf("hash-nan %llu\n", (unsigned long long)buri_rt_hash_f64(0x811c9dc5ull, 0.0 / 0.0));
  return 0;
}

/* `core/str`, at the boundary. The point of each line is a rule that would be
 * easy to get subtly wrong: scalar indices rather than byte offsets, views
 * rather than copies, JavaScript's whitespace set, and the `Option` shape. */
static int mode_text(void) {
  BuriStr out;
  uint32_t c = 0;
  int64_t n = 0;
  double d = 0;

  printf("len %llu\n", (unsigned long long)buri_rt_str_scalar_len((const uint8_t *)"aé漢", 6));

  /* Scalar 1 of "aé漢" starts at byte 1 and scalar 2 at byte 3. */
  buri_rt_str_slice(NULL, (const uint8_t *)"aé漢", 6, 1, 2, &out);
  printf("slice %.*s\n", bytes_of(out), chars_of(out));

  printf("charat %d ", buri_rt_str_char_at(S("aé漢"), 2, &c));
  printf("%u\n", c);
  printf("charat-past %d\n", buri_rt_str_char_at(S("ab"), 9, &c));

  /* U+FEFF is JavaScript whitespace and is not Unicode `White_Space`. */
  buri_rt_str_trim(NULL, (const uint8_t *)"\xef\xbb\xbf x \xef\xbb\xbf", 9, &out);
  printf("trim [%.*s]\n", bytes_of(out), chars_of(out));

  printf("starts %d ends %d contains %d\n", buri_rt_str_starts_with(S("hello"), S("he")),
         buri_rt_str_ends_with(S("hello"), S("lo")), buri_rt_str_contains(S("hello"), S("ell")));

  printf("indexof %d ", buri_rt_str_index_of(S("aé漢b"), S("b"), &n));
  printf("%lld\n", (long long)n);
  printf("indexof-none %d\n", buri_rt_str_index_of(S("abc"), S("z"), &n));

  /* The empty first half is the case a null `ptr` would misreport as `.None`,
   * which is why the runtime's empty string has an address. */
  BuriStr pair[2];
  printf("splitonce %d [%.*s][%.*s]\n", buri_rt_str_split_once(S(",b"), S(","), pair),
         bytes_of(pair[0]), chars_of(pair[0]), bytes_of(pair[1]), chars_of(pair[1]));
  printf("splitonce-none %d\n", buri_rt_str_split_once(S("ab"), S(","), pair));

  /* UTF-16 code-unit order: a surrogate pair sorts below U+FFFD. */
  printf("compare %d %d %d\n", buri_rt_str_compare(S("a"), S("b")),
         buri_rt_str_compare(S("a"), S("a")),
         buri_rt_str_compare(S("\xef\xbf\xbd"), S("\xf0\x90\x80\x80")));
  printf("eq %d %d\n", buri_rt_str_eq(S("a"), S("a")), buri_rt_str_eq(S("a"), S("b")));

  printf("toint %d ", buri_rt_str_to_int(S(" 42 "), &n));
  printf("%lld\n", (long long)n);
  /* Past `I64`, which is the only thing either backend refuses now. */
  printf("toint-wide %d ", buri_rt_str_to_int(S("9223372036854775807"), &n));
  printf("%lld\n", (long long)n);
  printf("toint-past %d\n", buri_rt_str_to_int(S("9223372036854775808"), &n));
  printf("tofloat %d %g\n", buri_rt_str_to_float(S("1.5e1"), &d), d);
  printf("tofloat-bad %d\n", buri_rt_str_to_float(S("Infinity"), &d));

  BuriList parts;
  buri_rt_str_split(S("a,b,c"), S(","), &parts);
  printf("split %llu", (unsigned long long)parts.len);
  for (uint64_t i = 0; i < parts.len; i++) {
    BuriStr *e = (BuriStr *)(parts.ptr + i * sizeof(BuriStr));
    printf(" %.*s", bytes_of(*e), chars_of(*e));
  }
  printf("\n");
  buri_rt_list_join(parts.ptr, parts.len, S("-"), &out);
  printf("join %.*s\n", bytes_of(out), chars_of(out));

  buri_rt_str_lines(S("a\nb\n"), &parts);
  printf("lines %llu\n", (unsigned long long)parts.len);
  buri_rt_str_split_any(S("a b,c"), S(" ,"), &parts);
  printf("splitany %llu\n", (unsigned long long)parts.len);

  buri_rt_str_replace(S("banana"), S("na"), S("NA"), &out);
  printf("replace %.*s\n", bytes_of(out), chars_of(out));
  buri_rt_str_repeat(S("ab"), 3, &out);
  printf("repeat %.*s\n", bytes_of(out), chars_of(out));
  buri_rt_str_repeat(S("ab"), -1, &out);
  printf("repeat-none [%.*s]\n", bytes_of(out), chars_of(out));
  buri_rt_str_to_upper(S("aé"), &out);
  printf("upper %.*s\n", bytes_of(out), chars_of(out));
  buri_rt_str_to_lower(S("AÉ"), &out);
  printf("lower %.*s\n", bytes_of(out), chars_of(out));
  buri_rt_str_pad_start(S("7"), 3, '0', &out);
  printf("padstart %.*s\n", bytes_of(out), chars_of(out));
  buri_rt_str_pad_end(S("7"), 3, '0', &out);
  printf("padend %.*s\n", bytes_of(out), chars_of(out));

  buri_rt_str_chars(S("aé"), &parts);
  printf("chars %llu %u %u\n", (unsigned long long)parts.len, *(uint32_t *)parts.ptr,
         *(uint32_t *)(parts.ptr + 4));
  buri_rt_str_from_chars(parts.ptr, parts.len, &out);
  printf("fromchars %.*s\n", bytes_of(out), chars_of(out));
  return 0;
}

/* `core/list`'s block-copying half, including the retain glue: a copied `[Str]`
 * has to take a count on every string block it now names, or dropping either
 * list frees what the other still holds. */
static int retains = 0;
static void count_retain(uint8_t *p) {
  (void)p;
  retains++;
}

static int mode_list(void) {
  int64_t src[4] = {10, 20, 30, 40};
  BuriList out;
  int64_t got = 0;

  printf("get %d ", buri_rt_list_get((const uint8_t *)src, 4, 2, 8, NULL, (uint8_t *)&got));
  printf("%lld\n", (long long)got);
  printf("get-past %d\n", buri_rt_list_get((const uint8_t *)src, 4, 4, 8, NULL, (uint8_t *)&got));
  printf("get-negative %d\n",
         buri_rt_list_get((const uint8_t *)src, 4, -1, 8, NULL, (uint8_t *)&got));

  buri_rt_list_concat((const uint8_t *)src, 2, (const uint8_t *)(src + 2), 2, 8, NULL, &out);
  printf("concat %llu %lld %lld\n", (unsigned long long)out.len, (long long)((int64_t *)out.ptr)[0],
         (long long)((int64_t *)out.ptr)[3]);
  buri_rt_free(out.ptr);

  int64_t item = 99;
  buri_rt_list_push((const uint8_t *)src, 4, (const uint8_t *)&item, 8, NULL, &out);
  printf("push %llu %lld\n", (unsigned long long)out.len, (long long)((int64_t *)out.ptr)[4]);
  buri_rt_free(out.ptr);

  buri_rt_list_reverse((const uint8_t *)src, 4, 8, NULL, &out);
  printf("reverse %lld %lld\n", (long long)((int64_t *)out.ptr)[0],
         (long long)((int64_t *)out.ptr)[3]);
  buri_rt_free(out.ptr);

  /* Clamped at both ends rather than aborting: `$list_slice` is
   * `xs.slice(a, b)`, and that is what it does. */
  buri_rt_list_slice((const uint8_t *)src, 4, -3, 99, 8, NULL, &out);
  printf("slice %llu\n", (unsigned long long)out.len);
  buri_rt_free(out.ptr);

  buri_rt_list_repeat((const uint8_t *)&item, 3, 8, count_retain, &out);
  printf("repeat %llu %d\n", (unsigned long long)out.len, retains);
  buri_rt_free(out.ptr);

  buri_rt_list_range(2, 5, &out);
  printf("range %llu %lld\n", (unsigned long long)out.len, (long long)((int64_t *)out.ptr)[2]);
  buri_rt_free(out.ptr);
  buri_rt_list_range(5, 2, &out);
  printf("range-empty %llu %d\n", (unsigned long long)out.len, out.ptr == NULL);
  return 0;
}

int main(int argc, char **argv) {
  buri_rt_argv_init(argc, (const uint8_t **)argv);
  const char *mode = argc > 1 ? argv[1] : "";

  if (strcmp(mode, "memory") == 0) {
    return mode_memory();
  }
  if (strcmp(mode, "render") == 0) {
    return mode_render();
  }
  if (strcmp(mode, "text") == 0) {
    return mode_text();
  }
  if (strcmp(mode, "list") == 0) {
    return mode_list();
  }
  if (strcmp(mode, "values") == 0) {
    return mode_values();
  }
  if (strcmp(mode, "streams") == 0) {
    return mode_streams();
  }
  if (strcmp(mode, "fs") == 0 && argc > 2) {
    return mode_fs(argv[2]);
  }
  if (strcmp(mode, "env") == 0) {
    return mode_env();
  }
  if (strcmp(mode, "clock-rand") == 0) {
    return mode_clock_rand();
  }
  if (strcmp(mode, "stdin-lines") == 0) {
    return mode_stdin_lines();
  }
  if (strcmp(mode, "stdin-bytes") == 0) {
    return mode_stdin_bytes();
  }
  if (strcmp(mode, "net") == 0 && argc > 2) {
    return mode_net(argv[2]);
  }
  if (strcmp(mode, "exit") == 0) {
    buri_rt_host_stdout_println(S("buffered, and flushed by the exit"));
    buri_rt_host_proc_exit_with(7);
  }
  if (strcmp(mode, "abort-div") == 0) {
    buri_rt_abort_div_zero();
  }
  if (strcmp(mode, "abort-shift") == 0) {
    buri_rt_abort_shift();
  }
  if (strcmp(mode, "abort-random") == 0) {
    buri_rt_host_rand_next_int(3, 3);
  }
  if (strcmp(mode, "abort-bounds") == 0) {
    buri_rt_abort_bounds(7, 3);
  }
  if (strcmp(mode, "abort-budget") == 0) {
    /* Under the budget passes through; over it aborts. */
    if (buri_rt_alloc_budget_check(512, 0, 1024) != 512) {
      fprintf(stderr, "a request inside the budget was refused\n");
      return 1;
    }
    buri_rt_alloc_budget_check(4096, 0, 1024);
  }
  if (strcmp(mode, "abort-after-print") == 0) {
    buri_rt_host_stdout_println(S("printed before the abort"));
    buri_rt_abort_div_zero();
  }

  fprintf(stderr, "unknown mode: %s\n", mode);
  return 2;
}
