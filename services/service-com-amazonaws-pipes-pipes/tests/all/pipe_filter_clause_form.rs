// SPDX-License-Identifier: CC0-1.0

//! The FORM a filter pattern's clauses have to be written in, end to end over the wire.
//!
//! A pipe's `FilterCriteria.Filters[].Pattern` is an EventBridge event pattern, and the
//! write that stores one compiles it. A pattern's member is a CLAUSE, written in one of
//! two forms: a nested pattern object, which descends into the nested document the record
//! carries under that member, or an array of candidate values, of which any one matching
//! admits the record. A member holding a bare value is in neither form — it names nothing
//! to descend into and no candidate to compare — so the pattern is not a pattern, and both
//! writes that can configure a pipe refuse it with `ValidationException` (400) naming
//! `Pattern` and the member that carried the bare value, storing nothing.
//!
//! The rows span what the rule generalises over: where the offending member sits (the
//! pattern's top level, one and two levels down, inside a branch of a `$or`), the kind of
//! bare value it holds (string, number, boolean, null), and the forms that ARE clauses
//! (a candidate list, a list of match-operator objects, a nested object, a `$or` of
//! patterns), which the same table asserts are accepted and read back verbatim.

use serde_json::json;

use crate::pipe_configuration_rules::{assert_refused, reported_filter_criteria};
use crate::pipe_lifecycle::{create, describe, router, PAST_SETTLE};
use crate::pipe_update::{create_body, update};

const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-role";
const SOURCE_SQS: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const TARGET_LAMBDA: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";

/// One row of the clause-form table: the pattern, and the member a refusal must name —
/// `None` for a pattern every clause of which is a clause.
struct Row {
    /// The case, which is also the pipe's name.
    case: &'static str,
    pattern: &'static str,
    /// The member whose value is in neither clause form, when the row carries one.
    offender: Option<&'static str>,
}

/// The reason a refusal states after the member that carried the bare value.
fn reason(member: &str) -> String {
    format!("Invalid Event Pattern. Reason: \"{member}\" must be an object or an array.")
}

/// RULE: every member of a filter pattern is a clause — a nested pattern object or an
/// array of candidate values — so a member holding a bare value is refused with
/// `ValidationException` (400) naming `Pattern` and that member, at whatever depth it
/// sits, and nothing is stored.
#[tokio::test]
async fn a_filter_pattern_clause_that_is_a_bare_value_is_refused() {
    let rows = [
        Row {
            case: "toplevelstring",
            pattern: r#"{"body": "keep"}"#,
            offender: Some("body"),
        },
        Row {
            case: "onelevelstring",
            pattern: r#"{"body": {"tag": "keep"}}"#,
            offender: Some("tag"),
        },
        Row {
            case: "twolevelsnumber",
            pattern: r#"{"body": {"order": {"count": 3}}}"#,
            offender: Some("count"),
        },
        Row {
            case: "booleanleaf",
            pattern: r#"{"body": {"paid": true}}"#,
            offender: Some("paid"),
        },
        Row {
            case: "nullleaf",
            pattern: r#"{"body": {"tag": null}}"#,
            offender: Some("tag"),
        },
        Row {
            case: "insideanorbranch",
            pattern: r#"{"$or": [{"body": {"tag": ["keep"]}}, {"body": {"tag": "drop"}}]}"#,
            offender: Some("tag"),
        },
        Row {
            case: "bareleafbesideagoodclause",
            pattern: r#"{"messageAttributes": {"tier": {"stringValue": ["gold"]}}, "body": "keep"}"#,
            offender: Some("body"),
        },
        // The forms that ARE clauses, at the same places the rows above offend.
        Row {
            case: "candidatelist",
            pattern: r#"{"body": ["keep"]}"#,
            offender: None,
        },
        Row {
            case: "nestedcandidatelist",
            pattern: r#"{"body": {"tag": ["keep"]}}"#,
            offender: None,
        },
        Row {
            case: "operatorobjectinalist",
            pattern: r#"{"body": {"tag": [{"prefix": "ke"}]}}"#,
            offender: None,
        },
        Row {
            case: "twolevelslist",
            pattern: r#"{"body": {"order": {"count": [3]}}}"#,
            offender: None,
        },
        Row {
            case: "orofpatterns",
            pattern: r#"{"$or": [{"body": {"tag": ["keep"]}}, {"body": ["PLAIN"]}]}"#,
            offender: None,
        },
    ];

    for row in &rows {
        let router = router();
        let source_parameters = json!({
            "FilterCriteria": {"Filters": [{"Pattern": row.pattern}]},
        });
        let created = create(
            &router,
            row.case,
            create_body(
                SOURCE_SQS,
                TARGET_LAMBDA,
                json!({"SourceParameters": source_parameters}),
            ),
        )
        .await;

        let Some(offender) = row.offender else {
            assert_eq!(
                created.status, 200,
                "case {:?}: a pattern whose clauses are clauses must be accepted, \
                 got {created:?}",
                row.case
            );
            let described = describe(&router, row.case).await;
            assert_eq!(
                reported_filter_criteria(&described),
                source_parameters["FilterCriteria"],
                "case {:?}: the accepted criteria must read back verbatim, got {described:?}",
                row.case
            );
            continue;
        };

        assert_refused(row.case, &created, "Pattern", &reason(offender));
        let described = describe(&router, row.case).await;
        assert_eq!(
            described.status, 404,
            "case {:?}: a refused CreatePipe must have stored no pipe, got {described:?}",
            row.case
        );
    }
}

/// RULE: the clause-form rule holds for `UpdatePipe` too — an update whose pattern carries
/// a bare value is refused the same way and leaves the stored criteria as they were.
#[tokio::test]
async fn an_update_whose_pattern_clause_is_a_bare_value_changes_nothing() {
    let router = router();
    let stored = json!({"FilterCriteria": {"Filters": [{"Pattern": "{\"body\":{\"tag\":[\"keep\"]}}"}]}});
    let created = create(
        &router,
        "clauseformupdate",
        create_body(
            SOURCE_SQS,
            TARGET_LAMBDA,
            json!({"SourceParameters": stored}),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture pipe must be created, got {created:?}"
    );
    tokio::time::sleep(PAST_SETTLE).await;

    let refused = update(
        &router,
        "clauseformupdate",
        json!({
            "RoleArn": ROLE,
            "SourceParameters": {
                "FilterCriteria": {"Filters": [{"Pattern": "{\"body\":{\"tag\":\"keep\"}}"}]},
            },
        }),
    )
    .await;
    assert_refused("clause-form-update", &refused, "Pattern", &reason("tag"));
    let described = describe(&router, "clauseformupdate").await;
    assert_eq!(
        reported_filter_criteria(&described),
        stored["FilterCriteria"],
        "a refused update must leave the stored criteria as they were, got {described:?}"
    );

    let replacement =
        json!({"FilterCriteria": {"Filters": [{"Pattern": "{\"body\":{\"tag\":[\"other\"]}}"}]}});
    let accepted = update(
        &router,
        "clauseformupdate",
        json!({"RoleArn": ROLE, "SourceParameters": replacement}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "an update whose clauses are clauses must be accepted, got {accepted:?}"
    );
    tokio::time::sleep(PAST_SETTLE).await;
    let described = describe(&router, "clauseformupdate").await;
    assert_eq!(
        reported_filter_criteria(&described),
        replacement["FilterCriteria"],
        "the accepted criteria must read back, got {described:?}"
    );
}
