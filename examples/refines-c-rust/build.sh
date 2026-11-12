#!/usr/bin/env bash
# Rebuilds the three artifacts and writes the two contracts with their hashes.
# Needs clang with the wasm32 target and rustc with the wasm32-wasip1 target
# (`rustup target add wasm32-wasip1`); Rust's toolchain ships the linker.
# Other compiler versions give other bytes, so the contracts are regenerated.
set -euo pipefail
cd "$(dirname "$0")"
WLD="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/gcc-ld/wasm-ld"
for src in fnv fnv_bug; do
  out=r; [[ $src == fnv_bug ]] && out=r_bug
  rustc --target wasm32-wasip1 --crate-type cdylib -O -C panic=abort "$src.rs" -o "$out.wasm"
done
# The C module gets the same memory size as the Rust one: both sides are run
# on one memory image, so the sizes must match.
bytes=1048576  # 16 pages, the size rustc gives the Rust module (check: wasm-objdump -x r.wasm)
clang --target=wasm32 -O2 -nostdlib -c fnv.c -o fnv.o
"$WLD" --no-entry --export=fnv1a --initial-memory=$bytes -o c.wasm fnv.o
rm -f fnv.o
for ref in r r_bug; do
  hash=$(shasum -a 256 "$ref.wasm" | cut -d' ' -f1)
  cat > "$ref.eqct" <<EQCT
; fnv1a compiled from C (target) must behave like fnv1a compiled from Rust
; (reference) on every byte string of up to 256 bytes in memory.
(contract eqc/0
  (target (func "fnv1a" (param \$p i32) (param \$n i32) (result i32)))
  (pre (and (readable (range \$p \$n)) (le_u \$n (c32 256))))
  (post (c32 1))
  (frame)
  (refines (artifact sha256:$hash) "fnv1a" (over (range \$p \$n))))
EQCT
done
echo "built c.wasm, r.wasm, r_bug.wasm; wrote r.eqct and r_bug.eqct"
