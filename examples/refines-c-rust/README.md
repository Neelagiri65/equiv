# A C function and its Rust rewrite, compared with `refines`

`fnv.c` and `fnv.rs` are the same 32-bit FNV-1a hash over `n` bytes at `p`,
written once in C and once in Rust. Each is compiled to WebAssembly
(`c.wasm` by clang, `r.wasm` by rustc). The contract `r.eqct` says the C
function must behave like the Rust one on every byte string of up to 256
bytes, and pins the Rust artifact by its sha256.

```
equiv check c.wasm r.eqct --reference r.wasm      # exit 0
equiv check c.wasm r_bug.eqct --reference r_bug.wasm  # exit 1
```

Output of the first, with equiv 0.30.0:

```
verdict: TestedN { n_cases: 64, seed: 7309752766010753025 }
note: the target and the reference returned the same result on all 64 inputs; 325 more were drawn but did not satisfy the precondition and 0 more could not have it evaluated
```

`fnv_bug.rs` (built as `r_bug.wasm`) multiplies by 16777618 instead of the
FNV prime 16777619. The check stops at the first input that tells them apart:

```
verdict: Counterexample { args: [0, 1], trap: false }
detail: on input [0, 1] the target returned 269250160 and the reference returned 2398081184
```

That is a test on 64 generated inputs, not a proof.

## Why this one works, and what does not yet

It fits what `equiv check` can do today (see "WASM contracts" in
[`docs/scope.md`](../../docs/scope.md)):

- **No imports.** The Rust side is `#![no_std]` and the C side is built with
  `-nostdlib`. A module that imports WASI or libc functions cannot be run.
- **The same memory size.** Both sides are run on one memory image, so
  `build.sh` links the C module with the 16 pages rustc gives the Rust one.
- **No static data and no stack use.** Memory is filled with random bytes
  before each call, which would overwrite lookup tables or constants a
  compiled module keeps in memory, and stack writes would break the frame
  clause. FNV-1a keeps its two constants in the code.
- **Integer arguments.** The byte string is passed as a pointer and a length.

## Rebuilding

`./build.sh` rebuilds the three artifacts and rewrites `r.eqct` and
`r_bug.eqct` with their hashes. It needs clang with the wasm32 target and
`rustup target add wasm32-wasip1`. The checked-in files were built with
rustc 1.95.0 and Apple clang 21.0.0; other versions give other bytes and
other hashes, which is why the contracts are regenerated.
