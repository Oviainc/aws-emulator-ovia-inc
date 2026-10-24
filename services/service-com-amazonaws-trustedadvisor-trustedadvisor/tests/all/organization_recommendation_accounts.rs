// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListOrganizationRecommendationAccounts`, driven over
//! HTTP through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules this suite pins, each over a TABLE of inputs rather than one call:
//!
//! * **the list reports exactly ONE `AccountRecommendationLifecycleSummary` per
//!   member account recommendation the organization recommendation aggregates,
//!   in member-recommendation-ARN order, each member read off that account's own
//!   recommendation** — its ARN, the account in that ARN, and the
//!   `lifecycleStage`/`updateReason`/`updateReasonCode`/`lastUpdatedAt` that
//!   recommendation carries, never the organization recommendation's own;
//! * `affectedAccountId` keeps only the member recommendations owned by that
//!   account, and keeps none when no member belongs to it;
//! * a lifecycle update a member account makes through
//!   `UpdateRecommendationLifecycle` is what this list reports for that account
//!   on the next read, and it moves no other member and not the organization
//!   recommendation itself;
//! * `nextToken` walks every entry exactly once at any page size, and a token
//!   this read did not issue is refused with 400 `ValidationException`;
//! * a well-formed identifier that names no organization recommendation is 404
//!   `ResourceNotFoundException`, and an input outside a modelled constraint is
//!   400 `ValidationException`.
//!
//! The wire contract comes from the model —
//! `smithy_query(query="operation",operation="ListOrganizationRecommendationAccounts",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — GET `/v1/organization-recommendations/{organizationRecommendationIdentifier}/accounts`
//! answers 200 with a required `accountRecommendationLifecycleSummaries`,
//! `maxResults` is `range 1..200`, `nextToken` is `length 4..10000`,
//! `affectedAccountId` is `length 12..12, pattern ^\d+$`, and the operation
//! binds `ResourceNotFoundException` 404 and `ValidationException` 400.
//!
//! The seeded expectations below are written out by hand; their source is the
//! `SEED` table in `src/state.rs`. Every call is made as an account whose
//! support plan includes the Trusted Advisor API; the gate ahead of every
//! handler is pinned in `tests/all/support_level_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::ops::types::{
    UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_REASON_CODE_VALUES,
    UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES,
};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::{get_path, organization_recommendation_arn};
use crate::recommendation_loop::{
    call, emulator_on_plan, error_code, field_of_each, label, recommendation_arn, timestamp_secs,
};

/// One seeded check, the lifecycle stage the seed left on the member account's
/// recommendation for it, and when that recommendation was last updated.
/// Source: the `SEED` table in `src/state.rs` (one day apart in SEED order,
/// which is not check-id order).
struct Seeded {
    check_id: &'static str,
    lifecycle_stage: Option<&'static str>,
    last_updated_at: &'static str,
}

const SEEDED: &[Seeded] = &[
    Seeded {
        check_id: "amazon-ebs-snapshots",
        lifecycle_stage: None,
        last_updated_at: "2024-01-03T00:00:00Z",
    },
    Seeded {
        check_id: "iam-access-key-rotation",
        lifecycle_stage: None,
        last_updated_at: "2024-01-04T00:00:00Z",
    },
    Seeded {
        check_id: "low-utilization-ec2-instances",
        lifecycle_stage: Some("pending_response"),
        last_updated_at: "2024-01-02T00:00:00Z",
    },
    Seeded {
        check_id: "rds-idle-db-instances",
        lifecycle_stage: Some("in_progress"),
        last_updated_at: "2024-01-05T00:00:00Z",
    },
];

/// The check whose organization recommendation the multi-account rows are built
/// on: its recommendation is `priority`, the type whose lifecycle a member
/// account can move.
const PRIORITY_CHECK: &str = "low-utilization-ec2-instances";

/// A `standard` check, left with its seeded single member so a row can assert
/// the untouched case beside the grown one.
const UNTOUCHED_CHECK: &str = "amazon-ebs-snapshots";

/// A mounted emulator on a plan that includes the Trusted Advisor API, with the
/// engine it was built on so a test can enrol organization member accounts the
/// way something outside this API would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    emulator_on_plan(SupportPlan::Business)
}

/// The modelled URI of `ListOrganizationRecommendationAccounts` for one seeded
/// check's organization recommendation.
fn accounts_path(check_id: &str) -> String {
    format!(
        "/v1/organization-recommendations/{}/accounts",
        label(&organization_recommendation_arn(check_id))
    )
}

/// `arn:aws:trustedadvisor::<account>:recommendation/<check>-recommendation` —
/// the ARN pattern `AccountRecommendationArn` declares, for a member account
/// other than the emulator's own.
fn member_recommendation_arn(account_id: &str, check_id: &str) -> String {
    format!("arn:aws:trustedadvisor::{account_id}:recommendation/{check_id}-recommendation")
}

/// Enrol one more member account of the organization: a recommendation of the
/// same check, owned by `account_id`, aggregated by that check's organization
/// recommendation.
///
/// Written in process because no modelled operation of this API enrols an
/// account or produces its check results — organization membership comes from
/// AWS Organizations, outside this service — exactly as
/// `tests/all/organization_recommendations.rs` writes a member resource's
/// status. The member list is deliberately grown in an order that is NOT ARN
/// order, so the page order under test is the handler's.
fn add_member_account(
    engine: &engine::InMemoryEngine,
    check_id: &str,
    account_id: &str,
    lifecycle_stage: Option<&str>,
) -> String {
    let seeded_arn = recommendation_arn(check_id);
    let organization_arn = organization_recommendation_arn(check_id);
    let state = service_state(engine).expect("the in-memory engine carries this service's state");
    let mut state = state.lock();
    let mut member = state
        .recommendations
        .get(&seeded_arn)
        .unwrap_or_else(|| panic!("no seeded recommendation for {check_id}"))
        .clone();
    member.arn = member_recommendation_arn(account_id, check_id);
    member.lifecycle_stage = lifecycle_stage.map(str::to_string);
    member.update_reason = None;
    member.update_reason_code = None;
    let arn = member.arn.clone();
    state.recommendations.insert(arn.clone(), member);
    state
        .organization_recommendations
        .get_mut(&organization_arn)
        .unwrap_or_else(|| panic!("no organization recommendation for {check_id}"))
        .member_recommendation_arns
        .push(arn.clone());
    arn
}

/// The summaries one read reports, asserting 200 first.
async fn summaries(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    body
}

/// The `accountRecommendationArn` of every summary one read reports.
async fn listed_arns(router: &MultiServiceRouter, case: &str, path: &str) -> Vec<String> {
    let body = summaries(router, case, path).await;
    field_of_each(
        &body,
        "accountRecommendationLifecycleSummaries",
        "accountRecommendationArn",
    )
}

/// The one summary a read reports for `account_id`, or a panic naming the body.
async fn summary_of(
    router: &MultiServiceRouter,
    case: &str,
    check_id: &str,
    account_id: &str,
) -> Value {
    let body = summaries(router, case, &accounts_path(check_id)).await;
    body["accountRecommendationLifecycleSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no accountRecommendationLifecycleSummaries in {body}"))
        .iter()
        .find(|summary| summary["accountId"].as_str() == Some(account_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the list reports no account {account_id}: {body}"))
}

#[tokio::test]
async fn the_accounts_list_reports_the_member_recommendation_of_every_seeded_check() {
    let (router, _engine) = emulator();
    let account = engine::EMULATOR_ACCOUNT_ID;

    // Rows: every seeded organization recommendation, which differ along the
    // member recommendation's lifecycle stage (two carry one, two carry none)
    // and its `lastUpdatedAt`.
    for row in SEEDED {
        let case = row.check_id;
        let body = summaries(&router, case, &accounts_path(case)).await;
        let listed = body["accountRecommendationLifecycleSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no summaries in {body}"));
        assert_eq!(
            listed.len(),
            1,
            "case {case}: expected one summary per member account recommendation, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case}: expected no nextToken on a complete page, got {:?}",
            body["nextToken"]
        );

        let summary = &listed[0];
        let expected_arn = recommendation_arn(case);
        assert_eq!(
            summary["accountRecommendationArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: expected accountRecommendationArn {expected_arn}, got {:?}",
            summary["accountRecommendationArn"]
        );
        assert_eq!(
            summary["accountId"].as_str(),
            Some(account),
            "case {case}: expected accountId {account}, got {:?}",
            summary["accountId"]
        );
        // `AccountId` is modelled `length 12..12, pattern ^\d+$`, and it is the
        // account segment of the recommendation's own ARN.
        let reported = summary["accountId"].as_str().unwrap_or_default();
        assert!(
            reported.len() == 12 && reported.chars().all(|c| c.is_ascii_digit()),
            "case {case}: expected a 12-digit accountId, got {reported:?}"
        );
        assert_eq!(
            expected_arn.split(':').nth(4),
            Some(reported),
            "case {case}: expected the accountId to be the ARN's account segment, got \
             {reported:?} for {expected_arn}"
        );
        assert_eq!(
            summary["lifecycleStage"].as_str(),
            row.lifecycle_stage,
            "case {case}: expected lifecycleStage {:?}, got {:?}",
            row.lifecycle_stage,
            summary["lifecycleStage"]
        );
        assert_eq!(
            timestamp_secs(&summary["lastUpdatedAt"]),
            timestamp_secs(&json!(row.last_updated_at)),
            "case {case}: expected lastUpdatedAt {}, got {:?}",
            row.last_updated_at,
            summary["lastUpdatedAt"]
        );
        // The seed attaches no update reason, and nothing in this API records a
        // TAM action, so neither attribution member is reported.
        for member in [
            "updateReason",
            "updateReasonCode",
            "updatedOnBehalfOf",
            "updatedOnBehalfOfJobTitle",
        ] {
            assert!(
                summary[member].is_null(),
                "case {case}: expected no {member} on a member nobody updated, got {:?}",
                summary[member]
            );
        }
    }
}

#[tokio::test]
async fn affected_account_id_keeps_only_that_accounts_member_recommendation() {
    let (router, engine) = emulator();
    let own = engine::EMULATOR_ACCOUNT_ID.to_string();

    // Three more member accounts of the organization, enrolled in an order that
    // is not ARN order, with different lifecycle stages.
    let enrolled = [
        ("444455556666", Some("dismissed")),
        ("111122223333", Some("resolved")),
        ("999988887777", None),
    ];
    for (account, stage) in enrolled {
        add_member_account(&engine, PRIORITY_CHECK, account, stage);
    }

    let mut every_arn: Vec<String> = enrolled
        .iter()
        .map(|(account, _)| member_recommendation_arn(account, PRIORITY_CHECK))
        .chain(std::iter::once(recommendation_arn(PRIORITY_CHECK)))
        .collect();
    // The rule is ARN order, so the expectation is that set sorted.
    every_arn.sort();

    // Rows: no filter (every member), each enrolled account (its one member),
    // the emulator's own account, an account that owns no member of this
    // organization recommendation, and the same filters on a check whose
    // organization recommendation still has its single seeded member.
    let mut rows: Vec<(String, String, Vec<String>)> = vec![(
        "no affectedAccountId".to_string(),
        accounts_path(PRIORITY_CHECK),
        every_arn.clone(),
    )];
    for (account, _) in enrolled {
        rows.push((
            format!("affectedAccountId={account}"),
            format!("{}?affectedAccountId={account}", accounts_path(PRIORITY_CHECK)),
            vec![member_recommendation_arn(account, PRIORITY_CHECK)],
        ));
    }
    rows.push((
        "affectedAccountId of the calling account".to_string(),
        format!("{}?affectedAccountId={own}", accounts_path(PRIORITY_CHECK)),
        vec![recommendation_arn(PRIORITY_CHECK)],
    ));
    rows.push((
        "affectedAccountId that owns no member".to_string(),
        format!(
            "{}?affectedAccountId=222233334444",
            accounts_path(PRIORITY_CHECK)
        ),
        Vec::new(),
    ));
    rows.push((
        format!("{UNTOUCHED_CHECK} with no filter"),
        accounts_path(UNTOUCHED_CHECK),
        vec![recommendation_arn(UNTOUCHED_CHECK)],
    ));
    rows.push((
        format!("{UNTOUCHED_CHECK} filtered to the calling account"),
        format!("{}?affectedAccountId={own}", accounts_path(UNTOUCHED_CHECK)),
        vec![recommendation_arn(UNTOUCHED_CHECK)],
    ));
    rows.push((
        format!("{UNTOUCHED_CHECK} filtered to an account enrolled elsewhere"),
        format!(
            "{}?affectedAccountId=111122223333",
            accounts_path(UNTOUCHED_CHECK)
        ),
        Vec::new(),
    ));

    for (case, path, expected) in rows {
        let arns = listed_arns(&router, &case, &path).await;
        assert_eq!(
            arns, expected,
            "case {case}: expected {expected:?} in ARN order, got {arns:?}"
        );
    }

    // Each enrolled account's own lifecycle stage is what its summary reports:
    // the members are separate resources, not copies of the organization
    // recommendation's stage.
    for (account, stage) in enrolled {
        let summary = summary_of(&router, account, PRIORITY_CHECK, account).await;
        assert_eq!(
            summary["lifecycleStage"].as_str(),
            stage,
            "case {account}: expected lifecycleStage {stage:?}, got {:?}",
            summary["lifecycleStage"]
        );
    }
}

#[tokio::test]
async fn paginating_the_accounts_list_walks_every_entry_exactly_once() {
    let (router, engine) = emulator();
    for account in ["444455556666", "111122223333", "999988887777"] {
        add_member_account(&engine, PRIORITY_CHECK, account, Some("in_progress"));
    }
    let mut expected: Vec<String> = ["444455556666", "111122223333", "999988887777"]
        .iter()
        .map(|account| member_recommendation_arn(account, PRIORITY_CHECK))
        .chain(std::iter::once(recommendation_arn(PRIORITY_CHECK)))
        .collect();
    expected.sort();

    // Rows differ along the page size: one entry per page, two, three, exactly
    // the member set, and a page larger than it (200 is the modelled maximum).
    for page_size in [1usize, 2, 3, 4, 200] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One round beyond the number of pages the size implies, so a token
        // that never clears is caught as a loop rather than hanging.
        for _ in 0..=expected.len() + 1 {
            let query = match &token {
                Some(value) => format!("?maxResults={page_size}&nextToken={}", label(value)),
                None => format!("?maxResults={page_size}"),
            };
            let case = format!("maxResults={page_size}");
            let body = summaries(
                &router,
                &case,
                &format!("{}{query}", accounts_path(PRIORITY_CHECK)),
            )
            .await;
            let page = field_of_each(
                &body,
                "accountRecommendationLifecycleSummaries",
                "accountRecommendationArn",
            );
            assert!(
                page.len() <= page_size,
                "case {case}: expected at most {page_size} per page, got {}",
                page.len()
            );
            seen.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case maxResults={page_size}: expected the token to clear, got {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case maxResults={page_size}: expected every member once in ARN order, got {seen:?}"
        );
    }

    // And the filtered sequence pages the same way: one member, one page, no
    // token.
    let arns = listed_arns(
        &router,
        "filtered to one account at maxResults=1",
        &format!(
            "{}?affectedAccountId=111122223333&maxResults=1",
            accounts_path(PRIORITY_CHECK)
        ),
    )
    .await;
    assert_eq!(
        arns,
        vec![member_recommendation_arn("111122223333", PRIORITY_CHECK)],
        "case filtered to one account: expected that account's member, got {arns:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_read_did_not_issue_is_refused() {
    let (router, engine) = emulator();
    for account in ["444455556666", "111122223333"] {
        add_member_account(&engine, PRIORITY_CHECK, account, None);
    }

    // A token THIS read issued, and one another list issued.
    let (_, first_page) = call(
        &router,
        "GET",
        &format!("{}?maxResults=1", accounts_path(PRIORITY_CHECK)),
        None,
    )
    .await;
    let own_token = first_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated page")
        .to_string();
    let (_, checks_page) = call(&router, "GET", "/v1/checks?maxResults=1", None).await;
    let checks_token = checks_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated ListChecks page")
        .to_string();

    // Rows: a token this service never issued (the modelled `length 4..10000`
    // is satisfied, so the refusal is the handler's own); a token another list
    // issued; this read's own token under a FILTER it was not issued under; and
    // its own token on ANOTHER organization recommendation's accounts, a
    // different sequence under the same operation.
    let refused: &[(&str, String)] = &[
        (
            "a token this service never issued",
            format!("{}?nextToken=%21%21%21%21", accounts_path(PRIORITY_CHECK)),
        ),
        (
            "a ListChecks token",
            format!(
                "{}?nextToken={}",
                accounts_path(PRIORITY_CHECK),
                label(&checks_token)
            ),
        ),
        (
            "its own token under an affectedAccountId filter",
            format!(
                "{}?affectedAccountId=111122223333&nextToken={}",
                accounts_path(PRIORITY_CHECK),
                label(&own_token)
            ),
        ),
        (
            "its own token on another organization recommendation's accounts",
            format!(
                "{}?nextToken={}",
                accounts_path(UNTOUCHED_CHECK),
                label(&own_token)
            ),
        ),
    ];

    for (case, path) in refused {
        let (status, body) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
    }

    // And the token IS accepted by the read that issued it, resuming strictly
    // after the entry it names.
    let mut every_arn: Vec<String> = ["444455556666", "111122223333"]
        .iter()
        .map(|account| member_recommendation_arn(account, PRIORITY_CHECK))
        .chain(std::iter::once(recommendation_arn(PRIORITY_CHECK)))
        .collect();
    every_arn.sort();
    let arns = listed_arns(
        &router,
        "the issuing read accepts its own token",
        &format!(
            "{}?maxResults=1&nextToken={}",
            accounts_path(PRIORITY_CHECK),
            label(&own_token)
        ),
    )
    .await;
    assert_eq!(
        arns,
        vec![every_arn[1].clone()],
        "case the issuing read accepts its own token: expected the second member, got {arns:?}"
    );
}

#[tokio::test]
async fn a_member_accounts_lifecycle_update_is_what_the_accounts_list_reports() {
    let (router, engine) = emulator();
    let own = engine::EMULATOR_ACCOUNT_ID.to_string();
    let other = "111122223333";
    add_member_account(&engine, PRIORITY_CHECK, other, Some("pending_response"));

    // Rows: every `UpdateRecommendationLifecycleStage` the model declares,
    // paired with a different `UpdateRecommendationLifecycleStageReasonCode`
    // each time (the codes are cycled so each stage carries one no earlier row
    // used), plus a final request that omits the reason members, which PUT
    // semantics clear.
    let stages = UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES;
    let codes = UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_REASON_CODE_VALUES;
    let mut rows: Vec<(String, Value, Option<&str>, Option<&str>)> = Vec::new();
    for (index, stage) in stages.iter().enumerate() {
        let code = codes[index % codes.len()];
        let reason = format!("moving to {stage} for the {code} reason");
        rows.push((
            format!("{stage} with {code}"),
            json!({
                "lifecycleStage": stage,
                "updateReason": reason,
                "updateReasonCode": code,
            }),
            Some(stage),
            Some(code),
        ));
    }
    rows.push((
        "a stage with no reason members".to_string(),
        json!({ "lifecycleStage": "in_progress" }),
        Some("in_progress"),
        None,
    ));

    let lifecycle_path = format!(
        "/v1/recommendations/{}/lifecycle",
        label(&recommendation_arn(PRIORITY_CHECK))
    );
    for (case, payload, expected_stage, expected_code) in rows {
        let expected_reason = payload["updateReason"].as_str().map(str::to_string);
        let before = timestamp_secs(&json!("2024-01-02T00:00:00Z"));
        let (status, body) = call(&router, "PUT", &lifecycle_path, Some(payload)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from UpdateRecommendationLifecycle, got {status} with \
             {body}"
        );

        let summary = summary_of(&router, &case, PRIORITY_CHECK, &own).await;
        assert_eq!(
            summary["lifecycleStage"].as_str(),
            expected_stage,
            "case {case}: expected lifecycleStage {expected_stage:?}, got {:?}",
            summary["lifecycleStage"]
        );
        assert_eq!(
            summary["updateReason"].as_str().map(str::to_string),
            expected_reason,
            "case {case}: expected updateReason {expected_reason:?}, got {:?}",
            summary["updateReason"]
        );
        assert_eq!(
            summary["updateReasonCode"].as_str(),
            expected_code,
            "case {case}: expected updateReasonCode {expected_code:?}, got {:?}",
            summary["updateReasonCode"]
        );
        let updated = timestamp_secs(&summary["lastUpdatedAt"]);
        assert!(
            updated > before,
            "case {case}: expected lastUpdatedAt stamped from the clock after the seeded \
             {before}, got {updated}"
        );

        // The write is confined to the member account that made it: the other
        // member keeps its own stage, and the organization recommendation keeps
        // the one the seed left.
        let untouched = summary_of(&router, &case, PRIORITY_CHECK, other).await;
        assert_eq!(
            untouched["lifecycleStage"].as_str(),
            Some("pending_response"),
            "case {case}: expected the other member to keep its stage, got {:?}",
            untouched["lifecycleStage"]
        );
        let (status, organization) =
            call(&router, "GET", &get_path(PRIORITY_CHECK), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetOrganizationRecommendation, got {status} with \
             {organization}"
        );
        assert_eq!(
            organization["organizationRecommendation"]["lifecycleStage"].as_str(),
            Some("pending_response"),
            "case {case}: expected the organization recommendation's own stage to be unmoved, \
             got {organization}"
        );
    }
}

#[tokio::test]
async fn an_identifier_that_names_nothing_is_not_found_and_a_bad_input_is_refused() {
    let (router, _engine) = emulator();

    // Rows differ along what makes the ARN name nothing: an all-zeros id, an id
    // no seed produced, a seeded id in another partition, and a seeded id with
    // the account-level suffix.
    let unknown = [
        (
            "an all-zeros organization recommendation id",
            "arn:aws:trustedadvisor:::organization-recommendation/00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an id no seed produced",
            "arn:aws:trustedadvisor:::organization-recommendation/not-a-seeded-one".to_string(),
        ),
        (
            "a seeded id in another partition",
            format!(
                "arn:aws-us-gov:trustedadvisor:::organization-recommendation/{PRIORITY_CHECK}-organization-recommendation"
            ),
        ),
        (
            "the account-level recommendation id",
            format!(
                "arn:aws:trustedadvisor:::organization-recommendation/{PRIORITY_CHECK}-recommendation"
            ),
        ),
    ];
    for (case, arn) in unknown {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/v1/organization-recommendations/{}/accounts", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
    }

    // Rows outside a modelled constraint, refused ahead of the handler: the
    // identifier's pattern, `maxResults`'s `range 1..200` on each side of each
    // published bound, `nextToken`'s `length 4..10000` minimum, and
    // `affectedAccountId`'s `length 12..12` and `pattern ^\d+$`.
    let path = accounts_path(PRIORITY_CHECK);
    let refused: &[(&str, String)] = &[
        (
            "an identifier outside the modelled pattern",
            "/v1/organization-recommendations/arn:aws:trustedadvisor:::not-one/accounts"
                .to_string(),
        ),
        (
            "maxResults below the modelled range",
            format!("{path}?maxResults=0"),
        ),
        (
            "maxResults past the modelled range",
            format!("{path}?maxResults=201"),
        ),
        (
            "a nextToken shorter than the modelled minimum",
            format!("{path}?nextToken=abc"),
        ),
        (
            "an affectedAccountId of eleven digits",
            format!("{path}?affectedAccountId=11112222333"),
        ),
        (
            "an affectedAccountId of thirteen digits",
            format!("{path}?affectedAccountId=1111222233334"),
        ),
        (
            "an affectedAccountId that is not digits",
            format!("{path}?affectedAccountId=notadigits12"),
        ),
    ];
    for (case, path) in refused {
        let (status, body) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
    }

    // Each published bound of `maxResults` is itself accepted.
    for page_size in [1, 200] {
        let case = format!("maxResults at the bound {page_size}");
        let arns = listed_arns(&router, &case, &format!("{path}?maxResults={page_size}")).await;
        assert_eq!(
            arns,
            vec![recommendation_arn(PRIORITY_CHECK)],
            "case {case}: expected the single seeded member, got {arns:?}"
        );
    }
}
