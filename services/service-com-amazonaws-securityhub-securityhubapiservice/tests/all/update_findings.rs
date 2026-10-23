// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the deprecated `UpdateFindings`: the note it writes to
//! every finding its `Filters` select, the `RecordState` it does NOT write, the filter
//! rules it refuses a request by, and the subscription it requires.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter` — and
//! `UpdateFindings` shares its URI with `GetFindings` (`PATCH /findings` against
//! `POST /findings`), so a route or a method that is wrong on the wire fails here. The
//! emulator harness and the finding builder are the ones `security_hub_core_loop`
//! already defines.

use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::RECORD_STATE_VALUES;

use crate::security_hub_core_loop::{
    base_finding, finding_with_id, returned_ids, Harness, PRODUCT_ARN,
};

/// One row of the selection table: the case name, the `Filters` the call carries, and
/// the ids of the seeded findings the note must land on.
type SelectionCase = (&'static str, Value, &'static [&'static str]);

/// One row of the refusal table: the case name, the `Filters` the call carries, and
/// the status the request must be refused with.
type RefusalCase = (&'static str, Value, u16);

/// `UpdateFindings` is `PATCH /findings` (`smithy_query(query="operation",
/// operation="UpdateFindings")`), which no harness helper covers: `update_findings` on
/// the shared harness is `BatchUpdateFindings` at `PATCH /findings/batchupdate`.
async fn update_findings(harness: &Harness, body: Value) -> (u16, Value) {
    harness.call("PATCH", "/findings", Some(body)).await
}

/// Every stored finding, newest first, read back through `GetFindings`.
async fn all_findings(harness: &Harness, case: &str) -> Value {
    let (status, body) = harness.get_findings(json!({ "MaxResults": 100 })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetFindings expected 200, got {status} with {body}"
    );
    body
}

/// The three findings every selection row runs against: they differ in the generator,
/// severity, title, workflow status and confidence a filter can select on, so one row
/// per filtered attribute picks a different subset of the same collection.
fn seeded_findings() -> Vec<Value> {
    let mut alpha = base_finding("alpha");
    alpha["GeneratorId"] = json!("generator/alpha");
    alpha["Title"] = json!("CloudFront distribution is public");
    alpha["Severity"] = json!({ "Label": "HIGH" });
    alpha["Workflow"] = json!({ "Status": "NEW" });
    alpha["Confidence"] = json!(90);

    let mut beta = base_finding("beta");
    beta["GeneratorId"] = json!("generator/beta");
    beta["Title"] = json!("S3 bucket allows public read");
    beta["Severity"] = json!({ "Label": "LOW" });
    beta["Workflow"] = json!({ "Status": "NOTIFIED" });
    beta["Confidence"] = json!(40);

    let mut gamma = base_finding("gamma");
    gamma["GeneratorId"] = json!("other/gamma");
    gamma["Title"] = json!("IAM policy is overly permissive");
    gamma["Severity"] = json!({ "Label": "LOW" });
    gamma["Workflow"] = json!({ "Status": "RESOLVED" });
    gamma["Confidence"] = json!(10);

    vec![alpha, beta, gamma]
}

/// Every id [`seeded_findings`] seeds.
const SEEDED_IDS: [&str; 3] = ["alpha", "beta", "gamma"];

/// The `Note` of a finding, or `None` when it carries none.
fn note_of(finding: &Value) -> Option<Value> {
    finding.get("Note").filter(|note| !note.is_null()).cloned()
}

/// Does `text` parse as the ISO-8601 `date-time` a Security Hub CSPM timestamp
/// member carries?
fn is_iso8601(text: &str) -> bool {
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .is_ok()
}

// ---------------------------------------------------------------------------
// The note the operation writes
// ---------------------------------------------------------------------------

/// RULE: `UpdateFindings` writes the requested `Note` to every finding of the
/// caller's hub that its `Filters` select — the same predicate `GetFindings` applies
/// to the same `AwsSecurityFindingFilters` — and to no other finding, answering the
/// modelled empty response; the service stamps `Note.UpdatedAt` itself, because
/// `NoteUpdate` carries only `Text` and `UpdatedBy`.
///
/// The table spans the forms a selection can take over one collection: a filter on an
/// id, on a prefix, on two values of one attribute, on substring, on a negated value,
/// on a numeric bound, on a timestamp window, two attributes joined, a filter member
/// set to nothing, and a filter that matches no finding at all. Each row runs on a
/// fresh hub seeded with the same three findings.
#[tokio::test]
async fn a_note_lands_on_exactly_the_findings_the_filters_select() {
    let cases: [SelectionCase; 10] = [
        (
            "Id EQUALS one finding",
            json!({ "Id": [{ "Value": "beta", "Comparison": "EQUALS" }] }),
            &["beta"],
        ),
        (
            "GeneratorId PREFIX",
            json!({ "GeneratorId": [{ "Value": "generator/", "Comparison": "PREFIX" }] }),
            &["alpha", "beta"],
        ),
        (
            "SeverityLabel EQUALS either of two values",
            json!({ "SeverityLabel": [
                { "Value": "LOW", "Comparison": "EQUALS" },
                { "Value": "CRITICAL", "Comparison": "EQUALS" },
            ] }),
            &["beta", "gamma"],
        ),
        (
            "Title CONTAINS a substring",
            json!({ "Title": [{ "Value": "public", "Comparison": "CONTAINS" }] }),
            &["alpha", "beta"],
        ),
        (
            "WorkflowStatus NOT_EQUALS",
            json!({ "WorkflowStatus": [{ "Value": "NEW", "Comparison": "NOT_EQUALS" }] }),
            &["beta", "gamma"],
        ),
        (
            "Confidence above a bound",
            json!({ "Confidence": [{ "Gte": 40 }] }),
            &["alpha", "beta"],
        ),
        (
            "CreatedAt inside a window",
            json!({ "CreatedAt": [{
                "Start": "2026-10-21T19:00:00Z",
                "End": "2026-10-21T21:00:00Z",
            }] }),
            &["alpha", "beta", "gamma"],
        ),
        (
            "two attributes, joined",
            json!({
                "GeneratorId": [{ "Value": "generator/", "Comparison": "PREFIX" }],
                "SeverityLabel": [{ "Value": "LOW", "Comparison": "EQUALS" }],
            }),
            &["beta"],
        ),
        ("a Filters that sets no member", json!({}), &SEEDED_IDS),
        (
            "a filter no finding satisfies",
            json!({ "Id": [{ "Value": "no-such-finding", "Comparison": "EQUALS" }] }),
            &[],
        ),
    ];

    for (case, filters, expected) in cases {
        let harness = Harness::with_findings(seeded_findings()).await;
        let (status, body) = update_findings(
            &harness,
            json!({
                "Filters": filters,
                "Note": { "Text": "triaged by the duty engineer", "UpdatedBy": "analyst-1" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateFindings expected 200, got {status} with {body}"
        );
        // `UpdateFindingsResponse` models no members, so the only correct body is an
        // empty one.
        assert!(
            body.is_null() || body == json!({}),
            "case {case:?}: expected the modelled empty response, got {body}"
        );

        let read_back = all_findings(&harness, case).await;
        let ids = returned_ids(&read_back);
        assert_eq!(
            ids.len(),
            SEEDED_IDS.len(),
            "case {case:?}: UpdateFindings must not add or drop findings, got {ids:?}"
        );
        for id in SEEDED_IDS {
            let finding = finding_with_id(&read_back, id);
            let note = note_of(&finding);
            if expected.contains(&id) {
                let note = note.unwrap_or_else(|| {
                    panic!("case {case:?}: finding {id:?} must carry the note, got {finding}")
                });
                assert_eq!(
                    note["Text"].as_str(),
                    Some("triaged by the duty engineer"),
                    "case {case:?}: finding {id:?} note text, got {note}"
                );
                assert_eq!(
                    note["UpdatedBy"].as_str(),
                    Some("analyst-1"),
                    "case {case:?}: finding {id:?} note author, got {note}"
                );
                let updated_at = note["UpdatedAt"].as_str().unwrap_or_default();
                assert!(
                    is_iso8601(updated_at),
                    "case {case:?}: finding {id:?} Note.UpdatedAt must be the ISO-8601 instant \
                     the service stamped, got {updated_at:?}"
                );
            } else {
                assert!(
                    note.is_none(),
                    "case {case:?}: finding {id:?} was not selected, so it must carry no note, \
                     got {note:?}"
                );
            }
        }
    }
}

/// RULE: a later `UpdateFindings` replaces the note a previous one wrote, so the
/// finding carries one note — the most recent — whoever wrote it.
#[tokio::test]
async fn a_later_note_replaces_the_one_before_it() {
    let harness = Harness::with_findings(seeded_findings()).await;
    let writes: [(&str, &str); 3] = [
        ("first triage", "analyst-1"),
        ("escalated to the service team", "analyst-2"),
        ("closed as a false positive", "analyst-3"),
    ];

    for (text, author) in writes {
        let case = text;
        let (status, body) = update_findings(
            &harness,
            json!({
                "Filters": { "Id": [{ "Value": "alpha", "Comparison": "EQUALS" }] },
                "Note": { "Text": text, "UpdatedBy": author },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateFindings expected 200, got {status} with {body}"
        );

        let read_back = all_findings(&harness, case).await;
        let note = note_of(&finding_with_id(&read_back, "alpha")).unwrap_or_else(|| {
            panic!("case {case:?}: finding alpha must carry a note, got {read_back}")
        });
        assert_eq!(
            note["Text"].as_str(),
            Some(text),
            "case {case:?}: expected the newest note text, got {note}"
        );
        assert_eq!(
            note["UpdatedBy"].as_str(),
            Some(author),
            "case {case:?}: expected the newest note author, got {note}"
        );
        // The other seeded findings were never selected, so none of them has a note.
        for id in ["beta", "gamma"] {
            let other = note_of(&finding_with_id(&read_back, id));
            assert!(
                other.is_none(),
                "case {case:?}: finding {id:?} must carry no note, got {other:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RecordState is not a member a caller writes
// ---------------------------------------------------------------------------

/// RULE: a finding's `RecordState` is owned by the finding provider (which sets it
/// through `BatchImportFindings`) and by Security Hub CSPM itself, never by a caller
/// updating a finding: neither `BatchUpdateFindingsRequest` nor an automation rule's
/// `AutomationRulesFindingFieldsUpdate` — the supported update surfaces — models a
/// `RecordState` member at all. So a `UpdateFindings` request naming `RecordState`
/// succeeds and leaves every selected finding's record state exactly as it was, while
/// the `Note` in the same request does land.
///
/// The table is every published `RecordState` as the provider imported it, crossed
/// with every published `RecordState` the request asks for — the catalog is the
/// crate's generated wire-value list for the enum (`src/ops/types/shared_types.rs`),
/// so it spans the enum as the model defines it — and each cross runs once with a note
/// in the same request and once without.
#[tokio::test]
async fn record_state_is_not_changed_by_update_findings() {
    for imported in RECORD_STATE_VALUES {
        for requested in RECORD_STATE_VALUES {
            for with_note in [true, false] {
                let case = format!(
                    "imported {imported}, requested {requested}, with_note {with_note}"
                );
                let mut finding = base_finding("subject");
                finding["RecordState"] = json!(imported);
                let harness = Harness::with_findings(vec![finding]).await;

                let mut request = json!({
                    "Filters": { "Id": [{ "Value": "subject", "Comparison": "EQUALS" }] },
                    "RecordState": requested,
                });
                if with_note {
                    request["Note"] = json!({ "Text": "archive me", "UpdatedBy": "analyst-1" });
                }
                let (status, body) = update_findings(&harness, request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateFindings expected 200, got {status} with {body}"
                );

                let read_back = all_findings(&harness, &case).await;
                let stored = finding_with_id(&read_back, "subject");
                assert_eq!(
                    stored["RecordState"].as_str(),
                    Some(*imported),
                    "case {case:?}: RecordState must stay as the provider set it, got {stored}"
                );
                let note = note_of(&stored);
                assert_eq!(
                    note.is_some(),
                    with_note,
                    "case {case:?}: the note in the same request must land, got {note:?}"
                );
            }
        }
    }
}

/// RULE: `RecordState` is an enum member, so a value outside `ACTIVE | ARCHIVED` is
/// refused at the request boundary (HTTP 400) and writes nothing.
///
/// The rows are values just outside the published set: the wrong case, a value from
/// another lifecycle enum, and the empty string.
#[tokio::test]
async fn a_record_state_outside_the_enum_is_refused() {
    for value in ["active", "SUPPRESSED", ""] {
        let case = format!("RecordState {value:?}");
        let harness = Harness::with_findings(seeded_findings()).await;
        let (status, body) = update_findings(
            &harness,
            json!({
                "Filters": {},
                "RecordState": value,
                "Note": { "Text": "should not land", "UpdatedBy": "analyst-1" },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );

        let read_back = all_findings(&harness, &case).await;
        for id in SEEDED_IDS {
            let note = note_of(&finding_with_id(&read_back, id));
            assert!(
                note.is_none(),
                "case {case:?}: a refused request writes nothing, but {id:?} carries {note:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request whose `Filters` the model's own rules reject is refused with the
/// modelled `InvalidInputException` (400), and one naming a filter member nothing
/// evaluates is answered with 501 — the same answers `GetFindings` gives the same
/// filters, because a write may no more act on a predicate nothing applied than a read
/// may report one. Either way nothing is written.
#[tokio::test]
async fn a_filter_the_rules_reject_refuses_the_whole_call() {
    let cases: [RefusalCase; 8] = [
        (
            "CONTAINS with NOT_CONTAINS on one field",
            json!({ "Title": [
                { "Value": "a", "Comparison": "CONTAINS" },
                { "Value": "b", "Comparison": "NOT_CONTAINS" },
            ] }),
            400,
        ),
        (
            "EQUALS with NOT_EQUALS on one field",
            json!({ "Id": [
                { "Value": "alpha", "Comparison": "EQUALS" },
                { "Value": "beta", "Comparison": "NOT_EQUALS" },
            ] }),
            400,
        ),
        (
            "CONTAINS_WORD is a V2-only comparison",
            json!({ "Title": [{ "Value": "public", "Comparison": "CONTAINS_WORD" }] }),
            400,
        ),
        (
            "a StringFilter that carries no Value",
            json!({ "Id": [{ "Comparison": "EQUALS" }] }),
            400,
        ),
        (
            "a DateFilter.Start that is not a timestamp",
            json!({ "CreatedAt": [{ "Start": "yesterday" }] }),
            400,
        ),
        (
            "more than twenty values on one attribute",
            json!({ "Id": (0..21).map(|i| json!({ "Value": format!("id-{i}") })).collect::<Vec<Value>>() }),
            400,
        ),
        (
            "a filter member nothing evaluates",
            json!({ "ResourceId": [{ "Value": "i-0123456789abcdef0", "Comparison": "EQUALS" }] }),
            501,
        ),
        (
            "a Keyword filter nothing evaluates",
            json!({ "Keyword": [{ "Value": "public" }] }),
            501,
        ),
    ];

    for (case, filters, expected_status) in cases {
        let harness = Harness::with_findings(seeded_findings()).await;
        let (status, body) = update_findings(
            &harness,
            json!({
                "Filters": filters,
                "Note": { "Text": "should not land", "UpdatedBy": "analyst-1" },
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );

        let read_back = all_findings(&harness, case).await;
        for id in SEEDED_IDS {
            let note = note_of(&finding_with_id(&read_back, id));
            assert!(
                note.is_none(),
                "case {case:?}: a refused request writes nothing, but {id:?} carries {note:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The bound on the note's text
// ---------------------------------------------------------------------------

/// RULE: a note is caller-sized text the service copies onto every finding the write
/// selects, and `Note$Text` is bounded — "Length Constraints: Minimum of 1. Maximum of
/// 512." (`smithy_query(query="doc", shape="Note", member="Text")`) — so a note within
/// the bound is written to every selected finding and one past it refuses the whole
/// call with `InvalidInputException` (400), writing nothing.
///
/// The rows are each published bound and one past each: the minimum (1 character), one
/// below it (the empty string, which `NonEmptyString`'s `@pattern("\\S")` refuses at
/// the boundary), the maximum (512) and one past it (513), plus a multi-byte text at
/// the maximum, since the bound counts characters.
#[tokio::test]
async fn a_note_longer_than_the_documented_bound_is_refused() {
    // The published maximum of `Note$Text`, cited above.
    const MAX_NOTE_TEXT_LEN: usize = 512;
    let cases: [(&str, String, u16); 5] = [
        ("one character, the published minimum", "a".to_string(), 200),
        ("the empty string, one below the minimum", String::new(), 400),
        (
            "the published maximum",
            "n".repeat(MAX_NOTE_TEXT_LEN),
            200,
        ),
        (
            "one character past the maximum",
            "n".repeat(MAX_NOTE_TEXT_LEN + 1),
            400,
        ),
        (
            "multi-byte characters up to the maximum",
            "é".repeat(MAX_NOTE_TEXT_LEN),
            200,
        ),
    ];

    for (case, text, expected_status) in cases {
        let harness = Harness::with_findings(seeded_findings()).await;
        let (status, body) = update_findings(
            &harness,
            json!({
                "Filters": {},
                "Note": { "Text": text, "UpdatedBy": "analyst-1" },
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );

        let read_back = all_findings(&harness, case).await;
        for id in SEEDED_IDS {
            let note = note_of(&finding_with_id(&read_back, id));
            if expected_status == 200 {
                let note = note.unwrap_or_else(|| {
                    panic!("case {case:?}: finding {id:?} must carry the note, got {read_back}")
                });
                assert_eq!(
                    note["Text"].as_str().map(str::chars).map(Iterator::count),
                    Some(text.chars().count()),
                    "case {case:?}: finding {id:?} must carry the note text in full, got {note}"
                );
            } else {
                assert!(
                    note.is_none(),
                    "case {case:?}: a refused request writes nothing, but {id:?} carries {note:?}"
                );
            }
        }
    }
}

/// RULE: every operation that writes a note holds it to the same documented bound, so
/// a note past it is refused on the `BatchUpdateFindings` path too (400) and on the
/// provider's own import path per finding (`FailedFindings` with `ErrorCode`
/// `InvalidInput`, the finding not stored).
#[tokio::test]
async fn the_note_bound_holds_on_every_path_that_writes_a_note() {
    // One past the published maximum of `Note$Text`.
    let too_long = "n".repeat(513);

    let harness = Harness::with_findings(seeded_findings()).await;
    let (status, body) = harness
        .update_findings(json!({
            "FindingIdentifiers": [{ "Id": "alpha", "ProductArn": PRODUCT_ARN }],
            "Note": { "Text": too_long.clone(), "UpdatedBy": "analyst-1" },
        }))
        .await;
    assert_eq!(
        status, 400,
        "case \"BatchUpdateFindings\": expected 400, got {status} with {body}"
    );
    let read_back = all_findings(&harness, "BatchUpdateFindings").await;
    let note = note_of(&finding_with_id(&read_back, "alpha"));
    assert!(
        note.is_none(),
        "case \"BatchUpdateFindings\": a refused request writes nothing, got {note:?}"
    );

    let mut imported = base_finding("imported-with-a-long-note");
    imported["Note"] = json!({
        "Text": too_long,
        "UpdatedBy": "provider",
        "UpdatedAt": "2026-10-21T21:30:00Z",
    });
    let (status, body) = harness.import(vec![imported]).await;
    assert_eq!(
        status, 200,
        "case \"BatchImportFindings\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        (
            body["SuccessCount"].as_u64(),
            body["FailedCount"].as_u64()
        ),
        (Some(0), Some(1)),
        "case \"BatchImportFindings\": the finding must be reported per finding, got {body}"
    );
    assert_eq!(
        body["FailedFindings"][0]["ErrorCode"].as_str(),
        Some("InvalidInput"),
        "case \"BatchImportFindings\": expected the per-finding InvalidInput code, got {body}"
    );
    let read_back = all_findings(&harness, "BatchImportFindings").await;
    let ids = returned_ids(&read_back);
    assert!(
        !ids.contains(&"imported-with-a-long-note".to_string()),
        "case \"BatchImportFindings\": a rejected finding must not be stored, got {ids:?}"
    );
}

/// RULE: every finding operation of this service answers an account that is not
/// subscribed to Security Hub CSPM in the request's Region with the modelled
/// `InvalidAccessException` (HTTP 401), so `UpdateFindings` does too — before and
/// after a subscription is removed.
#[tokio::test]
async fn an_unsubscribed_account_is_refused() {
    let never_enabled = Harness::new();
    let (status, body) = update_findings(
        &never_enabled,
        json!({
            "Filters": {},
            "Note": { "Text": "no hub here", "UpdatedBy": "analyst-1" },
        }),
    )
    .await;
    assert_eq!(
        status, 401,
        "case \"never enabled\": expected 401, got {status} with {body}"
    );

    let disabled = Harness::with_findings(seeded_findings()).await;
    let (status, body) = disabled.disable().await;
    assert_eq!(
        status, 200,
        "case \"after disable\": DisableSecurityHub expected 200, got {status} with {body}"
    );
    let (status, body) = update_findings(
        &disabled,
        json!({
            "Filters": {},
            "Note": { "Text": "hub is gone", "UpdatedBy": "analyst-1" },
        }),
    )
    .await;
    assert_eq!(
        status, 401,
        "case \"after disable\": expected 401, got {status} with {body}"
    );
}
