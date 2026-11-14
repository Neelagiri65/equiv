//! Read a receipt's canonical bytes back, structurally. `verify-receipt` uses
//! this for two things: a signed envelope whose payload is not a receipt is
//! rejected (a signature over arbitrary bytes says nothing about a review),
//! and the user is shown what the signed receipt actually says.
//!
//! Two layouts are known, both version 0: the review receipt written by
//! `equiv review` / `review-pr`, and the WASM-check receipt written by
//! `equiv check`. The layouts are those of this build; anything else is "not
//! an equiv receipt". The decode is strict (exact keys, exact shapes) and the
//! CBOR reader accepts canonical bytes only, so a payload that passes here
//! re-encodes to the same bytes.

use equiv_core::cbor::{self, Value};

pub struct View {
    /// Lines for the user: what kind of receipt, by which checker, what verdict.
    pub lines: Vec<String>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Receipt text is attacker-controlled once receipts travel: keep it on one
/// line and free of terminal control characters.
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

fn digest(v: &Value) -> Result<&[u8], String> {
    match v {
        Value::Bytes(b) if b.len() == 32 => Ok(b),
        _ => Err("a source digest is not 32 bytes".into()),
    }
}
fn uint(v: &Value) -> Result<u64, String> {
    match v {
        Value::Uint(n) => Ok(*n),
        _ => Err("a number field holds something else".into()),
    }
}
fn text(v: &Value) -> Result<&str, String> {
    match v {
        Value::Text(s) => Ok(s),
        _ => Err("a text field holds something else".into()),
    }
}
fn array(v: &Value) -> Result<&[Value], String> {
    match v {
        Value::Array(a) => Ok(a),
        _ => Err("a list field holds something else".into()),
    }
}

pub fn decode(bytes: &[u8]) -> Result<View, String> {
    let Ok(Value::Map(entries)) = cbor::from_bytes(bytes) else {
        return Err("the payload is not canonical receipt bytes".into());
    };
    let keys: Vec<u64> = entries.iter().map(|(k, _)| *k).collect();
    let get = |k: u64| entries.iter().find(|(key, _)| *key == k).map(|(_, v)| v);
    match get(0) {
        Some(Value::Uint(0)) => {}
        Some(_) => return Err("unknown receipt version".into()),
        None => return Err("the payload has no receipt version".into()),
    }
    let is_review = matches!(get(5), Some(Value::Array(a)) if matches!(a.first(), Some(Value::Text(_))));
    if is_review {
        review(&keys, &get)
    } else {
        wasm(&keys, &get)
    }
}

fn review<'a>(keys: &[u64], get: &impl Fn(u64) -> Option<&'a Value>) -> Result<View, String> {
    // Key 12 (what each side reported it ran) is present only for a review
    // that used the per-side options.
    if keys != (0..=11).collect::<Vec<u64>>() && keys != (0..=12).collect::<Vec<u64>>() {
        return Err("the fields are not those of a review receipt from this equiv version".into());
    }
    let f = |k: u64| get(k).ok_or_else(|| "a field is missing".to_string());
    let cand = digest(f(1)?)?;
    let refr = digest(f(2)?)?;
    digest(f(3)?)?;
    let checker = array(f(5)?)?;
    let [name, version] = checker else { return Err("the checker field is malformed".into()) };
    if text(name)? != "equiv-review" {
        return Err("the checker is not equiv-review".into());
    }
    let budget = uint(f(6)?)?;
    let python = text(f(7)?)?;
    let seed = uint(f(8)?)?;
    let inputs = array(f(9)?)?;
    if !(inputs.len() == 4 || inputs.len() == 5) {
        return Err("the input counts are malformed".into());
    }
    for i in inputs {
        uint(i)?;
    }
    let exceptions = uint(f(10)?)?;
    let tier = text(f(11)?)?;
    let v = array(f(4)?)?;
    let verdict = match v {
        [Value::Uint(0), n, nj] => {
            let (n, nj) = (uint(n)?, uint(nj)?);
            let nj = if nj == 0 { String::new() } else { format!("; {nj} large inputs not judged") };
            format!("EQUIVALENT: agreed on all {n} judged inputs (seed {seed}){nj}; {exceptions} of them raised on both sides")
        }
        [Value::Uint(1), input, c, r, judged, disagreeing, stopped] => {
            text(stopped)?;
            format!(
                "COUNTEREXAMPLE at input {}: candidate -> {}, reference -> {} ({} of {} judged inputs disagree)",
                clean(text(input)?),
                clean(text(c)?),
                clean(text(r)?),
                uint(disagreeing)?,
                uint(judged)?
            )
        }
        [Value::Uint(2), reason] => format!("ERROR (could not check): {}", clean(text(reason)?)),
        [Value::Uint(3), reason] => format!("REFUSED (could not check): {}", clean(text(reason)?)),
        _ => return Err("the verdict is malformed".into()),
    };
    let sides = match get(12) {
        Some(v) => match array(v)? {
            [r, c] => vec![
                format!("reference ran: {}", clean(text(r)?)),
                format!("candidate ran: {}", clean(text(c)?)),
            ],
            _ => return Err("the per-side report is malformed".into()),
        },
        None => Vec::new(),
    };
    Ok(View {
        lines: [
            format!(
                "kind:       review receipt (checker equiv-review {}, python {}, step budget {budget})",
                clean(text(version)?),
                clean(python)
            ),
            format!("verdict:    {verdict}"),
            format!("tier:       {}", clean(tier)),
            format!("candidate:  sha256 {}", hex(cand)),
            format!("reference:  sha256 {}", hex(refr)),
        ]
        .into_iter()
        .chain(sides)
        .collect(),
    })
}

fn wasm<'a>(keys: &[u64], get: &impl Fn(u64) -> Option<&'a Value>) -> Result<View, String> {
    if keys != [0, 1, 2, 3, 4, 5, 6, 7] && keys != [0, 1, 2, 4, 5, 6, 7] {
        return Err("the fields are not those of an equiv receipt from this version".into());
    }
    let f = |k: u64| get(k).ok_or_else(|| "a field is missing".to_string());
    let artifact = digest(f(1)?)?;
    let contract = digest(f(2)?)?;
    if let Some(r) = get(3) {
        digest(r)?;
    }
    for i in array(f(5)?)? {
        uint(i)?;
    }
    let [unroll, steps] = array(f(6)?)? else { return Err("the budgets are malformed".into()) };
    let (unroll, steps) = (uint(unroll)?, uint(steps)?);
    let [name, version] = array(f(7)?)? else { return Err("the checker field is malformed".into()) };
    let verdict = match array(f(4)?)? {
        [Value::Uint(0), k, s] => {
            format!("no violation exists within the bound (unroll {}, {} solver steps)", uint(k)?, uint(s)?)
        }
        [Value::Uint(1), args, trap] => {
            let args: Vec<String> = array(args)?.iter().map(|a| uint(a).map(|n| n.to_string())).collect::<Result<_, _>>()?;
            let trap = match uint(trap)? {
                0 => "",
                1 => " (a trap the contract does not allow)",
                _ => return Err("the verdict is malformed".into()),
            };
            format!("COUNTEREXAMPLE at arguments [{}]{trap}", args.join(", "))
        }
        [Value::Uint(2), n, seed] => format!("tested on {} cases (seed {}), no violation found", uint(n)?, uint(seed)?),
        [Value::Uint(3), code] if uint(code)? <= 6 => format!("UNKNOWN (could not check), reason code {}", uint(code)?),
        _ => return Err("the verdict is malformed".into()),
    };
    Ok(View {
        lines: vec![
            format!(
                "kind:       WASM check receipt (checker {} {}, unroll budget {unroll}, solver budget {steps})",
                clean(text(name)?),
                clean(text(version)?)
            ),
            format!("verdict:    {verdict}"),
            format!("artifact:   sha256 {}", hex(artifact)),
            format!("contract:   sha256 {}", hex(contract)),
        ],
    })
}
