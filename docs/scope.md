# Scope, inputs and limits

Moved from the README. The README has the short version.

## What is checked

`equiv` checks behavioural equivalence of a function against a reference, on
deterministically generated inputs. Generation has four deterministic layers:

1. **Inputs from the project's own code** (`review-pr`, or `review
   --inputs-from`): the literal arguments of every call to the function in the
   repository's tracked Python files, tests included, and pytest `parametrize`
   rows. Files are parsed with Python's `ast`, never run. Only values of exactly
   the declared types are kept, at most 500 calls per function. They are bound
   into the receipt: the provenance line counts them, and the spec hash covers
   them, so a receipt made with them is reproduced only by a run over the same
   files. `review-pr --no-harvest` turns this off. Which files are read
   depends on git: under a directory, only the `*.py` files git tracks are
   scanned (every `*.py` file below it only outside a git checkout). A test
   file that is untracked or ignored contributes nothing, and the same review
   over a checkout with different tracked files harvests different inputs and
   so has a different receipt id. Stacked `parametrize` decorators are read as
   their product, cut to its first 10,000 rows per test function.
2. **Boundary corners per type**: empty, sign and off-by-one values, uppercase,
   whitespace and control characters, strings longer than 8, lists longer than
   6, the float special values, and each string corner placed between two
   ordinary strings in a list.
3. **The source's own literals**: a constant `== 777` in the code is tested
   directly, with its neighbours. A number is read without its sign, so both
   signs are tried (777 and -777, each with its neighbours). A float argument
   gets every float literal (`1.5`, `.5`, `5.`, `1e-9`) and every int literal
   float64 holds exactly, each with both signs and the next float below and
   above. A string literal is tested by value, decoded as Python decodes it
   (octal, `\x`, `\u`, `\U`, `\a \b \f \v`, raw prefixes in any case and
   order); a literal that cannot be decoded exactly here (`\N{...}`, a lone
   surrogate escape) and a bytes literal are not offered. For a list or dict
   argument, the int literals (both signs, with neighbours) are placed inside
   lists (`[100]`, `[99, 100]`) and under dict keys (`"a"` and the first two
   string literals), and each string literal inside a three-element list. The
   scan reads tokens and does not evaluate: a threshold that is computed
   (`ord("d")`, `0.1 + 0.2`) is not aimed at.
4. **Seeded random cases.** Random strings come in three shapes: lowercase
   a-z (half), a small mixed alphabet with capitals, digits, punctuation,
   whitespace and control characters (a quarter), and numeric text with
   optional padding, sign and suffix (a quarter). A quarter of random string
   lists also hold a near duplicate of one element (the same text, its upper-
   or lowercase form, or with a leading space). Random list elements and
   dict values stay in -20..20; when a signature has a list or dict argument,
   one more seeded case is added for every eight random cases, with elements
   drawn from the source's literals and their neighbours and from wider
   ranges (up to the envelope below).

This is still bounded
testing, not exhaustive verification: a pass means no divergence was found on the
generated inputs. Two limits are deliberate. Random integers stay inside a magnitude envelope
(|n| up to 1,000,000) so a linear reference cannot hang; fixed large-magnitude
corners (the 32-bit wrap points, the float64 exactness cliff at 2^53 and the
64-bit limits, each with its negative) are tried after every other input, and
a version that cannot finish one of them inside the step budget has that input
recorded in the receipt as not judged rather than failing the review. The same
corners are tried as the only element of a list and as a dict value. Corners and
literals are tried one argument at a time, with every other argument at its
default (0, "", an empty list or dict, 1.0, False). On top of that, for every
ordered pair of non-bool arguments, the corners and literals of the first are
tried again with the second at a few fixed non-default values (ints 1 and 2,
strings "a" and "ab", one or two fixed values for the other types, plus the first
four int literals up to 4,096 or string literals of the source), every bool
argument is run at True (each alone, and all together), same-type string and
numeric pairs get a fixed set of related values, and the random cases draw every
argument at once. The cross cases are capped at 480 per review, shared equally
between the pairs, so with many arguments only the first corners of each pair are
crossed. This is not a cartesian product: a divergence that needs two specific
values at once, where the second is not one of those fixed values, a literal or
a random draw, may be missed. Methods, async and nested functions are listed as not checked, never judged. A function that did not change is not re-checked when something it calls did. It does not check intent, architecture, security. It
cannot judge new functionality that has no reference to compare against. A
passing result means behaviour was preserved on the tested inputs. It does not
mean the change is correct. Supported input types in this version are `int`,
`str`, `list[int]`, `list[str]`, `float`, `dict`, `bool`. Dict, list and tuple results are compared
structurally: a dict is a map (key order ignored); a tuple is compared element
by element and is never equal to a list with the same items; a namedtuple is
compared by class name, field names and values (so `(1, 2)` and `Point(x=1, y=2)`
differ). Anything else (a set, a custom object, a subclass of a namedtuple,
non-string keys) is refused. A subclass of `int`, `float`, `str`, `list` or
`dict` (an enum, a `Counter`, an `OrderedDict`, a numpy scalar) is compared by
its class name and its content: it agrees with the same class on the other side
and never with the plain type. A value that cannot be read (for example a list
that contains itself) is refused.

Each side runs in its own Python process, the reference first. The two never
share arguments, loaded modules or global state, and what the function prints is
ignored. Only the returned value or the raised error class is compared: a change
a function makes to its own arguments is not compared, and neither is the error
message. Each side must report an outcome for every input; a run that ends early
(the function exits the interpreter, or the process dies) is could not check,
never a pass. Each side checks that the file it loads is the file whose hash
is in the receipt; a file that changed after the review started is could not
check. This is not a sandbox: the reviewed code runs as you, with your files
and network. Imports resolve from the machine's Python environment
(`PYTHONPATH` included), and the receipt does not record installed packages.
`PYTHONOPTIMIZE`, `PYTHONWARNINGS`, `PYTHONDEVMODE` and `PYTHONINTMAXSTRDIGITS`
are not passed to the reviewed code. Float
reviews are admitted only inside the IEEE-754
correctly rounded operations, where the result is identical on every machine. A
function that reaches a transcendental (sin, exp, log, pow) is refused by name
rather than judged. Its last bit is not reproducible across maths libraries. The
same holds for a float function that calls something equiv cannot analyse, such
as a compiled or sandboxed twin.

`--same-host` (on `review` and `review-pr`) lifts only those two float refusals and
judges the function on this machine. The receipt tier is then `tested-same-host`,
the mode is bound into the receipt id, and every output says "same host only": the
verdict holds on this machine and this maths library, and another machine may give
a different one. Use it for a twin whose float behaviour you want compared at all,
for example a Rust reimplementation of Python's `round`. Every other refusal still
applies.

`--types-hinted` (on `review`) changes no input and no verdict: it records that
the argument types were supplied by the caller rather than read from the
function's type hints, as the MCP server does for a function without hints
(`docs/agents.md`). The tier gains `-hinted` after its base (`tested-hinted`,
`tested-same-host-hinted`, then `-thin` when the evidence is thin) and the mode
is bound into the receipt id.

Each call of either version, and each module load, runs under a step budget:
4,000,000 Python line and jump events by default (enough for a loop with a one-line body over any int in the random range), counted through
`sys.monitoring`. No clock is read, so the same budget gives the same result on
every host running the same Python minor; the receipt records which one ran. A version that exceeds it (an infinite loop, or a loop over a huge
literal-derived input) ends the review as could not check (exit `2`), naming the
budget, the input and the side; a counterexample found on an earlier input still
stands. Inputs from the project itself (harvested values, and every input of a
`str[in ...]` value-set review) are the exception: one that runs past the budget is
"not judged at this input", counted and printed, and the review goes on. This
holds when only one side runs out too: a compiled twin raises no Python events, so in
a twin review that is the normal case, but in a refactor it can also mean the new
version no longer finishes there, so read the not-judged count. A review where no
input was judged at all is "could not check", never green. Raise the
budget with `--budget N` on `review` and `review-pr`, `EQUIV_BUDGET`, or the
Action's `budget` input. The budget is written into the receipt. Work done inside C code (for
example `sum(range(10**18))`) raises no Python events and is not counted. For
that case a separate safety limit applies: if one function's check runs longer
than `EQUIV_SAFETY_SECONDS` (default 120) of wall-clock time, its evaluator
processes are stopped and the function is reported as could not check (exit
`2`). This limit is not part of the deterministic budget, so it never decides a
verdict and no receipt (signed or unsigned) is issued for a stopped function;
other functions in the same run still report. The budget needs Python 3.12 or
newer; on an older Python the review reports could not check instead of running
without a limit.

## How much evidence an "equivalent" carries

An "equivalent" is only as good as the inputs that reached the function's
normal path. Two rules keep it honest (since 0.19.0):

- If every generated input (10 or more) makes both versions raise, only errors
  were compared. That is reported as could not check (exit `2`), never as
  equivalent.
- If fewer than 10 inputs returned a value (the rest raised on both sides), the
  verdict stays equivalent but is labelled **thin evidence**: in the CLI line,
  in the PR comment's headline and row, and in the receipt itself (tier
  `tested-thin`). The count is computed from the receipt: judged inputs minus
  inputs that raised on both sides.

A function with fewer than 10 possible inputs (no arguments, a single `bool`)
is checked completely by those calls and is exempt from both rules. A
difference in which error is raised is still a counterexample.

## Exit codes

Exit codes: `0` equivalent, `1` diverges with a printed counterexample, `2`
could not check (including "only errors were compared"). For `review-pr` the code is a report over the whole run: `0`
every changed function was checked and preserved behaviour, `1` at least one
changed behaviour, `2` nothing diverged but at least one changed function was
not checked (safety stop, refused type, error, or no supported type hints; each
is listed in the comment, never shown as green). The Action fails the PR check
on `1` by default and on `2` only with `fail-on-not-checked: "true"`; a review
that did not run at all always fails it, whatever the inputs say: `3` (bad
manifest, bad arguments, bad key) or `4` (`review-pr` could not read the base
ref, so nothing was compared; this is not "could not check").

A function that the base manifest lists and that existed at base but no longer
exists in the PR (removed or renamed) is reported as a behaviour change and
counts as `1`. A listed function that never existed at base is `2`, as before.

`equiv --help` exits `0`. An unknown flag, a case count or seed that is not a
number, or an extra argument is `3`, and names what was wrong; nothing is run.

## The manifest

The manifest is optional. Without one, every function changed in the PR is
detected from the diff and checked if each parameter has a supported type hint;
the rest are listed as not checked, with the parameters that need a hint. To pin
an exact list instead, write the functions whose behaviour must be preserved
across a PR in a manifest at the repository root. The format of each line is
`<file> : <function> : <arg types>`, where arg types are `int`, `str`,
`list[int]`, `list[str]`, `float`, `dict` or `bool`, comma separated:

```
src/math.py : total : int
```

A **twin** line checks two files of the PR's own tree against each other on every
PR, not a function against its base-branch version. Use it for a port and its
reference (a Rust or C binding and the pure-Python version):

```
twin : src/fast.py : src/slow.py : parse : str[in .corpus.SQL]
twin : src/fast.py : src/slow.py : ratio : float : same-host errors-agree
```

**Which manifest is read.** `review-pr` reads the manifest from the base ref,
not from the PR. Every line listed at base runs exactly as written at base,
even if the PR edited, removed or narrowed it, or deleted the file; this
applies to twin lines too. Lines that exist only in the PR's copy run as well
and are named in the comment as new in this PR. When the two copies differ
the comment says so and lists the lines. If there is no manifest at base, the
PR's copy is used and the comment says the manifest is new in this PR. Without
this, a PR could switch its own check off by editing the file. What this does
not cover: on `pull_request` events GitHub runs the workflow file from the PR
itself, so a PR from someone with write access can still change the workflow;
protect `.github/workflows` with a ruleset or CODEOWNERS if that matters.

The first file is the candidate and the second the reference. The optional last field
takes `errors-agree` (both raising counts as agreement, whatever the class) and
`same-host` (the same-host float tier), separated by spaces. A difference blocks the
merge like any listed function, and the comment names both files.

### Input domains (parity checks)

A compiled twin (Rust, C, Cython) often rejects inputs that its Python reference
accepts: day 0, years beyond int32, several characters where one is meant. To check
only the inputs the function is meant to take, give an argument a domain:

- `int[lo..hi]`: integers from `lo` to `hi`, inclusive;
- `str[ascii]`: ASCII strings only;
- `str[1]`: exactly one character (one code point);
- `str[in mod.NAME]`: one of a fixed list of strings in your own code, such as encoding
  or Unicode range names. Write `.constant.NAME` for a module next to the function
  (same package), `pkg.constant.NAME` for an absolute import, or `NAME` for a name in
  the function's own module. A list, tuple, set or dict of strings works (for a dict,
  its keys). A name that cannot be resolved makes the function "not checked", never
  green;
- the same inside lists: `list[int[1..12]]`, `list[str[1]]`, `list[str[in mod.NAME]]`.

```
src/dates.py : week_day : int[1..9999], int[1..12], int[1..28]
src/pkg/utils.py : is_multi_byte_encoding : str[in .constant.IANA_SUPPORTED]
```

A value-set list is read from the PR's own version of the code (the head), so a PR that
edits the list also changes its own domain. Every value in it is bound into the receipt,
so a pass is tied to that exact list. Each value of a list of up to 100,000 names is
judged (one argument at a time). Combinations across two or more such arguments are
sampled, and the result says how many were tried: "combinations across value-set
arguments sampled: only X of Y tried". For a `list[str[in ...]]` argument every value
is put in at least one list (alone), and lists of several values are sampled: "(N
values; each tried in at least one list; combinations of values within a list
sampled)". A longer list is sampled, and the result says so: "(N values; only X
tried)". These counts are measured on the inputs that ran.

Random inputs, corners (including both ends of a range), source literals and harvested
inputs all stay inside the domain. Literal and harvested values outside it are skipped,
and the output says how many. The domains are printed with the result and bound into
the receipt id, so a pass under a domain cannot be mistaken for a pass on the whole
type. **A pass says nothing about inputs outside the domain.** Without domains,
generation and receipts are unchanged.

`equiv review --errors-agree` is for the same use: when both sides raise, it counts as
agreement whatever the exception class. One side raising while the other returns is
still a difference.

## WASM contracts (`equiv check`)

`equiv check <artifact.wasm> <contract> [--reference <ref.wasm>]` checks one
exported WebAssembly function against a contract written in the contract text
format (`.eqct`; `equiv fmt`, `hash` and `lint` handle it). The clauses are a
precondition, a postcondition, a frame (the memory the call may write) and,
optionally, `refines`. Inputs that satisfy the precondition are generated from
a fixed seed (boundary values first); the result is "tested on N inputs", a
counterexample, or could not check. It is a test, not a proof.

`refines` names a reference artifact by its sha256 and the export to compare
with: `(refines (artifact sha256:HEX) "export" (over $x ...))`. Pass that file
with `--reference`. On every input that satisfies the precondition, the target
and the reference are called with the same arguments on the same memory
image. They must return the same result and leave the same bytes inside the
frame regions; what the reference writes outside the frame is not compared
(the target is held to the frame on its own). A difference is a counterexample
that names what each side returned. The receipt carries the reference's hash,
so it states exactly which reference was compared.

In this version the check is could not check, with the reason, when:

- no reference is passed, or the file passed has another sha256;
- the `over` list is anything other than the target's own parameters, in
  order (`(range $p $n)` stands for `$p $n`), or the clause has an
  `assuming` part (neither has a defined meaning yet);
- the reference cannot be read, lacks the export, has another signature,
  imports functions, uses a memory it does not export, or has a memory of
  another size than the target's;
- the target declares more than one result, or a float type;
- the reference traps on an input where the precondition holds (a reference
  must be defined wherever the contract speaks);
- fewer inputs than requested satisfy the precondition.

What it does not do yet, which matters for comparing a compiled C function
with its Rust rewrite: memory is filled with random bytes before each call, so
a compiled module's static data (lookup tables, constants) is overwritten;
compiled code that writes stack scratch below its stack pointer breaks the
frame clause; the two compilers lay memory out differently; only 32-bit and
64-bit integer arguments exist (a pointer is an offset into the shared memory
image); a module that imports anything (WASI, libc) cannot run; and there is
no limit on the work a call does, so a call that never returns hangs the
check. Today it fits small freestanding integer kernels, such as the CRC32 and
UTF-8 examples in `conformance/`.

## How it works

Input generation and the verdict are computed in Rust from a fixed seed. The
language runtime runs the two versions, and parses (never runs) the project's
files when inputs are harvested from them; it never decides a verdict, which
Rust computes from the values both versions returned. Receipts are identical across hosts. Receipts can be
signed with a local ed25519 key or with keyless Sigstore (OIDC). The keyless
path binds the signature to a verifiable CI identity rather than a stored
secret. The tool is a single static binary; a review also needs Python 3.12 or
newer on the machine. It is prebuilt for macOS, Linux and Windows.
