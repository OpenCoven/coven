//! A proposal's revision is this digest over its authority bytes, so any change
//! here re-keys every stored proposal and every recorded decision request.
use super::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest(raw: &str) -> Result<String> {
    let mut hasher = Sha256::new();
    hash_canonical_json(raw.as_bytes(), &mut hasher, 0)?;
    Ok(hex(&hasher.finalize()))
}

fn sha256(canonical: &str) -> String {
    hex(&Sha256::digest(canonical.as_bytes()))
}

/// JSON's six-character escape for "A" (backslash, `u0041`).
fn escaped_a() -> String {
    format!("{}u0041", char::from(b'\\'))
}

#[test]
fn proposal_revision_hashes_sorted_compact_objects_and_ordered_arrays() {
    let canonical = r#"{"a":["x",2],"b":{"c":null,"d":true}}"#;
    assert_eq!(digest(canonical).unwrap(), sha256(canonical));
    assert_eq!(
        digest(" { \"b\" : { \"d\" : true ,\n\"c\" : null } , \"a\" : [ \"x\" , 2 ] } ").unwrap(),
        sha256(canonical)
    );
    assert_ne!(
        digest(r#"{"a":[2,"x"],"b":{"c":null,"d":true}}"#).unwrap(),
        sha256(canonical)
    );
}

#[test]
fn proposal_revision_sorts_keys_bytewise_and_reencodes_them() {
    assert_eq!(
        digest(r#"{"b":1,"aa":2,"a":3,"B":4}"#).unwrap(),
        sha256(r#"{"B":4,"a":3,"aa":2,"b":1}"#)
    );
    let escaped_key = format!(r#"{{"{}":1}}"#, escaped_a());
    assert_eq!(digest(&escaped_key).unwrap(), sha256(r#"{"A":1}"#));
}

#[test]
fn proposal_revision_hashes_scalars_as_written() {
    let escaped_value = format!(r#"{{"a":"{}"}}"#, escaped_a());
    assert_eq!(digest(&escaped_value).unwrap(), sha256(&escaped_value));
    assert_ne!(
        digest(&escaped_value).unwrap(),
        digest(r#"{"a":"A"}"#).unwrap()
    );
    assert_ne!(digest("[1.0]").unwrap(), digest("[1]").unwrap());
    assert_eq!(digest("[1e2, -0]").unwrap(), sha256("[1e2,-0]"));
}

#[test]
fn proposal_revision_keeps_the_last_duplicate_key() {
    assert_eq!(digest(r#"{"a":1,"a":2}"#).unwrap(), sha256(r#"{"a":2}"#));
}

#[test]
fn proposal_revision_does_not_split_strings_at_escaped_quotes_or_separators() {
    assert_eq!(
        digest(r#"[ "a\",b]" , {"k":"}"} , "\\" ]"#).unwrap(),
        sha256(r#"["a\",b]",{"k":"}"},"\\"]"#)
    );
}

#[test]
fn proposal_revision_refuses_nesting_beyond_the_validated_depth() {
    // `wrappers` containers around an empty one put it at depth `wrappers`.
    let nested = |open: &str, empty: &str, close: &str, wrappers: usize| {
        format!("{}{empty}{}", open.repeat(wrappers), close.repeat(wrappers))
    };
    for (open, empty, close) in [("[", "[]", "]"), ("{\"a\":", "{}", "}")] {
        assert!(
            digest(&nested(open, empty, close, PROPOSAL_JSON_MAX_DEPTH)).is_ok(),
            "{empty} at the limit"
        );
        let error = digest(&nested(open, empty, close, PROPOSAL_JSON_MAX_DEPTH + 1)).unwrap_err();
        assert!(
            error.to_string().contains("exceeded the validated depth"),
            "{empty}: {error:#}"
        );
    }
}

#[test]
fn proposal_revision_fails_closed_on_malformed_or_empty_input() {
    for (raw, message) in [
        ("", "canonical proposal JSON is empty"),
        (" \n\t", "canonical proposal JSON is empty"),
        ("[1,2", "canonical proposal array is not terminated"),
        ("[1 2]", "canonical proposal array is not terminated"),
        ("[[1,2]", "canonical proposal array is not terminated"),
        (
            r#"["abc]"#,
            "canonical proposal JSON contains an unterminated string",
        ),
        ("[,]", "canonical proposal JSON contains an empty scalar"),
        ("[[", "canonical proposal JSON ended before a value"),
        (
            r#"[{"a" 1}]"#,
            "canonical proposal JSON object is missing `:`",
        ),
        (
            r#"[{"a":1 "b":2}]"#,
            "canonical proposal JSON object is not terminated",
        ),
        ("[[1 2]]", "canonical proposal JSON array is not terminated"),
        (r#"{"a":1"#, "parsing canonical proposal object"),
    ] {
        let error = digest(raw).expect_err(raw);
        assert!(format!("{error:#}").contains(message), "{raw:?}: {error:#}");
    }
}
