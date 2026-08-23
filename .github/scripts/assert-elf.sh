#!/usr/bin/env bash
#
# The ELF properties of a linked Linux artifact, asked of a real linked image
# rather than reasoned about.
#
#   * `-Wl,--gc-sections` does not collect anything live. `build/link.rs`
#      passes it on every Linux link. The `.bss` block holding the Buri stack is
#      kept only because the shim relocates against it, and that is worth seeing.
#   * No executable stack. `stencil/elf.rs` writes an empty `.note.GNU-stack`
#      into every unit object, and it is the ABSENCE of that section that makes
#      a Linux linker mark the stack executable — so the property is invisible
#      in the source and visible only here.
#
# The binary this is pointed at is the one
# `cli/tests/native/stencil.rs::the_products_own_link_produces_a_program_that_runs`
# leaves behind: that test links through `build/link.rs` itself, with the
# product's own flags, so what is inspected is what `buri build` would ship.
#
# Usage: bash .github/scripts/assert-elf.sh <binary>

set -euo pipefail

bin=${1:-}
if [ -z "$bin" ] || [ ! -f "$bin" ]; then
  echo "::error::assert-elf.sh needs a path to a linked binary (got '${bin}')"
  exit 1
fi

# llvm-readelf first: it is on PATH wherever `cross_tools` is satisfied, and it
# is the one this was written against. GNU readelf prints the same columns.
if command -v llvm-readelf >/dev/null 2>&1; then
  readelf_cmd=llvm-readelf
elif command -v readelf >/dev/null 2>&1; then
  readelf_cmd=readelf
else
  echo "::error::neither llvm-readelf nor readelf is on PATH"
  exit 1
fi

status=0
fail() { echo "::error::$*"; status=1; }

echo "== $bin"

# ------------------------------------------------------- no exec stack ----
# One PT_GNU_STACK header, and its flags must not carry E. A missing header is
# also a failure: on Linux the kernel then falls back to the ABI default, which
# for an executable with no PT_GNU_STACK is an executable stack.
stack_line=$("$readelf_cmd" -lW "$bin" | grep -w GNU_STACK || true)
if [ -z "$stack_line" ]; then
  fail "the image has no PT_GNU_STACK header, which is exactly the state an absent .note.GNU-stack produces"
else
  # Columns: Type Offset VirtAddr PhysAddr FileSiz MemSiz Flg Align.
  flags=$(printf '%s\n' "$stack_line" | awk '{print $(NF-1)}')
  case "$flags" in
    *[!RWE]*|"") fail "could not read the GNU_STACK flags out of: $stack_line" ;;
    *E*)         fail "the stack is EXECUTABLE (flags $flags) — .note.GNU-stack did not reach the linker" ;;
    *)           echo "GNU_STACK flags: $flags" ;;
  esac
fi

# The other half of the stack check: a linker that had to guess says so, loudly, and a
# warning that nobody reads is a warning that is not a check. The caller greps
# its own link log; this only reports what the image says.

# -------------------------------------------------------- --gc-sections ----
# The Buri stack survived `--gc-sections`. `asm::STACK_SYMBOL`.
sym_line=$("$readelf_cmd" -sW "$bin" | grep -F 'buri$stencil$stack' | head -1 || true)
if [ -z "$sym_line" ]; then
  fail "buri\$stencil\$stack is not in the linked image — --gc-sections collected the block the stack guard depends on, or the shim stopped naming it"
else
  # Columns: Num: Value Size Type Bind Vis Ndx Name.
  #
  # The symbol's own `st_size` is 0 and that is correct rather than suspicious:
  # `elf.rs` emits the stack as the start of a zero-fill section and not as a
  # sized object, which is the same "one block and one symbol" decision §8.1
  # explains for Mach-O. So the reservation is asserted on the SECTION below,
  # and what is asserted here is only that the symbol is defined — an
  # undefined one is what a collected block looks like.
  sym_ndx=$(printf '%s\n' "$sym_line" | awk '{print $7}')
  echo "buri\$stencil\$stack: section index $sym_ndx"
  if [ "$sym_ndx" = "UND" ]; then
    fail "buri\$stencil\$stack is undefined in the image"
  fi
fi

# And the section it lives in is still zero-fill and still 65 MiB, which is what
# says the linker kept the reservation rather than a stub of it. 64 MiB usable
# plus a 1 MiB guard (asm.rs::STACK_BYTES); asserted as a floor rather than as
# the constant, because the constant is a unit test's business (asm.rs::tests)
# and what CI is asking is whether the block is still a block.
# `[[:space:]]\.bss[[:space:]]` and not `\.bss`, so that a `.rela.bss` or any
# other section whose name merely ends in it cannot answer for the real one.
bss_line=$("$readelf_cmd" -SW "$bin" | grep -E '[[:space:]]\.bss[[:space:]]' | head -1 || true)
if [ -z "$bss_line" ]; then
  fail "the image has no .bss at all"
else
  echo "$bss_line" | sed 's/^ *//'
  case "$bss_line" in
    *NOBITS*) ;;
    *) fail ".bss is not NOBITS, so the 65 MiB reservation is bytes on disk" ;;
  esac
  # `[ N] name type addr off size es flg lk inf al`, with the index stripped so
  # that the bracket does not become one field or two depending on the width.
  bss_size_hex=$(printf '%s\n' "$bss_line" | sed 's/^ *\[[ 0-9]*\] *//' | awk '{print $5}')
  if printf '%s' "$bss_size_hex" | grep -qE '^[0-9a-fA-F]+$'; then
    bss_size=$((16#$bss_size_hex))
    echo ".bss: $bss_size bytes"
    if [ "$bss_size" -lt 67108864 ]; then
      fail ".bss is $bss_size bytes, under the 64 MiB the Buri stack reserves — --gc-sections took the block"
    fi
  else
    fail "could not read the .bss size out of: $bss_line"
  fi
fi

exit "$status"
