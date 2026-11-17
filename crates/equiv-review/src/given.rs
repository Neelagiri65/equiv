//! Given inputs (DECISIONS #39): a caller supplies every input, as typed JSON
//! lines, instead of equiv generating them. Each line is a JSON array with one
//! tagged value per argument:
//!
//! `{"int": "<decimal>"}`, `{"str": "..."}`, `{"bytes": "<hex>"}`,
//! `{"float": "<16 hex digits: IEEE-754 bits>"}`, `{"bool": true}`,
//! `{"none": null}`, `{"list": [...]}`, `{"tuple": [...]}`,
//! `{"dict": [[key, value], ...]}`.
//!
//! Every value is carried exactly or the file is refused, with the line and
//! the reason: nothing is dropped, rounded or approximated. Dict keys are
//! int, str, bytes, bool, None or tuples of those (hashable on every side),
//! and a key may not repeat. The whole file is bound into the spec hash.

use crate::PyVal;
use serde_json::Value as J;

/// Python's default limit on int-to-str digits also applies to decimal int
/// literals; past it, the evaluator could not read the input back.
const MAX_INT_DIGITS: usize = 4300;

#[derive(Debug, Clone)]
pub struct GivenInputs {
    /// SHA-256 of the file's bytes, bound into the spec hash.
    pub sha256: [u8; 32],
    pub cases: Vec<Vec<PyVal>>,
    /// Some input carries a float (then the review needs `--same-host`: the
    /// functions behind a given-input review are usually native code, which
    /// equiv cannot analyse for host-dependent float results).
    pub has_float: bool,
}

pub fn parse(bytes: &[u8]) -> Result<GivenInputs, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the inputs file is not UTF-8".to_string())?;
    let mut cases = Vec::new();
    let mut has_float = false;
    let mut arity: Option<usize> = None;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        let v: J = serde_json::from_str(line).map_err(|e| format!("inputs file line {n}: not JSON ({e})"))?;
        let args = v.as_array().ok_or_else(|| format!("inputs file line {n}: expected a JSON array of arguments"))?;
        if let Some(a) = arity {
            if a != args.len() {
                return Err(format!("inputs file line {n}: {} arguments, earlier lines have {a}", args.len()));
            }
        }
        arity = Some(args.len());
        let mut case = Vec::with_capacity(args.len());
        for (j, a) in args.iter().enumerate() {
            let (py, shown) = value(a, &mut has_float).map_err(|e| format!("inputs file line {n}, argument {}: {e}", j + 1))?;
            case.push(PyVal::Given { py, shown });
        }
        cases.push(case);
    }
    if cases.is_empty() {
        return Err("the inputs file has no inputs".into());
    }
    Ok(GivenInputs { sha256: crate::sha256(bytes), cases, has_float })
}

/// (Python literal handed to the evaluator, readable form for a counterexample).
fn value(v: &J, has_float: &mut bool) -> Result<(String, String), String> {
    let o = v.as_object().filter(|o| o.len() == 1).ok_or("expected an object with exactly one tag, e.g. {\"int\": \"5\"}")?;
    let (tag, x) = o.iter().next().unwrap();
    Ok(match tag.as_str() {
        "int" => {
            let s = x.as_str().ok_or("int: expected a decimal string")?;
            let digits = s.strip_prefix('-').unwrap_or(s);
            let ok = !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && (digits == "0" || !digits.starts_with('0'))
                && s != "-0";
            if !ok {
                return Err(format!("int: `{s}` is not a canonical decimal integer"));
            }
            if digits.len() > MAX_INT_DIGITS {
                return Err(format!("int: more than {MAX_INT_DIGITS} digits"));
            }
            (s.to_string(), s.to_string())
        }
        "str" => {
            let s = x.as_str().ok_or("str: expected a JSON string")?;
            let lit = crate::py_str_literal(s);
            (lit.clone(), lit)
        }
        "bytes" => {
            let h = x.as_str().ok_or("bytes: expected a hex string")?;
            let b = unhex(h).ok_or("bytes: expected lowercase hex, two digits per byte")?;
            (format!("bytes.fromhex('{h}')"), bytes_repr(&b))
        }
        "float" => {
            let h = x.as_str().ok_or("float: expected 16 hex digits")?;
            if h.len() != 16 || !h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return Err("float: expected 16 lowercase hex digits (the IEEE-754 bits)".into());
            }
            *has_float = true;
            let bits = u64::from_str_radix(h, 16).unwrap();
            let f = f64::from_bits(bits);
            (format!("struct.unpack('>d', bytes.fromhex('{h}'))[0]"), if f.is_nan() { "nan".into() } else { format!("{f:?}") })
        }
        "bool" => {
            let b = x.as_bool().ok_or("bool: expected true or false")?;
            let s = if b { "True" } else { "False" };
            (s.into(), s.into())
        }
        "none" => {
            if !x.is_null() {
                return Err("none: expected null".into());
            }
            ("None".into(), "None".into())
        }
        "list" | "tuple" => {
            let xs = x.as_array().ok_or_else(|| format!("{tag}: expected an array"))?;
            let mut py = Vec::new();
            let mut shown = Vec::new();
            for e in xs {
                let (p, s) = value(e, has_float)?;
                py.push(p);
                shown.push(s);
            }
            if tag == "list" {
                (format!("[{}]", py.join(", ")), format!("[{}]", shown.join(", ")))
            } else {
                let one = if xs.len() == 1 { "," } else { "" };
                (format!("({}{one})", py.join(", ")), format!("({}{one})", shown.join(", ")))
            }
        }
        "dict" => {
            let pairs = x.as_array().ok_or("dict: expected an array of [key, value] pairs")?;
            let mut py = Vec::new();
            let mut shown = Vec::new();
            let mut keys: Vec<String> = Vec::new();
            for p in pairs {
                let kv = p.as_array().filter(|a| a.len() == 2).ok_or("dict: each pair must be [key, value]")?;
                if !hashable(&kv[0]) {
                    return Err("dict: a key must be int, str, bytes, bool, None or a tuple of those".into());
                }
                let (kp, ks) = value(&kv[0], has_float)?;
                if keys.contains(&kp) {
                    return Err(format!("dict: key {ks} appears twice"));
                }
                keys.push(kp.clone());
                let (vp, vs) = value(&kv[1], has_float)?;
                py.push(format!("{kp}: {vp}"));
                shown.push(format!("{ks}: {vs}"));
            }
            (format!("{{{}}}", py.join(", ")), format!("{{{}}}", shown.join(", ")))
        }
        other => return Err(format!("unknown tag `{other}` (int, str, bytes, float, bool, none, list, tuple, dict)")),
    })
}

fn hashable(v: &J) -> bool {
    let Some((tag, x)) = v.as_object().filter(|o| o.len() == 1).and_then(|o| o.iter().next()) else { return false };
    match tag.as_str() {
        "int" | "str" | "bytes" | "bool" | "none" => true,
        "tuple" => x.as_array().is_some_and(|xs| xs.iter().all(hashable)),
        _ => false,
    }
}

fn unhex(h: &str) -> Option<Vec<u8>> {
    if h.len() % 2 != 0 || !h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect()
}

/// Python's repr of a bytes object (single quotes; the forms repr uses).
fn bytes_repr(b: &[u8]) -> String {
    let mut s = String::from("b'");
    for &c in b {
        match c {
            b'\\' => s.push_str("\\\\"),
            b'\'' => s.push_str("\\'"),
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s.push('\'');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(line: &str) -> Result<Vec<(String, String)>, String> {
        parse(line.as_bytes()).map(|g| {
            g.cases[0]
                .iter()
                .map(|v| match v {
                    PyVal::Given { py, shown } => (py.clone(), shown.clone()),
                    _ => unreachable!(),
                })
                .collect()
        })
    }

    #[test]
    fn nested_values_become_exact_literals() {
        let got = lit(r#"[{"dict":[[{"bytes":"612f"},{"tuple":[{"int":"16384"},{"bytes":"00"}]}]]},{"bool":false}]"#).unwrap();
        assert_eq!(got[0].0, "{bytes.fromhex('612f'): (16384, bytes.fromhex('00'))}");
        assert_eq!(got[0].1, "{b'a/': (16384, b'\\x00')}");
        assert_eq!(got[1].0, "False");
        assert_eq!(lit(r#"[{"tuple":[{"none":null}]}]"#).unwrap()[0].0, "(None,)");
    }

    #[test]
    fn refuses_what_it_cannot_carry_exactly() {
        for bad in [
            r#"[{"int":"007"}]"#,
            r#"[{"int":"-0"}]"#,
            r#"[{"int":5}]"#,
            r#"[{"bytes":"ABC"}]"#,
            r#"[{"float":"1.5"}]"#,
            r#"[{"set":[]}]"#,
            r#"[{"dict":[[{"list":[]},{"int":"1"}]]}]"#,
            r#"[{"dict":[[{"int":"1"},{"int":"1"}],[{"int":"1"},{"int":"2"}]]}]"#,
            r#"[{"int":"1","str":"x"}]"#,
            "{}",
        ] {
            assert!(lit(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn every_line_must_have_the_same_arity_and_the_file_is_hashed() {
        assert!(parse(b"[{\"int\":\"1\"}]\n[]\n").is_err());
        assert!(parse(b"\n").is_err());
        let a = parse(b"[{\"int\":\"1\"}]\n").unwrap();
        let b = parse(b"[{\"int\":\"2\"}]\n").unwrap();
        assert_ne!(a.sha256, b.sha256);
        assert!(!a.has_float);
        assert!(parse(b"[{\"float\":\"3ff0000000000000\"}]").unwrap().has_float);
    }
}
