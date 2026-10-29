// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the LENS IDENTITY an operation that applies lenses to a
//! resource holds to — `CreateReviewTemplate` and `CreateWorkload`, beside the
//! `UpdateReviewTemplate` and `AssociateLenses` that apply lenses to the stored resource.
//!
//! The rules pinned here, each over a TABLE of lens lists rather than one recorded request:
//!
//! 1. A lens is ONE lens however a request spells it — "for Amazon Web Services official
//!    lenses, this is either the lens alias, such as `serverless`, or the lens ARN, such as
//!    `arn:aws:wellarchitected:us-east-1::lens/serverless`"
//!    (`smithy_query(query="shape", shape="LensAlias")`), and a lens ARN may pin a version
//!    after the alias — so the lens list a resource stores and reports keeps each named
//!    lens once, in the order the request first named it, spelled as that first naming
//!    sent it. `ReviewTemplateLenses` and `WorkloadLenses` carry no `@uniqueItems`
//!    (`smithy_query(query="shape", shape="ReviewTemplateLenses,WorkloadLenses")`), so the
//!    boundary validator does not apply this: the handler does.
//! 2. A resource's lens list and its lens reviews therefore AGREE about how many lenses it
//!    applies: one review per reported lens, each readable back under the name the list
//!    reports, so the list is never longer than the reviews.
//! 3. Which path a lens arrived through does not change the list: a resource created with a
//!    list and one the same lenses were applied to afterwards report the same lenses.
//! 4. The per-resource lens quota counts the DISTINCT lenses the resource would end up
//!    applying, so a list naming one lens many times is metered for the one lens it
//!    applies, while a list naming more distinct lenses than the quota allows is refused
//!    with the modelled 402 `ServiceQuotaExceededException` and stores nothing.
//!
//! Every resource is minted over HTTP and everything asserted crosses the HTTP boundary:
//! requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{
    created_arn, created_id, encode_path_segment, member, minimal_create, minimal_template,
    official_lens_arn, Answer, Client, SIGNED_REGION,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// Aliases AWS publishes lenses under, used here wherever a row needs more distinct lenses
/// than the per-resource quota allows.
///
/// SOURCE: the Lens Catalog of the AWS Well-Architected Tool user guide
/// (<https://docs.aws.amazon.com/wellarchitected/latest/userguide/lenses.html>), the same
/// catalog `state::AWS_OFFICIAL_LENS_ALIASES` carries — written out here so the tests pin
/// it rather than read it back out of the code under test.
const OFFICIAL_LENSES: [&str; 14] = [
    "connectedmobility",
    "dataanalytics",
    "devops",
    "financialservices",
    "games",
    "genai",
    "healthcareindustry",
    "iot",
    "machinelearning",
    "mergersandacquisitions",
    "migration",
    "sap",
    "serverless",
    "softwareasaservice",
];

/// A version-pinned official lens ARN: "a lens ARN may also pin a version after the alias",
/// which names the same lens as the bare alias. The version is a lens version string, not
/// one this emulator has to publish, because what is under test is the NAME.
fn pinned_lens_arn(region: &str, alias: &str, version: &str) -> String {
    format!("{}/{version}", official_lens_arn(region, alias))
}

/// One row of the lens-list table: the `Lenses` a create sends, and the list the created
/// resource must then report — each lens once, in the order first named, as that naming
/// spelled it.
struct LensListCase {
    case: &'static str,
    slug: &'static str,
    sent: Vec<String>,
    applied: Vec<String>,
}

/// The lens lists under test. The rows differ along what the rule generalises over: how
/// many lenses a list names, WHICH spelling names a lens (a bare alias, that lens's
/// official ARN in another Region, a version-pinned ARN), how many times one lens is
/// repeated, where the repeats sit in the list, and how many entries the list carries
/// against the number of lenses it names.
fn lens_list_cases() -> Vec<LensListCase> {
    vec![
        LensListCase {
            case: "two lenses, no repeat",
            slug: "two-distinct",
            sent: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
            applied: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        LensListCase {
            case: "one lens as its alias and then as its official ARN",
            slug: "alias-then-arn",
            sent: vec![
                FRAMEWORK_LENS.to_string(),
                official_lens_arn("us-east-1", FRAMEWORK_LENS),
            ],
            applied: vec![FRAMEWORK_LENS.to_string()],
        },
        LensListCase {
            case: "one lens as its official ARN and then as its alias",
            slug: "arn-then-alias",
            sent: vec![
                official_lens_arn(SIGNED_REGION, "serverless"),
                "serverless".to_string(),
            ],
            applied: vec![official_lens_arn(SIGNED_REGION, "serverless")],
        },
        LensListCase {
            case: "a version-pinned ARN names the lens its alias names",
            slug: "pinned-arn",
            sent: vec![
                "devops".to_string(),
                pinned_lens_arn("us-east-1", "devops", "2024-10-03"),
            ],
            applied: vec!["devops".to_string()],
        },
        LensListCase {
            case: "the same spelling twice",
            slug: "same-spelling-twice",
            sent: vec!["sap".to_string(), "sap".to_string()],
            applied: vec!["sap".to_string()],
        },
        LensListCase {
            case: "three spellings of one lens",
            slug: "three-spellings",
            sent: vec![
                "genai".to_string(),
                official_lens_arn("eu-west-1", "genai"),
                pinned_lens_arn("us-east-1", "genai", "2025-01-02"),
            ],
            applied: vec!["genai".to_string()],
        },
        LensListCase {
            case: "repeats interleaved with other lenses keep the first-named order",
            slug: "interleaved-repeats",
            sent: vec![
                "serverless".to_string(),
                "iot".to_string(),
                official_lens_arn("us-east-1", "serverless"),
                "games".to_string(),
                "iot".to_string(),
            ],
            applied: vec![
                "serverless".to_string(),
                "iot".to_string(),
                "games".to_string(),
            ],
        },
        LensListCase {
            case: "as many entries as the quota allows lenses, all naming one lens",
            slug: "quota-entries-one-lens",
            sent: vec!["machinelearning".to_string(); state::MAX_LENSES_PER_REVIEW_TEMPLATE],
            applied: vec!["machinelearning".to_string()],
        },
        LensListCase {
            case: "more entries than the quota allows lenses, all naming one lens",
            slug: "past-quota-entries-one-lens",
            sent: vec!["financialservices".to_string(); state::MAX_LENSES_PER_REVIEW_TEMPLATE + 1],
            applied: vec!["financialservices".to_string()],
        },
        LensListCase {
            case: "as many distinct lenses as the quota allows",
            slug: "quota-distinct-lenses",
            sent: OFFICIAL_LENSES[..state::MAX_LENSES_PER_REVIEW_TEMPLATE]
                .iter()
                .map(ToString::to_string)
                .collect(),
            applied: OFFICIAL_LENSES[..state::MAX_LENSES_PER_REVIEW_TEMPLATE]
                .iter()
                .map(ToString::to_string)
                .collect(),
        },
    ]
}

/// The `Lenses` a template or workload projection reports, as sent.
fn lenses_of(resource: &Value) -> Vec<String> {
    resource
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

/// The `ReviewTemplate` `GetReviewTemplate` reports for a stored template.
async fn stored_template(client: &Client, template_arn: &str, case: &str) -> Value {
    let answer = client.get_review_template(template_arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplate 200, got {answer:?}"
    );
    answer
        .body
        .get("ReviewTemplate")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `Workload` `GetWorkload` reports for a stored workload.
async fn stored_workload(client: &Client, workload_id: &str, case: &str) -> Value {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetWorkload 200, got {answer:?}"
    );
    answer.body.get("Workload").cloned().unwrap_or(Value::Null)
}

/// The `ReviewTemplateSummary$Lenses` `ListReviewTemplates` reports for one template.
async fn summarized_lenses(client: &Client, template_arn: &str, case: &str) -> Vec<String> {
    let answer = client.list_review_templates("").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListReviewTemplates 200, got {answer:?}"
    );
    let summaries = answer
        .body
        .get("ReviewTemplates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let summary = summaries
        .iter()
        .find(|summary| summary.get("TemplateArn").and_then(Value::as_str) == Some(template_arn))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must report the template under test, got {answer:?}")
        });
    lenses_of(summary)
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

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`.
async fn get_lens_review(client: &Client, workload_id: &str, lens: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await
}

/// `AssociateLenses`: `PATCH /workloads/{WorkloadId}/associateLenses`.
async fn associate_lenses(client: &Client, workload_id: &str, lenses: &[String]) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateLenses"),
            Some(json!({ "LensAliases": lenses })),
        )
        .await
}

/// Create a review template applying `lenses` as sent, and answer its ARN.
async fn template_applying(client: &Client, slug: &str, lenses: &[String]) -> String {
    let mut request = minimal_template(&format!("tpl-{slug}"), &format!("tok-tpl-{slug}"));
    request["Lenses"] = json!(lenses);
    created_arn(&client.create_review_template(request).await, slug)
}

/// Create a workload attaching `lenses` as sent, and answer its id.
async fn workload_attaching(client: &Client, slug: &str, lenses: &[String]) -> String {
    let mut request = minimal_create(&format!("wl-{slug}"), &format!("tok-wl-{slug}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, slug)
}

/// Rule 1 and 2, for a REVIEW TEMPLATE: the created template reports each named lens once,
/// in the order first named and as that naming spelled it, through `GetReviewTemplate` and
/// `ListReviewTemplates` alike — and holds exactly one lens review per reported lens.
#[tokio::test]
async fn a_created_review_template_applies_each_named_lens_once() {
    let client = Client::new();
    for row in lens_list_cases() {
        let case = row.case;
        let template_arn = template_applying(&client, row.slug, &row.sent).await;

        let template = stored_template(&client, &template_arn, case).await;
        assert_eq!(
            lenses_of(&template),
            row.applied,
            "case {case:?}: sent {:?}, expected ReviewTemplate$Lenses {:?}, got {:?}",
            row.sent,
            row.applied,
            lenses_of(&template)
        );
        let summarized = summarized_lenses(&client, &template_arn, case).await;
        assert_eq!(
            summarized, row.applied,
            "case {case:?}: expected ReviewTemplateSummary$Lenses {:?}, got {summarized:?}",
            row.applied
        );

        // Rule 2: every lens the list reports has a review under the name it reports, so
        // the list and the reviews agree about which lenses the template applies.
        for lens in &row.applied {
            let answer = client
                .get_review_template_lens_review(&template_arn, lens)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: lens {lens:?} is reported applied, so its lens review must \
                 be readable, got {answer:?}"
            );
            let reported = answer
                .body
                .get("LensReview")
                .and_then(|review| review.get("LensAlias"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                reported, lens,
                "case {case:?}: the lens review must name the lens as the template records \
                 it, got {answer:?}"
            );
        }
    }
}

/// Rule 1 and 2, for a WORKLOAD: the created workload reports each named lens once and is
/// reviewed against exactly those lenses.
#[tokio::test]
async fn a_created_workload_attaches_each_named_lens_once() {
    let client = Client::new();
    for row in lens_list_cases() {
        let case = row.case;
        let workload_id = workload_attaching(&client, row.slug, &row.sent).await;

        let workload = stored_workload(&client, &workload_id, case).await;
        assert_eq!(
            lenses_of(&workload),
            row.applied,
            "case {case:?}: sent {:?}, expected Workload$Lenses {:?}, got {:?}",
            row.sent,
            row.applied,
            lenses_of(&workload)
        );

        // Rule 2: one review per reported lens, and each readable under the name the
        // workload records.
        let mut expected = row.applied.clone();
        expected.sort();
        let reviewed = reviewed_lenses(&client, &workload_id, case).await;
        assert_eq!(
            reviewed, expected,
            "case {case:?}: expected one lens review per reported lens {expected:?}, got \
             {reviewed:?}"
        );
        for lens in &row.applied {
            let answer = get_lens_review(&client, &workload_id, lens).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: lens {lens:?} is reported attached, so its lens review must \
                 be readable, got {answer:?}"
            );
        }
    }
}

/// Rule 1, as an equivalence: a list that names a lens several ways asks for the same
/// resource as the list that names it once, down to the question roll-up a read reports
/// over the lenses the resource applies.
#[tokio::test]
async fn naming_a_lens_several_ways_asks_for_the_same_resource_as_naming_it_once() {
    let client = Client::new();
    // (case, the list under test, the list it must be equivalent to)
    let rows: Vec<(&str, &str, Vec<String>, Vec<String>)> = vec![
        (
            "an alias beside its own ARN",
            "equiv-alias-arn",
            vec![
                FRAMEWORK_LENS.to_string(),
                official_lens_arn("us-east-1", FRAMEWORK_LENS),
            ],
            vec![FRAMEWORK_LENS.to_string()],
        ),
        (
            "a repeated lens beside a lens named once",
            "equiv-repeat-and-single",
            vec![
                FRAMEWORK_LENS.to_string(),
                "serverless".to_string(),
                pinned_lens_arn("us-east-1", FRAMEWORK_LENS, "2024-10-03"),
                official_lens_arn("eu-west-1", "serverless"),
            ],
            vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        ),
    ];
    for (case, slug, sent, equivalent) in rows {
        let under_test = template_applying(&client, slug, &sent).await;
        let reference = template_applying(&client, &format!("{slug}-ref"), &equivalent).await;

        let tested = stored_template(&client, &under_test, case).await;
        let expected = stored_template(&client, &reference, case).await;
        assert_eq!(
            lenses_of(&tested),
            lenses_of(&expected),
            "case {case:?}: sent {sent:?}, expected the lenses of the template applying \
             {equivalent:?}, got {:?}",
            lenses_of(&tested)
        );
        assert_eq!(
            tested.get("QuestionCounts"),
            expected.get("QuestionCounts"),
            "case {case:?}: the question roll-up is counted over the lenses the template \
             applies, so it must match the template applying {equivalent:?}, got {tested:?}"
        );
    }
}

/// One row of the arrival-path table: the lenses the create sends, the lenses applied to
/// the stored resource afterwards, and the lens list the resource must then report.
struct ArrivalPathCase {
    case: &'static str,
    slug: &'static str,
    created: Vec<String>,
    associated: Vec<String>,
    applied: Vec<String>,
}

/// Rule 3, for a review template: the lens list does not depend on whether its lenses
/// arrived through the create or through the update that applies lenses to the stored
/// template.
#[tokio::test]
async fn a_review_templates_lens_list_does_not_depend_on_the_path_its_lenses_arrived_through() {
    let client = Client::new();
    let rows = vec![
        ArrivalPathCase {
            case: "a repeated lens applied by the update",
            slug: "path-update-repeat",
            created: vec![FRAMEWORK_LENS.to_string()],
            associated: vec![
                "serverless".to_string(),
                official_lens_arn("us-east-1", "serverless"),
            ],
            applied: vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()],
        },
        ArrivalPathCase {
            case: "a lens the create already applied, re-applied by the update",
            slug: "path-update-already-applied",
            created: vec![FRAMEWORK_LENS.to_string(), "iot".to_string()],
            associated: vec![
                official_lens_arn("us-east-1", "iot"),
                "sap".to_string(),
                pinned_lens_arn("us-east-1", "sap", "2024-10-03"),
            ],
            applied: vec![
                FRAMEWORK_LENS.to_string(),
                "iot".to_string(),
                "sap".to_string(),
            ],
        },
    ];
    for row in rows {
        let ArrivalPathCase {
            case,
            slug,
            created,
            associated,
            applied,
        } = row;
        let template_arn = template_applying(&client, slug, &created).await;
        let answer = client
            .update_review_template(
                &template_arn,
                json!({ "LensesToAssociate": associated.clone() }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateReviewTemplate 200, got {answer:?}"
        );

        let template = stored_template(&client, &template_arn, case).await;
        assert_eq!(
            lenses_of(&template),
            applied,
            "case {case:?}: created with {created:?} then applying {associated:?}, expected \
             {applied:?}, got {:?}",
            lenses_of(&template)
        );

        // The same lenses sent to the CREATE instead: both paths store one entry per lens,
        // so the two templates report the same list.
        let mut sent = created.clone();
        sent.extend(associated.clone());
        let created_whole = template_applying(&client, &format!("{slug}-create"), &sent).await;
        let whole = stored_template(&client, &created_whole, case).await;
        assert_eq!(
            lenses_of(&whole),
            applied,
            "case {case:?}: a create sending {sent:?} must report the same lenses as the \
             update path, got {:?}",
            lenses_of(&whole)
        );
    }
}

/// Rule 3, for a workload: the lens list does not depend on whether its lenses arrived
/// through the create or through `AssociateLenses`.
#[tokio::test]
async fn a_workloads_lens_list_does_not_depend_on_the_path_its_lenses_arrived_through() {
    let client = Client::new();
    let created = vec![FRAMEWORK_LENS.to_string()];
    let associated = vec![
        "serverless".to_string(),
        official_lens_arn("us-east-1", "serverless"),
        pinned_lens_arn("us-east-1", "serverless", "2024-10-03"),
    ];
    let applied = vec![FRAMEWORK_LENS.to_string(), "serverless".to_string()];
    let case = "a repeated lens associated with a stored workload";

    let workload_id = workload_attaching(&client, "path-associate", &created).await;
    let answer = associate_lenses(&client, &workload_id, &associated).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected AssociateLenses 200, got {answer:?}"
    );
    let workload = stored_workload(&client, &workload_id, case).await;
    assert_eq!(
        lenses_of(&workload),
        applied,
        "case {case:?}: created with {created:?} then associating {associated:?}, expected \
         {applied:?}, got {:?}",
        lenses_of(&workload)
    );

    let mut sent = created.clone();
    sent.extend(associated.clone());
    let created_whole = workload_attaching(&client, "path-create-whole", &sent).await;
    let whole = stored_workload(&client, &created_whole, case).await;
    assert_eq!(
        lenses_of(&whole),
        applied,
        "case {case:?}: a create sending {sent:?} must report the same lenses as the \
         association path, got {:?}",
        lenses_of(&whole)
    );
}

/// Rule 4: the per-resource lens quota counts DISTINCT lenses. The rows walk the published
/// bound — one lens, the bound itself, and one lens past it — and separate the number of
/// ENTRIES a list carries from the number of lenses it names.
#[tokio::test]
async fn the_per_resource_lens_quota_counts_distinct_lenses() {
    let client = Client::new();
    let quota = state::MAX_LENSES_PER_REVIEW_TEMPLATE;
    assert_eq!(
        quota, state::MAX_LENSES_PER_WORKLOAD,
        "this suite sends one list to both creates, so the two quotas must agree, got \
         {quota} and {}",
        state::MAX_LENSES_PER_WORKLOAD
    );
    // (case, slug, the list sent, whether it is within the quota)
    let rows: Vec<(&str, &str, Vec<String>, bool)> = vec![
        (
            "one lens",
            "quota-one",
            vec![FRAMEWORK_LENS.to_string()],
            true,
        ),
        (
            "as many entries as the quota allows lenses, naming two lenses",
            "quota-entries-two-lenses",
            std::iter::repeat_n(FRAMEWORK_LENS.to_string(), quota - 1)
                .chain(std::iter::once("serverless".to_string()))
                .collect(),
            true,
        ),
        (
            "twice as many entries as the quota allows lenses, naming one lens",
            "quota-double-entries-one-lens",
            vec![FRAMEWORK_LENS.to_string(); quota * 2],
            true,
        ),
        (
            "as many distinct lenses as the quota allows",
            "quota-at-bound",
            OFFICIAL_LENSES[..quota]
                .iter()
                .map(ToString::to_string)
                .collect(),
            true,
        ),
        (
            "one distinct lens past the quota",
            "quota-past-bound",
            OFFICIAL_LENSES[..quota + 1]
                .iter()
                .map(ToString::to_string)
                .collect(),
            false,
        ),
        (
            "one distinct lens past the quota, each named twice",
            "quota-past-bound-repeated",
            OFFICIAL_LENSES[..quota + 1]
                .iter()
                .flat_map(|alias| {
                    [
                        (*alias).to_string(),
                        official_lens_arn("us-east-1", alias),
                    ]
                })
                .collect(),
            false,
        ),
    ];

    for (case, slug, sent, within) in rows {
        let mut template_request =
            minimal_template(&format!("tpl-{slug}"), &format!("tok-tpl-{slug}"));
        template_request["Lenses"] = json!(sent);
        let answer = client.create_review_template(template_request).await;
        let mut workload_request = minimal_create(&format!("wl-{slug}"), &format!("tok-wl-{slug}"));
        workload_request["Lenses"] = json!(sent);
        let workload_answer = client.create(workload_request).await;

        if within {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {} entries naming lenses within the quota of {quota} must be \
                 created, got {answer:?}",
                sent.len()
            );
            assert_eq!(
                workload_answer.status, 200,
                "case {case:?}: {} entries naming lenses within the quota of {quota} must be \
                 created, got {workload_answer:?}",
                sent.len()
            );
            continue;
        }

        assert_eq!(
            answer.status, 402,
            "case {case:?}: more distinct lenses than the quota of {quota} must be refused, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ServiceQuotaExceededException",
            "case {case:?}: unexpected error code, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            format!("The quota of {quota} lenses per review template has been reached"),
            "case {case:?}: unexpected Message, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "QuotaCode"),
            "LensesPerReviewTemplate",
            "case {case:?}: unexpected QuotaCode, got {answer:?}"
        );
        assert_eq!(
            workload_answer.status, 402,
            "case {case:?}: more distinct lenses than the quota of {quota} must be refused, \
             got {workload_answer:?}"
        );
        assert_eq!(
            workload_answer.error_code, "ServiceQuotaExceededException",
            "case {case:?}: unexpected error code, got {workload_answer:?}"
        );
        assert_eq!(
            member(&workload_answer, "Message"),
            format!("The quota of {quota} lenses per workload has been reached"),
            "case {case:?}: unexpected Message, got {workload_answer:?}"
        );
        assert_eq!(
            member(&workload_answer, "QuotaCode"),
            "LensesPerWorkload",
            "case {case:?}: unexpected QuotaCode, got {workload_answer:?}"
        );

        // A refusal stores nothing: neither resource exists to be read back.
        let listing = client.list_review_templates("").await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: expected ListReviewTemplates 200, got {listing:?}"
        );
        let named = listing
            .body
            .get("ReviewTemplates")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .filter_map(|summary| summary.get("TemplateName").and_then(Value::as_str))
                    .any(|name| name == format!("tpl-{slug}"))
            })
            .unwrap_or(false);
        assert!(
            !named,
            "case {case:?}: a refused create must store no template, got {listing:?}"
        );
    }
}
