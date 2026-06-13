//! Minimal canonical CBOR (RFC 8949 §4.2 deterministic encoding), restricted
//! to the subset the eqc format uses: unsigned ints, byte strings, text
//! strings, definite arrays, and definite maps with unsigned-int keys.
//!
//! Written from scratch so determinism is by construction and strictness is
//! total: the decoder rejects indefinite lengths, non-shortest-form integers,
//! unsorted/duplicate map keys, and every major type the format does not use.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Uint(u64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    /// Keys are unsigned ints, must be strictly ascending.
    Map(Vec<(u64, Value)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborError {
    Truncated,
    TrailingBytes,
    IndefiniteLength,
    NonShortestForm,
    UnsupportedMajorType(u8),
    NonUintMapKey,
    UnsortedMapKeys,
    InvalidUtf8,
    TooDeep,
}

const MAX_DEPTH: usize = 64;

// ---------- encoding ----------

fn write_head(major: u8, val: u64, out: &mut Vec<u8>) {
    let m = major << 5;
    if val < 24 {
        out.push(m | val as u8);
    } else if val <= 0xff {
        out.push(m | 24);
        out.push(val as u8);
    } else if val <= 0xffff {
        out.push(m | 25);
        out.extend_from_slice(&(val as u16).to_be_bytes());
    } else if val <= 0xffff_ffff {
        out.push(m | 26);
        out.extend_from_slice(&(val as u32).to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&val.to_be_bytes());
    }
}

pub fn encode(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Uint(n) => write_head(0, *n, out),
        Value::Bytes(b) => {
            write_head(2, b.len() as u64, out);
            out.extend_from_slice(b);
        }
        Value::Text(s) => {
            write_head(3, s.len() as u64, out);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Array(items) => {
            write_head(4, items.len() as u64, out);
            for it in items {
                encode(it, out);
            }
        }
        Value::Map(entries) => {
            // Canonical: keys strictly ascending (bytewise order == numeric
            // order for uint keys). Encoders must supply them sorted; this is
            // a debug-time invariant, not a silent re-sort.
            debug_assert!(entries.windows(2).all(|w| w[0].0 < w[1].0));
            write_head(5, entries.len() as u64, out);
            for (k, val) in entries {
                write_head(0, *k, out);
                encode(val, out);
            }
        }
    }
}

pub fn to_bytes(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode(v, &mut out);
    out
}

// ---------- strict decoding ----------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, CborError> {
        let b = *self.buf.get(self.pos).ok_or(CborError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CborError> {
        let end = self.pos.checked_add(n).ok_or(CborError::Truncated)?;
        if end > self.buf.len() {
            return Err(CborError::Truncated);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Read a head, enforcing shortest-form encoding.
    fn head(&mut self) -> Result<(u8, u64), CborError> {
        let b = self.byte()?;
        let major = b >> 5;
        let info = b & 0x1f;
        let val = match info {
            0..=23 => info as u64,
            24 => {
                let v = self.byte()? as u64;
                if v < 24 {
                    return Err(CborError::NonShortestForm);
                }
                v
            }
            25 => {
                let v = u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64;
                if v <= 0xff {
                    return Err(CborError::NonShortestForm);
                }
                v
            }
            26 => {
                let v = u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64;
                if v <= 0xffff {
                    return Err(CborError::NonShortestForm);
                }
                v
            }
            27 => {
                let v = u64::from_be_bytes(self.take(8)?.try_into().unwrap());
                if v <= 0xffff_ffff {
                    return Err(CborError::NonShortestForm);
                }
                v
            }
            31 => return Err(CborError::IndefiniteLength),
            _ => return Err(CborError::NonShortestForm),
        };
        Ok((major, val))
    }

    fn value(&mut self, depth: usize) -> Result<Value, CborError> {
        if depth > MAX_DEPTH {
            return Err(CborError::TooDeep);
        }
        let (major, val) = self.head()?;
        match major {
            0 => Ok(Value::Uint(val)),
            2 => Ok(Value::Bytes(self.take(val as usize)?.to_vec())),
            3 => {
                let raw = self.take(val as usize)?;
                let s = std::str::from_utf8(raw).map_err(|_| CborError::InvalidUtf8)?;
                Ok(Value::Text(s.to_owned()))
            }
            4 => {
                let mut items = Vec::new();
                for _ in 0..val {
                    items.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(items))
            }
            5 => {
                let mut entries: Vec<(u64, Value)> = Vec::new();
                for _ in 0..val {
                    let (km, kv) = self.head()?;
                    if km != 0 {
                        return Err(CborError::NonUintMapKey);
                    }
                    if let Some((last, _)) = entries.last() {
                        if kv <= *last {
                            return Err(CborError::UnsortedMapKeys);
                        }
                    }
                    entries.push((kv, self.value(depth + 1)?));
                }
                Ok(Value::Map(entries))
            }
            m => Err(CborError::UnsupportedMajorType(m)),
        }
    }
}

pub fn from_bytes(buf: &[u8]) -> Result<Value, CborError> {
    let mut r = Reader { buf, pos: 0 };
    let v = r.value(0)?;
    if r.pos != buf.len() {
        return Err(CborError::TrailingBytes);
    }
    Ok(v)
}
