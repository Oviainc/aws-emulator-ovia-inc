// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateReviewTemplate`, the operation that revises a stored
//! review template.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. A revision writes the members the request carries onto the STORED template and
//!    leaves every member it does not name as it stands; the answer is the template as
//!    stored, so a later `GetReviewTemplate` reports exactly what the update answered.
//! 2. `LensesToAssociate` applies lenses to the template and `LensesToDisassociate`
//!    unapplies them, so the template's lens reviews — and the answers recorded under an
//!    unapplied lens, which exist only while that lens is applied — move with the lens
//!    set.
//! 3. The Well-Architected Framework lens cannot be unapplied
//!    (`LensesToDisassociate` doc), so a request naming it is refused whole and revises
//!    nothing.
//! 4. A lens the template does not apply has no review to unapply (400 naming `LensAlias`),
//!    and a lens name that resolves to no lens cannot be applied (400 naming the lens the
//!    service could not get) — each resolved before anything is
//!    written, so a request naming one good and one bad lens revises nothing.
//! 5. A revision is bounded by the lens ceiling a stored template carries, and a
//!    `TemplateArn` naming no stored template is a 404.
//! 6. A request that revises nothing — no writable member, a value the template already
//!    holds, or a lens it already applies — moves neither the template nor its
//!    `UpdatedAt`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the review-template seeding and the answer builder live in
//! the shared `super::workload_harness`.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::workload_harness::{
    answered, created_arn, member, minimal_template, official_lens_arn, Answer, Client,
    SIGNED_REGION,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A second AWS-official lens alias this emulator publishes, applied beside the framework
/// lens wherever a row needs a lens that CAN be unapplied
/// (`state::AWS_OFFICIAL_LENS_ALIASES`).
const SECOND_LENS: &str = "serverless";

/// A third AWS-official lens alias, for the rows that need two lenses to apply or unapply
/// at once.
const THIRD_LENS: &str = "devops";

/// How far into the past a stored template's `UpdatedAt` is moved before the request under
/// test, so a write the handler makes is observable whatever the clock's granularity
/// between two in-process requests — and so a write it must NOT make cannot hide behind
/// that granularity either.
const AGE: Duration = Duration::from_secs(3600);

/// The `ReviewTemplate` an answer carries, `Value::Null` for an answer that carries none.
fn template_of(answer: &Answer) -> Value {
    answer
        .body
        .get("ReviewTemplate")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Move a stored review template's `UpdatedAt`, and that of every lens review it holds,
/// back by [`AGE`]. Only the SEED reaches state directly; the write under test is the
/// handler's.
fn age_review_template(client: &Client, template_arn: &str) {
    let aged = SystemTime::now() - AGE;
    let state = client.store();
    let mut store = state.lock();
    let template = store
        .review_templates
        .get_mut(template_arn)
        .expect("the review template under test must exist");
    template.updated_at = aged;
    for review in template.lens_reviews.values_mut() {
        review.updated_at = aged;
    }
}

/// Create a review template and answer its ARN. `lenses` is applied as sent.
async fn template_applying(client: &Client, case: &str, lenses: &[&str]) -> String {
    let mut request = minimal_template(&format!("tpl-{case}"), &format!("tok-{case}"));
    request["Lenses"] = json!(lenses);
    created_arn(&client.create_review_template(request).await, case)
}

/// The `Lenses` member of a template projection, as sent.
fn lenses_of(template: &Value) -> Vec<String> {
    template
        .get("Lenses")
        .and_then(Value::as_array)
        .map(|lenses| {
            lenses
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Rule 1. The rows differ along WHICH writable members the request carries and along the
/// published bounds of their values, because the stored template is revised member by
/// member rather than replaced by one recorded request body.
#[tokio::test]
async fn a_revision_writes_the_members_the_request_carries_and_leaves_the_rest() {
    // (case, the request body, what the template must report afterwards)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "name-only",
            json!({ "TemplateName": "a-renamed-template" }),
            json!({ "TemplateName": "a-renamed-template" }),
        ),
        (
            "description-only",
            json!({ "Description": "a revised description" }),
            json!({ "Description": "a revised description" }),
        ),
        (
            "notes-only",
            json!({ "Notes": "notes written by the revision" }),
            json!({ "Notes": "notes written by the revision" }),
        ),
        (
            "every-writable-member",
            json!({
                "TemplateName": "every-member-renamed",
                "Description": "every member revised",
                "Notes": "every member's notes",
            }),
            json!({
                "TemplateName": "every-member-renamed",
                "Description": "every member revised",
                "Notes": "every member's notes",
            }),
        ),
        // `TemplateName` is @length(3, 100), `Description` @length(3, 250) and `Notes`
        // @length(max 2084): each at its published minimum, then at its published maximum.
        (
            "members-at-the-minimum-length",
            json!({ "TemplateName": "abc", "Description": "xyz" }),
            json!({ "TemplateName": "abc", "Description": "xyz" }),
        ),
        (
            "members-at-the-maximum-length",
            json!({
                "TemplateName": "n".repeat(100),
                "Description": "d".repeat(250),
                "Notes": "o".repeat(2084),
            }),
            json!({
                "TemplateName": "n".repeat(100),
                "Description": "d".repeat(250),
                "Notes": "o".repeat(2084),
            }),
        ),
    ];

    for (case, request, expected) in cases {
        let client = Client::new();
        let template_arn = template_applying(&client, case, &[FRAMEWORK_LENS]).await;
        let before = template_of(&client.get_review_template(&template_arn).await);

        let updated = client.update_review_template(&template_arn, request.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateReviewTemplate 200, got {updated:?}"
        );
        let revised = template_of(&updated);

        // The members the request carried are the new values of those members.
        for (name, value) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                revised.get(name),
                Some(value),
                "case {case:?}: expected {name} to be {value:?}, got {revised:?}"
            );
        }
        // Every member the request did NOT name stands as it was.
        for name in ["TemplateArn", "Lenses", "Owner", "Tags", "UpdateStatus"] {
            assert_eq!(
                revised.get(name),
                before.get(name),
                "case {case:?}: {name} must be left as it stood, got {revised:?}"
            );
        }
        for name in ["TemplateName", "Description", "Notes"] {
            if expected.get(name).is_none() {
                assert_eq!(
                    revised.get(name).cloned().unwrap_or(Value::Null),
                    before.get(name).cloned().unwrap_or(Value::Null),
                    "case {case:?}: an unnamed {name} must be left as it stood, got {revised:?}"
                );
            }
        }

        // The answer is the template as STORED, so a read reports exactly what it said.
        let fetched = template_of(&client.get_review_template(&template_arn).await);
        assert_eq!(
            fetched, revised,
            "case {case:?}: GetReviewTemplate must report what the update answered"
        );
        // A read must not change what a later caller sees.
        let reread = template_of(&client.get_review_template(&template_arn).await);
        assert_eq!(
            reread, fetched,
            "case {case:?}: a second GetReviewTemplate must report the same template"
        );
    }
}

/// Rule 1, continued: a revision replaces the member it names rather than adding to it, so
/// two revisions in a row leave the second one's values — including notes written over
/// notes the template already carried.
#[tokio::test]
async fn a_second_revision_replaces_what_the_first_wrote() {
    let client = Client::new();
    let template_arn = template_applying(&client, "successive", &[FRAMEWORK_LENS]).await;

    // (case, body, expected notes)
    let revisions: Vec<(&str, Value, Option<&str>)> = vec![
        ("first-notes", json!({ "Notes": "the first notes" }), Some("the first notes")),
        (
            "notes-over-notes",
            json!({ "Notes": "the second notes" }),
            Some("the second notes"),
        ),
        // `Notes` is @length(max 2084) with no minimum, so the empty string is a value a
        // caller may write and it replaces what stood.
        ("notes-emptied", json!({ "Notes": "" }), Some("")),
        // A revision that names no notes leaves the notes it finds.
        ("notes-untouched", json!({ "Description": "another description" }), Some("")),
    ];

    for (case, body, expected_notes) in revisions {
        let updated = client.update_review_template(&template_arn, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateReviewTemplate 200, got {updated:?}"
        );
        let revised = template_of(&updated);
        assert_eq!(
            revised.get("Notes").and_then(Value::as_str),
            expected_notes,
            "case {case:?}: expected Notes {expected_notes:?}, got {revised:?}"
        );
    }
}

/// Rule 2. The rows differ along how the two lens lists overlap what the template already
/// applies and along how each lens is NAMED (alias or ARN), because a lens is one lens
/// however it is spelled.
#[tokio::test]
async fn applying_and_unapplying_lenses_moves_the_templates_lens_set() {
    struct Case {
        case: &'static str,
        applied: &'static [&'static str],
        body: Value,
        expected: Vec<String>,
    }

    let cases: Vec<Case> = vec![
        Case {
            case: "apply-one-lens",
            applied: &[FRAMEWORK_LENS],
            body: json!({ "LensesToAssociate": ["serverless"] }),
            expected: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        Case {
            case: "apply-two-lenses",
            applied: &[FRAMEWORK_LENS],
            body: json!({ "LensesToAssociate": [SECOND_LENS, THIRD_LENS] }),
            expected: vec![
                FRAMEWORK_LENS.to_string(),
                SECOND_LENS.to_string(),
                THIRD_LENS.to_string(),
            ],
        },
        Case {
            // "either the lens alias … or the lens ARN" (`LensAlias` doc): the name as sent
            // is what the template records.
            case: "apply-a-lens-by-arn",
            applied: &[FRAMEWORK_LENS],
            body: json!({ "LensesToAssociate": [official_lens_arn("us-east-1", "serverless")] }),
            expected: vec![
                FRAMEWORK_LENS.to_string(),
                official_lens_arn("us-east-1", "serverless"),
            ],
        },
        Case {
            // The same lens named twice asks for ONE review.
            case: "apply-the-same-lens-twice",
            applied: &[FRAMEWORK_LENS],
            body: json!({
                "LensesToAssociate": [
                    "serverless",
                    official_lens_arn("us-east-1", "serverless"),
                ]
            }),
            expected: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        Case {
            // A lens the template already applies is not applied again, and the name the
            // first apply recorded stands.
            case: "apply-a-lens-already-applied",
            applied: &[FRAMEWORK_LENS, "serverless"],
            body: json!({ "LensesToAssociate": ["serverless"] }),
            expected: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        Case {
            case: "apply-a-lens-already-applied-under-another-name",
            applied: &[FRAMEWORK_LENS, "serverless"],
            body: json!({
                "LensesToAssociate": [official_lens_arn("us-west-2", "serverless")]
            }),
            expected: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        Case {
            case: "unapply-one-lens",
            applied: &[FRAMEWORK_LENS, "serverless"],
            body: json!({ "LensesToDisassociate": ["serverless"] }),
            expected: vec![FRAMEWORK_LENS.to_string()],
        },
        Case {
            case: "unapply-a-lens-named-by-arn",
            applied: &[FRAMEWORK_LENS, "serverless"],
            body: json!({
                "LensesToDisassociate": [official_lens_arn("us-east-1", "serverless")]
            }),
            expected: vec![FRAMEWORK_LENS.to_string()],
        },
        Case {
            case: "unapply-a-lens-applied-by-arn",
            applied: &[FRAMEWORK_LENS, "serverless"],
            body: json!({ "LensesToDisassociate": ["serverless"] }),
            expected: vec![FRAMEWORK_LENS.to_string()],
        },
        Case {
            case: "apply-and-unapply-in-one-request",
            applied: &[FRAMEWORK_LENS, SECOND_LENS],
            body: json!({
                "LensesToAssociate": [THIRD_LENS],
                "LensesToDisassociate": [SECOND_LENS],
            }),
            expected: vec![FRAMEWORK_LENS.to_string(), THIRD_LENS.to_string()],
        },
        Case {
            // The same lens in both lists: the review it held goes and a fresh one is
            // applied, so the template ends up applying it under the name sent here.
            case: "the-same-lens-in-both-lists",
            applied: &[FRAMEWORK_LENS, "serverless"],
            body: json!({
                "LensesToAssociate": [official_lens_arn("us-east-1", "serverless")],
                "LensesToDisassociate": ["serverless"],
            }),
            expected: vec![
                FRAMEWORK_LENS.to_string(),
                official_lens_arn("us-east-1", "serverless"),
            ],
        },
        Case {
            // A template created with the framework lens beside a lens this emulator ships
            // no content for still revises: lens CONTENT is not what the lens set is.
            case: "unapply-a-lens-with-no-shipped-content",
            applied: &[FRAMEWORK_LENS, "mergersandacquisitions"],
            body: json!({ "LensesToDisassociate": ["mergersandacquisitions"] }),
            expected: vec![FRAMEWORK_LENS.to_string()],
        },
    ];

    for row in cases {
        let Case {
            case,
            applied,
            body,
            expected,
        } = row;
        let client = Client::new();
        let template_arn = template_applying(&client, case, applied).await;

        let updated = client.update_review_template(&template_arn, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateReviewTemplate 200, got {updated:?}"
        );
        assert_eq!(
            lenses_of(&template_of(&updated)),
            expected,
            "case {case:?}: expected the template to apply {expected:?}, got {updated:?}"
        );

        // The lens set the answer reports is the STORED one, and every lens in it has a
        // lens review while none outside it does.
        let fetched = template_of(&client.get_review_template(&template_arn).await);
        assert_eq!(
            lenses_of(&fetched),
            expected,
            "case {case:?}: GetReviewTemplate must report the revised lens set, got {fetched:?}"
        );
        let applied_aliases: BTreeSet<&str> = expected
            .iter()
            .map(|lens| state::lens_alias(lens))
            .collect();
        for lens in applied {
            let review = client
                .get_review_template_lens_review(&template_arn, lens)
                .await;
            let expected_status = if applied_aliases.contains(state::lens_alias(lens)) {
                200
            } else {
                400
            };
            assert_eq!(
                review.status, expected_status,
                "case {case:?}: a lens review of {lens:?} must answer {expected_status}, got {review:?}"
            );
        }
    }
}

/// Rule 2, the other half: the answers recorded under a lens review exist only while that
/// lens is applied, so unapplying the lens drops them — and applying the lens again brings
/// a review with nothing answered rather than the answers the earlier review held.
#[tokio::test]
async fn unapplying_a_lens_drops_the_answers_recorded_under_it() {
    let client = Client::new();
    let template_arn =
        template_applying(&client, "answers-follow-the-lens", &[FRAMEWORK_LENS, SECOND_LENS]).await;

    // A `QuestionId` the applied lens really asks, read from the lens's own question set
    // rather than spelled out here, so this suite does not pin another lens's numbering.
    let listed = client
        .list_review_template_answers(&template_arn, SECOND_LENS, "")
        .await;
    let question_id = listed
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first())
        .and_then(|summary| summary.get("QuestionId"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the applied lens must ask at least one question, got {listed:?}"))
        .to_string();
    // Answered means a record exists for the question; which best practices it selects is
    // what `super::review_template_answer_updates` is about, so this one selects none.
    client.record_template_answer(&template_arn, SECOND_LENS, &question_id, answered(&[], true, None));

    let answer_status = |answer: &Answer| -> String {
        answer
            .body
            .get("Answer")
            .and_then(|value| value.get("AnswerStatus"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    // The seeded answer is there to begin with, so the rows below fail on the revision and
    // not on a question nothing ever answered.
    let seeded = client
        .get_review_template_answer(&template_arn, SECOND_LENS, &question_id)
        .await;
    assert_eq!(
        answer_status(&seeded),
        "ANSWERED",
        "case \"seeded\": the seeded answer must be recorded, got {seeded:?}"
    );

    let unapplied = client
        .update_review_template(
            &template_arn,
            json!({ "LensesToDisassociate": [SECOND_LENS] }),
        )
        .await;
    assert_eq!(
        unapplied.status, 200,
        "case \"unapply\": expected UpdateReviewTemplate 200, got {unapplied:?}"
    );
    let gone = client
        .get_review_template_answer(&template_arn, SECOND_LENS, &question_id)
        .await;
    assert_eq!(
        gone.status, 400,
        "case \"unapply\": an unapplied lens has no answer to read, got {gone:?}"
    );

    let reapplied = client
        .update_review_template(&template_arn, json!({ "LensesToAssociate": [SECOND_LENS] }))
        .await;
    assert_eq!(
        reapplied.status, 200,
        "case \"reapply\": expected UpdateReviewTemplate 200, got {reapplied:?}"
    );
    let fresh = client
        .get_review_template_answer(&template_arn, SECOND_LENS, &question_id)
        .await;
    assert_eq!(
        answer_status(&fresh),
        "UNANSWERED",
        "case \"reapply\": a lens applied afresh carries no earlier answer, got {fresh:?}"
    );
}

/// Rule 3. The rows differ along how the framework lens is NAMED and what else the list
/// carries, because what a request asks for does not depend on the spelling.
#[tokio::test]
async fn the_framework_lens_cannot_be_unapplied() {
    let cases: Vec<(&str, Value)> = vec![
        ("by-alias", json!([FRAMEWORK_LENS])),
        ("by-arn", json!([official_lens_arn("us-east-1", FRAMEWORK_LENS)])),
        (
            "by-arn-in-the-signed-region",
            json!([official_lens_arn(SIGNED_REGION, FRAMEWORK_LENS)]),
        ),
        ("beside-another-lens", json!([SECOND_LENS, FRAMEWORK_LENS])),
    ];

    for (case, lenses) in cases {
        let client = Client::new();
        let template_arn =
            template_applying(&client, case, &[FRAMEWORK_LENS, "serverless"]).await;
        age_review_template(&client, &template_arn);
        let before = template_of(&client.get_review_template(&template_arn).await);

        let answer = client
            .update_review_template(&template_arn, json!({ "LensesToDisassociate": lenses }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: unapplying the framework lens must be 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            member(&answer, "Message").contains("LensesToDisassociate"),
            "case {case:?}: the refusal must name the member that carried the aliases, got {answer:?}"
        );

        // A refused request revises nothing — the lens set and `UpdatedAt` included.
        let after = template_of(&client.get_review_template(&template_arn).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused revision must leave the template as it stood"
        );
    }
}

/// Rule 4. The rows differ along WHY the named lens cannot be applied or unapplied, and
/// each asserts the template was left exactly as it stood: every lens is resolved before
/// anything is written, so a request naming one good lens and one bad one revises nothing.
#[tokio::test]
async fn a_lens_that_cannot_be_resolved_refuses_the_whole_revision() {
    // (case, body, expected status, expected error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "unapply-a-lens-the-template-does-not-apply",
            json!({ "LensesToDisassociate": [THIRD_LENS] }),
            400,
            "ValidationException",
        ),
        (
            "unapply-one-applied-and-one-unapplied-lens",
            json!({ "LensesToDisassociate": [SECOND_LENS, THIRD_LENS] }),
            400,
            "ValidationException",
        ),
        (
            "unapply-a-lens-nothing-publishes",
            json!({ "LensesToDisassociate": ["notalens"] }),
            400,
            "ValidationException",
        ),
        (
            "apply-a-lens-nothing-publishes",
            json!({ "LensesToAssociate": ["notalens"] }),
            400,
            "ValidationException",
        ),
        (
            "apply-one-published-and-one-unpublished-lens",
            json!({ "LensesToAssociate": [THIRD_LENS, "notalens"] }),
            400,
            "ValidationException",
        ),
        (
            "apply-a-custom-lens-arn-no-import-minted",
            json!({
                "LensesToAssociate": [format!(
                    "arn:aws:wellarchitected:{SIGNED_REGION}:{}:lens/{}",
                    engine::emulator_account_id(),
                    "0".repeat(32)
                )]
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in cases {
        let client = Client::new();
        let template_arn =
            template_applying(&client, case, &[FRAMEWORK_LENS, "serverless"]).await;
        age_review_template(&client, &template_arn);
        let before = template_of(&client.get_review_template(&template_arn).await);

        let answer = client.update_review_template(&template_arn, body.clone()).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, expected_code,
            "case {case:?}: expected {expected_code}, got {answer:?}"
        );
        let after = template_of(&client.get_review_template(&template_arn).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused revision must leave the template as it stood"
        );
    }
}

/// Rule 5, half one: a template applies at most `state::MAX_LENSES_PER_REVIEW_TEMPLATE`
/// lenses, so a revision that would carry it past that ceiling is refused. Checked at the
/// ceiling, which is accepted, one past it, which is refused, and at a request that unapplies
/// as many lenses as it applies, which stays at the ceiling.
#[tokio::test]
async fn the_lens_ceiling_bounds_a_revised_template() {
    const MAX_LENSES: usize = state::MAX_LENSES_PER_REVIEW_TEMPLATE;
    // The rows need MAX_LENSES + 1 lenses that all resolve, so the ceiling is what the
    // over-the-ceiling row meets rather than lens resolution.
    assert!(
        state::AWS_OFFICIAL_LENS_ALIASES.len() > MAX_LENSES,
        "the AWS-official lens catalog must carry more than {MAX_LENSES} aliases to test the \
         ceiling, got {}",
        state::AWS_OFFICIAL_LENS_ALIASES.len()
    );
    let catalog: Vec<&str> = state::AWS_OFFICIAL_LENS_ALIASES.to_vec();

    // (case, how many lenses the template starts with, the revision, expected status)
    let cases: Vec<(&str, usize, Value, u16)> = vec![
        (
            "one-under-the-ceiling",
            MAX_LENSES - 1,
            json!({ "LensesToAssociate": [catalog[MAX_LENSES - 1]] }),
            200,
        ),
        (
            "at-the-ceiling",
            MAX_LENSES,
            json!({ "LensesToAssociate": [catalog[MAX_LENSES]] }),
            409,
        ),
        (
            "at-the-ceiling-swapping-one-lens-for-another",
            MAX_LENSES,
            json!({
                "LensesToAssociate": [catalog[MAX_LENSES]],
                "LensesToDisassociate": [catalog[MAX_LENSES - 1]],
            }),
            200,
        ),
        (
            "at-the-ceiling-applying-a-lens-it-already-applies",
            MAX_LENSES,
            json!({ "LensesToAssociate": [catalog[1]] }),
            200,
        ),
    ];

    for (case, start, body, expected_status) in cases {
        let client = Client::new();
        let applied: Vec<&str> = catalog.iter().take(start).copied().collect();
        // No row's revision may name the one lens that cannot be unapplied, so the rows
        // meet the ceiling rather than that refusal.
        assert!(
            !body.to_string().contains(FRAMEWORK_LENS),
            "case {case:?}: the row must not name the framework lens, got {body:?}"
        );
        let template_arn = template_applying(&client, case, &applied).await;

        let answer = client.update_review_template(&template_arn, body.clone()).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if expected_status == 409 {
            assert_eq!(
                answer.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                template_arn,
                "case {case:?}: the refusal must name the template it is about, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "REVIEW_TEMPLATE",
                "case {case:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
            );
            // The refused revision left the lens set alone.
            let fetched = template_of(&client.get_review_template(&template_arn).await);
            assert_eq!(
                lenses_of(&fetched).len(),
                start,
                "case {case:?}: a refused revision must leave the lens set at {start}, got {fetched:?}"
            );
        } else {
            let fetched = template_of(&client.get_review_template(&template_arn).await);
            assert!(
                lenses_of(&fetched).len() <= MAX_LENSES,
                "case {case:?}: a revised template applies at most {MAX_LENSES} lenses, got {fetched:?}"
            );
        }
    }
}

/// Rule 5, half two: the `TemplateArn` is RESOLVED against the store, before anything the
/// request carries is judged. The rows differ along the ARN that resolves nothing.
#[tokio::test]
async fn a_template_arn_that_names_no_template_is_a_not_found() {
    let client = Client::new();
    let present = template_applying(&client, "present", &[FRAMEWORK_LENS]).await;
    let absent = |template_id: &str| {
        format!(
            "arn:aws:wellarchitected:{SIGNED_REGION}:{}:review-template/{template_id}",
            engine::emulator_account_id()
        )
    };

    // (case, the ARN, the body — including ones that would themselves be refused)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "never-minted",
            absent(&"0".repeat(32)),
            json!({ "TemplateName": "a-renamed-template" }),
        ),
        (
            "never-minted-with-no-writable-member",
            absent(&"f".repeat(32)),
            json!({}),
        ),
        (
            "stored-id-in-another-region",
            present.replace(SIGNED_REGION, "eu-west-1"),
            json!({ "Description": "a revised description" }),
        ),
        (
            // The ARN is resolved BEFORE the lens lists are judged, so a request that would
            // also be refused for its lenses is answered with this 404.
            "never-minted-unapplying-the-framework-lens",
            absent("0123456789abcdef0123456789abcdef"),
            json!({ "LensesToDisassociate": [FRAMEWORK_LENS] }),
        ),
    ];

    for (case, template_arn, body) in cases {
        assert_ne!(
            template_arn, present,
            "case {case:?}: the row must not name the template that exists"
        );
        let answer = client.update_review_template(&template_arn, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            template_arn,
            "case {case:?}: ResourceId must be the ARN as sent, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            "REVIEW_TEMPLATE",
            "case {case:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
        );
    }

    // The template that DOES exist still revises, so the rows above fail on resolution and
    // not on a broken route.
    let updated = client
        .update_review_template(&present, json!({ "Description": "still revisable" }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"present\": the stored template must still revise, got {updated:?}"
    );
}

/// Rule 6. The rows differ along the way the request asks for no change: it carries no
/// writable member, it carries the value the template already holds, or it applies a lens
/// the template already applies. A write that DOES change something moves the instant, so
/// the rows cannot pass by the handler never writing at all.
#[tokio::test]
async fn a_revision_that_changes_nothing_moves_no_instant() {
    // (case, body, whether the template must report a new UpdatedAt)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("no-writable-member", json!({}), false),
        (
            "the-stored-name",
            json!({ "TemplateName": "tpl-no-op" }),
            false,
        ),
        (
            "the-stored-description",
            json!({ "Description": "a review template under test" }),
            false,
        ),
        (
            "a-lens-already-applied",
            json!({ "LensesToAssociate": [FRAMEWORK_LENS] }),
            false,
        ),
        (
            "a-lens-already-applied-named-by-arn",
            json!({ "LensesToAssociate": [official_lens_arn("us-east-1", FRAMEWORK_LENS)] }),
            false,
        ),
        (
            "a-different-name",
            json!({ "TemplateName": "a-name-the-template-did-not-hold" }),
            true,
        ),
        (
            "a-different-description",
            json!({ "Description": "a description the template did not hold" }),
            true,
        ),
        ("fresh-notes", json!({ "Notes": "notes it did not hold" }), true),
        (
            "a-lens-it-did-not-apply",
            json!({ "LensesToAssociate": ["serverless"] }),
            true,
        ),
    ];

    for (case, body, expect_moved) in cases {
        let client = Client::new();
        // The template's name is what the "the-stored-name" row sends back, so it is named
        // exactly that.
        let mut request = minimal_template("tpl-no-op", &format!("tok-{case}"));
        request["Lenses"] = json!([FRAMEWORK_LENS]);
        let template_arn = created_arn(&client.create_review_template(request).await, case);
        age_review_template(&client, &template_arn);
        let before = template_of(&client.get_review_template(&template_arn).await);
        let aged = before
            .get("UpdatedAt")
            .and_then(Value::as_f64)
            .unwrap_or_default();

        let answer = client.update_review_template(&template_arn, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateReviewTemplate 200, got {answer:?}"
        );
        let after = template_of(&answer);
        let updated_at = after
            .get("UpdatedAt")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if expect_moved {
            assert!(
                updated_at > aged,
                "case {case:?}: a revision must move UpdatedAt past {aged}, got {after:?}"
            );
        } else {
            assert_eq!(
                updated_at, aged,
                "case {case:?}: a request that revises nothing must leave UpdatedAt at {aged}, got {after:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a request that revises nothing must report the template as it stood"
            );
        }
        // Whatever the request asked for, the answer is the template as stored.
        let fetched = template_of(&client.get_review_template(&template_arn).await);
        assert_eq!(
            fetched, after,
            "case {case:?}: GetReviewTemplate must report what the update answered"
        );
    }
}
