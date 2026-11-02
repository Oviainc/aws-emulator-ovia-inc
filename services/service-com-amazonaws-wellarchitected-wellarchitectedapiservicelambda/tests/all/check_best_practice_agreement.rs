// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that the two reads of a best practice's Trusted Advisor checks —
//! the per-ACCOUNT view (`ListCheckDetails`) and the SUMMARIZED one (`ListCheckSummaries`) —
//! resolve and refuse the same request the same way.
//!
//! The rule these pin, over a table of requests rather than one: both operations report the
//! checks of ONE best practice of ONE lens review of ONE workload and differ only in whether
//! the report is expanded per account, so "which best practice does this request name, and
//! when is it refused" is ONE rule — the workload and its review of the lens, then the
//! pillar the lens defines, the question that pillar asks and the choice that question
//! offers, then the workload's Trusted Advisor activation — and the two must answer every
//! request with the same resolution or the same refusal.
//!
//! Each suite's own behaviour stays pinned by `super::check_details` and
//! `super::check_summaries`; what these drive is the shared walk, through an operation on
//! each side of it.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handlers all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, official_lens_arn, Answer, Client};

/// The alias of the lens whose best practices this emulator publishes Trusted Advisor
/// checks for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// An AWS-official lens this emulator ships a question set for and NO checks: a check is
/// associated to a best practice of a named lens, so a request naming this one resolves and
/// reports nothing.
const OTHER_LENS: &str = "serverless";

/// An AWS-official lens the workloads under test do NOT review.
const UNREVIEWED_LENS: &str = "iot";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The Region the shared harness signs its requests for, which is the Region the `LensArn`
/// of a reported check is built in.
const REQUEST_REGION: &str = "us-west-2";

/// `TrustedAdvisorIntegrationStatus`'s two members
/// (`smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`), spelled out
/// here rather than read back out of the code under test.
const INTEGRATION_ENABLED: &str = "ENABLED";
/// See [`INTEGRATION_ENABLED`].
const INTEGRATION_DISABLED: &str = "DISABLED";

/// `POST /workloads/{WorkloadId}/checks` — the per-ACCOUNT view.
async fn list_check_details(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/checks"),
            Some(body),
        )
        .await
}

/// `POST /workloads/{WorkloadId}/checkSummaries` — the SUMMARIZED view.
async fn list_check_summaries(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/checkSummaries"),
            Some(body),
        )
        .await
}

/// A request body naming one best practice of `lens`.
fn best_practice(lens: &str, pillar_id: &str, question_id: &str, choice_id: &str) -> Value {
    json!({
        "LensArn": official_lens_arn(REQUEST_REGION, lens),
        "PillarId": pillar_id,
        "QuestionId": question_id,
        "ChoiceId": choice_id,
    })
}

/// A workload reviewing both lenses under test, covering `account_ids` beside the account
/// that owns it, with its Trusted Advisor integration at `integration`.
async fn workload(
    client: &Client,
    name: &str,
    account_ids: &[&str],
    integration: &str,
) -> String {
    let mut body = minimal_create(name, &format!("tok-{name}"));
    body["Lenses"] = json!([FRAMEWORK_LENS, OTHER_LENS]);
    body["AccountIds"] = json!(account_ids);
    body["DiscoveryConfig"] = json!({ "TrustedAdvisorIntegrationStatus": integration });
    created_id(&client.create(body).await, name)
}

/// One member of a reported entry as a string, empty when the entry carries none.
fn string_member(entry: &Value, name: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The entries of a reported page, under the member the operation reports them in.
fn entries(answer: &Answer, member: &str) -> Vec<Value> {
    answer
        .body
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The check ids a page reported, in the order they first appear. The per-account view
/// repeats each check once per account, so first-appearance order is the catalog order both
/// views enumerate in.
fn check_ids(entries: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for entry in entries {
        let id = string_member(entry, "Id");
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The `(LensArn, PillarId, QuestionId, ChoiceId)` each reported entry names the best
/// practice by, deduplicated: every entry of one page names the same one.
fn best_practices_named(entries: &[Value]) -> Vec<[String; 4]> {
    let mut named: Vec<[String; 4]> = Vec::new();
    for entry in entries {
        let key = [
            string_member(entry, "LensArn"),
            string_member(entry, "PillarId"),
            string_member(entry, "QuestionId"),
            string_member(entry, "ChoiceId"),
        ];
        if !named.contains(&key) {
            named.push(key);
        }
    }
    named
}

/// The message a refusal states.
fn message(answer: &Answer) -> String {
    string_member(&answer.body, "Message")
}

/// One row of the resolution table: a request both views are asked, and the check ids the
/// best practice it names resolves to.
struct Resolved {
    case: &'static str,
    lens: &'static str,
    pillar_id: &'static str,
    question_id: &'static str,
    choice_id: &'static str,
    check_ids: &'static [&'static str],
}

/// The best practice a request names resolves to the same checks in both views: rows differ
/// along how many checks the best practice has, which pillar and question it belongs to, and
/// whether the lens it names publishes checks at all.
///
/// The expected ids are spelled out rather than read back out of the code under test.
#[tokio::test]
async fn both_views_resolve_a_request_to_the_same_best_practice() {
    let client = Client::new();
    // Two accounts beside the owner, so the per-account view reports each check three times
    // where the summarized one reports it once — the difference the two operations are FOR.
    let workload_id = workload(
        &client,
        "agree-resolve",
        &["111122223333", "444455556666"],
        INTEGRATION_ENABLED,
    )
    .await;

    let rows: &[Resolved] = &[
        Resolved {
            case: "three-checks",
            lens: FRAMEWORK_LENS,
            pillar_id: "security",
            question_id: "sec-03",
            choice_id: "sec-03-bp-07",
            check_ids: &[
                "amazon-s3-bucket-permissions",
                "amazon-rds-public-snapshots",
                "amazon-ebs-public-snapshots",
            ],
        },
        Resolved {
            case: "two-checks-in-another-pillar",
            lens: FRAMEWORK_LENS,
            pillar_id: "reliability",
            question_id: "rel-10",
            choice_id: "rel-10-bp-01",
            check_ids: &["amazon-rds-multi-az", "elb-cross-zone-load-balancing"],
        },
        Resolved {
            case: "one-check",
            lens: FRAMEWORK_LENS,
            pillar_id: "sustainability",
            question_id: "sus-04",
            choice_id: "sus-04-bp-04",
            check_ids: &["underutilized-amazon-ebs-volumes"],
        },
        Resolved {
            case: "best-practice-with-no-check",
            lens: FRAMEWORK_LENS,
            pillar_id: "operationalExcellence",
            question_id: "ops-01",
            choice_id: "ops-01-bp-01",
            check_ids: &[],
        },
        Resolved {
            case: "another-lens-publishes-no-check",
            lens: OTHER_LENS,
            pillar_id: "security",
            question_id: "serverless-sec-01",
            choice_id: "serverless-sec-01-bp-01",
            check_ids: &[],
        },
    ];

    for row in rows {
        let request = best_practice(row.lens, row.pillar_id, row.question_id, row.choice_id);
        let details = list_check_details(&client, &workload_id, request.clone()).await;
        let summaries = list_check_summaries(&client, &workload_id, request).await;
        assert_eq!(
            (details.status, summaries.status),
            (200, 200),
            "case {:?}: both views resolve the request, got {details:?} and {summaries:?}",
            row.case
        );

        let detail_entries = entries(&details, "CheckDetails");
        let summary_entries = entries(&summaries, "CheckSummaries");
        let want: Vec<String> = row.check_ids.iter().map(ToString::to_string).collect();
        assert_eq!(
            check_ids(&detail_entries),
            want,
            "case {:?}: the per-account view reports the associated checks, got {detail_entries:?}",
            row.case
        );
        assert_eq!(
            check_ids(&summary_entries),
            want,
            "case {:?}: the summarized view reports the same checks, got {summary_entries:?}",
            row.case
        );

        // Both views name the best practice the request named, by the same lens ARN.
        let named = best_practices_named(&detail_entries);
        assert_eq!(
            named,
            best_practices_named(&summary_entries),
            "case {:?}: both views name the same best practice, got {detail_entries:?} \
             and {summary_entries:?}",
            row.case
        );
        for key in &named {
            assert_eq!(
                key,
                &[
                    official_lens_arn(REQUEST_REGION, row.lens),
                    row.pillar_id.to_string(),
                    row.question_id.to_string(),
                    row.choice_id.to_string(),
                ],
                "case {:?}: expected the best practice asked about, got {key:?}",
                row.case
            );
        }
    }
}

/// The activation gate is the same gate for both views: a workload whose Trusted Advisor
/// integration is not activated has no check fetched, so neither view reports one.
#[tokio::test]
async fn both_views_share_the_activation_gate() {
    let client = Client::new();
    let request = best_practice(FRAMEWORK_LENS, "security", "sec-03", "sec-03-bp-07");

    for (case, integration, want) in [
        (
            "activated",
            INTEGRATION_ENABLED,
            vec![
                "amazon-s3-bucket-permissions".to_string(),
                "amazon-rds-public-snapshots".to_string(),
                "amazon-ebs-public-snapshots".to_string(),
            ],
        ),
        ("not-activated", INTEGRATION_DISABLED, Vec::new()),
    ] {
        let workload_id =
            workload(&client, &format!("agree-gate-{case}"), &[], integration).await;
        let details = list_check_details(&client, &workload_id, request.clone()).await;
        let summaries = list_check_summaries(&client, &workload_id, request.clone()).await;
        assert_eq!(
            (details.status, summaries.status),
            (200, 200),
            "case {case:?}: both views resolve the request, got {details:?} and {summaries:?}"
        );
        assert_eq!(
            check_ids(&entries(&details, "CheckDetails")),
            want,
            "case {case:?}: the per-account view reports what the activation implies, \
             got {details:?}"
        );
        assert_eq!(
            check_ids(&entries(&summaries, "CheckSummaries")),
            want,
            "case {case:?}: the summarized view reports what the activation implies, \
             got {summaries:?}"
        );
    }
}

/// One row of the refusal table: a request both views are asked, and the status and wire
/// error code it is refused with.
struct Refused {
    case: &'static str,
    workload: Option<&'static str>,
    body: Value,
    status: u16,
    error_code: &'static str,
}

/// Both views refuse the same request identically — same status, same wire error code, same
/// message — because the walk that refuses it is one walk: rows differ along which part of
/// the request names nothing, from the outermost resource inwards.
#[tokio::test]
async fn both_views_refuse_a_request_identically() {
    let client = Client::new();
    let workload_id = workload(&client, "agree-refuse", &[], INTEGRATION_ENABLED).await;

    let rows: &[Refused] = &[
        Refused {
            case: "absent-workload",
            workload: Some(ABSENT_HEX_ID),
            body: best_practice(FRAMEWORK_LENS, "security", "sec-01", "sec-01-bp-02"),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        Refused {
            case: "lens-not-reviewed",
            workload: None,
            body: best_practice(UNREVIEWED_LENS, "security", "sec-01", "sec-01-bp-02"),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        Refused {
            case: "pillar-the-lens-does-not-define",
            workload: None,
            body: best_practice(FRAMEWORK_LENS, "notapillar", "sec-01", "sec-01-bp-02"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "question-of-another-pillar",
            workload: None,
            body: best_practice(FRAMEWORK_LENS, "reliability", "sec-01", "sec-01-bp-02"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "question-the-lens-does-not-ask",
            workload: None,
            body: best_practice(FRAMEWORK_LENS, "security", "sec-99", "sec-99-bp-01"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "choice-of-another-question",
            workload: None,
            body: best_practice(FRAMEWORK_LENS, "security", "sec-02", "sec-01-bp-02"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "choice-the-question-does-not-offer",
            workload: None,
            body: best_practice(FRAMEWORK_LENS, "security", "sec-01", "sec-01-bp-99"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            // The pillar is judged before the question and the question before the choice,
            // so a request wrong at every level is refused at the outermost one — by both
            // views, in the same order.
            case: "wrong-at-every-level",
            workload: None,
            body: best_practice(FRAMEWORK_LENS, "notapillar", "sec-99", "sec-99-bp-99"),
            status: 400,
            error_code: "ValidationException",
        },
    ];

    for row in rows {
        let target = row.workload.unwrap_or(&workload_id);
        let details = list_check_details(&client, target, row.body.clone()).await;
        let summaries = list_check_summaries(&client, target, row.body.clone()).await;

        for (view, answer) in [("CheckDetails", &details), ("CheckSummaries", &summaries)] {
            assert_eq!(
                answer.status, row.status,
                "case {:?} ({view}): expected HTTP {}, got {answer:?}",
                row.case, row.status
            );
            assert_eq!(
                answer.error_code, row.error_code,
                "case {:?} ({view}): expected {}, got {answer:?}",
                row.case, row.error_code
            );
            assert!(
                !message(answer).is_empty(),
                "case {:?} ({view}): a refusal states why, got {answer:?}",
                row.case
            );
        }
        assert_eq!(
            message(&details),
            message(&summaries),
            "case {:?}: both views word the refusal identically, got {details:?} \
             and {summaries:?}",
            row.case
        );
    }
}
