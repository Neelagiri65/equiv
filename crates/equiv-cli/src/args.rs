//! Strict command-line reading. Every argument is either a positional, a
//! known switch or a known flag with its value; anything else is a usage
//! error that names it. Nothing is silently ignored: a mistyped flag that was
//! dropped would change what a run tested without saying so.

pub struct Parsed {
    pub pos: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Parsed {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|(f, _)| f == flag)
    }
    /// The value of a flag that takes one (the last, if repeated).
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.flags.iter().rev().find(|(f, _)| f == flag).and_then(|(_, v)| v.as_deref())
    }
    pub fn values(&self, flag: &str) -> Vec<&str> {
        self.flags.iter().filter(|(f, _)| f == flag).filter_map(|(_, v)| v.as_deref()).collect()
    }
}

/// `args` are the arguments after the subcommand. `switches` take no value,
/// `valued` take exactly one.
pub fn parse(cmd: &str, args: &[String], switches: &[&str], valued: &[&str]) -> Result<Parsed, String> {
    let mut out = Parsed { pos: Vec::new(), flags: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if switches.contains(&a.as_str()) {
            out.flags.push((a.clone(), None));
        } else if valued.contains(&a.as_str()) {
            match it.next() {
                Some(v) if !v.starts_with("--") => out.flags.push((a.clone(), Some(v.clone()))),
                _ => return Err(format!("{cmd}: {a} needs a value")),
            }
        } else if a.len() > 1 && a.starts_with('-') {
            return Err(format!("{cmd}: unknown flag `{a}` (see `equiv --help`)"));
        } else {
            out.pos.push(a.clone());
        }
    }
    Ok(out)
}

/// A whole number argument; a value that is not one is an error naming it.
pub fn number<T: std::str::FromStr>(what: &str, raw: &str) -> Result<T, String> {
    raw.trim().parse::<T>().map_err(|_| format!("{what} must be a whole number, got `{raw}`"))
}
