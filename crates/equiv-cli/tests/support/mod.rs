//! Shared by the agent-integration tests (mcp_server, hook_claude_code,
//! init_claude_code): a strict JSON reader written with std only, so the
//! tests do not judge the server's output with the same library that wrote
//! it, plus the temp-repo helpers the other CLI tests also use.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number as written (validated against the JSON grammar).
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    /// Members in the order they were written.
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    /// Walk object keys and array indexes: `j.at(&["result", "tools", "0", "name"])`.
    pub fn at(&self, path: &[&str]) -> Option<&Json> {
        let mut cur = self;
        for p in path {
            cur = match cur {
                Json::Obj(_) => cur.get(p)?,
                Json::Arr(a) => a.get(p.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(cur)
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn int(&self) -> Option<i64> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }
    pub fn keys(&self) -> Vec<&str> {
        match self {
            Json::Obj(m) => m.iter().map(|(k, _)| k.as_str()).collect(),
            _ => Vec::new(),
        }
    }
    pub fn is_obj(&self) -> bool {
        matches!(self, Json::Obj(_))
    }
}

/// Parse one complete JSON text. Duplicate keys, raw control characters in
/// strings, lone surrogates and trailing input are errors.
pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = Reader { s, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != s.len() {
        return Err(format!("trailing input at byte {}", p.i));
    }
    Ok(v)
}

struct Reader<'a> {
    s: &'a str,
    i: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", c as char, self.i))
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 128 {
            return Err("nested too deeply".into());
        }
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let mut m: Vec<(String, Json)> = Vec::new();
                self.ws();
                if self.eat(b'}') {
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.expect(b':')?;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    if m.iter().any(|(have, _)| *have == k) {
                        return Err(format!("duplicate key `{k}`"));
                    }
                    m.push((k, v));
                    self.ws();
                    if self.eat(b',') {
                        continue;
                    }
                    self.expect(b'}')?;
                    return Ok(Json::Obj(m));
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat(b']') {
                    return Ok(Json::Arr(a));
                }
                loop {
                    self.ws();
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b',') {
                        continue;
                    }
                    self.expect(b']')?;
                    return Ok(Json::Arr(a));
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(format!("unexpected input at byte {}", self.i)),
        }
    }
    fn word(&mut self, w: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(w) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(format!("bad literal at byte {}", self.i))
        }
    }
    fn digits(&mut self) -> usize {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        self.i - start
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        self.eat(b'-');
        if self.eat(b'0') {
            if matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err("leading zero".into());
            }
        } else if self.digits() == 0 {
            return Err(format!("bad number at byte {start}"));
        }
        if self.eat(b'.') && self.digits() == 0 {
            return Err(format!("bad fraction at byte {start}"));
        }
        if self.eat(b'e') || self.eat(b'E') {
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                return Err(format!("bad exponent at byte {start}"));
            }
        }
        Ok(Json::Num(self.s[start..self.i].to_string()))
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or("short \\u escape")?;
        let v = u32::from_str_radix(h, 16).map_err(|_| format!("bad \\u escape `{h}`"))?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or("unterminated string")?;
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let e = self.peek().ok_or("unterminated escape")?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if !self.s[self.i..].starts_with("\\u") {
                                    return Err("lone high surrogate".into());
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err("bad low surrogate".into());
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err("lone low surrogate".into());
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code).ok_or("bad code point")?);
                        }
                        _ => return Err(format!("bad escape at byte {}", self.i)),
                    }
                }
                c if c < 0x20 => return Err(format!("raw control character in a string at byte {}", self.i)),
                _ => {
                    let ch = self.s[self.i..].chars().next().ok_or("bad character")?;
                    out.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
    }
}

/// A JSON string literal for `s` (for building hook events and requests).
pub fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A fresh, empty directory for one test.
pub fn fresh_dir(prefix: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("{prefix}_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The temp root, canonical: git's repository discovery stops there, so only
/// the repositories the tests create are ever seen (the temp dir may itself
/// sit inside some checkout).
pub fn ceiling() -> PathBuf {
    std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir())
}

pub fn git(d: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(d)
        .env("GIT_CEILING_DIRECTORIES", ceiling())
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// A git repository in `d` with `files` committed.
pub fn repo_with(d: &Path, files: &[(&str, &str)]) {
    for (name, body) in files {
        let p = d.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }
    git(d, &["init", "-q"]);
    git(d, &["add", "."]);
    git(d, &["commit", "-q", "-m", "base"]);
}

#[test]
fn the_reader_is_strict() {
    assert_eq!(parse(r#"{"a":[1,-2.5e3,true,null,"x\u00e9\ud83d\ude00\n"]}"#).unwrap().at(&["a", "4"]).unwrap().str(), Some("xé😀\n"));
    for bad in ["{", "[1,]", "{\"a\":1,\"a\":2}", "\"\u{1}\"", "01", "1.", "{} x", "\"\\ud800\"", "nul"] {
        assert!(parse(bad).is_err(), "{bad}");
    }
    let j = parse(r#"{"z":1,"a":2}"#).unwrap();
    assert_eq!(j.keys(), ["z", "a"]);
    assert_eq!(parse(&quote("a\"b\\c\n\u{2}")).unwrap().str(), Some("a\"b\\c\n\u{2}"));
}
