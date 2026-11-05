// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateLensReview`, read back through `GetLensReview`: what
//! a caller writes to ONE lens review of ONE workload.
//!
//! The rules these pin, over TABLES of inputs rather than one recorded request:
//!
//! * an update REPLACES exactly the members the request supplies — the review's notes, the
//!   pillar notes map and the Jira question selection — leaves every member it omits as
//!   stored, and the lens review a read reports is the one the last update wrote, by
//!   either spelling of the lens (`LensAlias` is "either the lens alias … or the lens
//!   ARN");
//! * an identifier a request supplies has to name something of the lens UNDER REVIEW
//!   where this emulator holds that lens's content: a `PillarNotes` key and a selected
//!   pillar must name a pillar the lens is organized into, and a selected question must be
//!   one that pillar asks. One that does not is a modelled 400 `ValidationException`
//!   naming the offending member, and nothing is stored. For a lens this emulator holds no
//!   content for there is no pillar or question set to judge an identifier against, so the
//!   value is stored as given and read back;
//! * every other member of the lens review stays DERIVED on each read — the lens's own
//!   name, version, status and ARN, one pillar summary per pillar of the lens in the
//!   lens's own order, and the `Risk` roll-ups of the lens's and each pillar's questions
//!   from the workload's stored answers;
//! * a lens review exists exactly while the lens is associated with the workload, so a
//!   lens the workload does not carry, and a `WorkloadId` nothing is stored under, both
//!   resolve to a modelled 404 — from the write as well as from the read;
//! * a milestone records the workload at an instant, so notes written after one is
//!   recorded are not in the review that milestone froze.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::workload_harness::{
    answered, assert_field_validation, created_id, encode_path_segment, minimal_create,
    official_lens_arn, Answer, Client,
};

/// The pillars of the AWS Well-Architected Framework lens, in the order the framework
/// presents them: the `PillarId` each is named by on the wire and the `PillarName` the
/// framework publishes for it. Spelled out here — the members are
/// `smithy_query(query="shape", shape="PillarReviewSummary")` — so the tests pin the
/// catalog rather than reading it back out of the code under test.
const FRAMEWORK_PILLARS: [(&str, &str); 6] = [
    ("operationalExcellence", "Operational Excellence"),
    ("security", "Security"),
    ("reliability", "Reliability"),
    ("performance", "Performance Efficiency"),
    ("costOptimization", "Cost Optimization"),
    ("sustainability", "Sustainability"),
];

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes that this emulator's AWS-official lens catalog carries no
/// content for, so it is organized into no pillars and asks nothing here (the catalog
/// itself is pinned by `super::official_lens_catalog`).
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// The Region the shared harness signs its requests for, which is the Region an
/// AWS-official lens's `LensArn` is reported in.
const REGION: &str = "us-west-2";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The longest `Notes` the model accepts: `Notes` is `@length(max: 2084)`
/// (`smithy_query(query="operation", operation="UpdateLensReview")`).
const NOTES_MAX: usize = 2084;

/// The `ValidationExceptionReason` a refused member value is classified under, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// `UpdateLensReview`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}`. The lens
/// travels percent-encoded in ONE path segment, exactly as an SDK sends an ARN-spelled
/// alias.
async fn update_lens_review(
    client: &Client,
    workload_id: &str,
    lens: &str,
    body: Value,
) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            Some(body),
        )
        .await
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`, with the
/// optional `MilestoneNumber` bound to the query string.
async fn get_lens_review(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone_number: Option<i64>,
) -> Answer {
    let suffix = milestone_number
        .map(|number| format!("?MilestoneNumber={number}"))
        .unwrap_or_default();
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}{suffix}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await
}

/// A workload carrying exactly `lens`, and its `WorkloadId`.
async fn workload_with_lens(client: &Client, name: &str, lens: &str) -> String {
    let mut body = minimal_create(name, &format!("tok-{name}"));
    body["Lenses"] = json!([lens]);
    created_id(&client.create(body).await, name)
}

/// The `LensReview` of a successful answer, with the `WorkloadId` it reports checked
/// against the workload the request named.
fn lens_review(answer: &Answer, workload_id: &str, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("WorkloadId").and_then(Value::as_str),
        Some(workload_id),
        "case {case:?}: WorkloadId must be the workload under review, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"))
}

/// The `Notes` a lens review or a pillar summary carries, `None` when it carries none.
fn notes(owner: &Value) -> Option<&str> {
    owner.get("Notes").and_then(Value::as_str)
}

/// The `PillarReviewSummaries` of a lens review.
fn pillar_summaries(review: &Value, case: &str) -> Vec<Value> {
    review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PillarReviewSummaries must be present, got {review:?}")
        })
}

/// A lens review's `UpdatedAt`, the restJson1 epoch-seconds timestamp.
fn updated_at(review: &Value, case: &str) -> f64 {
    review
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: UpdatedAt must be present, got {review:?}"))
}

/// One `RiskCounts` map of a response, as a sorted bucket table.
fn risk_counts(owner: &Value, case: &str) -> BTreeMap<String, i64> {
    owner
        .get("RiskCounts")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(risk, count)| (risk.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_else(|| panic!("case {case:?}: RiskCounts must be present, got {owner:?}"))
}

/// The `JiraConfiguration` a lens review carries, `None` when it carries none.
fn jira_configuration(review: &Value) -> Option<&Value> {
    review.get("JiraConfiguration")
}

/// Assert that a refusal is the modelled 400 `ValidationException` naming `member`.
fn assert_refused(answer: &Answer, member: &str, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected a ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Reason").and_then(Value::as_str),
        Some(REASON_FIELD_VALIDATION_FAILED),
        "case {case:?}: a refused member value is a field validation failure, got {answer:?}"
    );
    assert_eq!(
        answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("Name").and_then(Value::as_str))
                    .collect::<Vec<_>>()
            }),
        Some(vec![member]),
        "case {case:?}: the finding must name the offending member, got {answer:?}"
    );
}

/// The notes a request supplies are the notes both the write and the following read
/// report, over a TABLE of note values spanning what `Notes` accepts: empty, a
/// one-character note, punctuation and newlines, and the published maximum length — read
/// back by EITHER spelling of the lens.
#[tokio::test]
async fn lens_notes_round_trip_over_the_accepted_note_values() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "lens-notes", FRAMEWORK_LENS).await;
    let lens_arn = official_lens_arn(REGION, FRAMEWORK_LENS);

    let longest = "n".repeat(NOTES_MAX);
    let cases: [(&str, &str); 4] = [
        ("empty", ""),
        ("one-character", "x"),
        ("punctuation-and-newlines", "line one.\nline two — done!"),
        ("published-maximum", longest.as_str()),
    ];

    for (case, value) in cases {
        let written = update_lens_review(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            json!({ "LensNotes": value }),
        )
        .await;
        let review = lens_review(&written, &workload_id, case);
        assert_eq!(
            notes(&review),
            Some(value),
            "case {case:?}: the update must report the notes it wrote, got {review:?}"
        );

        // The same review, read back by the alias and by the ARN the alias resolves to.
        for spelling in [FRAMEWORK_LENS, lens_arn.as_str()] {
            let read = get_lens_review(&client, &workload_id, spelling, None).await;
            let review = lens_review(&read, &workload_id, case);
            assert_eq!(
                notes(&review),
                Some(value),
                "case {case:?}: a read by {spelling:?} must report the stored notes, got \
                 {review:?}"
            );
        }
    }

    // A write through the ARN spelling reaches the SAME review the alias names.
    let written = update_lens_review(
        &client,
        &workload_id,
        &lens_arn,
        json!({ "LensNotes": "written through the ARN" }),
    )
    .await;
    lens_review(&written, &workload_id, "arn-write");
    let read = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let review = lens_review(&read, &workload_id, "arn-write");
    assert_eq!(
        notes(&review),
        Some("written through the ARN"),
        "case \"arn-write\": either spelling names one review, got {review:?}"
    );
}

/// `Notes` is accepted up to its published maximum and refused one past it, and the
/// refused request stores nothing.
#[tokio::test]
async fn notes_past_the_published_maximum_are_refused() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "notes-bound", FRAMEWORK_LENS).await;

    let cases: [(&str, usize, u16); 3] = [
        ("one-under-the-maximum", NOTES_MAX - 1, 200),
        ("at-the-maximum", NOTES_MAX, 200),
        ("one-past-the-maximum", NOTES_MAX + 1, 400),
    ];

    for (case, length, expected) in cases {
        let value = "n".repeat(length);
        let answer = update_lens_review(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            json!({ "LensNotes": value }),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
    }

    let read = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let review = lens_review(&read, &workload_id, "after-the-refusal");
    assert_eq!(
        notes(&review).map(str::len),
        Some(NOTES_MAX),
        "case \"after-the-refusal\": a refused update must store nothing, got {review:?}"
    );
}

/// A supplied `PillarNotes` map REPLACES the stored one, and each pillar summary reports
/// the notes the map gives for its own pillar and none where the map gives it none — over
/// a TABLE of maps spanning no pillar, one, several and every pillar of the lens.
#[tokio::test]
async fn pillar_notes_replace_the_stored_map() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "pillar-notes", FRAMEWORK_LENS).await;

    let every: Vec<(&str, String)> = FRAMEWORK_PILLARS
        .iter()
        .map(|(pillar_id, _)| (*pillar_id, format!("notes for {pillar_id}")))
        .collect();
    let cases: [(&str, Vec<(&str, String)>); 4] = [
        ("none", Vec::new()),
        (
            "one",
            vec![("security", "watch the perimeter".to_string())],
        ),
        (
            "three",
            vec![
                ("reliability", "test the failovers".to_string()),
                ("sustainability", String::new()),
                ("performance", "measure first".to_string()),
            ],
        ),
        ("every", every),
    ];

    for (case, entries) in cases {
        let supplied: BTreeMap<&str, String> = entries.iter().cloned().collect();
        let written = update_lens_review(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            json!({ "PillarNotes": supplied }),
        )
        .await;
        let written_review = lens_review(&written, &workload_id, case);

        let read = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
        let read_review = lens_review(&read, &workload_id, case);

        for review in [&written_review, &read_review] {
            let summaries = pillar_summaries(review, case);
            assert_eq!(
                summaries.len(),
                FRAMEWORK_PILLARS.len(),
                "case {case:?}: one summary per pillar of the lens, got {summaries:?}"
            );
            for (summary, (pillar_id, pillar_name)) in summaries.iter().zip(FRAMEWORK_PILLARS) {
                assert_eq!(
                    summary.get("PillarId").and_then(Value::as_str),
                    Some(pillar_id),
                    "case {case:?}: the pillars must come in the lens's own order, got \
                     {summaries:?}"
                );
                assert_eq!(
                    summary.get("PillarName").and_then(Value::as_str),
                    Some(pillar_name),
                    "case {case:?}: pillar {pillar_id:?} must report its published name, got \
                     {summary:?}"
                );
                assert_eq!(
                    notes(summary),
                    supplied.get(pillar_id).map(String::as_str),
                    "case {case:?}: pillar {pillar_id:?} must report exactly the supplied \
                     notes, got {summary:?}"
                );
            }
        }
    }
}

/// A member the request omits keeps its stored value, so the three members this operation
/// writes are written independently; a request that supplies none of them changes nothing
/// at all, not even the review's `UpdatedAt`.
#[tokio::test]
async fn an_omitted_member_keeps_its_stored_value() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "partial-update", FRAMEWORK_LENS).await;

    let seeded = update_lens_review(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        json!({
            "LensNotes": "the lens note",
            "PillarNotes": { "security": "the security note" },
            "JiraConfiguration": {
                "SelectedPillars": [{ "PillarId": "security", "SelectedQuestionIds": ["identities"] }],
            },
        }),
    )
    .await;
    let seeded_review = lens_review(&seeded, &workload_id, "all-three");
    let seeded_at = updated_at(&seeded_review, "all-three");

    // Supplying only `LensNotes` leaves the pillar notes and the Jira selection as stored.
    let lens_only = update_lens_review(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        json!({ "LensNotes": "a newer lens note" }),
    )
    .await;
    let review = lens_review(&lens_only, &workload_id, "lens-notes-only");
    assert_eq!(
        notes(&review),
        Some("a newer lens note"),
        "case \"lens-notes-only\": the supplied member must be replaced, got {review:?}"
    );
    let summaries = pillar_summaries(&review, "lens-notes-only");
    assert_eq!(
        notes(&summaries[1]),
        Some("the security note"),
        "case \"lens-notes-only\": the omitted PillarNotes must be left as stored, got \
         {summaries:?}"
    );
    assert_eq!(
        jira_configuration(&review),
        Some(&json!({
            "SelectedPillars": [{ "PillarId": "security", "SelectedQuestionIds": ["identities"] }],
        })),
        "case \"lens-notes-only\": the omitted JiraConfiguration must be left as stored, got \
         {review:?}"
    );

    // Supplying only `PillarNotes` leaves the lens notes as stored and REPLACES the map.
    let pillars_only = update_lens_review(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        json!({ "PillarNotes": { "reliability": "a reliability note" } }),
    )
    .await;
    let review = lens_review(&pillars_only, &workload_id, "pillar-notes-only");
    assert_eq!(
        notes(&review),
        Some("a newer lens note"),
        "case \"pillar-notes-only\": the omitted LensNotes must be left as stored, got {review:?}"
    );
    let summaries = pillar_summaries(&review, "pillar-notes-only");
    assert_eq!(
        notes(&summaries[1]),
        None,
        "case \"pillar-notes-only\": the supplied map replaces the stored one, got {summaries:?}"
    );
    assert_eq!(
        notes(&summaries[2]),
        Some("a reliability note"),
        "case \"pillar-notes-only\": the supplied map must be stored, got {summaries:?}"
    );

    // Supplying NO member writes nothing — not even `UpdatedAt`.
    let before = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let before = lens_review(&before, &workload_id, "no-member");
    let nothing = update_lens_review(&client, &workload_id, FRAMEWORK_LENS, json!({})).await;
    let after = lens_review(&nothing, &workload_id, "no-member");
    assert_eq!(
        (notes(&after), updated_at(&after, "no-member")),
        (notes(&before), updated_at(&before, "no-member")),
        "case \"no-member\": an update that supplies no member must change nothing, before \
         {before:?} after {after:?}"
    );
    assert!(
        updated_at(&before, "no-member") >= seeded_at,
        "case \"no-member\": UpdatedAt must not go backwards, seeded at {seeded_at} then \
         {before:?}"
    );
}

/// The Jira question selection a request supplies replaces the stored one and is reported
/// by the write and by the read, over a TABLE of selections spanning no pillar, one pillar
/// with no question listed, one pillar with several questions and two pillars at once.
#[tokio::test]
async fn the_jira_selection_round_trips() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "jira-selection", FRAMEWORK_LENS).await;

    let cases: [(&str, Value); 4] = [
        ("no-pillar", json!({ "SelectedPillars": [] })),
        (
            "one-pillar-no-question",
            json!({ "SelectedPillars": [{ "PillarId": "sustainability" }] }),
        ),
        (
            "one-pillar-several-questions",
            json!({
                "SelectedPillars": [
                    { "PillarId": "reliability", "SelectedQuestionIds": ["manage-service-quotas", "plan-for-dr"] },
                ],
            }),
        ),
        (
            "two-pillars",
            json!({
                "SelectedPillars": [
                    { "PillarId": "security", "SelectedQuestionIds": ["application-security"] },
                    { "PillarId": "costOptimization", "SelectedQuestionIds": [] },
                ],
            }),
        ),
    ];

    for (case, selection) in cases {
        let written = update_lens_review(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            json!({ "JiraConfiguration": selection }),
        )
        .await;
        let review = lens_review(&written, &workload_id, case);
        assert_eq!(
            jira_configuration(&review),
            Some(&selection),
            "case {case:?}: the update must report the selection it wrote, got {review:?}"
        );

        let read = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
        let review = lens_review(&read, &workload_id, case);
        assert_eq!(
            jira_configuration(&review),
            Some(&selection),
            "case {case:?}: the read must report the stored selection, got {review:?}"
        );
    }
}

/// An identifier that names nothing of the lens under review is refused and nothing is
/// stored: a `PillarNotes` key, a selected pillar and a selected question are each judged
/// against the lens's own content, and the finding names the member that carried it.
#[tokio::test]
async fn an_identifier_the_lens_does_not_define_is_refused() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "undefined-ids", FRAMEWORK_LENS).await;

    // The state the refusals must leave untouched.
    let seeded = update_lens_review(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        json!({
            "LensNotes": "the stored note",
            "PillarNotes": { "security": "the stored security note" },
        }),
    )
    .await;
    lens_review(&seeded, &workload_id, "seed");

    let cases: [(&str, Value, &str); 7] = [
        (
            "one-undefined-pillar-note",
            json!({ "PillarNotes": { "notAPillar": "x" } }),
            "PillarNotes",
        ),
        (
            "a-defined-and-an-undefined-pillar-note",
            json!({ "PillarNotes": { "security": "ok", "notAPillar": "x" } }),
            "PillarNotes",
        ),
        (
            // A `PillarName` is not a `PillarId`: the keys are the lens's ids.
            "pillar-note-keyed-by-pillar-name",
            json!({ "PillarNotes": { "Security": "x" } }),
            "PillarNotes",
        ),
        (
            "undefined-selected-pillar",
            json!({
                "JiraConfiguration": { "SelectedPillars": [{ "PillarId": "notAPillar" }] },
            }),
            "JiraConfiguration",
        ),
        (
            "a-defined-and-an-undefined-selected-pillar",
            json!({
                "JiraConfiguration": {
                    "SelectedPillars": [
                        { "PillarId": "security" },
                        { "PillarId": "notAPillar" },
                    ],
                },
            }),
            "JiraConfiguration",
        ),
        (
            "selected-question-the-pillar-does-not-ask",
            json!({
                "JiraConfiguration": {
                    "SelectedPillars": [
                        { "PillarId": "security", "SelectedQuestionIds": ["notAQuestion"] },
                    ],
                },
            }),
            "JiraConfiguration.SelectedPillars.security",
        ),
        (
            // `rel-01` is a question of the lens — of ANOTHER pillar — so it is no
            // question of the pillar that selected it.
            "selected-question-of-another-pillar",
            json!({
                "JiraConfiguration": {
                    "SelectedPillars": [
                        { "PillarId": "security", "SelectedQuestionIds": ["securely-operate", "manage-service-quotas"] },
                    ],
                },
            }),
            "JiraConfiguration.SelectedPillars.security",
        ),
    ];

    for (case, body, member) in cases {
        let answer = update_lens_review(&client, &workload_id, FRAMEWORK_LENS, body).await;
        assert_refused(&answer, member, case);
    }

    let read = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let review = lens_review(&read, &workload_id, "after-the-refusals");
    assert_eq!(
        notes(&review),
        Some("the stored note"),
        "case \"after-the-refusals\": a refused update must store nothing, got {review:?}"
    );
    let summaries = pillar_summaries(&review, "after-the-refusals");
    assert_eq!(
        notes(&summaries[1]),
        Some("the stored security note"),
        "case \"after-the-refusals\": a refused update must store nothing, got {summaries:?}"
    );
    assert_eq!(
        jira_configuration(&review),
        None,
        "case \"after-the-refusals\": a refused selection must store nothing, got {review:?}"
    );
}

/// A lens this emulator holds no content for is organized into no pillars and asks nothing
/// HERE, so there is no pillar or question set to judge an identifier against: the notes
/// and the selection are stored as given, and the pillars a caller wrote notes for are
/// reported back with no `PillarName`.
#[tokio::test]
async fn a_lens_without_content_stores_the_identifiers_as_given() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "no-content-lens", UNSHIPPED_LENS).await;

    let written = update_lens_review(
        &client,
        &workload_id,
        UNSHIPPED_LENS,
        json!({
            "LensNotes": "notes on a lens with no content here",
            "PillarNotes": { "zeta": "last", "alpha": "first" },
            "JiraConfiguration": {
                "SelectedPillars": [{ "PillarId": "alpha", "SelectedQuestionIds": ["q-1"] }],
            },
        }),
    )
    .await;
    let written_review = lens_review(&written, &workload_id, "no-content");

    let read = get_lens_review(&client, &workload_id, UNSHIPPED_LENS, None).await;
    let read_review = lens_review(&read, &workload_id, "no-content");

    for review in [&written_review, &read_review] {
        assert_eq!(
            notes(review),
            Some("notes on a lens with no content here"),
            "case \"no-content\": the notes must be stored as given, got {review:?}"
        );
        let summaries = pillar_summaries(review, "no-content");
        // Only the pillars a caller wrote notes for, in `PillarId` order.
        assert_eq!(
            summaries
                .iter()
                .map(|summary| (
                    summary.get("PillarId").and_then(Value::as_str),
                    notes(summary),
                    summary.get("PillarName").and_then(Value::as_str),
                ))
                .collect::<Vec<_>>(),
            vec![
                (Some("alpha"), Some("first"), None),
                (Some("zeta"), Some("last"), None),
            ],
            "case \"no-content\": the written pillars must read back with no published name, \
             got {summaries:?}"
        );
        assert_eq!(
            jira_configuration(review),
            Some(&json!({
                "SelectedPillars": [{ "PillarId": "alpha", "SelectedQuestionIds": ["q-1"] }],
            })),
            "case \"no-content\": the selection must be stored as given, got {review:?}"
        );
    }
}

/// Writing notes changes nothing the review DERIVES: the lens's own metadata and ARN stay
/// as the lens content gives them, and the risk roll-ups stay what the workload's stored
/// answers make them.
#[tokio::test]
async fn writing_notes_leaves_the_derived_members_alone() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "derived-members", FRAMEWORK_LENS).await;
    // One answered question, so the roll-up under test is not all-zero.
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "manage-service-quotas",
        answered(&[], true, None),
    );

    let before = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let before = lens_review(&before, &workload_id, "before");

    let written = update_lens_review(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        json!({
            "LensNotes": "a note changes no count",
            "PillarNotes": { "reliability": "nor does a pillar note" },
        }),
    )
    .await;
    let after = lens_review(&written, &workload_id, "after");

    for member in ["LensAlias", "LensArn", "LensName", "LensVersion", "LensStatus"] {
        assert_eq!(
            after.get(member),
            before.get(member),
            "case \"derived\": {member} is lens content, so a note must not move it, before \
             {before:?} after {after:?}"
        );
    }
    assert_eq!(
        after.get("LensArn").and_then(Value::as_str),
        Some(official_lens_arn(REGION, FRAMEWORK_LENS).as_str()),
        "case \"derived\": the review must report the lens's own ARN, got {after:?}"
    );
    assert_eq!(
        risk_counts(&after, "after"),
        risk_counts(&before, "before"),
        "case \"derived\": the roll-up counts answers, not notes, before {before:?} after \
         {after:?}"
    );
    for (index, (pillar_id, _)) in FRAMEWORK_PILLARS.iter().enumerate() {
        assert_eq!(
            risk_counts(&pillar_summaries(&after, "after")[index], pillar_id),
            risk_counts(&pillar_summaries(&before, "before")[index], pillar_id),
            "case \"derived\": pillar {pillar_id:?} counts answers, not notes"
        );
    }
}

/// A milestone records the workload at an instant, so notes written AFTER one is recorded
/// are in the live review and not in the one that milestone froze.
#[tokio::test]
async fn a_milestone_freezes_the_notes_it_recorded() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "frozen-notes", FRAMEWORK_LENS).await;

    let recorded_before = update_lens_review(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        json!({ "LensNotes": "before the milestone" }),
    )
    .await;
    lens_review(&recorded_before, &workload_id, "before-the-milestone");

    let milestone = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": "frozen", "ClientRequestToken": "tok-frozen" })),
        )
        .await;
    assert_eq!(
        milestone.status, 200,
        "case \"milestone\": expected CreateMilestone 200, got {milestone:?}"
    );

    let written = update_lens_review(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        json!({ "LensNotes": "after the milestone" }),
    )
    .await;
    lens_review(&written, &workload_id, "after-the-milestone");

    let frozen = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, Some(1)).await;
    let frozen = lens_review(&frozen, &workload_id, "frozen");
    assert_eq!(
        notes(&frozen),
        Some("before the milestone"),
        "case \"frozen\": the milestone reports the notes it recorded, got {frozen:?}"
    );

    let live = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let live = lens_review(&live, &workload_id, "live");
    assert_eq!(
        notes(&live),
        Some("after the milestone"),
        "case \"live\": the live review reports the last write, got {live:?}"
    );
}

/// The review that is written has to exist: the workload is RESOLVED, so an id nothing is
/// stored under is the modelled 404, while the lens is a VALUE of the request, so a lens
/// the workload does not carry is the modelled 400 naming `LensAlias`. Nothing is stored
/// either way.
#[tokio::test]
async fn a_review_that_does_not_exist_cannot_be_written() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "write-lookup", FRAMEWORK_LENS).await;

    let absent = update_lens_review(
        &client,
        ABSENT_HEX_ID,
        FRAMEWORK_LENS,
        json!({ "LensNotes": "nowhere" }),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"absent-workload\": expected HTTP 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code, "ResourceNotFoundException",
        "case \"absent-workload\": expected a ResourceNotFoundException, got {absent:?}"
    );

    // The rows differ along how the unattached lens is named, because what the request asks
    // for does not depend on the spelling.
    for (case, lens) in [
        ("lens-not-attached", UNSHIPPED_LENS),
        (
            "lens-not-attached-by-arn",
            "arn:aws:wellarchitected:us-west-2::lens/serverless",
        ),
    ] {
        let answer = update_lens_review(
            &client,
            &workload_id,
            lens,
            json!({ "LensNotes": "nowhere" }),
        )
        .await;
        assert_field_validation(
            case,
            &answer,
            "LensAlias",
            &format!("No lens review for lens alias {lens} was found in workload {workload_id}."),
        );
    }

    let read = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let review = lens_review(&read, &workload_id, "after-the-misses");
    assert_eq!(
        notes(&review),
        None,
        "case \"after-the-misses\": a refused write must store nothing, got {review:?}"
    );
}
