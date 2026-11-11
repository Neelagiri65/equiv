//! Limits of the text surface and their agreement with the canonical
//! encoding: whatever parses must encode to bytes the decoder takes back,
//! and anything beyond a limit is a clean error, never an abort.

use equiv_core::cbor::CborError;
use equiv_core::codec::{decode_contract, encode_contract, DecodeError};
use equiv_core::text::parse_contract;

fn params(n: usize) -> String {
    (0..n).map(|i| format!("(param $p{i} i32)")).collect::<Vec<_>>().join(" ")
}

fn nots(n: usize, leaf: &str) -> String {
    format!("{}{leaf}{}", "(not ".repeat(n), ")".repeat(n))
}

fn roundtrips(src: &str) {
    let c = parse_contract(src).unwrap_or_else(|e| panic!("should parse: {e:?}"));
    let bytes = encode_contract(&c);
    assert_eq!(decode_contract(&bytes).as_ref(), Ok(&c), "encoder output must decode");
}

#[test]
fn param_256_does_not_alias_param_0() {
    let src = format!(
        "(contract eqc/0 (target (func \"f\" {})) (pre (eq $p256 (c32 0))) (post (c32 1)) (frame))",
        params(257)
    );
    assert!(parse_contract(&src).is_err(), "257 params must be refused");
    // Unused 257th parameter is refused as well: the signature itself is
    // beyond what argument references can index.
    let src = format!(
        "(contract eqc/0 (target (func \"f\" {})) (pre (c32 1)) (post (c32 1)) (frame))",
        params(257)
    );
    assert!(parse_contract(&src).is_err());
}

#[test]
fn param_255_is_the_last_referable_and_roundtrips() {
    let src = format!(
        "(contract eqc/0 (target (func \"f\" {})) (pre (eq $p255 (c32 0))) (post (c32 1)) (frame))",
        params(256)
    );
    roundtrips(&src);
    let c = parse_contract(&src).unwrap();
    assert!(format!("{:?}", c.pre).contains("Arg(255)"));
}

#[test]
fn decoder_refuses_more_params_than_can_be_indexed() {
    let src = format!(
        "(contract eqc/0 (target (func \"f\" {})) (pre (c32 1)) (post (c32 1)) (frame))",
        params(256)
    );
    let mut c = parse_contract(&src).unwrap();
    c.target.params.push(equiv_core::ast::ValType::I32);
    let bytes = encode_contract(&c);
    assert!(matches!(decode_contract(&bytes), Err(DecodeError::Validate(_))));
}

#[test]
fn duplicate_clauses_are_refused() {
    let t = "(target (func \"f\" (param $x i32) (result i32)))";
    for body in [
        "(pre (c32 1)) (post (eq result $x)) (post (c32 1)) (frame)",
        "(pre (c32 0)) (pre (c32 1)) (post (c32 1)) (frame)",
        "(pre (c32 1)) (post (c32 1)) (frame) (frame (range $x (c32 4)))",
        "(assume no-trap) (assume no-imports) (pre (c32 1)) (post (c32 1)) (frame)",
    ] {
        let src = format!("(contract eqc/0 {t} {body})");
        assert!(parse_contract(&src).is_err(), "must refuse: {body}");
    }
    let two_targets = format!("(contract eqc/0 {t} {t} (pre (c32 1)) (post (c32 1)) (frame))");
    assert!(parse_contract(&two_targets).is_err());
    let dup_name = "(contract eqc/0 (target (func \"f\" (param $x i32) (param $x i64))) (pre (c32 1)) (post (c32 1)) (frame))";
    assert!(parse_contract(dup_name).is_err());
    let h = "ab".repeat(32);
    for r in [
        format!("(refines (artifact sha256:{h}) \"g\" (over $x) (over $x))"),
        format!("(refines (artifact sha256:{h}) \"g\" (over $x) (assuming (c32 1)) (assuming (c32 0)))"),
    ] {
        let src = format!("(contract eqc/0 {t} (pre (c32 1)) (post (c32 1)) (frame) {r})");
        assert!(parse_contract(&src).is_err(), "must refuse: {r}");
    }
}

#[test]
fn absurd_nesting_is_a_clean_error() {
    let src = format!(
        "(contract eqc/0 (target (func \"f\" (param $x i32))) (pre {}) (post (c32 1)) (frame))",
        nots(200_000, "(c32 1)")
    );
    assert!(parse_contract(&src).is_err());
}

/// The deepest expression each position accepts encodes and decodes; one
/// level more is refused by the parser (and, hand-built, by the decoder).
#[test]
fn depth_limit_matches_the_decoder_in_every_position() {
    let t = "(target (func \"f\" (param $x i32)))";
    let h = "ab".repeat(32);
    // `nots(n, leaf)` nests n + 1 levels (the leaf is one).
    let cases: [(&str, usize, Box<dyn Fn(&str) -> String>); 4] = [
        ("pre", 63, Box::new(|e| format!("(pre {e}) (post (c32 1)) (frame)"))),
        ("frame", 62, Box::new(|e| format!("(pre (c32 1)) (post (c32 1)) (frame (range (c32 0) {e}))"))),
        ("over", 61, Box::new({
            let h = h.clone();
            move |e| format!("(pre (c32 1)) (post (c32 1)) (frame) (refines (artifact sha256:{h}) \"g\" (over {e}))")
        })),
        ("assuming", 62, Box::new({
            let h = h.clone();
            move |e| format!("(pre (c32 1)) (post (c32 1)) (frame) (refines (artifact sha256:{h}) \"g\" (over) (assuming {e}))")
        })),
    ];
    for (name, max, wrap) in cases {
        // frame wraps the expression in a `range`, which is one level itself.
        let own = if name == "frame" { 1 } else { 0 };
        let ok = format!("(contract eqc/0 {t} {})", wrap(&nots(max - 1 - own, "(c32 1)")));
        roundtrips(&ok);
        let over = format!("(contract eqc/0 {t} {})", wrap(&nots(max - own, "(c32 1)")));
        assert!(parse_contract(&over).is_err(), "{name}: one level past the limit must be refused");
    }
}

#[test]
fn wide_and_is_refused_or_roundtrips() {
    let t = "(target (func \"f\" (param $x i32)))";
    let and = |n: usize| {
        format!(
            "(contract eqc/0 {t} (pre (and {})) (post (c32 1)) (frame))",
            vec!["(c32 1)"; n].join(" ")
        )
    };
    // The old failure: 80 arguments parsed, encoded, and then failed to decode.
    assert!(parse_contract(&and(80)).is_err());
    assert!(parse_contract(&and(100_000)).is_err());
    // n arguments fold to n levels (n - 1 joins over a leaf).
    roundtrips(&and(63));
    assert!(parse_contract(&and(64)).is_err());
}

/// The decoder's own limit is unchanged: bytes nested past it are refused.
#[test]
fn decoder_depth_limit_unchanged() {
    let mut bytes = vec![0x81u8; 66];
    bytes.push(0x00);
    assert_eq!(equiv_core::cbor::from_bytes(&bytes), Err(CborError::TooDeep));
    let mut bytes = vec![0x81u8; 64];
    bytes.push(0x00);
    assert!(equiv_core::cbor::from_bytes(&bytes).is_ok());
}
