//! Inputs harvested from the project's own code (0.17.0).
//!
//! Generated inputs only know a function's argument types. A project's tests
//! and call sites know what the function is actually called with: real
//! identifiers, real file names, real edge cases someone already thought of.
//! This module reads those values out of Python source with `ast` and
//! `ast.literal_eval` (parsing only: nothing in the scanned files is run),
//! keeps the ones that match the declared signature, and hands them to the
//! generator as extra inputs.
//!
//! What is collected, per function `f`:
//! - every call `f(...)` or `x.f(...)` whose arguments are literals
//!   (positional, or keyword when the keyword names a parameter of `f`);
//! - pytest `parametrize` rows, when the test body calls `f` with the
//!   parametrized names.
//! A call with some non-literal arguments still contributes its literal ones,
//! one argument at a time.
//!
//! Harvested inputs are bound into the receipt (`spec_sha256` covers them and
//! the receipt counts them), so a receipt made with them is reproduced only by
//! a run over the same files. Extra inputs cannot cause a false alarm: both
//! versions get the same input, and any disagreement is a real one.

use super::{evaluator_command, hex, sha256, ArgType, PyVal, INT_ENVELOPE, STR_LEN_CAP};
use std::path::{Path, PathBuf};

/// One harvested call: a value per argument position, `None` where the call
/// site passed something that is not a literal of the declared type.
pub type Slots = Vec<Option<PyVal>>;

/// At most this many distinct harvested calls per function, in file order.
pub const HARVEST_CAP: usize = 500;
/// Source files larger than this are skipped (generated or vendored code).
const FILE_CAP_BYTES: u64 = 1_000_000;
/// Longest list kept from a literal.
const LIST_CAP: usize = 64;

const HARVEST_PY: &str = r#"import ast, struct, sys
spec_path, files_path = sys.argv[1], sys.argv[2]
want = {}
for line in open(spec_path, encoding='utf-8').read().splitlines():
    if not line: continue
    name, cand = line.split('\t', 1)
    params = None
    try:
        t = ast.parse(open(cand, encoding='utf-8').read())
        for n in t.body:
            if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name == name:
                a = n.args
                params = [x.arg for x in a.posonlyargs + a.args]
                break
    except Exception:
        pass
    if params is not None: want[name] = params
def enc(v):
    if isinstance(v, bool): return 'b:' + ('1' if v else '0')
    if isinstance(v, int): return 'i:' + str(v)
    if isinstance(v, float): return 'f:' + struct.pack('>d', v).hex()
    if isinstance(v, str):
        try: return 's:' + v.encode('utf-8').hex()
        except UnicodeEncodeError: return 'n:'
    if isinstance(v, list):
        if not v: return 'e:'
        if all(isinstance(e, int) and not isinstance(e, bool) for e in v): return 'li:' + ','.join(str(e) for e in v)
        if all(isinstance(e, str) for e in v):
            try: return 'ls:' + ','.join(e.encode('utf-8').hex() for e in v)
            except UnicodeEncodeError: return 'n:'
    if isinstance(v, dict) and all(isinstance(k, str) for k in v) and all(isinstance(x, int) and not isinstance(x, bool) for x in v.values()):
        try: return 'd:' + ','.join(k.encode('utf-8').hex() + '=' + str(x) for k, x in v.items())
        except UnicodeEncodeError: return 'n:'
    return 'n:'
def lit(node):
    try: return True, ast.literal_eval(node)
    except Exception: return False, None
def callee(c):
    f = c.func
    if isinstance(f, ast.Name): return f.id
    if isinstance(f, ast.Attribute): return f.attr
    return None
def slots_of(c, params, env):
    # env: parametrized name -> value, for names bound by a parametrize row
    s = ['n:'] * len(params)
    for i, a in enumerate(c.args):
        if isinstance(a, ast.Starred) or i >= len(params): return None
        if isinstance(a, ast.Name) and a.id in env: s[i] = enc(env[a.id]); continue
        ok, v = lit(a)
        if ok: s[i] = enc(v)
    for k in c.keywords:
        if k.arg is None: return None
        if k.arg not in params: continue
        j = params.index(k.arg)
        if isinstance(k.value, ast.Name) and k.value.id in env: s[j] = enc(env[k.value.id]); continue
        ok, v = lit(k.value)
        if ok: s[j] = enc(v)
    if all(x == 'n:' for x in s): return None
    return s
def emit(name, s):
    sys.stdout.write(name + '\t' + '\x1f'.join(s) + '\n')
def rows(dec):
    # pytest.mark.parametrize("a, b", [(1, 2), ...]) -> (names, [row dicts])
    if not (isinstance(dec, ast.Call) and callee(dec) == 'parametrize' and len(dec.args) >= 2): return None
    ok, names = lit(dec.args[0])
    if not ok: return None
    if isinstance(names, str): names = [x.strip() for x in names.split(',') if x.strip()]
    if not isinstance(names, (list, tuple)) or not all(isinstance(x, str) for x in names): return None
    vals = dec.args[1]
    if not isinstance(vals, (ast.List, ast.Tuple)): return None
    out = []
    for e in vals.elts:
        ok, v = lit(e)
        if not ok: continue
        if len(names) == 1: v = (v,)
        if not isinstance(v, (tuple, list)) or len(v) != len(names): continue
        out.append(dict(zip(names, v)))
    return out
for path in open(files_path, encoding='utf-8').read().splitlines():
    if not path: continue
    try: t = ast.parse(open(path, encoding='utf-8').read())
    except Exception: continue
    covered = set()
    for fn in ast.walk(t):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)): continue
        envs = []
        for d in fn.decorator_list:
            r = rows(d)
            if r: envs = [dict(a, **b) for a in (envs or [{}]) for b in r]
        if not envs: continue
        for c in ast.walk(fn):
            if isinstance(c, ast.Call) and callee(c) in want:
                for env in envs:
                    s = slots_of(c, want[callee(c)], env)
                    if s: emit(callee(c), s)
                covered.add(id(c))
    for c in ast.walk(t):
        if isinstance(c, ast.Call) and id(c) not in covered and callee(c) in want:
            s = slots_of(c, want[callee(c)], {})
            if s: emit(callee(c), s)
"#;

/// Python files worth scanning under `root`: every `*.py` file git tracks
/// (or, outside a git checkout, every `*.py` file below `root`), sorted, so
/// the scan order and hence the harvested inputs are the same on every host.
pub fn python_files(root: &Path) -> Vec<PathBuf> {
    let listed = std::process::Command::new("git")
        .args(["-c", "core.quotepath=false", "ls-files", "-z", "--", "*.py"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success());
    let mut files: Vec<PathBuf> = match listed {
        Some(o) => String::from_utf8_lossy(&o.stdout)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(|s| root.join(s))
            .collect(),
        None => {
            let mut out = Vec::new();
            walk(root, &mut out);
            out
        }
    };
    files.retain(|p| std::fs::metadata(p).map(|m| m.is_file() && m.len() <= FILE_CAP_BYTES).unwrap_or(false));
    files.sort();
    files.dedup();
    files
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "__pycache__" || name == "node_modules" || name == "target" {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else if name.ends_with(".py") {
            out.push(p);
        }
    }
}

/// Expand `--inputs-from` arguments: a file is taken as is, a directory is
/// scanned for Python files.
pub fn expand_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            out.extend(python_files(p));
        } else if p.is_file() {
            out.push(p.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Harvest literal call inputs for several functions in one pass over
/// `files`. `wanted` is (function name, path of the file defining it, its
/// signature); the defining file supplies the parameter names that keyword
/// arguments are matched against. Returns, per entry of `wanted`, the
/// distinct harvested calls in file order (at most `HARVEST_CAP`). A Python
/// failure yields no harvested inputs, never an error: harvesting only adds.
pub fn harvest(wanted: &[(String, PathBuf, Vec<ArgType>)], files: &[PathBuf]) -> Vec<Vec<Slots>> {
    let mut result: Vec<Vec<Slots>> = vec![Vec::new(); wanted.len()];
    if wanted.is_empty() || files.is_empty() {
        return result;
    }
    let spec_txt: String = wanted.iter().map(|(n, p, _)| format!("{n}\t{}\n", p.display())).collect();
    let files_txt: String = files.iter().map(|p| format!("{}\n", p.display())).collect();
    let tag = hex(&sha256(format!("{spec_txt}\u{0}{files_txt}").as_bytes()))[..16].to_string();
    let dir = std::env::temp_dir();
    let (sp, fp, pp) = (
        dir.join(format!("equiv_hvs_{tag}.txt")),
        dir.join(format!("equiv_hvf_{tag}.txt")),
        dir.join(format!("equiv_hvp_{tag}.py")),
    );
    if std::fs::write(&sp, &spec_txt).is_err()
        || std::fs::write(&fp, &files_txt).is_err()
        || std::fs::write(&pp, HARVEST_PY).is_err()
    {
        return result;
    }
    let out = evaluator_command().arg(&pp).arg(&sp).arg(&fp).env("PYTHONDONTWRITEBYTECODE", "1").output();
    let Ok(out) = out else { return result };
    if !out.status.success() {
        return result;
    }
    let mut seen: Vec<std::collections::BTreeSet<String>> = vec![Default::default(); wanted.len()];
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((name, rest)) = line.split_once('\t') else { continue };
        for (k, (wname, _, sig)) in wanted.iter().enumerate() {
            if wname != name || result[k].len() >= HARVEST_CAP {
                continue;
            }
            let raw: Vec<&str> = rest.split('\u{1f}').collect();
            if raw.len() != sig.len() {
                continue;
            }
            let slots: Slots = raw.iter().zip(sig).map(|(r, t)| decode(r, *t)).collect();
            if slots.iter().all(Option::is_none) {
                continue;
            }
            let key: String = slots.iter().map(|s| s.as_ref().map(|v| v.to_py()).unwrap_or_default() + "\u{0}").collect();
            if seen[k].insert(key) {
                result[k].push(slots);
            }
        }
    }
    result
}

fn unhex(s: &str) -> Option<String> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

/// One encoded value, kept only if it is exactly the declared type (no
/// coercion: an int literal for a float parameter is skipped, as the review
/// itself is type-strict) and of reviewable size.
fn decode(raw: &str, ty: ArgType) -> Option<PyVal> {
    let (kind, body) = raw.split_once(':')?;
    let v = match (kind, ty) {
        ("i", ArgType::Int) => PyVal::Int(body.parse().ok()?),
        ("b", ArgType::Bool) => PyVal::Bool(body == "1"),
        ("f", ArgType::Float) => PyVal::Float(u64::from_str_radix(body, 16).ok()?),
        ("s", ArgType::Str) => PyVal::Str(unhex(body)?),
        ("e", ArgType::ListInt) => PyVal::ListInt(Vec::new()),
        ("e", ArgType::ListStr) => PyVal::ListStr(Vec::new()),
        ("li", ArgType::ListInt) => {
            PyVal::ListInt(body.split(',').map(|x| x.parse().ok()).collect::<Option<Vec<i64>>>()?)
        }
        ("ls", ArgType::ListStr) => PyVal::ListStr(body.split(',').map(unhex).collect::<Option<Vec<String>>>()?),
        ("d", ArgType::Dict) if body.is_empty() => PyVal::Dict(Vec::new()),
        ("d", ArgType::Dict) => PyVal::Dict(
            body.split(',')
                .map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    Some((unhex(k)?, v.parse().ok()?))
                })
                .collect::<Option<Vec<(String, i64)>>>()?,
        ),
        _ => return None,
    };
    let fits = match &v {
        PyVal::Str(s) => s.chars().count() as i64 <= STR_LEN_CAP,
        PyVal::ListInt(l) => l.len() <= LIST_CAP,
        PyVal::ListStr(l) => l.len() <= LIST_CAP && l.iter().all(|s| s.chars().count() as i64 <= STR_LEN_CAP),
        PyVal::Dict(d) => d.len() <= LIST_CAP,
        _ => true,
    };
    fits.then_some(v)
}

/// Whether a value stays inside the envelope an O(n) reference must survive.
/// Harvested values outside it (a test's `f(10**12)`) run with the
/// large-magnitude corners, where running out of budget is "not judged at
/// this input" instead of ending the review.
pub(crate) fn inside_envelope(v: &PyVal) -> bool {
    match v {
        PyVal::Int(n) => n.unsigned_abs() <= INT_ENVELOPE as u64,
        PyVal::ListInt(l) => l.iter().all(|n| n.unsigned_abs() <= INT_ENVELOPE as u64),
        PyVal::Dict(d) => d.iter().all(|(_, n)| n.unsigned_abs() <= INT_ENVELOPE as u64),
        _ => true,
    }
}

/// Stable bytes for the harvested inputs, appended to the spec bytes when
/// there are any (so a review without harvested inputs hashes as before).
pub(crate) fn canonical(h: &[Slots]) -> Vec<u8> {
    let mut v = b"\x00harvested\x00".to_vec();
    for s in h {
        for x in s {
            match x {
                Some(p) => v.extend_from_slice(p.to_py().as_bytes()),
                None => v.push(b'_'),
            }
            v.push(0x1f);
        }
        v.push(b'\n');
    }
    v
}
