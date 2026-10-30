// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for EVERY PROJECTION of a stored workload's lens list.
//!
//! A workload's lens list is reported by three readers — `Workload$Lenses` through
//! `GetWorkload`, the same member on the `Workload` `UpdateWorkload` answers with, and
//! `WorkloadSummary$Lenses` through `ListWorkloads`
//! (`smithy_query(query="operation", operation="GetWorkload,UpdateWorkload,ListWorkloads")`)
//! — and everything derived from it, the lens reviews `ListLensReviews` reports and the
//! `RiskCounts` roll-up counted over the lenses the workload reviews.
//!
//! The rules pinned here, each over a TABLE of lens lists rather than one recorded request:
//!
//! 1. A lens is ONE lens however a request spells it — "for Amazon Web Services official
//!    lenses, this is either the lens alias, such as `serverless`, or the lens ARN, such as
//!    `arn:aws:wellarchitected:us-east-1::lens/serverless`"
//!    (`smithy_query(query="shape", shape="LensAlias")`), and a lens ARN may pin a version
//!    after the alias — so EVERY projection of a workload's lens list reports each applied
//!    lens exactly once, the three readers agree, and the list is exactly as long as the
//!    workload's lens reviews: a workload never reports two lenses and one review of them
//!    at the same time.
//! 2. Therefore a list that names its lenses several ways produces the same workload as the
//!    list naming each of them once: the same lens list through all three readers, the same
//!    lens reviews, and the same `RiskCounts` roll-up — a repeat is not counted twice.
//! 3. The lenses an applied review template contributes ("if a review template that
//!    specifies lenses is applied to the workload, those lenses are applied to the workload
//!    in addition to these lenses",
//!    `smithy_query(query="doc", shape="CreateWorkloadInput", member="Lenses")`) obey the
//!    same identity rule: a template lens the request already named, however either of them
//!    spelled it, leaves the list one entry for that lens, under the name the request sent.
//!
//! `super::applied_lens_identity` pins rule 1 for the list `GetWorkload` reports and for a
//! review template's own list; this suite is about the OTHER readers of a workload's list
//! and about what the roll-up counts.
//!
//! Every resource is minted over HTTP and everything asserted crosses the HTTP boundary:
//! requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{
    created_arn, created_id, minimal_create, minimal_template, official_lens_arn, stored_workload,
    Client, SIGNED_REGION,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A version-pinned official lens ARN: a lens ARN may pin a version after the alias, which
/// names the same lens as the bare alias. The version is a lens version string, not one
/// this emulator has to publish, because what is under test is the NAME.
fn pinned_lens_arn(region: &str, alias: &str, version: &str) -> String {
    format!("{}/{version}", official_lens_arn(region, alias))
}

/// One row of the lens-list table: the `Lenses` a create sends, and the list every
/// projection of the created workload must then report — each lens once, in the order first
/// named, as that naming spelled it.
struct LensListCase {
    case: &'static str,
    slug: &'static str,
    sent: Vec<String>,
    applied: Vec<String>,
}

/// The lens lists under test. The rows differ along what the rule generalises over: how
/// many lenses a list names, WHICH spelling names a lens (a bare alias, that lens's official
/// ARN in the signing Region or in another one, a version-pinned ARN), how many times one
/// lens is repeated, where the repeats sit, and how many entries the list carries against
/// the number of lenses it names — including more entries than the per-workload lens quota
/// allows LENSES, which names one lens and is therefore served.
fn lens_list_cases() -> Vec<LensListCase> {
    vec![
        LensListCase {
            case: "one lens named three times running",
            slug: "thrice-in-a-row",
            sent: vec![FRAMEWORK_LENS.to_string(); 3],
            applied: vec![FRAMEWORK_LENS.to_string()],
        },
        LensListCase {
            case: "two lenses, each named twice, the repeats separated",
            slug: "two-lenses-separated-repeats",
            sent: vec![
                FRAMEWORK_LENS.to_string(),
                "serverless".to_string(),
                official_lens_arn("us-east-1", FRAMEWORK_LENS),
                pinned_lens_arn(SIGNED_REGION, "serverless", "2024-10-03"),
            ],
            applied: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        LensListCase {
            case: "a version-pinned ARN first, then the bare alias",
            slug: "pinned-then-alias",
            sent: vec![
                pinned_lens_arn("us-east-1", "iot", "2024-10-03"),
                "iot".to_string(),
            ],
            applied: vec![pinned_lens_arn("us-east-1", "iot", "2024-10-03")],
        },
        // The control row for the rules below: this list names no lens twice, so what it
        // pins is that the de-duplication keeps a list with no repeat exactly as sent,
        // order included.
        LensListCase {
            case: "two lenses named once each",
            slug: "two-distinct-lenses",
            sent: vec!["devops".to_string(), "sap".to_string()],
            applied: vec!["devops".to_string(), "sap".to_string()],
        },
        LensListCase {
            case: "one more entry than the lens quota allows lenses, all naming one lens",
            slug: "past-quota-entries-one-lens",
            sent: vec!["genai".to_string(); state::MAX_LENSES_PER_WORKLOAD + 1],
            applied: vec!["genai".to_string()],
        },
    ]
}

/// The `Lenses` a workload projection reports, as sent.
fn lenses_of(workload: &Value) -> Vec<String> {
    workload
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

/// Create a workload attaching `lenses` as sent, optionally applying review templates, and
/// answer its id.
async fn workload_attaching(
    client: &Client,
    slug: &str,
    lenses: &[String],
    templates: &[String],
) -> String {
    let mut request = minimal_create(&format!("wl-{slug}"), &format!("tok-wl-{slug}"));
    request["Lenses"] = json!(lenses);
    if !templates.is_empty() {
        request["ReviewTemplateArns"] = json!(templates);
    }
    created_id(&client.create(request).await, slug)
}

/// The `Workload` `UpdateWorkload` answers with for a write that touches a member of its
/// own: the lens list is a member of the workload the answer carries, so the update reports
/// it too.
async fn updated_workload(client: &Client, workload_id: &str, case: &str) -> Value {
    let answer = client
        .update(workload_id, json!({ "Notes": "touched by the projection suite" }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateWorkload 200, got {answer:?}"
    );
    answer.body.get("Workload").cloned().unwrap_or(Value::Null)
}

/// The `WorkloadSummary` `ListWorkloads` reports for one stored workload.
async fn summarized_workload(client: &Client, workload_id: &str, case: &str) -> Value {
    let answer = client.list(json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListWorkloads 200, got {answer:?}"
    );
    let summaries = answer
        .body
        .get("WorkloadSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    summaries
        .iter()
        .find(|summary| summary.get("WorkloadId").and_then(Value::as_str) == Some(workload_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must report the workload under test, got {answer:?}")
        })
}

/// The `LensAlias` of every lens review `ListLensReviews` reports for a workload, sorted:
/// the listing pages in its own stable order, so what a row pins is the SET of reviews and
/// how many there are.
async fn reviewed_lenses(client: &Client, workload_id: &str, case: &str) -> Vec<String> {
    let answer = client
        .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    let mut reviewed: Vec<String> = answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("LensAlias").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    reviewed.sort();
    reviewed
}

/// Rule 1: each of the three readers of a workload's lens list reports the applied lenses,
/// each once, and the list is exactly as long as the workload's lens reviews.
#[tokio::test]
async fn every_reader_of_a_workloads_lens_list_reports_each_applied_lens_once() {
    let client = Client::new();
    for row in lens_list_cases() {
        let case = row.case;
        let workload_id = workload_attaching(&client, row.slug, &row.sent, &[]).await;

        for (reader, workload) in [
            (
                "GetWorkload",
                stored_workload(&client, &workload_id, case).await,
            ),
            (
                "UpdateWorkload",
                updated_workload(&client, &workload_id, case).await,
            ),
            (
                "ListWorkloads",
                summarized_workload(&client, &workload_id, case).await,
            ),
        ] {
            assert_eq!(
                lenses_of(&workload),
                row.applied,
                "case {case:?}: sent {:?}, expected {reader} to report Lenses {:?}, got {:?}",
                row.sent,
                row.applied,
                lenses_of(&workload)
            );
        }

        // The list and the reviews agree about how many lenses the workload has: one
        // review per reported lens.
        let mut expected = row.applied.clone();
        expected.sort();
        let reviewed = reviewed_lenses(&client, &workload_id, case).await;
        assert_eq!(
            reviewed, expected,
            "case {case:?}: a workload reporting Lenses {:?} must hold exactly those lens \
             reviews, got {reviewed:?}",
            row.applied
        );
    }
}

/// Rule 2: a list naming its lenses several ways asks for the same workload as the list
/// naming each of them once — the same lens list through every reader, the same lens
/// reviews, and the same `RiskCounts`, which is counted over the lenses the workload
/// reviews and so counts a repeated lens once.
#[tokio::test]
async fn naming_a_lens_several_ways_asks_for_the_same_workload_as_naming_it_once() {
    let client = Client::new();
    for row in lens_list_cases() {
        let case = row.case;
        let under_test = workload_attaching(&client, row.slug, &row.sent, &[]).await;
        let reference =
            workload_attaching(&client, &format!("{}-ref", row.slug), &row.applied, &[]).await;

        let tested = stored_workload(&client, &under_test, case).await;
        let expected = stored_workload(&client, &reference, case).await;
        assert_eq!(
            lenses_of(&tested),
            lenses_of(&expected),
            "case {case:?}: sent {:?}, expected the lenses of the workload attaching {:?}, got \
             {:?}",
            row.sent,
            row.applied,
            lenses_of(&tested)
        );
        // The roll-up has to BE reported for comparing two of them to mean anything: a read
        // of a workload carries `RiskCounts`, one count per `Risk` the model publishes, so
        // the bucket a created workload's outstanding questions land in has to be there.
        for (reader, workload) in [("GetWorkload", &tested), ("the reference", &expected)] {
            assert!(
                workload
                    .get("RiskCounts")
                    .and_then(Value::as_object)
                    .is_some_and(|counts| counts.contains_key("UNANSWERED")),
                "case {case:?}: {reader} must report the workload's risk roll-up, got {workload:?}"
            );
        }
        assert_eq!(
            tested.get("RiskCounts"),
            expected.get("RiskCounts"),
            "case {case:?}: the risk roll-up is counted over the lenses the workload reviews, so \
             it must match the workload attaching {:?}, got {tested:?}",
            row.applied
        );

        let tested_summary = summarized_workload(&client, &under_test, case).await;
        let expected_summary = summarized_workload(&client, &reference, case).await;
        assert_eq!(
            tested_summary.get("RiskCounts"),
            expected_summary.get("RiskCounts"),
            "case {case:?}: the list summary's risk roll-up must match the workload attaching \
             {:?}, got {tested_summary:?}",
            row.applied
        );

        let tested_reviews = reviewed_lenses(&client, &under_test, case).await;
        let expected_reviews = reviewed_lenses(&client, &reference, case).await;
        assert_eq!(
            tested_reviews, expected_reviews,
            "case {case:?}: expected the lens reviews of the workload attaching {:?}, got \
             {tested_reviews:?}",
            row.applied
        );
    }
}

/// One row of the review-template table: the lenses the applied template carries, the
/// lenses the create names itself, and the list every reader of the created workload must
/// then report.
struct TemplateMergeCase {
    case: &'static str,
    slug: &'static str,
    template_lenses: Vec<String>,
    sent: Vec<String>,
    applied: Vec<String>,
}

/// Rule 3: the lenses an applied review template contributes are subject to the same
/// identity rule as the ones the request names, whichever of the two spelled the lens which
/// way.
#[tokio::test]
async fn a_review_templates_lenses_are_applied_once_beside_the_requests_own() {
    let client = Client::new();
    let rows = vec![
        TemplateMergeCase {
            case: "the template names a lens the request already named, as its ARN",
            slug: "template-arn-request-alias",
            template_lenses: vec![official_lens_arn("us-east-1", "serverless")],
            sent: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
            applied: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        TemplateMergeCase {
            case: "the request names as an ARN a lens the template names as its alias",
            slug: "template-alias-request-arn",
            template_lenses: vec!["devops".to_string()],
            sent: vec![official_lens_arn(SIGNED_REGION, "devops")],
            applied: vec![official_lens_arn(SIGNED_REGION, "devops")],
        },
        TemplateMergeCase {
            case: "the template brings a lens the request does not name",
            slug: "template-brings-a-lens",
            template_lenses: vec!["sap".to_string()],
            sent: vec![FRAMEWORK_LENS.to_string()],
            applied: vec![FRAMEWORK_LENS.to_string(), "sap".to_string()],
        },
        TemplateMergeCase {
            case: "the request repeats its own lens and the template repeats it too",
            slug: "repeats-on-both-sides",
            template_lenses: vec![
                pinned_lens_arn("us-east-1", "iot", "2024-10-03"),
                "machinelearning".to_string(),
            ],
            sent: vec![
                "iot".to_string(),
                official_lens_arn("eu-west-1", "iot"),
                "iot".to_string(),
            ],
            applied: vec!["iot".to_string(), "machinelearning".to_string()],
        },
    ];

    for row in rows {
        let case = row.case;
        let mut template_request =
            minimal_template(&format!("tpl-{}", row.slug), &format!("tok-tpl-{}", row.slug));
        template_request["Lenses"] = json!(row.template_lenses);
        let template_arn = created_arn(&client.create_review_template(template_request).await, case);

        let workload_id =
            workload_attaching(&client, row.slug, &row.sent, std::slice::from_ref(&template_arn))
                .await;

        for (reader, workload) in [
            (
                "GetWorkload",
                stored_workload(&client, &workload_id, case).await,
            ),
            (
                "UpdateWorkload",
                updated_workload(&client, &workload_id, case).await,
            ),
            (
                "ListWorkloads",
                summarized_workload(&client, &workload_id, case).await,
            ),
        ] {
            assert_eq!(
                lenses_of(&workload),
                row.applied,
                "case {case:?}: a create naming {:?} and applying a template carrying {:?} must \
                 have {reader} report Lenses {:?}, got {:?}",
                row.sent,
                row.template_lenses,
                row.applied,
                lenses_of(&workload)
            );
        }

        let mut expected = row.applied.clone();
        expected.sort();
        let reviewed = reviewed_lenses(&client, &workload_id, case).await;
        assert_eq!(
            reviewed, expected,
            "case {case:?}: a workload reporting Lenses {:?} must hold exactly those lens \
             reviews, got {reviewed:?}",
            row.applied
        );
    }
}
