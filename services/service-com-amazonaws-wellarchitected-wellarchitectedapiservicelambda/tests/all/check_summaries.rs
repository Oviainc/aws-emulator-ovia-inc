// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListCheckSummaries`: the Trusted Advisor checks of ONE best
//! practice of ONE lens review of ONE workload, summarized over the accounts the workload
//! relates to.
//!
//! The rule these pin, over tables of inputs rather than one recorded request: a check is
//! associated to ONE best practice of ONE lens, so the summaries reported for a request are
//! the checks this emulator publishes for the choice the request's
//! `(LensArn, PillarId, QuestionId, ChoiceId)` names — ONE entry per check however many
//! accounts the workload relates to, carrying how many of those accounts carry each
//! `CheckStatus` and the one status the check carries for the workload — and only while that
//! workload's Trusted Advisor integration is activated. A best practice with no associated
//! check, a best practice of another lens and a workload whose integration is not activated
//! therefore report nothing, a workload that does not exist and a lens the workload does not
//! review do not resolve, and a pillar, question or choice the lens's content does not
//! define is refused. The summaries are paged by `MaxResults`/`NextToken`, and the whole
//! read writes nothing.
//!
//! A check's RESULT is what Trusted Advisor read from an account's resources, which this
//! emulator neither runs nor holds: every account therefore carries one of the two
//! `CheckStatus` members that say there is no result for it, and a summary's counts are
//! those of the per-account view `ListCheckDetails` reports for the same best practice.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::workload_harness::{created_id, minimal_create, official_lens_arn, Answer, Client};

/// The alias of the lens whose best practices this emulator publishes Trusted Advisor
/// checks for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// An AWS-official lens this emulator ships a question set for and NO checks: a check is
/// associated to a best practice of a named lens, so the association does not carry over to
/// another lens's question set.
const OTHER_LENS: &str = "serverless";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The Region the shared harness signs its requests for, which is the Region the `LensArn`
/// in a reported summary is built in.
const REQUEST_REGION: &str = "us-west-2";

/// `TrustedAdvisorIntegrationStatus`'s two members
/// (`smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`), spelled out
/// here rather than read back out of the code under test.
const INTEGRATION_ENABLED: &str = "ENABLED";
/// See [`INTEGRATION_ENABLED`].
const INTEGRATION_DISABLED: &str = "DISABLED";

/// The `CheckStatus` a check carries for the account that OWNS the workload, and the one it
/// carries for any other account related to it. Both are members of the modelled enum
/// (`smithy_query(query="shape", shape="CheckStatus")`), spelled out here rather than read
/// back out of the code under test.
const OWNER_ACCOUNT_STATUS: &str = "NOT_AVAILABLE";
/// See [`OWNER_ACCOUNT_STATUS`].
const OTHER_ACCOUNT_STATUS: &str = "FETCH_FAILED";

/// Every `CheckStatus` member (`smithy_query(query="shape", shape="CheckStatus")`), spelled
/// out here rather than read back out of the code under test.
const CHECK_STATUS_MEMBERS: &[&str] = &["OKAY", "WARNING", "ERROR", "NOT_AVAILABLE", "FETCH_FAILED"];

/// `CheckProvider`'s one member — every check this emulator reports comes from Trusted
/// Advisor (`smithy_query(query="shape", shape="CheckProvider")`).
const PROVIDER: &str = "TRUSTED_ADVISOR";

/// The modelled `MaxResults` range for this operation, `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListCheckSummaries")`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 50);

/// The modelled `CheckStatusCount` range, `@range(min: 1, max: 101)`
/// (`smithy_query(query="shape", shape="CheckStatusCount")`) — one account per count, so its
/// ceiling is the 100 accounts a `WorkloadAccountIds` holds beside the account the workload
/// lives in.
const CHECK_STATUS_COUNT_RANGE: (i64, i64) = (1, 101);

/// `POST /workloads/{WorkloadId}/checkSummaries`: every member but the workload id travels
/// in the body, which is where restJson1 binds a member the model gives no HTTP binding.
async fn list_check_summaries(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/checkSummaries"),
            Some(body),
        )
        .await
}

/// `POST /workloads/{WorkloadId}/checks` — the per-ACCOUNT view of the same checks, which a
/// summary's counts are asserted against.
async fn list_check_details(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/checks"),
            Some(body),
        )
        .await
}

/// A request body naming one best practice of the framework lens.
fn best_practice(pillar_id: &str, question_id: &str, choice_id: &str) -> Value {
    json!({
        "LensArn": official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS),
        "PillarId": pillar_id,
        "QuestionId": question_id,
        "ChoiceId": choice_id,
    })
}

/// A workload reviewing both lenses under test, covering `account_ids` beside the account
/// that owns it, with its Trusted Advisor integration left at `integration` when one is
/// given and unset otherwise.
async fn workload(
    client: &Client,
    name: &str,
    account_ids: &[&str],
    integration: Option<&str>,
) -> String {
    let mut body = minimal_create(name, &format!("tok-{name}"));
    body["Lenses"] = json!([FRAMEWORK_LENS, OTHER_LENS]);
    body["AccountIds"] = json!(account_ids);
    if let Some(status) = integration {
        body["DiscoveryConfig"] = json!({ "TrustedAdvisorIntegrationStatus": status });
    }
    created_id(&client.create(body).await, name)
}

fn summaries_of(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("CheckSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn string_member(entry: &Value, name: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `AccountSummary` of one reported summary, as `status -> account count`.
fn account_summary(summary: &Value) -> BTreeMap<String, i64> {
    summary
        .get("AccountSummary")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(status, count)| (status.clone(), count.as_i64().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of the checks a page reported, in order.
fn reported(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| string_member(summary, "Id"))
        .collect()
}

/// The account that owns every workload this emulator serves.
fn owner_account() -> String {
    engine::emulator_account_id().to_string()
}

/// One row of the association table: a best practice, and the ids of the checks this
/// emulator publishes for it, in reporting order.
struct Associated {
    case: &'static str,
    pillar_id: &'static str,
    question_id: &'static str,
    choice_id: &'static str,
    check_ids: &'static [&'static str],
}

/// The summaries of a best practice are ONE per check the catalog associates to it, and
/// nothing else: rows differ along how many checks the best practice has (three, two, one,
/// none), which pillar it belongs to and which question offers it.
///
/// The expected ids are spelled out rather than read back out of the code under test, and
/// the rows cover best practices in four different pillars.
#[tokio::test]
async fn summaries_reported_are_the_checks_associated_to_the_best_practice() {
    let client = Client::new();
    // Two accounts beside the owner, so a per-check summary is visibly ONE entry where the
    // per-account view reports three.
    let workload_id = workload(
        &client,
        "assoc-host",
        &["111122223333", "444455556666"],
        Some(INTEGRATION_ENABLED),
    )
    .await;

    let rows: &[Associated] = &[
        Associated {
            case: "three-checks",
            pillar_id: "security",
            question_id: "permissions",
            choice_id: "sec_permissions_analyze_cross_account",
            check_ids: &[
                "amazon-s3-bucket-permissions",
                "amazon-rds-public-snapshots",
                "amazon-ebs-public-snapshots",
            ],
        },
        Associated {
            case: "two-checks",
            pillar_id: "reliability",
            question_id: "fault-isolation",
            choice_id: "rel_fault_isolation_multiaz_region_system",
            check_ids: &["amazon-rds-multi-az", "elb-cross-zone-load-balancing"],
        },
        Associated {
            case: "one-check",
            pillar_id: "costOptimization",
            question_id: "resource-type-size-number",
            choice_id: "cost_type_size_number_resources_data",
            check_ids: &["low-utilization-amazon-ec2-instances"],
        },
        Associated {
            case: "one-check-in-another-pillar",
            pillar_id: "sustainability",
            question_id: "data-patterns",
            choice_id: "sus_data_block_storage_provisioning",
            check_ids: &["underutilized-amazon-ebs-volumes"],
        },
        Associated {
            case: "no-check-for-this-choice",
            pillar_id: "security",
            question_id: "permissions",
            choice_id: "sec_permissions_define",
            check_ids: &[],
        },
        Associated {
            case: "no-check-for-this-question",
            pillar_id: "operationalExcellence",
            question_id: "organization-priorities",
            choice_id: "ops_priorities_ext_cust_needs",
            check_ids: &[],
        },
    ];

    for row in rows {
        let answer = list_check_summaries(
            &client,
            &workload_id,
            best_practice(row.pillar_id, row.question_id, row.choice_id),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {answer:?}",
            row.case
        );
        let summaries = summaries_of(&answer);
        assert_eq!(
            reported(&summaries),
            row.check_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {:?}: expected one summary per associated check, got {summaries:?}",
            row.case
        );
        assert!(
            answer.body.get("NextToken").is_none(),
            "case {:?}: one page holds every summary of a best practice, got {answer:?}",
            row.case
        );

        // Every reported summary names the best practice it was asked about, the lens it is
        // associated to and the provider it comes from.
        for summary in &summaries {
            assert_eq!(
                string_member(summary, "PillarId"),
                row.pillar_id,
                "case {:?}: expected the pillar asked about, got {summary:?}",
                row.case
            );
            assert_eq!(
                string_member(summary, "QuestionId"),
                row.question_id,
                "case {:?}: expected the question asked about, got {summary:?}",
                row.case
            );
            assert_eq!(
                string_member(summary, "ChoiceId"),
                row.choice_id,
                "case {:?}: expected the choice asked about, got {summary:?}",
                row.case
            );
            assert_eq!(
                string_member(summary, "LensArn"),
                official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS),
                "case {:?}: expected the reviewed lens's ARN, got {summary:?}",
                row.case
            );
            assert_eq!(
                string_member(summary, "Provider"),
                PROVIDER,
                "case {:?}: expected the Trusted Advisor provider, got {summary:?}",
                row.case
            );
            assert!(
                !string_member(summary, "Name").is_empty()
                    && !string_member(summary, "Description").is_empty(),
                "case {:?}: a reported summary carries the check's name and description, \
                 got {summary:?}",
                row.case
            );
            // No check ran, so there is no instant at which a summary was last updated.
            assert!(
                summary.get("UpdatedAt").is_none(),
                "case {:?}: a check with no result carries no update time, got {summary:?}",
                row.case
            );
        }
    }
}

/// A check is associated to a best practice OF ONE LENS: the same best practice read
/// through another lens's review reports nothing, even when that lens asks about the same
/// subject.
#[tokio::test]
async fn summaries_belong_to_the_lens_whose_best_practice_they_inspect() {
    let client = Client::new();
    let workload_id = workload(&client, "lens-scope", &[], Some(INTEGRATION_ENABLED)).await;

    // The framework lens's "Store and use secrets securely" best practice has a check.
    let framework = list_check_summaries(
        &client,
        &workload_id,
        best_practice("security", "identities", "sec_identities_secrets"),
    )
    .await;
    assert_eq!(
        reported(&summaries_of(&framework)),
        vec!["exposed-access-keys".to_string()],
        "case \"framework-lens\": expected the associated check, got {framework:?}"
    );

    // The serverless lens asks about secrets too, and this emulator associates no check to
    // ITS best practice.
    let other = list_check_summaries(
        &client,
        &workload_id,
        json!({
            "LensArn": official_lens_arn(REQUEST_REGION, OTHER_LENS),
            "PillarId": "security",
            "QuestionId": "serverless-sec-03",
            "ChoiceId": "serverless-sec-03-bp-03",
        }),
    )
    .await;
    assert_eq!(
        other.status, 200,
        "case \"other-lens\": expected 200, got {other:?}"
    );
    assert_eq!(
        summaries_of(&other),
        Vec::<Value>::new(),
        "case \"other-lens\": a best practice of another lens has no check, got {other:?}"
    );
}

/// One row of the account table: the accounts a workload covers beside the one that owns it,
/// and the per-status account counts a check's summary then carries.
struct Covered {
    case: &'static str,
    account_ids: Vec<String>,
    expected_counts: BTreeMap<String, i64>,
    expected_status: &'static str,
}

/// How many accounts each status is expected for, as a map.
fn counts(entries: &[(&str, i64)]) -> BTreeMap<String, i64> {
    entries
        .iter()
        .map(|(status, count)| ((*status).to_string(), *count))
        .collect()
}

/// A summary covers every account related to the workload in ONE entry: its `AccountSummary`
/// counts how many of those accounts carry each `CheckStatus`, and its `Status` is the one
/// the check carries for the workload. Rows differ along how many accounts the workload
/// covers and whether it names its own account or one of them twice.
///
/// Each row is also asserted against the per-ACCOUNT view of the same best practice, so the
/// two readings of one check cannot disagree about which accounts it was fetched for.
#[tokio::test]
async fn a_summary_counts_the_accounts_related_to_the_workload() {
    let client = Client::new();
    let owner = owner_account();
    let names = |ids: &[&str]| -> Vec<String> { ids.iter().map(ToString::to_string).collect() };

    let rows: Vec<Covered> = vec![
        Covered {
            case: "owner-only",
            account_ids: Vec::new(),
            expected_counts: counts(&[(OWNER_ACCOUNT_STATUS, 1)]),
            expected_status: OWNER_ACCOUNT_STATUS,
        },
        Covered {
            case: "one-other-account",
            account_ids: names(&["111122223333"]),
            expected_counts: counts(&[(OWNER_ACCOUNT_STATUS, 1), (OTHER_ACCOUNT_STATUS, 1)]),
            expected_status: OTHER_ACCOUNT_STATUS,
        },
        Covered {
            case: "three-other-accounts",
            account_ids: names(&["111122223333", "444455556666", "777788889999"]),
            expected_counts: counts(&[(OWNER_ACCOUNT_STATUS, 1), (OTHER_ACCOUNT_STATUS, 3)]),
            expected_status: OTHER_ACCOUNT_STATUS,
        },
        Covered {
            case: "names-the-owner-account-too",
            account_ids: vec![owner.clone(), "111122223333".to_string()],
            expected_counts: counts(&[(OWNER_ACCOUNT_STATUS, 1), (OTHER_ACCOUNT_STATUS, 1)]),
            expected_status: OTHER_ACCOUNT_STATUS,
        },
        Covered {
            case: "names-an-account-twice",
            account_ids: names(&["111122223333", "111122223333"]),
            expected_counts: counts(&[(OWNER_ACCOUNT_STATUS, 1), (OTHER_ACCOUNT_STATUS, 1)]),
            expected_status: OTHER_ACCOUNT_STATUS,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let account_ids: Vec<&str> = row.account_ids.iter().map(String::as_str).collect();
        let workload_id = workload(
            &client,
            &format!("accounts-{index}"),
            &account_ids,
            Some(INTEGRATION_ENABLED),
        )
        .await;
        // One check only, so the whole answer is that check's summary.
        let request = best_practice("security", "securely-operate", "sec_securely_operate_aws_account");

        let answer = list_check_summaries(&client, &workload_id, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {answer:?}",
            row.case
        );
        let summaries = summaries_of(&answer);
        assert_eq!(
            reported(&summaries),
            vec!["mfa-on-root-account".to_string()],
            "case {:?}: expected one summary for the one associated check, got {summaries:?}",
            row.case
        );
        let summary = &summaries[0];
        assert_eq!(
            account_summary(summary),
            row.expected_counts,
            "case {:?}: expected the per-status account counts, got {summary:?}",
            row.case
        );
        assert_eq!(
            string_member(summary, "Status"),
            row.expected_status,
            "case {:?}: expected the status the check carries for the workload, got {summary:?}",
            row.case
        );
        for (status, count) in account_summary(summary) {
            assert!(
                CHECK_STATUS_MEMBERS.contains(&status.as_str()),
                "case {:?}: a counted status is a modelled CheckStatus, got {status:?}",
                row.case
            );
            assert!(
                (CHECK_STATUS_COUNT_RANGE.0..=CHECK_STATUS_COUNT_RANGE.1).contains(&count),
                "case {:?}: a counted status carries a CheckStatusCount inside \
                 {CHECK_STATUS_COUNT_RANGE:?}, got {count} for {status:?}",
                row.case
            );
        }

        // The same best practice read per account: the summary's counts are exactly that
        // enumeration grouped by status.
        let details = list_check_details(&client, &workload_id, request).await;
        assert_eq!(
            details.status, 200,
            "case {:?}: expected ListCheckDetails 200, got {details:?}",
            row.case
        );
        let mut grouped: BTreeMap<String, i64> = BTreeMap::new();
        for detail in details
            .body
            .get("CheckDetails")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            *grouped
                .entry(string_member(&detail, "Status"))
                .or_insert(0) += 1;
        }
        assert_eq!(
            account_summary(summary),
            grouped,
            "case {:?}: the summary's counts must be the per-account view grouped by status, \
             got {summary:?} against {details:?}",
            row.case
        );
    }
}

/// Checks are fetched only while the workload's Trusted Advisor integration is activated, so
/// the reported set moves with the `DiscoveryConfig` a caller writes — and with nothing else.
#[tokio::test]
async fn summaries_are_reported_only_while_the_integration_is_activated() {
    let client = Client::new();
    let request = best_practice("costOptimization", "pricing-model", "cost_pricing_model_analysis");
    let expected: Vec<String> = [
        "amazon-ec2-reserved-instance-optimization",
        "savings-plans",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();

    // A workload whose integration nobody configured: the service defaults it to DISABLED,
    // so nothing is fetched.
    let workload_id = workload(&client, "integration", &[], None).await;
    let unset = list_check_summaries(&client, &workload_id, request.clone()).await;
    assert_eq!(
        unset.status, 200,
        "case \"unset\": expected 200, got {unset:?}"
    );
    assert_eq!(
        summaries_of(&unset),
        Vec::<Value>::new(),
        "case \"unset\": a workload with no activated integration reports no check, \
         got {unset:?}"
    );

    for (case, status, want) in [
        ("activated", INTEGRATION_ENABLED, expected.clone()),
        ("deactivated-again", INTEGRATION_DISABLED, Vec::new()),
    ] {
        let updated = client
            .update(
                &workload_id,
                json!({ "DiscoveryConfig": { "TrustedAdvisorIntegrationStatus": status } }),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateWorkload 200, got {updated:?}"
        );
        let answer = list_check_summaries(&client, &workload_id, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            reported(&summaries_of(&answer)),
            want,
            "case {case:?}: expected the checks the integration status implies, got {answer:?}"
        );
    }
}

/// Every reported summary, following `NextToken` to the end. Answers the check ids in the
/// order they were served and how many pages it took.
async fn collect(
    client: &Client,
    case: &str,
    workload_id: &str,
    request: &Value,
    max_results: Option<i64>,
) -> (Vec<String>, usize) {
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut next_token: Option<String> = None;

    loop {
        let mut body = request.clone();
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let answer = list_check_summaries(client, workload_id, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {pages} must be a 200, got {answer:?}"
        );
        let page = summaries_of(&answer);
        if let Some(max) = max_results {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max,
                "case {case:?}: a page holds at most MaxResults={max} summaries, got {}",
                page.len()
            );
        }
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert!(
            next_token.is_none() || !page.is_empty(),
            "case {case:?}: a page that carries a NextToken must not be empty, got {answer:?}"
        );
        collected.extend(reported(&page));
        pages += 1;
        assert!(
            pages <= 32,
            "case {case:?}: paging must terminate, got {pages} pages"
        );
        if next_token.is_none() {
            break;
        }
    }

    (collected, pages)
}

/// Paging serves every reported summary exactly once, in the same order, whatever page size
/// the request asks for: rows differ along `MaxResults`, including both published bounds.
/// The accounts the workload covers do not enter the count — a summary covers them all.
#[tokio::test]
async fn paging_serves_every_summary_once() {
    let client = Client::new();
    let workload_id = workload(
        &client,
        "paging",
        &["111122223333", "444455556666"],
        Some(INTEGRATION_ENABLED),
    )
    .await;
    // Three checks over three accounts: three summaries, so a page size of one or two cuts
    // the enumeration inside the best practice.
    let request = best_practice("costOptimization", "decomission-resources", "cost_decomissioning_resources_decommission");

    let (unpaged, pages) = collect(&client, "unpaged", &workload_id, &request, None).await;
    assert_eq!(
        unpaged.len(),
        3,
        "case \"unpaged\": expected one summary per associated check, got {unpaged:?}"
    );
    assert_eq!(
        pages, 1,
        "case \"unpaged\": a full page holds every summary, got {pages} pages"
    );

    for max_results in [MAX_RESULTS_RANGE.0, 2, 3, MAX_RESULTS_RANGE.1] {
        let case = format!("max-results-{max_results}");
        let (paged, pages) =
            collect(&client, &case, &workload_id, &request, Some(max_results)).await;
        assert_eq!(
            paged, unpaged,
            "case {case:?}: paging must serve the same summaries in the same order, \
             got {paged:?}"
        );
        let page_size = usize::try_from(max_results).unwrap_or(usize::MAX);
        let expected_pages = unpaged.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages of {max_results}, got {pages}"
        );
    }
}

/// One row of the refusal table: a request, and the status and wire error code it is refused
/// with.
struct Refused {
    case: &'static str,
    workload: Option<&'static str>,
    body: Value,
    status: u16,
    error_code: &'static str,
}

/// What the read refuses: a workload that does not exist and a lens the workload does not
/// review do not resolve (404), and a pillar, question or choice the lens's content does not
/// define — or a `MaxResults` outside the modelled range, or a token this operation did not
/// issue — is a bad request (400). Rows differ along which part of the request is wrong.
#[tokio::test]
async fn the_read_refuses_what_the_lens_does_not_define() {
    let client = Client::new();
    let workload_id = workload(&client, "refusals", &[], Some(INTEGRATION_ENABLED)).await;
    let unreviewed_lens = official_lens_arn(REQUEST_REGION, "iot");

    let mut rows: Vec<Refused> = vec![
        Refused {
            case: "absent-workload",
            workload: Some(ABSENT_HEX_ID),
            body: best_practice("security", "securely-operate", "sec_securely_operate_aws_account"),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        Refused {
            case: "lens-not-reviewed",
            workload: None,
            body: json!({
                "LensArn": unreviewed_lens,
                "PillarId": "security",
                "QuestionId": "securely-operate",
                "ChoiceId": "sec_securely_operate_aws_account",
            }),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        Refused {
            case: "pillar-the-lens-does-not-define",
            workload: None,
            body: best_practice("notapillar", "securely-operate", "sec_securely_operate_aws_account"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "question-of-another-pillar",
            workload: None,
            body: best_practice("reliability", "securely-operate", "sec_securely_operate_aws_account"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "question-the-lens-does-not-ask",
            workload: None,
            body: best_practice("security", "sec-99", "sec-99-bp-01"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "choice-of-another-question",
            workload: None,
            body: best_practice("security", "identities", "sec_securely_operate_aws_account"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "choice-the-question-does-not-offer",
            workload: None,
            body: best_practice("security", "securely-operate", "sec-01-bp-99"),
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            case: "token-this-service-did-not-issue",
            workload: None,
            body: {
                let mut body = best_practice("security", "securely-operate", "sec_securely_operate_aws_account");
                body["NextToken"] = json!("notatoken");
                body
            },
            status: 400,
            error_code: "ValidationException",
        },
        Refused {
            // A cursor of the per-ACCOUNT enumeration is not a cursor of this one: a
            // summary covers every account at once, so a position named by account is
            // undecodable here.
            case: "token-of-the-per-account-enumeration",
            workload: None,
            body: {
                let mut body = best_practice("security", "securely-operate", "sec_securely_operate_aws_account");
                body["NextToken"] = json!(state::encode_check_next_token(
                    "111122223333",
                    "mfa-on-root-account"
                ));
                body
            },
            status: 400,
            error_code: "ValidationException",
        },
    ];

    // The modelled `MaxResults` range, at each published bound and one past it.
    for (case, max_results, status) in [
        ("max-results-below-the-range", MAX_RESULTS_RANGE.0 - 1, 400),
        ("max-results-at-the-minimum", MAX_RESULTS_RANGE.0, 200),
        ("max-results-at-the-maximum", MAX_RESULTS_RANGE.1, 200),
        ("max-results-above-the-range", MAX_RESULTS_RANGE.1 + 1, 400),
    ] {
        let mut body = best_practice("security", "securely-operate", "sec_securely_operate_aws_account");
        body["MaxResults"] = json!(max_results);
        if status == 200 {
            let answer = list_check_summaries(&client, &workload_id, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: MaxResults={max_results} is inside the modelled range, \
                 got {answer:?}"
            );
            continue;
        }
        rows.push(Refused {
            case,
            workload: None,
            body,
            status: 400,
            error_code: "ValidationException",
        });
    }

    for row in &rows {
        let answer = list_check_summaries(
            &client,
            row.workload.unwrap_or(&workload_id),
            row.body.clone(),
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected HTTP {}, got {answer:?}",
            row.case, row.status
        );
        assert_eq!(
            answer.error_code, row.error_code,
            "case {:?}: expected {}, got {answer:?}",
            row.case, row.error_code
        );
        assert!(
            !answer
                .body
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .is_empty(),
            "case {:?}: a refusal states why, got {answer:?}",
            row.case
        );
    }
}

/// A token one request issued resumes the next request's page, and a token of this
/// operation's own scheme that names a check the enumeration does not hold is refused rather
/// than served as an empty page.
#[tokio::test]
async fn a_resumed_page_starts_just_past_the_token() {
    let client = Client::new();
    let workload_id = workload(
        &client,
        "resume",
        &["111122223333"],
        Some(INTEGRATION_ENABLED),
    )
    .await;
    let mut request = best_practice("security", "permissions", "sec_permissions_analyze_cross_account");
    request["MaxResults"] = json!(2);

    let first = list_check_summaries(&client, &workload_id, request.clone()).await;
    assert_eq!(
        first.status, 200,
        "case \"first-page\": expected 200, got {first:?}"
    );
    let token = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case \"first-page\": three summaries at two per page leave a NextToken, got {first:?}"
    );

    let mut resumed_request = request.clone();
    resumed_request["NextToken"] = json!(token);
    let resumed = list_check_summaries(&client, &workload_id, resumed_request).await;
    assert_eq!(
        resumed.status, 200,
        "case \"second-page\": expected 200, got {resumed:?}"
    );
    let served: BTreeSet<String> = reported(&summaries_of(&first))
        .into_iter()
        .chain(reported(&summaries_of(&resumed)))
        .collect();
    assert_eq!(
        served.len(),
        3,
        "case \"second-page\": the resumed page must not repeat the first, got {served:?}"
    );

    // A check of ANOTHER best practice is a position this enumeration does not hold.
    let mut stale_request = request.clone();
    stale_request["NextToken"] = json!(state::encode_check_summary_next_token(
        "mfa-on-root-account"
    ));
    let stale = list_check_summaries(&client, &workload_id, stale_request).await;
    assert_eq!(
        stale.status, 400,
        "case \"token-past-the-enumeration\": expected 400, got {stale:?}"
    );
    assert_eq!(
        stale.error_code, "ValidationException",
        "case \"token-past-the-enumeration\": expected a ValidationException, got {stale:?}"
    );
}

/// The read reports; it changes nothing a later caller sees. Two identical reads answer the
/// same thing, and the workload's own `UpdatedAt` does not move across them.
#[tokio::test]
async fn listing_summaries_changes_nothing_a_later_read_sees() {
    let client = Client::new();
    let workload_id = workload(
        &client,
        "reporting",
        &["111122223333"],
        Some(INTEGRATION_ENABLED),
    )
    .await;
    let request = best_practice("reliability", "back-up-data", "rel_backing_up_data_automated_backups_data");

    let before = client.get(&workload_id).await;
    let first = list_check_summaries(&client, &workload_id, request.clone()).await;
    let second = list_check_summaries(&client, &workload_id, request).await;
    let after = client.get(&workload_id).await;

    assert_eq!(
        summaries_of(&first),
        summaries_of(&second),
        "case \"repeated-read\": two reads of the same best practice must agree, \
         got {first:?} then {second:?}"
    );
    assert_eq!(
        before
            .body
            .get("Workload")
            .and_then(|workload| workload.get("UpdatedAt")),
        after
            .body
            .get("Workload")
            .and_then(|workload| workload.get("UpdatedAt")),
        "case \"repeated-read\": reporting summaries must not update the workload, got \
         {before:?} then {after:?}"
    );
}

/// The precedence a check's ONE status over several accounts is taken at ranks every
/// `CheckStatus` the model publishes, each once, and the summary of a set of accounts
/// reports the status furthest along it that any of them carries.
#[tokio::test]
async fn the_summarized_status_is_the_furthest_along_the_status_order() {
    let ranked: Vec<&str> = state::CHECK_STATUS_BY_SEVERITY.to_vec();
    assert_eq!(
        ranked,
        CHECK_STATUS_MEMBERS.to_vec(),
        "the status precedence must rank every modelled CheckStatus once, got {ranked:?}"
    );

    // Rows differ along the accounts summarized: only the owner, accounts whose statuses
    // differ, and the same account named twice. In each case the reported status is the one
    // furthest along the precedence above.
    let owner = owner_account();
    let rows: Vec<(&str, Vec<String>, Option<&str>)> = vec![
        ("no-account", Vec::new(), None),
        (
            "owner-only",
            vec![owner.clone()],
            Some(OWNER_ACCOUNT_STATUS),
        ),
        (
            "another-account-only",
            vec!["111122223333".to_string()],
            Some(OTHER_ACCOUNT_STATUS),
        ),
        (
            "both",
            vec![owner.clone(), "111122223333".to_string()],
            Some(OTHER_ACCOUNT_STATUS),
        ),
        (
            "both-in-the-other-order",
            vec!["111122223333".to_string(), owner.clone()],
            Some(OTHER_ACCOUNT_STATUS),
        ),
    ];

    for (case, accounts, expected) in rows {
        let summary = state::check_account_summary(&accounts, &owner);
        let status = summary.as_ref().map(|summary| summary.status);
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status furthest along the precedence, got {summary:?}"
        );
        let Some(summary) = summary else { continue };
        let counted: i64 = summary
            .accounts_by_status
            .values()
            .map(|count| i64::from(*count))
            .sum();
        assert_eq!(
            counted,
            i64::try_from(accounts.len()).unwrap_or_default(),
            "case {case:?}: the counts must cover every summarized account once, got {summary:?}"
        );
        let rank = |status: &str| {
            state::CHECK_STATUS_BY_SEVERITY
                .iter()
                .position(|ranked| *ranked == status)
        };
        for status in summary.accounts_by_status.keys() {
            assert!(
                rank(status) <= rank(summary.status),
                "case {case:?}: no counted status may outrank the reported one, got {summary:?}"
            );
        }
    }
}
