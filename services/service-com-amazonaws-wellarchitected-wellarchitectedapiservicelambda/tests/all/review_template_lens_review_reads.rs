// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetReviewTemplateLensReview` as a READ.
//!
//! The suite in `super::review_template_lens_review` pins WHAT the lens review of a
//! review template reports. These two tests pin the two rules that are about the read
//! ITSELF, over TABLES of inputs:
//!
//! * "Get a lens review associated with a review template" is a REPORT: it answers from
//!   the stored review template and writes nothing, so however many times it is called it
//!   answers the same thing and leaves the template exactly as the next caller would have
//!   seen it;
//! * both of its members travel as `@httpLabel`s, and each is checked against its
//!   published constraints before anything is looked up — a label outside them is the
//!   modelled 400 `ValidationException` naming that member, while a label inside them
//!   that names nothing stored is the modelled 404 `ResourceNotFoundException`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the review-template seeding live in the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{answered, official_lens_arn, review_template, Client};

/// The alias of the one lens this emulator ships a question set for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes but this emulator ships no question set for.
const UNSHIPPED_LENS: &str = "serverless";

/// The Region the harness signs every request for.
const REGION: &str = "us-west-2";

/// The published bounds of `GetReviewTemplateLensReview`'s two `@httpLabel` members, from
/// `smithy_query(query="operation", operation="GetReviewTemplateLensReview")`:
/// `TemplateArn` is `@length(50..250)` with the `review-template` ARN `@pattern`, and
/// `LensAlias` is `@length(1..128)`.
const TEMPLATE_ARN_MIN: usize = 50;
const TEMPLATE_ARN_MAX: usize = 250;
const LENS_ALIAS_MIN: usize = 1;
const LENS_ALIAS_MAX: usize = 128;

/// The wire error codes the two refusal classes carry
/// (`smithy_query(query="operation", operation="GetReviewTemplateLensReview")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The `ANSWERED` bucket of a `QuestionCounts` map, from
/// `smithy_query(query="shape", shape="Question")`.
const ANSWERED: &str = "ANSWERED";

/// Everything a `TemplateArn` holds besides the Region word: `arn:aws:wellarchitected:`,
/// the two `:` separators around the account id, the 12-digit account id, the
/// `review-template/` resource prefix and the 32-character resource id.
const TEMPLATE_ARN_FIXED: usize =
    "arn:aws:wellarchitected:".len() + 1 + 12 + 1 + "review-template/".len() + 32;

/// A `TemplateArn` of exactly `total` characters that satisfies the modelled pattern
/// `^arn:aws…:wellarchitected:[a-z]{2}(-gov|-iso([a-z])?)?-[a-z]+-\d:\d{12}:(review-template)/[a-f0-9]{32}$`.
///
/// The pattern leaves the `[a-z]+` of the Region word unbounded, so stretching it is what
/// lets the published `@length` bounds be probed with values the pattern ACCEPTS — the
/// refusal is then about the length and not about the characters. No value the pattern
/// accepts is shorter than `TEMPLATE_ARN_FIXED + 6`, which is why the published minimum
/// of 50 is probed below with a value that breaks the pattern too.
fn template_arn_of_length(total: usize) -> String {
    let region_len = total
        .checked_sub(TEMPLATE_ARN_FIXED)
        .filter(|region_len| *region_len >= 6)
        .unwrap_or_else(|| panic!("no TemplateArn the pattern accepts is {total} characters long"));
    let region = format!("us-{}-1", "a".repeat(region_len - "us--1".len()));
    let arn = format!(
        "arn:aws:wellarchitected:{region}:{}:review-template/{}",
        engine::emulator_account_id(),
        "0".repeat(32)
    );
    assert_eq!(
        arn.len(),
        total,
        "case \"arn-builder\": expected a {total}-character TemplateArn, got {arn:?}"
    );
    arn
}

/// One row of the read table: the case name, the lenses the template applies, the
/// spelling the read names its lens by, the notes written to that lens review, the
/// `QuestionId`s the template holds an answer for, and the `ANSWERED` roll-up the read
/// must report — which counts only the questions the lens under review ASKS, so a
/// question the lens does not define adds nothing to it.
type ReadRow<'a> = (
    &'a str,
    &'a [&'a str],
    &'a str,
    Option<&'a str>,
    &'a [&'a str],
    i64,
);

/// A read is a REPORT: repeating it answers the same thing, and the review template a
/// later caller reads is the one that was stored before the reads.
///
/// Rows differ along the lens the template applies, whether this emulator ships that
/// lens's content, the spelling the read names the lens by (bare alias or official lens
/// ARN), whether notes have been written to the lens review, and how many of the
/// template's questions are answered.
#[tokio::test]
async fn repeated_reads_report_the_same_review_and_change_nothing() {
    let client = Client::new();
    let framework_arn = official_lens_arn(REGION, FRAMEWORK_LENS);

    let rows: [ReadRow; 6] = [
        (
            "no-notes-no-answers",
            &[FRAMEWORK_LENS],
            FRAMEWORK_LENS,
            None,
            &[],
            0,
        ),
        (
            "notes-only",
            &[FRAMEWORK_LENS],
            FRAMEWORK_LENS,
            Some("the notes of this lens review"),
            &[],
            0,
        ),
        (
            "answers-only",
            &[FRAMEWORK_LENS],
            FRAMEWORK_LENS,
            None,
            &["securely-operate", "evolve-operations"],
            2,
        ),
        (
            "notes-and-answers",
            &[FRAMEWORK_LENS],
            FRAMEWORK_LENS,
            Some("notes beside stored answers"),
            &["plan-for-dr", "cloud-financial-management", "dev-deploy-patterns"],
            3,
        ),
        (
            "read-by-official-lens-arn",
            &[FRAMEWORK_LENS],
            framework_arn.as_str(),
            Some("named by the lens ARN"),
            &["performance-monitoring"],
            1,
        ),
        // This emulator ships no question set for this lens, so the lens ASKS nothing and
        // the answer the row records — a question only the framework lens defines — is
        // counted by no roll-up.
        (
            "lens-without-shipped-content",
            &[UNSHIPPED_LENS],
            UNSHIPPED_LENS,
            Some("a lens this emulator holds no content for"),
            &["securely-operate"],
            0,
        ),
    ];

    for (case, lenses, read_by, notes, answers, expected_answered) in rows {
        let template_arn = review_template(&client, case, lenses).await;
        if let Some(notes) = notes {
            let written = client
                .update_review_template_lens_review(
                    &template_arn,
                    read_by,
                    json!({ "LensNotes": notes }),
                )
                .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected UpdateReviewTemplateLensReview 200, got {written:?}"
            );
        }
        for question_id in answers {
            client.record_template_answer(
                &template_arn,
                read_by,
                question_id,
                answered(&[("choice", "SELECTED")], true, None),
            );
        }

        let template_before = client.get_review_template(&template_arn).await;
        assert_eq!(
            template_before.status, 200,
            "case {case:?}: expected GetReviewTemplate 200 before the reads, got \
             {template_before:?}"
        );

        let first = client
            .get_review_template_lens_review(&template_arn, read_by)
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected GetReviewTemplateLensReview 200, got {first:?}"
        );
        let review = first
            .body
            .get("LensReview")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {first:?}"));
        assert_eq!(
            review.get("Notes").and_then(Value::as_str),
            notes,
            "case {case:?}: the read reports the notes the store holds, got {review:?}"
        );
        assert_eq!(
            review
                .get("QuestionCounts")
                .and_then(|counts| counts.get(ANSWERED))
                .and_then(Value::as_i64),
            Some(expected_answered),
            "case {case:?}: the lens roll-up counts the stored answers to questions this \
             lens asks, got {review:?}"
        );

        // Three more reads: nothing a read reports may depend on how many reads came
        // before it, down to the whole serialized body.
        for repeat in 1..=3 {
            let again = client
                .get_review_template_lens_review(&template_arn, read_by)
                .await;
            assert_eq!(
                (again.status, &again.body),
                (first.status, &first.body),
                "case {case:?}: read {repeat} must answer exactly what the first read did, \
                 got {again:?}"
            );
        }

        // And the template itself is untouched by having been read.
        let template_after = client.get_review_template(&template_arn).await;
        assert_eq!(
            (template_after.status, &template_after.body),
            (template_before.status, &template_before.body),
            "case {case:?}: reading a lens review must leave the review template as the \
             next caller would have seen it, got {template_after:?}"
        );
    }
}

/// One row of the label table: the case, the `TemplateArn` and the `LensAlias` the request
/// carries, the status it must answer, the member its `Message` must name, and the member a
/// `Fields` finding must name — `None` where the refusal carries no finding.
type LabelRow<'row> = (
    &'row str,
    &'row str,
    &'row str,
    u16,
    Option<&'row str>,
    Option<&'row str>,
);

/// Each `@httpLabel` is judged against its own published constraints before anything is
/// looked up: outside them the answer is the modelled 400 naming that member, inside them a
/// `TemplateArn` that names nothing stored is the modelled 404 and a `LensAlias` that names
/// no lens review of the stored template is the modelled 400 whose `Fields` names it — and
/// either way nothing is stored.
///
/// Rows differ along which member is at fault, which constraint it breaks (`@length` at
/// each published bound and one past it, the ARN `@pattern` in its resource type, its
/// account id and its resource id) and whether the other member is resolvable, so a
/// refusal is never a side effect of the request's other label.
#[tokio::test]
async fn a_label_outside_its_published_constraints_is_refused_before_any_lookup() {
    let client = Client::new();
    let stored_arn = review_template(&client, "label-bounds", &[FRAMEWORK_LENS]).await;
    let stored_before = client.get_review_template(&stored_arn).await;
    assert_eq!(
        stored_before.status, 200,
        "case \"seed\": expected GetReviewTemplate 200, got {stored_before:?}"
    );

    let longest_alias = "a".repeat(LENS_ALIAS_MAX);
    let too_long_alias = "a".repeat(LENS_ALIAS_MAX + 1);
    let shortest_alias = "a".repeat(LENS_ALIAS_MIN);
    let on_the_arn_maximum = template_arn_of_length(TEMPLATE_ARN_MAX);
    let one_under_the_arn_maximum = template_arn_of_length(TEMPLATE_ARN_MAX - 1);
    let one_past_the_arn_maximum = template_arn_of_length(TEMPLATE_ARN_MAX + 1);
    // Shorter than the published minimum. The pattern accepts nothing this short either,
    // so this row pins the refusal and not which of the two constraints caught it.
    let under_the_arn_minimum = format!(
        "arn:aws:wellarchitected:{}",
        "a".repeat(TEMPLATE_ARN_MIN - 1 - "arn:aws:wellarchitected:".len())
    );
    let other_resource_type = format!(
        "arn:aws:wellarchitected:{REGION}:{}:workload/{}",
        engine::emulator_account_id(),
        "0".repeat(32)
    );
    let short_account = format!(
        "arn:aws:wellarchitected:{REGION}:{}:review-template/{}",
        "1".repeat(11),
        "0".repeat(32)
    );
    let non_hex_resource_id = format!(
        "arn:aws:wellarchitected:{REGION}:{}:review-template/{}",
        engine::emulator_account_id(),
        "z".repeat(32)
    );

    let rows: [LabelRow<'_>; 11] = [
        (
            "arn-under-the-published-minimum",
            under_the_arn_minimum.as_str(),
            FRAMEWORK_LENS,
            400,
            Some("templateArn"),
            None,
        ),
        (
            "arn-one-under-the-published-maximum",
            one_under_the_arn_maximum.as_str(),
            FRAMEWORK_LENS,
            404,
            None,
            None,
        ),
        (
            "arn-on-the-published-maximum",
            on_the_arn_maximum.as_str(),
            FRAMEWORK_LENS,
            404,
            None,
            None,
        ),
        (
            "arn-one-past-the-published-maximum",
            one_past_the_arn_maximum.as_str(),
            FRAMEWORK_LENS,
            400,
            Some("templateArn"),
            None,
        ),
        (
            "arn-of-another-resource-type",
            other_resource_type.as_str(),
            FRAMEWORK_LENS,
            400,
            Some("templateArn"),
            None,
        ),
        (
            "arn-with-an-11-digit-account",
            short_account.as_str(),
            FRAMEWORK_LENS,
            400,
            Some("templateArn"),
            None,
        ),
        (
            "arn-with-a-non-hex-resource-id",
            non_hex_resource_id.as_str(),
            FRAMEWORK_LENS,
            400,
            Some("templateArn"),
            None,
        ),
        // Inside its published constraints, so the handler judges it: the alias names no
        // lens review of the stored template, which is a value of the request and so a 400
        // whose `Fields` names the member that carried it.
        (
            "alias-on-the-published-minimum",
            stored_arn.as_str(),
            shortest_alias.as_str(),
            400,
            Some("lens alias"),
            Some("LensAlias"),
        ),
        (
            "alias-on-the-published-maximum",
            stored_arn.as_str(),
            longest_alias.as_str(),
            400,
            Some("lens alias"),
            Some("LensAlias"),
        ),
        (
            "alias-one-past-the-published-maximum",
            stored_arn.as_str(),
            too_long_alias.as_str(),
            400,
            Some("lensAlias"),
            None,
        ),
        // BOTH labels break their published constraints and neither names anything
        // stored: the answer is the boundary's 400 naming the ARN, so the constraint check
        // runs before the lookup rather than after it.
        (
            "malformed-arn-beside-a-malformed-lens",
            non_hex_resource_id.as_str(),
            too_long_alias.as_str(),
            400,
            Some("templateArn"),
            None,
        ),
    ];

    for (case, template_arn, lens_alias, expected, member, fields_member) in rows {
        let answer = client
            .get_review_template_lens_review(template_arn, lens_alias)
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
        let expected_code = if expected == 400 {
            VALIDATION_EXCEPTION
        } else {
            RESOURCE_NOT_FOUND_EXCEPTION
        };
        assert_eq!(
            answer.error_code, expected_code,
            "case {case:?}: expected the error code {expected_code}, got {answer:?}"
        );
        let message = answer
            .body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        if let Some(member) = member {
            assert!(
                message.contains(&member.to_lowercase()),
                "case {case:?}: the refusal must name the member {member:?}, got {answer:?}"
            );
        }
        assert!(
            !message.is_empty(),
            "case {case:?}: Message is @required on every modelled error of this \
             operation, got {answer:?}"
        );
        match fields_member {
            // A member value refused while the request is still being read as a shape is
            // not classified: the modelled `ValidationExceptionReason` and `Fields`
            // describe a rule the service ran over a request it could read, and no rule
            // ran here. This is the envelope `super::validation_envelopes` pins for the
            // same constraint classes reached through a body member.
            None if expected == 400 => assert_eq!(
                (answer.body.get("Reason"), answer.body.get("Fields")),
                (None, None),
                "case {case:?}: a label outside its shape's constraints carries the \
                 Message alone, got {answer:?}"
            ),
            // A value the handler judged against the lens content IS classified, and the
            // finding names the member that carried it.
            Some(named) => {
                assert_eq!(
                    answer.body.get("Reason").and_then(Value::as_str),
                    Some("FIELD_VALIDATION_FAILED"),
                    "case {case:?}: a refused value is classified, got {answer:?}"
                );
                assert_eq!(
                    answer
                        .body
                        .get("Fields")
                        .and_then(Value::as_array)
                        .and_then(|fields| fields.first())
                        .and_then(|field| field.get("Name"))
                        .and_then(Value::as_str),
                    Some(named),
                    "case {case:?}: the finding must name {named}, got {answer:?}"
                );
            }
            None => {}
        }
    }

    // A refused read is still a read: the seeded template is exactly as it was, and its
    // own lens review still answers.
    let stored_after = client.get_review_template(&stored_arn).await;
    assert_eq!(
        (stored_after.status, &stored_after.body),
        (stored_before.status, &stored_before.body),
        "case \"after-the-refusals\": a refused read must change nothing, got {stored_after:?}"
    );
    let resolvable = client
        .get_review_template_lens_review(&stored_arn, FRAMEWORK_LENS)
        .await;
    assert_eq!(
        resolvable.status, 200,
        "case \"after-the-refusals\": the lens review the template does apply must still \
         answer 200, got {resolvable:?}"
    );
    assert_eq!(
        resolvable.body.get("TemplateArn").and_then(Value::as_str),
        Some(stored_arn.as_str()),
        "case \"after-the-refusals\": the answer names the template under review, got \
         {resolvable:?}"
    );
}
