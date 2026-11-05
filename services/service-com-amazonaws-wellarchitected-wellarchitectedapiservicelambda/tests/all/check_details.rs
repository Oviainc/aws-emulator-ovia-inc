// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListCheckDetails`: the Trusted Advisor checks of ONE best
//! practice of ONE lens review of ONE workload.
//!
//! The rule these pin, over tables of inputs rather than one recorded request: a check is
//! associated to ONE best practice of ONE lens, so the details reported for a request are
//! the checks this emulator publishes for the choice the request's
//! `(LensArn, PillarId, QuestionId, ChoiceId)` names, reported once per account related to
//! the workload — the account that owns it, then each account its `AccountIds` names — and
//! only while that workload's Trusted Advisor integration is activated. A best practice
//! with no associated check, a best practice of another lens and a workload whose
//! integration is not activated therefore report nothing, a workload that does not exist
//! and a lens the workload does not review do not resolve, and a pillar, question or choice
//! the lens's content does not define is refused. The reported checks are paged by
//! `MaxResults`/`NextToken`, and the whole read writes nothing.
//!
//! A check's RESULT is what Trusted Advisor read from an account's resources, which this
//! emulator neither runs nor holds: every reported check therefore carries one of the two
//! `CheckStatus` members that say there is no result for the account, with the
//! `CheckFailureReason` for it, and no flagged-resource count.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::BTreeSet;

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
/// in a reported check is built in.
const REQUEST_REGION: &str = "us-west-2";

/// `TrustedAdvisorIntegrationStatus`'s two members
/// (`smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`), spelled out
/// here rather than read back out of the code under test.
const INTEGRATION_ENABLED: &str = "ENABLED";
/// See [`INTEGRATION_ENABLED`].
const INTEGRATION_DISABLED: &str = "DISABLED";

/// The `CheckStatus` / `CheckFailureReason` pair a check carries for the account that OWNS
/// the workload, and the pair it carries for any other account related to it. Both are
/// members of the modelled enums (`smithy_query(query="shape", shape="CheckStatus")`,
/// `shape="CheckFailureReason"`), spelled out here rather than read back out of the code
/// under test.
const OWNER_ACCOUNT_RESULT: (&str, &str) = ("NOT_AVAILABLE", "PREMIUM_SUPPORT_REQUIRED");
/// See [`OWNER_ACCOUNT_RESULT`].
const OTHER_ACCOUNT_RESULT: (&str, &str) = ("FETCH_FAILED", "ASSUME_ROLE_ERROR");

/// `CheckProvider`'s one member — every check this emulator reports comes from Trusted
/// Advisor (`smithy_query(query="shape", shape="CheckProvider")`).
const PROVIDER: &str = "TRUSTED_ADVISOR";

/// The modelled `MaxResults` range for this operation, `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListCheckDetails")`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 50);

/// `POST /workloads/{WorkloadId}/checks`: every member but the workload id travels in the
/// body, which is where restJson1 binds a member the model gives no HTTP binding.
async fn list_check_details(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/checks"),
            Some(body),
        )
        .await
}

/// A `ListCheckDetails` body naming one best practice of the framework lens.
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

fn details_of(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("CheckDetails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn string_member(detail: &Value, name: &str) -> String {
    detail
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Each reported check as `(AccountId, Id)` — the pair that identifies one reported check,
/// since a check is reported once per account.
fn reported(details: &[Value]) -> Vec<(String, String)> {
    details
        .iter()
        .map(|detail| (string_member(detail, "AccountId"), string_member(detail, "Id")))
        .collect()
}

/// The account that owns every workload this emulator serves, which is the first account a
/// check is reported for.
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

/// The checks of a best practice are the ones the catalog associates to it, and nothing
/// else: rows differ along how many checks the best practice has (three, two, one, none),
/// which pillar it belongs to and which question offers it.
///
/// The expected ids are spelled out rather than read back out of the code under test, and
/// the rows cover best practices in four different pillars — none of them the one the stub
/// issue cited, which named no best practice at all.
#[tokio::test]
async fn checks_reported_are_the_ones_associated_to_the_best_practice() {
    let client = Client::new();
    let workload_id = workload(&client, "assoc-host", &[], Some(INTEGRATION_ENABLED)).await;

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
        let answer = list_check_details(
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
        let details = details_of(&answer);
        let ids: Vec<String> = details
            .iter()
            .map(|detail| string_member(detail, "Id"))
            .collect();
        assert_eq!(
            ids,
            row.check_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {:?}: expected the associated checks, got {ids:?}",
            row.case
        );
        assert!(
            answer.body.get("NextToken").is_none(),
            "case {:?}: one page holds every check of a best practice, got {answer:?}",
            row.case
        );

        // Every reported check names the best practice it was asked about, the lens it is
        // associated to and the provider it comes from.
        for detail in &details {
            assert_eq!(
                string_member(detail, "PillarId"),
                row.pillar_id,
                "case {:?}: expected the pillar asked about, got {detail:?}",
                row.case
            );
            assert_eq!(
                string_member(detail, "QuestionId"),
                row.question_id,
                "case {:?}: expected the question asked about, got {detail:?}",
                row.case
            );
            assert_eq!(
                string_member(detail, "ChoiceId"),
                row.choice_id,
                "case {:?}: expected the choice asked about, got {detail:?}",
                row.case
            );
            assert_eq!(
                string_member(detail, "LensArn"),
                official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS),
                "case {:?}: expected the reviewed lens's ARN, got {detail:?}",
                row.case
            );
            assert_eq!(
                string_member(detail, "Provider"),
                PROVIDER,
                "case {:?}: expected the Trusted Advisor provider, got {detail:?}",
                row.case
            );
            assert!(
                !string_member(detail, "Name").is_empty()
                    && !string_member(detail, "Description").is_empty(),
                "case {:?}: a reported check carries its name and description, got {detail:?}",
                row.case
            );
        }
    }
}

/// A check is associated to a best practice OF ONE LENS: the same best practice read
/// through another lens's review reports nothing, even when that lens asks about the same
/// subject.
#[tokio::test]
async fn checks_belong_to_the_lens_whose_best_practice_they_inspect() {
    let client = Client::new();
    let workload_id = workload(&client, "lens-scope", &[], Some(INTEGRATION_ENABLED)).await;

    // The framework lens's "Store and use secrets securely" best practice has a check.
    let framework = list_check_details(
        &client,
        &workload_id,
        best_practice("security", "identities", "sec_identities_secrets"),
    )
    .await;
    assert_eq!(
        reported(&details_of(&framework)),
        vec![(owner_account(), "exposed-access-keys".to_string())],
        "case \"framework-lens\": expected the associated check, got {framework:?}"
    );

    // The serverless lens asks about secrets too, and this emulator associates no check to
    // ITS best practice.
    let other = list_check_details(
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
    assert_eq!(other.status, 200, "case \"other-lens\": expected 200, got {other:?}");
    assert_eq!(
        details_of(&other),
        Vec::<Value>::new(),
        "case \"other-lens\": a best practice of another lens has no check, got {other:?}"
    );
}

/// One row of the account table: the accounts a workload covers beside the one that owns
/// it, and the accounts a check is then reported for, in order.
struct Covered {
    case: &'static str,
    account_ids: Vec<String>,
    reported_for: Vec<String>,
}

/// A check is reported once per account related to the workload — the owner first, then the
/// accounts the workload names, each once — and what it reports for an account is why it
/// has no result there. Rows differ along how many accounts the workload covers and whether
/// it names its own account.
#[tokio::test]
async fn a_check_is_reported_once_per_account_related_to_the_workload() {
    let client = Client::new();
    let owner = owner_account();
    let names = |ids: &[&str]| -> Vec<String> { ids.iter().map(ToString::to_string).collect() };

    let rows: Vec<Covered> = vec![
        Covered {
            case: "owner-only",
            account_ids: Vec::new(),
            reported_for: Vec::new(),
        },
        Covered {
            case: "one-other-account",
            account_ids: names(&["111122223333"]),
            reported_for: names(&["111122223333"]),
        },
        Covered {
            case: "two-other-accounts",
            account_ids: names(&["111122223333", "444455556666"]),
            reported_for: names(&["111122223333", "444455556666"]),
        },
        Covered {
            case: "names-the-owner-account-too",
            account_ids: vec![owner.clone(), "111122223333".to_string()],
            reported_for: names(&["111122223333"]),
        },
        Covered {
            case: "names-an-account-twice",
            account_ids: names(&["111122223333", "111122223333"]),
            reported_for: names(&["111122223333"]),
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
        let answer = list_check_details(
            &client,
            &workload_id,
            // One check only, so the enumeration is exactly the accounts.
            best_practice("security", "securely-operate", "sec_securely_operate_aws_account"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {answer:?}",
            row.case
        );
        let details = details_of(&answer);
        let expected: Vec<(String, String)> = std::iter::once(owner.clone())
            .chain(row.reported_for.iter().cloned())
            .map(|account| (account, "mfa-on-root-account".to_string()))
            .collect();
        assert_eq!(
            reported(&details),
            expected,
            "case {:?}: expected one check per related account, got {details:?}",
            row.case
        );

        for detail in &details {
            let account_id = string_member(detail, "AccountId");
            let (status, reason) = if account_id == owner {
                OWNER_ACCOUNT_RESULT
            } else {
                OTHER_ACCOUNT_RESULT
            };
            assert_eq!(
                (
                    string_member(detail, "Status").as_str(),
                    string_member(detail, "Reason").as_str()
                ),
                (status, reason),
                "case {:?}: expected the reason the check has no result for {account_id:?}, \
                 got {detail:?}",
                row.case
            );
            // No check ran, so there is no flagged-resource count and no instant at which
            // a result was last updated.
            assert!(
                detail.get("FlaggedResources").is_none() && detail.get("UpdatedAt").is_none(),
                "case {:?}: a check with no result carries no count and no update time, \
                 got {detail:?}",
                row.case
            );
        }
    }
}

/// Checks are fetched only while the workload's Trusted Advisor integration is activated,
/// so the reported set moves with the `DiscoveryConfig` a caller writes — and with nothing
/// else.
#[tokio::test]
async fn checks_are_reported_only_while_the_integration_is_activated() {
    let client = Client::new();
    let request = best_practice("costOptimization", "pricing-model", "cost_pricing_model_analysis");
    let expected: Vec<(String, String)> = [
        "amazon-ec2-reserved-instance-optimization",
        "savings-plans",
    ]
    .iter()
    .map(|check| (owner_account(), (*check).to_string()))
    .collect();

    // A workload whose integration nobody configured: the service defaults it to DISABLED,
    // so nothing is fetched.
    let workload_id = workload(&client, "integration", &[], None).await;
    let unset = list_check_details(&client, &workload_id, request.clone()).await;
    assert_eq!(unset.status, 200, "case \"unset\": expected 200, got {unset:?}");
    assert_eq!(
        details_of(&unset),
        Vec::<Value>::new(),
        "case \"unset\": a workload with no activated integration reports no check, got {unset:?}"
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
        let answer = list_check_details(&client, &workload_id, request.clone()).await;
        assert_eq!(answer.status, 200, "case {case:?}: expected 200, got {answer:?}");
        assert_eq!(
            reported(&details_of(&answer)),
            want,
            "case {case:?}: expected the checks the integration status implies, got {answer:?}"
        );
    }
}

/// Every reported check, following `NextToken` to the end. Answers them in the order they
/// were served and how many pages it took.
async fn collect(
    client: &Client,
    case: &str,
    workload_id: &str,
    request: &Value,
    max_results: Option<i64>,
) -> (Vec<(String, String)>, usize) {
    let mut collected: Vec<(String, String)> = Vec::new();
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
        let answer = list_check_details(client, workload_id, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {pages} must be a 200, got {answer:?}"
        );
        let page = details_of(&answer);
        if let Some(max) = max_results {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max,
                "case {case:?}: a page holds at most MaxResults={max} details, got {}",
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

/// Paging serves every reported check exactly once, in the same order, whatever page size
/// the request asks for: rows differ along `MaxResults`, including both published bounds.
#[tokio::test]
async fn paging_serves_every_reported_check_once() {
    let client = Client::new();
    let workload_id = workload(
        &client,
        "paging",
        &["111122223333", "444455556666"],
        Some(INTEGRATION_ENABLED),
    )
    .await;
    // Three checks over three accounts: nine details, so a page size of one through nine
    // cuts the enumeration inside an account as well as between accounts.
    let request = best_practice("costOptimization", "decomission-resources", "cost_decomissioning_resources_decommission");

    let (unpaged, pages) = collect(&client, "unpaged", &workload_id, &request, None).await;
    assert_eq!(
        unpaged.len(),
        9,
        "case \"unpaged\": expected three checks for each of three accounts, got {unpaged:?}"
    );
    assert_eq!(
        pages, 1,
        "case \"unpaged\": a full page holds every detail, got {pages} pages"
    );

    for max_results in [MAX_RESULTS_RANGE.0, 2, 4, 9, MAX_RESULTS_RANGE.1] {
        let case = format!("max-results-{max_results}");
        let (paged, pages) =
            collect(&client, &case, &workload_id, &request, Some(max_results)).await;
        assert_eq!(
            paged, unpaged,
            "case {case:?}: paging must serve the same details in the same order, got {paged:?}"
        );
        let page_size = usize::try_from(max_results).unwrap_or(usize::MAX);
        let expected_pages = unpaged.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages of {max_results}, got {pages}"
        );
    }
}

/// One row of the refusal table: a request, and the status and wire error code it is
/// refused with.
struct Refused {
    case: &'static str,
    workload: Option<&'static str>,
    body: Value,
    status: u16,
    error_code: &'static str,
}

/// What the read refuses: a workload that does not exist and a lens the workload does not
/// review do not resolve (404), and a pillar, question or choice the lens's content does not
/// define — or a `MaxResults` outside the modelled range, or a token this service did not
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
            let answer = list_check_details(&client, &workload_id, body).await;
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
        let answer = list_check_details(
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
/// operation's own scheme that names a position the enumeration does not hold is refused
/// rather than served as an empty page.
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

    let first = list_check_details(&client, &workload_id, request.clone()).await;
    assert_eq!(first.status, 200, "case \"first-page\": expected 200, got {first:?}");
    let token = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case \"first-page\": six details at two per page leave a NextToken, got {first:?}"
    );

    let mut resumed_request = request.clone();
    resumed_request["NextToken"] = json!(token);
    let resumed = list_check_details(&client, &workload_id, resumed_request).await;
    assert_eq!(
        resumed.status, 200,
        "case \"second-page\": expected 200, got {resumed:?}"
    );
    let served: BTreeSet<(String, String)> = reported(&details_of(&first))
        .into_iter()
        .chain(reported(&details_of(&resumed)))
        .collect();
    assert_eq!(
        served.len(),
        4,
        "case \"second-page\": the resumed page must not repeat the first, got {served:?}"
    );

    // The same request against a workload covering only its owner holds three details, so
    // the token above names a position that enumeration does not hold.
    let narrower = workload(&client, "resume-narrow", &[], Some(INTEGRATION_ENABLED)).await;
    let mut stale_request = request.clone();
    stale_request["NextToken"] = json!(state::encode_check_next_token(
        "111122223333",
        "amazon-s3-bucket-permissions"
    ));
    let stale = list_check_details(&client, &narrower, stale_request).await;
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
async fn listing_checks_changes_nothing_a_later_read_sees() {
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
    let first = list_check_details(&client, &workload_id, request.clone()).await;
    let second = list_check_details(&client, &workload_id, request).await;
    let after = client.get(&workload_id).await;

    assert_eq!(
        details_of(&first),
        details_of(&second),
        "case \"repeated-read\": two reads of the same best practice must agree, \
         got {first:?} then {second:?}"
    );
    assert_eq!(
        before.body.get("Workload").and_then(|workload| workload.get("UpdatedAt")),
        after.body.get("Workload").and_then(|workload| workload.get("UpdatedAt")),
        "case \"repeated-read\": reporting checks must not update the workload, got \
         {before:?} then {after:?}"
    );
}

/// Every row of the check catalog names a best practice the framework lens actually asks
/// about, under an id no other row uses, and the values the handler reports for a check's
/// provider, status and reason are members of the enums the model publishes.
///
/// This is what keeps the association above resolvable: a check is matched to a `ChoiceId`
/// through the question's own choice catalog, so a row naming a question the lens does not
/// ask — or a best practice that question does not offer — would report nothing at all.
#[tokio::test]
async fn every_catalog_row_names_a_best_practice_of_the_framework_lens() {
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for check in state::TRUSTED_ADVISOR_CHECK_CATALOG {
        assert!(
            ids.insert(check.id),
            "case {:?}: a check id is used by one check only",
            check.id
        );
        let question = state::lens_question(FRAMEWORK_LENS, check.question_id);
        let Some(question) = question else {
            panic!(
                "case {:?}: expected a framework-lens question {:?}, got none",
                check.id, check.question_id
            );
        };
        assert!(
            question
                .best_practices
                .iter()
                .any(|practice| practice.title == check.best_practice),
            "case {:?}: expected {:?} among the best practices of {:?}, got {:?}",
            check.id,
            check.best_practice,
            check.question_id,
            question.best_practices
        );
        assert!(
            !check.name.is_empty() && !check.description.is_empty(),
            "case {:?}: a catalog row carries the check's name and description",
            check.id
        );
    }
    assert!(
        !ids.is_empty(),
        "the catalog must publish at least one check, got {ids:?}"
    );

    // The result values the handler reports, judged against the modelled value sets.
    let owner = owner_account();
    for (case, account_id, expected) in [
        ("owner-account", owner.clone(), OWNER_ACCOUNT_RESULT),
        (
            "another-account",
            "111122223333".to_string(),
            OTHER_ACCOUNT_RESULT,
        ),
    ] {
        let result = state::check_result(&account_id, &owner);
        assert_eq!(
            (result.status, result.reason),
            expected,
            "case {case:?}: expected the modelled status and reason, got {result:?}"
        );
    }
}
