// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetLensReviewReport`: the DOCUMENT one lens review of one
//! workload is reported as.
//!
//! The rules these pin, over TABLES of inputs rather than one request:
//!
//! * a report exists for exactly the lens reviews the workload has, and it IDENTIFIES that
//!   review's lens — the `LensAlias` as the request named it, beside the `LensArn` that
//!   spelling resolves to — whichever half of the lens catalog holds the lens;
//! * `Base64String` is "the Base64-encoded string representation of a lens review report"
//!   that "can be used to create a PDF file" (`smithy_query(query="shape",
//!   shape="Base64String")`), so what it carries decodes to a PDF document;
//! * everything that document SAYS is derived from the reviewed workload's stored state:
//!   the workload it is a report of, the lens the review's own read reports, and the
//!   `Risk` roll-ups of the review and of each pillar of the lens, which the report and
//!   `GetLensReview` therefore agree about on every row;
//! * a report is a READ: two reports of an unchanged review are byte for byte the same
//!   document, and a report taken after a write reports that write;
//! * a milestone records the workload at an instant, so the report of a milestone's review
//!   is the report of the review that milestone froze;
//! * the workload and the milestone are RESOLVED (a 404 each), while the lens is a VALUE of
//!   the request (a 400 naming `LensAlias`), and a `MilestoneNumber` outside the modelled
//!   range is refused before any of that.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`; the custom-lens import and publish helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_review_answers` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::wait_for_the_import_to_land;
use super::custom_lens_review_answers::{import_shape, publish, Shape};
use super::workload_harness::{
    assert_field_validation, assert_not_found, created_id, encode_path_segment, framework_answer,
    minimal_create, official_lens_arn, Answer, Client, InPlace,
};

/// The alias of the AWS Well-Architected Framework lens, and the name and version this
/// emulator's AWS-official catalog publishes it at (`state::FRAMEWORK_LENS_ALIAS`,
/// `FRAMEWORK_LENS_NAME`, `FRAMEWORK_LENS_VERSION`), spelled out here rather than read back
/// out of the code under test.
const FRAMEWORK_LENS: &str = "wellarchitected";
const FRAMEWORK_LENS_NAME: &str = "AWS Well-Architected Framework";
const FRAMEWORK_LENS_VERSION: &str = "2025-02-25";

/// How many questions the framework lens asks: the published Well-Architected Framework
/// numbering (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6).
const FRAMEWORK_QUESTIONS: i64 = 11 + 11 + 13 + 5 + 11 + 6;

/// The version this emulator's catalog publishes two SPECIALISED AWS-official lenses at,
/// and the names the catalog gives them — rows the issue this suite was written for cited
/// neither of. The Serverless lens is published at a version of AWS's own; the SAP lens's
/// row carries this emulator's content revision
/// (`state::official_lenses::AUTHORED_CONTENT_REVISION`).
const SERVERLESS_LENS: &str = "serverless";
const SERVERLESS_LENS_NAME: &str = "Serverless Lens";
const SERVERLESS_LENS_VERSION: &str = "2020-02-04";
const SAP_LENS: &str = "sap";
const SAP_LENS_NAME: &str = "SAP Lens";
const SAP_LENS_VERSION: &str = "2025-01-15";

/// A lens alias AWS publishes that this emulator's AWS-official catalog carries no content
/// for, so it is organized into no pillars and asks nothing here: a review of it reports no
/// lens name and no version, and the report has to say so rather than invent either.
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// The shape of the custom lens document one row imports: `(PillarId, questions, choices
/// per question)`, as `super::custom_lens_import::lens_document` builds a document from.
const CUSTOM_SHAPE: Shape = &[("operationalExcellence", 2, 2), ("security", 1, 3)];

/// The version `super::custom_lens_review_answers::publish` publishes an imported lens at.
const CUSTOM_LENS_VERSION: &str = "1.0";

/// The Region the shared harness signs its requests for.
const REGION: &str = "us-west-2";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The `Risk` members the enum publishes (`smithy_query(query="shape", shape="Risk")`),
/// which is what a lens review's roll-up carries a bucket for.
const RISKS: [&str; 5] = ["HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE", "UNANSWERED"];

/// What the document prints where the review reports no value for a member it names — a
/// lens this emulator holds no content for has no name and no version to print.
const UNRECORDED: &str = "(not recorded)";

/// The longest `Notes` a lens review accepts: `Notes` is `@length(max: 2084)`
/// (`smithy_query(query="operation", operation="UpdateLensReview")`).
const NOTES_MAX: usize = 2084;

/// `GetLensReviewReport`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/report`,
/// with the optional `MilestoneNumber` bound to the query string. The lens travels
/// percent-encoded in ONE path segment, exactly as an SDK sends an ARN-spelled alias.
async fn get_report(
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
                "/workloads/{workload_id}/lensReviews/{}/report{suffix}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}` — the review the
/// report is a report OF, read so the two can be checked against each other.
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

/// The `LensReviewReport` of a successful answer, with the `WorkloadId` and the
/// `MilestoneNumber` the response reports checked against what the request named: a report
/// of the live review names no milestone.
fn report_of(
    answer: &Answer,
    workload_id: &str,
    milestone_number: Option<i64>,
    case: &str,
) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("WorkloadId").and_then(Value::as_str),
        Some(workload_id),
        "case {case:?}: WorkloadId must be the workload under review, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("MilestoneNumber").and_then(Value::as_i64),
        milestone_number,
        "case {case:?}: MilestoneNumber must be the milestone the request named, got \
         {answer:?}"
    );
    answer
        .body
        .get("LensReviewReport")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: LensReviewReport must be present, got {answer:?}")
        })
}

/// The `LensReview` of a successful `GetLensReview`.
fn lens_review_of(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"))
}

/// One string member of a JSON object, `None` when it carries none.
fn text_member<'a>(owner: &'a Value, name: &str) -> Option<&'a str> {
    owner.get(name).and_then(Value::as_str)
}

/// The report's `Base64String`, decoded.
///
/// The bytes have to be a PDF document, so the file header and the end-of-file marker every
/// PDF carries are checked here: "This data can be used to create a PDF file"
/// (`smithy_query(query="shape", shape="Base64String")`).
fn document_bytes(report: &Value, case: &str) -> Vec<u8> {
    let encoded = text_member(report, "Base64String")
        .unwrap_or_else(|| panic!("case {case:?}: Base64String must be present, got {report:?}"));
    let bytes = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
        panic!("case {case:?}: Base64String must be base64 ({error}), got {encoded:?}")
    });
    assert!(
        bytes.starts_with(b"%PDF-"),
        "case {case:?}: the decoded report must be a PDF document, got {:?}",
        String::from_utf8_lossy(&bytes[..bytes.len().min(32)])
    );
    assert!(
        String::from_utf8_lossy(&bytes).contains("%%EOF"),
        "case {case:?}: the decoded report must carry the PDF end-of-file marker"
    );
    bytes
}

/// What the report document SHOWS, as text a `contains` check can read: the page's content
/// stream is written uncompressed, so each shown line is in it as a PDF literal string —
/// whose `\(` and `\)` escapes are undone here so a line carrying parentheses reads as it
/// is shown.
fn document_text(report: &Value, case: &str) -> String {
    let bytes = document_bytes(report, case);
    String::from_utf8_lossy(&bytes)
        .replace("\\(", "(")
        .replace("\\)", ")")
}

/// Assert that the document shows `line`.
fn assert_shows(text: &str, line: &str, case: &str) {
    assert!(
        text.contains(line),
        "case {case:?}: the report document must show {line:?}"
    );
}

/// The `PillarId`s the document reports a section for, in the document's own order.
fn documented_pillars(text: &str) -> Vec<String> {
    text.split("Pillar: ")
        .skip(1)
        .filter_map(|section| {
            let line = section.split(['\n', ')']).next().unwrap_or_default();
            line.rsplit('(').next().map(ToString::to_string)
        })
        .collect()
}

/// The `PillarId`s a lens review reports a summary for, in the review's own order.
fn review_pillars(review: &Value, case: &str) -> Vec<String> {
    review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: PillarReviewSummaries must be present, got {review:?}")
        })
        .iter()
        .map(|summary| {
            text_member(summary, "PillarId")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a pillar summary must name its PillarId")
                })
                .to_string()
        })
        .collect()
}

/// One `RiskCounts` map of a response, as the bucket table the enum publishes: every
/// `Risk`, so a bucket the response omits is read as missing rather than skipped.
fn risk_counts(owner: &Value, case: &str) -> Vec<(String, i64)> {
    let counts = owner.get("RiskCounts").and_then(Value::as_object);
    RISKS
        .iter()
        .map(|risk| {
            let count = counts
                .and_then(|counts| counts.get(*risk))
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: RiskCounts must carry {risk}, got {owner:?}")
                });
            ((*risk).to_string(), count)
        })
        .collect()
}

/// Assert the document shows exactly the roll-up `review` reports — for the review as a
/// whole and for each of its pillars — so the report and the review's own read cannot
/// disagree about what the stored answers count to.
fn assert_rollups_agree(text: &str, review: &Value, case: &str) {
    for (risk, count) in risk_counts(review, case) {
        assert_shows(text, &format!("  {risk}: {count}"), case);
    }
    let summaries = review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for summary in &summaries {
        let pillar_id = text_member(summary, "PillarId").unwrap_or_default();
        let section = text
            .split(&format!("({pillar_id})"))
            .nth(1)
            .unwrap_or_else(|| panic!("case {case:?}: the report must have a {pillar_id} section"));
        for (risk, count) in risk_counts(summary, case) {
            assert!(
                section.contains(&format!("  {risk}: {count}")),
                "case {case:?}: the {pillar_id:?} section must count {risk} as {count}"
            );
        }
    }
}

/// One row of the lens table: a lens as a create and a read name it, with what the catalog
/// publishes for it.
struct Row {
    case: &'static str,
    /// The lens as the request names it — an alias for an AWS-official lens, an ARN for a
    /// custom one.
    lens: String,
    /// The `LensArn` that spelling resolves to.
    lens_arn: String,
    /// The `LensName` the lens's content gives it, absent for a lens this emulator holds no
    /// content for.
    lens_name: Option<&'static str>,
    /// The `LensVersion` the review is against, absent for the same reason.
    lens_version: Option<String>,
}

/// Rule 1, 2 and 3. Every lens review of a workload is reported as a PDF document that
/// identifies the review's lens and says what the review's own read says: over a table
/// spanning both halves of the lens catalog — the framework lens, two specialised official
/// lenses, an official lens this emulator holds no content for, and an imported custom lens
/// named by its ARN — plus both spellings of one of them.
#[tokio::test]
async fn every_lens_review_is_reported_as_a_document_derived_from_it() {
    let client = Client::new();

    // The one custom lens the table carries: imported, then published, because a lens
    // "cannot be applied to workloads … until it's published with CreateLensVersion"
    // (`ImportLens` doc).
    let custom_arn = import_shape(&client, "custom", CUSTOM_SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "custom", &custom_arn).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "framework-lens-by-alias",
            lens: FRAMEWORK_LENS.to_string(),
            lens_arn: official_lens_arn(REGION, FRAMEWORK_LENS),
            lens_name: Some(FRAMEWORK_LENS_NAME),
            lens_version: Some(FRAMEWORK_LENS_VERSION.to_string()),
        },
        Row {
            case: "framework-lens-by-arn",
            lens: official_lens_arn(REGION, FRAMEWORK_LENS),
            lens_arn: official_lens_arn(REGION, FRAMEWORK_LENS),
            lens_name: Some(FRAMEWORK_LENS_NAME),
            lens_version: Some(FRAMEWORK_LENS_VERSION.to_string()),
        },
        Row {
            case: "specialised-official-lens",
            lens: SERVERLESS_LENS.to_string(),
            lens_arn: official_lens_arn(REGION, SERVERLESS_LENS),
            lens_name: Some(SERVERLESS_LENS_NAME),
            lens_version: Some(SERVERLESS_LENS_VERSION.to_string()),
        },
        Row {
            case: "another-specialised-official-lens",
            lens: SAP_LENS.to_string(),
            lens_arn: official_lens_arn(REGION, SAP_LENS),
            lens_name: Some(SAP_LENS_NAME),
            lens_version: Some(SAP_LENS_VERSION.to_string()),
        },
        Row {
            case: "official-lens-without-content",
            lens: UNSHIPPED_LENS.to_string(),
            lens_arn: official_lens_arn(REGION, UNSHIPPED_LENS),
            lens_name: None,
            lens_version: None,
        },
        Row {
            case: "imported-custom-lens",
            lens: custom_arn.clone(),
            lens_arn: custom_arn.clone(),
            lens_name: Some("Lens custom"),
            lens_version: Some(CUSTOM_LENS_VERSION.to_string()),
        },
    ];

    for row in &rows {
        let case = row.case;
        let workload_id = workload_with_lens(&client, case, &row.lens).await;

        let answer = get_report(&client, &workload_id, &row.lens, None).await;
        let report = report_of(&answer, &workload_id, None, case);

        assert_eq!(
            text_member(&report, "LensAlias"),
            Some(row.lens.as_str()),
            "case {case:?}: the report names the lens as the request did, got {report:?}"
        );
        assert_eq!(
            text_member(&report, "LensArn"),
            Some(row.lens_arn.as_str()),
            "case {case:?}: the report carries the ARN that spelling resolves to, got \
             {report:?}"
        );

        // The review the report is a report OF, read through the operation that reports it.
        let review = lens_review_of(
            &get_lens_review(&client, &workload_id, &row.lens, None).await,
            case,
        );
        assert_eq!(
            text_member(&review, "LensName"),
            row.lens_name,
            "case {case:?}: the catalog must publish this lens's name, got {review:?}"
        );
        assert_eq!(
            text_member(&review, "LensVersion"),
            row.lens_version.as_deref(),
            "case {case:?}: the review must be against the published version, got {review:?}"
        );

        let text = document_text(&report, case);
        // A lens is identified by its alias, which an ARN carries as its last segment, so a
        // report of a review named either way documents the one lens.
        let alias = row.lens.rsplit('/').next().unwrap_or(&row.lens);
        assert_shows(&text, &format!("Workload: {case} ({workload_id})"), case);
        assert_shows(
            &text,
            &format!(
                "Lens: {} ({alias})",
                row.lens_name.unwrap_or(UNRECORDED)
            ),
            case,
        );
        assert_shows(&text, &format!("Lens ARN: {}", row.lens_arn), case);
        assert_shows(
            &text,
            &format!(
                "Lens version: {}",
                row.lens_version.as_deref().unwrap_or(UNRECORDED)
            ),
            case,
        );
        assert_eq!(
            documented_pillars(&text),
            review_pillars(&review, case),
            "case {case:?}: the report documents the pillars the review reports, got {text:?}"
        );
        assert_rollups_agree(&text, &review, case);

        // A read reports; it must not change what the next caller sees, and the document of
        // an unchanged review is the same document.
        let again = get_report(&client, &workload_id, &row.lens, None).await;
        assert_eq!(
            report_of(&again, &workload_id, None, case),
            report,
            "case {case:?}: a second report of an unchanged review must be the same report"
        );
    }
}

/// Rule 3 and 4. The document follows the review's stored state: the notes a caller writes
/// to the review and to its pillars appear in it, and an answer recorded against a question
/// moves the roll-ups the document prints — so a report taken before a write differs from
/// the one taken after it exactly where the review does.
#[tokio::test]
async fn the_document_follows_what_is_written_to_the_review() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "written-review", FRAMEWORK_LENS).await;

    let fresh = report_of(
        &get_report(&client, &workload_id, FRAMEWORK_LENS, None).await,
        &workload_id,
        None,
        "fresh",
    );
    let fresh_text = document_text(&fresh, "fresh");
    // Nothing is answered yet, so every question the lens asks counts as UNANSWERED.
    assert_shows(
        &fresh_text,
        &format!("  UNANSWERED: {FRAMEWORK_QUESTIONS}"),
        "fresh",
    );
    assert_shows(&fresh_text, "  HIGH: 0", "fresh");
    assert!(
        !fresh_text.contains("Lens review notes"),
        "case \"fresh\": a review nothing has been written to documents no notes"
    );

    // The notes a caller writes to the review and to one of its pillars.
    let notes = "First line of the review notes.\nSecond line, after a break.";
    let pillar_notes = "Security pillar notes.";
    let written = client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/lensReviews/{FRAMEWORK_LENS}"),
            Some(json!({
                "LensNotes": notes,
                "PillarNotes": { "security": pillar_notes },
            })),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "case \"notes\": expected UpdateLensReview 200, got {written:?}"
    );

    let noted = report_of(
        &get_report(&client, &workload_id, FRAMEWORK_LENS, None).await,
        &workload_id,
        None,
        "notes",
    );
    let noted_text = document_text(&noted, "notes");
    assert_shows(&noted_text, "Lens review notes", "notes");
    for line in notes.split('\n') {
        assert_shows(&noted_text, line, "notes");
    }
    assert_shows(&noted_text, pillar_notes, "notes");
    assert_ne!(
        text_member(&noted, "Base64String"),
        text_member(&fresh, "Base64String"),
        "case \"notes\": a write to the review must change the document it is reported as"
    );

    // An answer recorded against one question moves that question out of UNANSWERED and
    // into the bucket its answer carries, in the document exactly as in the review. The
    // answer records no best practice in place (its last choice does not apply), which is
    // what rates it HIGH.
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "identities",
        framework_answer("identities", InPlace::NothingAndTheLastDoesNotApply),
    );
    let answered = report_of(
        &get_report(&client, &workload_id, FRAMEWORK_LENS, None).await,
        &workload_id,
        None,
        "answered",
    );
    let answered_text = document_text(&answered, "answered");
    assert_shows(&answered_text, "  HIGH: 1", "answered");
    assert_shows(
        &answered_text,
        &format!("  UNANSWERED: {}", FRAMEWORK_QUESTIONS - 1),
        "answered",
    );
    assert_ne!(
        text_member(&answered, "Base64String"),
        text_member(&noted, "Base64String"),
        "case \"answered\": a recorded answer must change the document"
    );
    let review = lens_review_of(
        &get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await,
        "answered",
    );
    assert_rollups_agree(&answered_text, &review, "answered");
}

/// Rule 5. A milestone records the workload at an instant, so the report of a milestone's
/// review is the report of the review that milestone froze — while the live review's report
/// follows the writes that came after it. The milestone the request named is echoed and
/// documented; the live report names no milestone.
#[tokio::test]
async fn a_milestone_is_reported_as_the_review_it_froze() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "frozen-report", FRAMEWORK_LENS).await;

    let recorded = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": "frozen", "ClientRequestToken": "tok-frozen" })),
        )
        .await;
    assert_eq!(
        recorded.status, 200,
        "case \"milestone\": expected CreateMilestone 200, got {recorded:?}"
    );

    let frozen = report_of(
        &get_report(&client, &workload_id, FRAMEWORK_LENS, Some(1)).await,
        &workload_id,
        Some(1),
        "frozen",
    );
    let frozen_text = document_text(&frozen, "frozen");
    assert_shows(&frozen_text, "Milestone number: 1", "frozen");
    assert_shows(
        &frozen_text,
        &format!("  UNANSWERED: {FRAMEWORK_QUESTIONS}"),
        "frozen",
    );

    // A write the milestone did not record: an answer with no best practice in place.
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "identities",
        framework_answer("identities", InPlace::NothingAndTheLastDoesNotApply),
    );

    let live = report_of(
        &get_report(&client, &workload_id, FRAMEWORK_LENS, None).await,
        &workload_id,
        None,
        "live",
    );
    let live_text = document_text(&live, "live");
    assert_shows(&live_text, "  HIGH: 1", "live");
    assert!(
        !live_text.contains("Milestone number:"),
        "case \"live\": a report of the live review names no milestone"
    );

    let again = report_of(
        &get_report(&client, &workload_id, FRAMEWORK_LENS, Some(1)).await,
        &workload_id,
        Some(1),
        "frozen-again",
    );
    assert_eq!(
        text_member(&again, "Base64String"),
        text_member(&frozen, "Base64String"),
        "case \"frozen-again\": the milestone's report is the review it froze"
    );
}

/// Rule 6. What the request has to resolve, and what it is refused for: the workload and the
/// milestone are resources (a 404 each, with the modelled `ResourceType` and the identifier
/// as sent), the lens is a VALUE of the request (a 400 naming `LensAlias`), and a
/// `MilestoneNumber` outside `@range(min: 1, max: 100)` is refused before either — while
/// each published bound of that range is served.
#[tokio::test]
async fn the_report_refuses_what_it_cannot_resolve() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "refusals", FRAMEWORK_LENS).await;

    let absent = get_report(&client, ABSENT_HEX_ID, FRAMEWORK_LENS, None).await;
    assert_not_found(
        "absent-workload",
        &absent,
        "WORKLOAD",
        ABSENT_HEX_ID,
        &format!("workload with ID {ABSENT_HEX_ID}"),
    );

    // No milestone has been recorded, so every number in the modelled range names none.
    for number in [1, 100] {
        let answer = get_report(&client, &workload_id, FRAMEWORK_LENS, Some(number)).await;
        assert_not_found(
            &format!("milestone-{number}"),
            &answer,
            "MILESTONE",
            &format!("{workload_id}.{number}"),
            &format!(
                "milestone with number {number} in a workload with ID {workload_id}"
            ),
        );
    }

    // A lens the workload does not review names no review to report, by either spelling.
    for lens in [
        SERVERLESS_LENS.to_string(),
        official_lens_arn(REGION, SERVERLESS_LENS),
    ] {
        let answer = get_report(&client, &workload_id, &lens, None).await;
        assert_field_validation(
            &format!("unreviewed-lens-{lens}"),
            &answer,
            "LensAlias",
            &format!("No lens review for lens alias {lens} was found in workload {workload_id}."),
        );
    }

    // One past each published bound of `MilestoneNumber` is refused by the boundary, before
    // the workload or the lens is resolved.
    for number in [0, 101] {
        let answer = get_report(&client, &workload_id, FRAMEWORK_LENS, Some(number)).await;
        assert_eq!(
            answer.status, 400,
            "case \"milestone-{number}\": a value outside the modelled range is a 400, got \
             {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case \"milestone-{number}\": the refusal must be the modelled error, got {answer:?}"
        );
    }
}

/// Rule 2 and 3, at the longest value a review can carry. `Notes` is `@length(max: 2084)`
/// (`smithy_query(query="operation", operation="UpdateLensReview")`), which is more text
/// than one page of the document holds, so a report of a review carrying the longest notes
/// the model admits is still ONE document a reader opens — wrapped to the page's text
/// column and continued onto as many pages as it takes.
#[tokio::test]
async fn the_longest_notes_the_model_admits_are_documented_in_full() {
    let client = Client::new();
    let workload_id = workload_with_lens(&client, "long-notes", FRAMEWORK_LENS).await;

    // Built from one repeated word, so every wrap falls between words and the document can
    // be checked for text that crossed no line break.
    let word = "architecture ";
    let notes: String = word.repeat(NOTES_MAX / word.len());
    assert!(
        notes.len() <= NOTES_MAX,
        "the notes under test must satisfy @length(max: {NOTES_MAX}), got {}",
        notes.len()
    );
    let written = client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/lensReviews/{FRAMEWORK_LENS}"),
            Some(json!({ "LensNotes": notes })),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "case \"long-notes\": expected UpdateLensReview 200, got {written:?}"
    );

    let report = report_of(
        &get_report(&client, &workload_id, FRAMEWORK_LENS, None).await,
        &workload_id,
        None,
        "long-notes",
    );
    let text = document_text(&report, "long-notes");
    assert_shows(&text, "Lens review notes", "long-notes");
    assert_shows(&text, &word.repeat(2), "long-notes");
    // One `/MediaBox` per page of the document: the notes alone are more lines than a page
    // holds, so there is more than one.
    assert!(
        text.matches("/MediaBox").count() > 1,
        "case \"long-notes\": a report longer than one page must carry more pages, got {} \
         page(s)",
        text.matches("/MediaBox").count()
    );
}
