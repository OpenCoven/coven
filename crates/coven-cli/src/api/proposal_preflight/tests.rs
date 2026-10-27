use super::*;

// Moving the structural scan after serde decoding changes this rejection:
// the unsupported field must win over the invalid content encoding.
#[test]
fn proposal_preflight_structure_precedes_content_decode() {
    let error = validate_proposal_envelope_preflight(
        br#"{"edits":[{"contents":{"encoding":"unsupported","data":"x"}}],"unknown":null}"#,
    )
    .expect_err("structural rejection must precede content decoding");
    assert!(
        error.to_string().contains("unknown field `unknown`"),
        "{error:#}"
    );
    assert!(!format!("{error:#}").contains("unknown encoding"));
}

// Both collections must be checked even though nested edits supply the budget.
#[test]
fn proposal_preflight_validates_shadowed_legacy_edits() {
    let error = validate_proposal_envelope_preflight_with_limits(
        br#"{"pending":{"edits":[]},"edits":[{"contents":{"encoding":"utf8","data":"abc"}}]}"#,
        ProposalEnvelopeLimits {
            max_edits: 1,
            max_content_bytes: 2,
        },
    )
    .expect_err("shadowed edits cannot bypass memory bounds");
    assert!(matches!(
        ward::ward_edit_budget_failure(&error),
        Some(ward::WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes: 3,
            max_bytes: 2
        })
    ));
}

#[test]
fn proposal_preflight_nested_count_precedes_legacy_memory_error() {
    let error = validate_proposal_envelope_preflight_with_limits(
        br#"{"edits":[{"contents":{"encoding":"utf8","data":"abc"}}],"pending":{"edits":[{"contents":{"encoding":"utf8","data":""}},{"contents":{"encoding":"utf8","data":""}}]}}"#,
        ProposalEnvelopeLimits { max_edits: 1, max_content_bytes: 2 },
    ).expect_err("nested edit count wins independently of JSON field order");
    assert!(matches!(
        ward::ward_edit_budget_failure(&error),
        Some(ward::WardEditBudgetFailure::BatchEditCount {
            attempted_edits: 2,
            max_edits: 1
        })
    ));
}

#[test]
fn proposal_preflight_nested_budget_and_duplicate_fields() -> Result<()> {
    let budget = validate_proposal_envelope_preflight(
        br#"{"pending":{"edits":[{"contents":{"encoding":"utf8","data":"\u00e9"}}]},"edits":[{"contents":{"encoding":"base64","data":"YWJj"}}]}"#,
    )?;
    assert_eq!(
        budget.retained_content_bytes(),
        2,
        "nested UTF-8 bytes supply the budget"
    );
    let error = validate_proposal_envelope_preflight(br#"{"edits":[],"edits":[]}"#)
        .expect_err("duplicate authority fields must not become last-wins");
    assert!(
        format!("{error:#}").contains("duplicate field `edits`"),
        "{error:#}"
    );
    Ok(())
}

#[test]
fn proposal_preflight_rejects_one_huge_materialized_after_with_small_pending_edit() -> Result<()> {
    let error = validate_proposal_envelope_preflight_with_limits(
        br#"{
            "pending":{"edits":[
                {"surface":"reviewed/a.md","contents":{"encoding":"utf8","data":"x"}}
            ]},
            "materialized_diff":{"surfaces":[
                {"surface":"reviewed/a.md","before":[],"after":[1,2,3,4,5]}
            ]}
        }"#,
        ProposalEnvelopeLimits {
            max_edits: 32,
            max_content_bytes: 4,
        },
    )
    .expect_err("one oversized duplicate after-image must fail preflight");

    assert!(matches!(
        ward::ward_edit_budget_failure(&error),
        Some(ward::WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes: 5,
            max_bytes: 4,
        })
    ));
    Ok(())
}

#[test]
fn proposal_preflight_rejects_unknown_top_level_fields_before_value_allocation() {
    let raw = format!(
        r#"{{
            "edits":[
                {{"surface":"reviewed/a.md","contents":{{"encoding":"utf8","data":"x"}}}}
            ],
            "unknown":"{}"
        }}"#,
        "x".repeat(128 * 1024)
    );

    let error = validate_proposal_envelope_preflight(raw.as_bytes())
        .expect_err("unknown proposal fields must fail the streaming preflight");

    assert!(
        format!("{error:#}").contains("unknown field"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn proposal_preflight_rejects_deep_probe_detail_before_value_allocation() {
    let mut detail = "null".to_string();
    for _ in 0..40 {
        detail = format!(r#"{{"nested":{detail}}}"#);
    }
    let raw = format!(
        r#"{{
            "edits":[
                {{"surface":"reviewed/a.md","contents":{{"encoding":"utf8","data":"x"}}}}
            ],
            "probes":[{{
                "target":"reviewed/a.md",
                "surface":"reviewed/a.md",
                "baselineSha256":null,
                "proposedSha256":"{}",
                "status":"unscored",
                "results":[{{
                    "id":"size-delta",
                    "configuredSurface":"reviewed/**",
                    "configurationSha256":"{}",
                    "status":"unscored",
                    "summary":"bounded",
                    "detail":{detail}
                }}]
            }}]
        }}"#,
        "0".repeat(64),
        "0".repeat(64)
    );

    let error = validate_proposal_envelope_preflight(raw.as_bytes())
        .expect_err("deep probe detail must fail before generic Value parsing");

    assert!(
        error.to_string().contains("nesting depth"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn proposal_preflight_rejects_probe_cardinality_bomb_before_value_allocation() {
    let probes = std::iter::repeat_n("null", ward::WARD_EDIT_MAX_COUNT + 1)
        .collect::<Vec<_>>()
        .join(",");
    let raw = format!(
        r#"{{
            "edits":[
                {{"surface":"reviewed/a.md","contents":{{"encoding":"utf8","data":"x"}}}}
            ],
            "probes":[{probes}]
        }}"#
    );

    let error = validate_proposal_envelope_preflight(raw.as_bytes())
        .expect_err("probe cardinality must be bounded before generic Value parsing");

    assert!(
        error.to_string().contains("probes"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn proposal_preflight_rejects_unknown_nested_probe_fields() {
    let raw = format!(
        r#"{{
            "edits":[
                {{"surface":"reviewed/a.md","contents":{{"encoding":"utf8","data":"x"}}}}
            ],
            "probes":[{{
                "target":"reviewed/a.md",
                "surface":"reviewed/a.md",
                "baselineSha256":null,
                "proposedSha256":"{}",
                "status":"unscored",
                "results":[{{
                    "id":"size-delta",
                    "configuredSurface":"reviewed/**",
                    "configurationSha256":"{}",
                    "status":"unscored",
                    "summary":"bounded",
                    "detail":null,
                    "unknown":"not allowed"
                }}]
            }}]
        }}"#,
        "0".repeat(64),
        "0".repeat(64)
    );

    let error = validate_proposal_envelope_preflight(raw.as_bytes())
        .expect_err("unknown nested probe fields must fail strict preflight");

    assert!(
        format!("{error:#}").contains("unknown field"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn proposal_preflight_rejects_too_many_materialized_surfaces() -> Result<()> {
    let error = validate_proposal_envelope_preflight_with_limits(
        br#"{
            "pending":{"edits":[
                {"surface":"reviewed/a.md","contents":{"encoding":"utf8","data":""}}
            ]},
            "materialized_diff":{"surfaces":[
                {"surface":"reviewed/a.md","before":[],"after":[]},
                {"surface":"reviewed/b.md","before":[],"after":[]},
                {"surface":"reviewed/c.md","before":[],"after":[]}
            ]}
        }"#,
        ProposalEnvelopeLimits {
            max_edits: 2,
            max_content_bytes: 16,
        },
    )
    .expect_err("materialized surface cardinality must fail preflight");

    assert!(matches!(
        ward::ward_edit_budget_failure(&error),
        Some(ward::WardEditBudgetFailure::BatchEditCount {
            attempted_edits: 3,
            max_edits: 2,
        })
    ));
    Ok(())
}

#[test]
fn proposal_preflight_rejects_materialized_after_aggregate_overflow() -> Result<()> {
    let error = validate_proposal_envelope_preflight_with_limits(
        br#"{
            "pending":{"edits":[
                {"surface":"reviewed/a.md","contents":{"encoding":"utf8","data":"ab"}},
                {"surface":"reviewed/b.md","contents":{"encoding":"utf8","data":"cd"}}
            ]},
            "materialized_diff":{"surfaces":[
                {"surface":"reviewed/a.md","before":[],"after":[97,98]},
                {"surface":"reviewed/b.md","before":[],"after":[99,100,101]}
            ]}
        }"#,
        ProposalEnvelopeLimits {
            max_edits: 32,
            max_content_bytes: 4,
        },
    )
    .expect_err("individually bounded after-images must share an aggregate");

    assert!(matches!(
        ward::ward_edit_budget_failure(&error),
        Some(ward::WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes: 5,
            max_bytes: 4,
        })
    ));
    Ok(())
}

#[test]
fn proposal_preflight_aggregate_accounting_is_overflow_safe() -> Result<()> {
    let error = validate_combined_proposal_bytes(
        u64::MAX,
        1,
        ProposalEnvelopeLimits {
            max_edits: usize::MAX,
            max_content_bytes: u64::MAX,
        },
    )
    .expect_err("integer overflow must fail even at the numeric ceiling");

    assert!(matches!(
        ward::ward_edit_budget_failure(&error),
        Some(ward::WardEditBudgetFailure::BatchRetainedMemory {
            attempted_bytes: u64::MAX,
            max_bytes: u64::MAX,
        })
    ));
    Ok(())
}

#[test]
fn proposal_preflight_accepts_exact_duplicate_content_boundaries() -> Result<()> {
    let budget = validate_proposal_envelope_preflight_with_limits(
        br#"{
            "pending":{"edits":[
                {"surface":"reviewed/a.md","contents":{"encoding":"utf8","data":""}}
            ]},
            "materialized_diff":{"surfaces":[
                {"surface":"reviewed/a.md","before":[1,2,3,4],"after":[]}
            ]},
            "region_evidence":[
                {"replay_bytes":[1,2,3,4]}
            ],
            "decisionState":{"beforeImages":[
                {"target":"reviewed/a.md","contents":{"encoding":"base64","data":"AQIDBA=="}}
            ]}
        }"#,
        ProposalEnvelopeLimits {
            max_edits: 1,
            max_content_bytes: 4,
        },
    )?;

    assert_eq!(budget.retained_content_bytes(), 4);
    Ok(())
}
