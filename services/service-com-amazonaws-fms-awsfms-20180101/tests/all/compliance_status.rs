// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListComplianceStatus`.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`], with the
//! `X-Amz-Target` header and a SigV4 credential scope — so the whole
//! deserialize → handle → serialize walk runs and the assertions are about the
//! WIRE response.
//!
//! The rules asserted here:
//!
//! 1. The read is organisation-scoped, so with no Firewall Manager administrator
//!    association in the account/Region it is refused with HTTP 400
//!    `AccessDeniedException` — whatever else the request carries, and before the
//!    policy id is looked up.
//! 2. A `PolicyId` naming no stored policy is the modelled
//!    `ResourceNotFoundException`.
//! 3. Answered, it reports ONE `PolicyComplianceStatus` per organisation account
//!    the named policy applies to, in ascending account-id order: the accounts an
//!    `IncludeMap` names when the policy specifies one, otherwise every
//!    organisation account except the ones its `ExcludeMap` names (an `ORG_UNIT`
//!    key names accounts Organizations holds and this emulator does not, so it
//!    contributes none).
//! 4. Each entry's evaluation is the violations the store records against THAT
//!    policy in THAT account: `ViolatorCount` is how many, `ComplianceStatus` is
//!    `COMPLIANT` exactly when there are none, and `EvaluationLimitExceeded` is
//!    set exactly when there are more than the documented 100. `LastUpdated` is
//!    the later of the policy's last update and the newest violation recorded
//!    against the account.
//! 5. The page is bounded by `MaxResults` and by the model's maximum for it, the
//!    `NextToken` round-trips, and a token this service never minted is refused.
//! 6. Nothing is written: repeating the read answers identically and the store is
//!    unchanged.
//!
//! The violation inventory the operation summarises is seeded through the store
//! (as the `protection_status` and `administrator_gated_operations` suites seed
//! theirs): Firewall Manager learns a violation by evaluating the member
//! accounts' resources, this emulator stands in for no such evaluator, and so the
//! only honest source of a violation is one somebody put in the store.
//!
//! Each rule is asserted over a TABLE: the rows differ in which scope map and
//! which scope key names the accounts, in account-id length and digits, in how
//! many accounts a policy applies to, in violator count (including zero, one, the
//! documented limit and one past it), and in page size.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{
    POLICY_COMPLIANCE_STATUS_TYPE_VALUES, VIOLATION_REASON_VALUES,
};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the store seam live in one place for this test target, so
// no two suites can drift in how a request is signed or the store is seeded.
use crate::administrator_gated_operations::with_store;
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// The largest page the operation serves, which is the maximum the model's
/// `com.amazonaws.fms#PaginationMaxResults` declares for `MaxResults` (range
/// 1..100, from `smithy_query(query="operation",
/// operation="ListComplianceStatus")`).
const MAX_PAGE_SIZE: usize = 100;

/// A `MaxResults` one past each published bound of that range.
const BELOW_MIN_MAX_RESULTS: i64 = 0;
const ABOVE_MAX_MAX_RESULTS: i64 = 101;

/// The number of noncompliant resources an account may hold before
/// `EvaluationResult$EvaluationLimitExceeded` is set: that member's own
/// documentation says it "indicates that over 100 resources are noncompliant with
/// the Firewall Manager policy".
const EVALUATION_VIOLATOR_LIMIT: usize = 100;

/// A `PolicyId` (the model's `com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "99999999-8888-7777-6666-555555555555";

/// The account associated as the Firewall Manager administrator, which is also an
/// account of its own organisation ("This account must be a member account of the
/// organization", `AssociateAdminAccount`).
const ADMIN: &str = "123456789012";

/// Accounts a second stored policy names, so the organisation this store records
/// holds accounts beside the administrator's own.
const ROSTER: &[&str] = &["111111111111", "222222222222"];

/// The instant the stored policy's last update is pinned to, so `LastUpdated` can
/// be asserted exactly: 2026-01-01T00:00:00Z in Unix seconds, the form Firewall
/// Manager documents for its `TimeStamp` members.
const POLICY_UPDATED_SECS: u64 = 1_767_225_600;

/// How much later than the policy's last update a seeded violation is recorded.
const VIOLATION_OFFSET_SECS: u64 = 3_600;

/// `secs` seconds after the Unix epoch.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// The `message` member of an awsJson1_1 error envelope, under either casing: the
/// modelled error shapes of this service spell it `Message`, the synthetic
/// authorization refusal `message`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Make `admin` the Firewall Manager default administrator, which is what opens
/// the organisation-scoped gate.
async fn associate(router: &MultiServiceRouter, admin: &str) {
    let (status, body) = call(
        router,
        "AssociateAdminAccount",
        &json!({ "AdminAccount": admin }),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateAdminAccount({admin}) must answer 200, got {status} with {body}"
    );
}

/// A policy document carrying the given scope map(s). Only the members `PutPolicy`
/// requires, plus the maps under test, so a row differs from the others ONLY in
/// its scope maps.
///
/// A row names at most ONE of the two maps: "You can specify inclusions or
/// exclusions, but not both" (`Policy$IncludeMap` / `Policy$ExcludeMap`).
fn policy_document(name: &str, include: Option<Value>, exclude: Option<Value>) -> Value {
    let mut policy = json!({
        "PolicyName": name,
        "SecurityServicePolicyData": {
            "Type": "WAFV2",
            "ManagedServiceData": "{\"type\":\"WAFV2\"}"
        },
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    });
    if let Some(include) = include {
        policy["IncludeMap"] = include;
    }
    if let Some(exclude) = exclude {
        policy["ExcludeMap"] = exclude;
    }
    policy
}

/// Store one policy and return the `PolicyId` the service minted for it.
async fn put_policy(
    router: &MultiServiceRouter,
    name: &str,
    include: Option<Value>,
    exclude: Option<Value>,
) -> String {
    let document = policy_document(name, include, exclude);
    let (status, body) = call(router, "PutPolicy", &json!({ "Policy": document })).await;
    assert_eq!(
        status, 200,
        "case {name}: expected PutPolicy to answer 200, got {status} with {body}"
    );
    body["Policy"]["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: PutPolicy minted no PolicyId: {body}"))
        .to_string()
}

/// The accounts the organisation records beside the administrator, contributed by
/// a policy that names them: the roster every scope row is read against.
async fn put_roster(router: &MultiServiceRouter) {
    put_policy(
        router,
        "organisation-roster",
        Some(json!({ "ACCOUNT": ROSTER })),
        None,
    )
    .await;
}

/// Pin the stored policy's last update to a known instant, so the `LastUpdated`
/// the compliance status reports can be asserted exactly rather than as a window
/// around the clock.
fn pin_updated_at(engine: &Arc<engine::InMemoryEngine>, policy_id: &str, when: SystemTime) {
    with_store(engine, |store| {
        let record = store
            .policies
            .get_mut(policy_id)
            .expect("the policy that was just stored");
        record.updated_at = when;
    });
}

/// Record `count` violations of `policy_id` against resources of `account`, as a
/// Firewall Manager policy evaluation records them.
///
/// `pub(crate)` so a sibling suite in this test target seeds the violation
/// inventory through this one helper rather than writing the record a second
/// way.
pub(crate) fn seed_violations(
    engine: &Arc<engine::InMemoryEngine>,
    policy_id: &str,
    account: &str,
    count: usize,
    detected_at: SystemTime,
) {
    with_store(engine, |store| {
        for n in 0..count {
            let record = state::PolicyViolationRecord {
                policy_id: policy_id.to_string(),
                account_id: account.to_string(),
                resource_id: format!("violator-{n:04}"),
                resource_type: Some("AWS::ElasticLoadBalancingV2::LoadBalancer".to_string()),
                // From the model's own catalog for `ViolationReason`
                // (`ops::types::VIOLATION_REASON_VALUES`), cycled so the rows do
                // not all carry one reason.
                violation_reason: Some(
                    VIOLATION_REASON_VALUES[n % VIOLATION_REASON_VALUES.len()].to_string(),
                ),
                metadata: Some(HashMap::from([(
                    "ViolationIndex".to_string(),
                    n.to_string(),
                )])),
                detected_at,
            };
            // The modelled projection of a recorded violation reports exactly
            // what was recorded, which is what the compliance DETAIL slice reads.
            let violator = record.violator();
            assert_eq!(
                (
                    violator.resource_id.as_deref(),
                    violator.resource_type.as_deref(),
                    violator.violation_reason.as_deref(),
                ),
                (
                    Some(record.resource_id.as_str()),
                    record.resource_type.as_deref(),
                    record.violation_reason.as_deref(),
                ),
                "case {policy_id}/{account}/{n}: the ComplianceViolator projection must read \
                 off the record, got {violator:?}"
            );
            store.policy_violations.insert(record.key(), record);
        }
    });
}

/// Call `ListComplianceStatus`.
async fn list(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "ListComplianceStatus", request).await
}

/// The `PolicyComplianceStatus` entries of a response, asserting the member is
/// there at all.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    body["PolicyComplianceStatusList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no PolicyComplianceStatusList in {body}"))
        .clone()
}

/// The `MemberAccount` of each entry, in the order they were served.
fn served_accounts(case: &str, body: &Value) -> Vec<String> {
    entries(case, body)
        .iter()
        .map(|entry| {
            entry["MemberAccount"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: an entry carries no MemberAccount: {entry}"))
                .to_string()
        })
        .collect()
}

/// The single `EvaluationResult` an entry carries.
fn evaluation(case: &str, entry: &Value) -> Value {
    let results = entry["EvaluationResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: an entry carries no EvaluationResults: {entry}"));
    assert_eq!(
        results.len(),
        1,
        "case {case}: expected one EvaluationResult, got {results:?}"
    );
    results[0].clone()
}

/// `EvaluationResult$ViolatorCount`. An absent member is the modelled default
/// (`com.amazonaws.fms#ResourceCount` has `default: 0`).
fn violator_count(result: &Value) -> i64 {
    result
        .get("ViolatorCount")
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// `EvaluationResult$EvaluationLimitExceeded`. An absent member is the modelled
/// default (`com.amazonaws.fms#Boolean` has `default: false`).
fn limit_exceeded(result: &Value) -> bool {
    result
        .get("EvaluationLimitExceeded")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Rule 1: no administrator association, no organisation-scoped answer — and the
/// refusal does not depend on what else the request carries, nor on whether the
/// policy exists.
#[tokio::test]
async fn list_compliance_status_is_refused_until_an_administrator_is_associated() {
    let (router, engine) = emulator_with_engine();
    // A stored policy, so the gate is shown to run before the id is looked up:
    // the known and the unknown id are refused identically. Writing a policy is
    // organisation-scoped too, so it is created with an administrator associated
    // and the association is then removed through the store seam — which is the
    // precondition rule 1 is about: a store that holds policies but no
    // administrator.
    associate(&router, ADMIN).await;
    let policy_id = put_policy(&router, "gated", None, None).await;
    with_store(&engine, |store| store.admin_account = None);

    // Request shapes spanning everything the operation accepts: a known id, an id
    // naming nothing, each published `MaxResults` bound, and a `NextToken` this
    // service never minted.
    let shapes = [
        ("known-policy", json!({ "PolicyId": policy_id })),
        ("unknown-policy", json!({ "PolicyId": UNKNOWN_POLICY_ID })),
        (
            "max-results-at-the-minimum",
            json!({ "PolicyId": policy_id, "MaxResults": 1 }),
        ),
        (
            "max-results-at-the-maximum",
            json!({ "PolicyId": policy_id, "MaxResults": MAX_PAGE_SIZE }),
        ),
        (
            "foreign-next-token",
            json!({ "PolicyId": policy_id, "NextToken": "not.a.token.this.service.minted" }),
        ),
    ];

    for (shape, request) in &shapes {
        let (status, body) = list(&router, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {shape}: with no administrator association the call must be refused, \
             got {status} with {body}"
        );
    }

    // The refusal's message is built from the request: it names the calling
    // account and the Region the request was signed for.
    for region in [REGION, "eu-central-1", "ap-southeast-4"] {
        let (status, body) = call_in(
            &router,
            "ListComplianceStatus",
            &json!({ "PolicyId": policy_id }),
            region,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {region}: expected the authorization refusal, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("No default admin could be found for account {ACCOUNT} in Region {region}"),
            "case {region}: the refusal must name the calling account and the Region of the \
             request, got {body}"
        );
    }

    // A refused call writes nothing.
    let violations = with_store(&engine, |store| store.policy_violations.len());
    assert_eq!(
        violations, 0,
        "a refused call must leave the violation inventory empty, got {violations} entries"
    );

    // With an administrator associated, the same shapes are judged on their own
    // merits.
    associate(&router, ADMIN).await;
    for (shape, request) in &shapes {
        let (status, body) = list(&router, request).await;
        let expected = match *shape {
            "unknown-policy" => (400, "ResourceNotFoundException"),
            "foreign-next-token" => (400, "ValidationException"),
            _ => (200, ""),
        };
        if expected.0 == 200 {
            assert_eq!(
                status, 200,
                "case {shape}: an associated administrator must be answered, got {status} \
                 with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                expected,
                "case {shape}: expected {expected:?}, got {status} with {body}"
            );
        }
    }
}

/// Rule 2: a `PolicyId` naming no stored policy is the modelled not-found — also
/// once the policy it named has been deleted.
#[tokio::test]
async fn an_unknown_or_deleted_policy_is_not_found() {
    let (router, _engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let policy_id = put_policy(&router, "to-be-deleted", None, None).await;

    // Well-formed `PolicyId`s (36 characters of `^[a-z0-9A-Z-]{36}$`) that name
    // nothing: the issue's shape, an all-digit one, and an all-dash one.
    let unknown = [
        UNKNOWN_POLICY_ID,
        "000000000000000000000000000000000000",
        "------------------------------------",
    ];
    for id in unknown {
        let (status, body) = list(&router, &json!({ "PolicyId": id })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {id}: expected the modelled not-found, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {id}: expected the service's not-found text, got {body}"
        );
    }

    let (status, body) = list(&router, &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        status, 200,
        "case stored: the stored policy must be answered, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "DeletePolicy",
        &json!({ "PolicyId": policy_id, "DeleteAllPolicyResources": false }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePolicy must answer 200, got {status} with {body}"
    );
    let (status, body) = list(&router, &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case deleted: a deleted policy must be not found, got {status} with {body}"
    );
}

/// One row of the scope table: the policy's scope maps, and the accounts the
/// policy therefore applies to.
struct ScopeRow {
    name: &'static str,
    include: Option<Value>,
    exclude: Option<Value>,
    expected: &'static [&'static str],
}

/// Rule 3 (and rule 6): the reported accounts are exactly the accounts the stored
/// policy applies to, in ascending order, each entry projecting the policy; and
/// repeating the read answers identically.
#[tokio::test]
async fn the_reported_accounts_are_the_accounts_the_policy_applies_to() {
    // The organisation, for a row that specifies no `IncludeMap`: the
    // administrator plus the roster, plus whatever the row's own maps name.
    let rows = vec![
        ScopeRow {
            name: "no-scope-maps",
            include: None,
            exclude: None,
            expected: &["111111111111", "123456789012", "222222222222"],
        },
        ScopeRow {
            name: "included-accounts",
            include: Some(json!({ "ACCOUNT": ["222222222222", "333333333333"] })),
            exclude: None,
            expected: &["222222222222", "333333333333"],
        },
        ScopeRow {
            name: "included-account-at-the-minimum-length",
            // "7" is an `AWSAccountId` at its minimum length (1..1024, `^[0-9]+$`).
            include: Some(json!({ "ACCOUNT": ["7"] })),
            exclude: None,
            expected: &["7"],
        },
        ScopeRow {
            name: "organizational-units-only",
            // An OU is "the equivalent of specifying all accounts in the OU",
            // which is membership Organizations holds and this emulator does not.
            include: Some(json!({ "ORG_UNIT": ["ou-abc1-12345678"] })),
            exclude: None,
            expected: &[],
        },
        ScopeRow {
            name: "included-accounts-and-organizational-units",
            include: Some(
                json!({ "ACCOUNT": ["444444444444"], "ORG_UNIT": ["ou-abc1-87654321"] }),
            ),
            exclude: None,
            expected: &["444444444444"],
        },
        ScopeRow {
            name: "included-values-that-are-not-account-ids",
            // A `CustomerPolicyScopeId` is a free-form string, so a value that is
            // not an `AWSAccountId` names no account.
            include: Some(json!({ "ACCOUNT": ["not-an-account-id", "555555555555", "12_34"] })),
            exclude: None,
            expected: &["555555555555"],
        },
        ScopeRow {
            name: "excluded-account-of-the-roster",
            exclude: Some(json!({ "ACCOUNT": ["111111111111"] })),
            include: None,
            expected: &["123456789012", "222222222222"],
        },
        ScopeRow {
            name: "excluded-account-the-roster-does-not-name",
            // The excluded account joins the organisation the store records (the
            // policy names it) and is then left out of this policy's scope.
            exclude: Some(json!({ "ACCOUNT": ["666666666666"] })),
            include: None,
            expected: &["111111111111", "123456789012", "222222222222"],
        },
        ScopeRow {
            name: "excluded-accounts-and-organizational-units",
            exclude: Some(json!({ "ACCOUNT": ["222222222222"], "ORG_UNIT": ["ou-abc1-11112222"] })),
            include: None,
            expected: &["111111111111", "123456789012"],
        },
        ScopeRow {
            name: "include-map-that-names-nothing",
            // A map whose every id list is empty scopes the policy to nothing, so
            // it is no inclusion: the whole organisation applies.
            include: Some(json!({ "ACCOUNT": [] })),
            exclude: None,
            expected: &["111111111111", "123456789012", "222222222222"],
        },
    ];

    for row in &rows {
        // A fresh emulator per row, so one row's accounts cannot join another
        // row's organisation.
        let (router, engine) = emulator_with_engine();
        associate(&router, ADMIN).await;
        put_roster(&router).await;
        let policy_id = put_policy(&router, row.name, row.include.clone(), row.exclude.clone()).await;
        pin_updated_at(&engine, &policy_id, at(POLICY_UPDATED_SECS));

        let (status, body) = list(&router, &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {body}",
            row.name
        );
        assert_eq!(
            served_accounts(row.name, &body),
            row.expected
                .iter()
                .map(|account| (*account).to_string())
                .collect::<Vec<String>>(),
            "case {}: expected exactly the accounts the policy applies to, in ascending \
             order, got {}",
            row.name,
            body["PolicyComplianceStatusList"]
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {}: a last page must carry no NextToken, got {body}",
            row.name
        );

        // Every entry projects the stored policy, and — with nothing recorded
        // against any account — reports it compliant as of the policy's own last
        // update.
        for entry in entries(row.name, &body) {
            let account = entry["MemberAccount"].as_str().unwrap_or_default();
            assert_eq!(
                (
                    entry["PolicyOwner"].as_str(),
                    entry["PolicyId"].as_str(),
                    entry["PolicyName"].as_str()
                ),
                (Some(ACCOUNT), Some(policy_id.as_str()), Some(row.name)),
                "case {}/{account}: the entry must project the stored policy, got {entry}",
                row.name
            );
            let result = evaluation(row.name, &entry);
            assert_eq!(
                (
                    result["ComplianceStatus"].as_str(),
                    violator_count(&result),
                    limit_exceeded(&result)
                ),
                (Some("COMPLIANT"), 0, false),
                "case {}/{account}: with no violation recorded the account is compliant, \
                 got {result}",
                row.name
            );
            assert_eq!(
                entry["LastUpdated"].as_f64(),
                Some(POLICY_UPDATED_SECS as f64),
                "case {}/{account}: LastUpdated must be the policy's last update, got {entry}",
                row.name
            );
        }

        // A read changes nothing a later caller sees.
        let (repeat_status, repeat) = list(&router, &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            (repeat_status, &repeat),
            (status, &body),
            "case {}: repeating the read must answer identically, got {repeat_status} with \
             {repeat}",
            row.name
        );
        let violations = with_store(&engine, |store| store.policy_violations.len());
        assert_eq!(
            violations, 0,
            "case {}: the read must record no violation, got {violations} entries",
            row.name
        );
    }
}

/// One row of the evaluation table: how many violations are recorded against an
/// account, and what the evaluation therefore reports.
struct EvaluationRow {
    account: &'static str,
    violations: usize,
}

/// Rule 4: the evaluation is the violations the store records against that policy
/// and that account — the count, the compliance status it implies, the documented
/// limit flag, and the `LastUpdated` the newest violation moves.
#[tokio::test]
async fn the_evaluation_counts_the_violations_recorded_against_the_account() {
    // The model's own catalog, so the statuses asserted below are the modelled
    // ones.
    assert_eq!(
        POLICY_COMPLIANCE_STATUS_TYPE_VALUES.to_vec(),
        vec!["COMPLIANT", "NON_COMPLIANT"],
        "the model's PolicyComplianceStatusType catalog changed: \
         {POLICY_COMPLIANCE_STATUS_TYPE_VALUES:?}"
    );

    // Violator counts spanning the class: none, one, a few, exactly the
    // documented limit, and one past it.
    let rows = [
        EvaluationRow {
            account: "100000000001",
            violations: 0,
        },
        EvaluationRow {
            account: "100000000002",
            violations: 1,
        },
        EvaluationRow {
            account: "100000000003",
            violations: 3,
        },
        EvaluationRow {
            account: "100000000004",
            violations: EVALUATION_VIOLATOR_LIMIT,
        },
        EvaluationRow {
            account: "100000000005",
            violations: EVALUATION_VIOLATOR_LIMIT + 1,
        },
    ];

    let (router, engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let accounts: Vec<&str> = rows.iter().map(|row| row.account).collect();
    let policy_id = put_policy(
        &router,
        "evaluated",
        Some(json!({ "ACCOUNT": accounts })),
        None,
    )
    .await;
    // A SECOND policy over the same accounts, with violations of its own: one
    // policy's evaluation must not count another's.
    let other_policy_id = put_policy(
        &router,
        "other-evaluated",
        Some(json!({ "ACCOUNT": accounts })),
        None,
    )
    .await;
    pin_updated_at(&engine, &policy_id, at(POLICY_UPDATED_SECS));
    pin_updated_at(&engine, &other_policy_id, at(POLICY_UPDATED_SECS));

    let detected_at = at(POLICY_UPDATED_SECS + VIOLATION_OFFSET_SECS);
    for row in &rows {
        seed_violations(&engine, &policy_id, row.account, row.violations, detected_at);
        // Seven violations of the OTHER policy against the same account, recorded
        // later still, so a leak would show up in both the count and LastUpdated.
        seed_violations(
            &engine,
            &other_policy_id,
            row.account,
            7,
            at(POLICY_UPDATED_SECS + 2 * VIOLATION_OFFSET_SECS),
        );
    }

    let (status, body) = list(&router, &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        status, 200,
        "expected 200, got {status} with {body}"
    );
    let served = entries("evaluated", &body);
    assert_eq!(
        served.len(),
        rows.len(),
        "expected one entry per account in scope, got {body}"
    );

    for (row, entry) in rows.iter().zip(served.iter()) {
        let case = row.account;
        assert_eq!(
            entry["MemberAccount"].as_str(),
            Some(case),
            "case {case}: the entries must be in ascending account order, got {entry}"
        );
        let result = evaluation(case, entry);
        let expected_status = if row.violations == 0 {
            "COMPLIANT"
        } else {
            "NON_COMPLIANT"
        };
        assert_eq!(
            (
                result["ComplianceStatus"].as_str(),
                violator_count(&result),
                limit_exceeded(&result)
            ),
            (
                Some(expected_status),
                row.violations as i64,
                row.violations > EVALUATION_VIOLATOR_LIMIT
            ),
            "case {case}: {} recorded violations must be reported as {expected_status} with \
             that count, got {result}",
            row.violations
        );
        // "Timestamp of the last update to the EvaluationResult objects": the
        // policy's own last update, or the newest violation recorded against this
        // account when that is later.
        let expected_last_updated = if row.violations == 0 {
            POLICY_UPDATED_SECS
        } else {
            POLICY_UPDATED_SECS + VIOLATION_OFFSET_SECS
        };
        assert_eq!(
            entry["LastUpdated"].as_f64(),
            Some(expected_last_updated as f64),
            "case {case}: expected LastUpdated {expected_last_updated}, got {entry}"
        );
    }

    // The same read answers identically, and the other policy's own evaluation
    // reports ITS seven violations per account.
    let (repeat_status, repeat) = list(&router, &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        (repeat_status, &repeat),
        (status, &body),
        "repeating the read must answer identically, got {repeat_status} with {repeat}"
    );
    let (status, other) = list(&router, &json!({ "PolicyId": other_policy_id })).await;
    assert_eq!(
        status, 200,
        "the other policy must be answered, got {status} with {other}"
    );
    for entry in entries("other-evaluated", &other) {
        let case = entry["MemberAccount"].as_str().unwrap_or_default().to_string();
        let result = evaluation(&case, &entry);
        assert_eq!(
            (
                result["ComplianceStatus"].as_str(),
                violator_count(&result),
                limit_exceeded(&result)
            ),
            (Some("NON_COMPLIANT"), 7, false),
            "case other/{case}: expected the other policy's own seven violators, got {result}"
        );
    }
}

/// Read every page, following `NextToken`, and return the accounts in the order
/// they were served. Asserts the page bound and the token invariant on each page.
async fn page_through(
    router: &MultiServiceRouter,
    policy_id: &str,
    case: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let bound = max_results.unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE);
    let mut served: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // Enough iterations for the table below at the smallest page size, so a token
    // that never clears fails here instead of looping forever.
    for page in 0..1_000 {
        let mut request = json!({ "PolicyId": policy_id });
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = list(router, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: page {page} must answer 200, got {status} with {body}"
        );
        let accounts = served_accounts(case, &body);
        assert!(
            accounts.len() <= bound,
            "case {case}: page {page} must hold at most {bound} entries, got {}",
            accounts.len()
        );
        served.extend(accounts);
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return served;
        }
        assert_eq!(
            served.len() % bound,
            0,
            "case {case}: page {page} carried a NextToken after a short page (served {} so far)",
            served.len()
        );
    }
    panic!("case {case}: NextToken never cleared after 1000 pages");
}

/// Rule 5: the page is bounded and the token round-trips, for page sizes spanning
/// the model's range and for a caller that sends no `MaxResults` at all; a token
/// this service never minted is refused.
#[tokio::test]
async fn the_compliance_page_is_bounded_and_the_token_round_trips() {
    // More accounts than one page holds, so a default page genuinely truncates.
    let accounts: Vec<String> = (1..=151).map(|n| format!("{n:012}")).collect();
    let (router, engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let policy_id = put_policy(
        &router,
        "paged",
        Some(json!({ "ACCOUNT": accounts })),
        None,
    )
    .await;
    pin_updated_at(&engine, &policy_id, at(POLICY_UPDATED_SECS));
    let expected: Vec<String> = {
        let mut sorted = accounts.clone();
        sorted.sort();
        sorted
    };
    assert!(
        expected.len() > MAX_PAGE_SIZE,
        "the table must span more than one page, got {} accounts",
        expected.len()
    );

    // Page sizes: the model's minimum, an odd size that does not divide the total,
    // the model's maximum, and no `MaxResults` at all.
    for max_results in [Some(1), Some(7), Some(MAX_PAGE_SIZE), None] {
        let case = format!("max-results-{max_results:?}");
        let served = page_through(&router, &policy_id, &case, max_results).await;
        assert_eq!(
            served, expected,
            "case {case}: the pages together must serve every account once, in order"
        );
    }

    // The default page is bounded by the model's maximum even though the caller
    // asked for nothing, and its token resumes exactly after it.
    let (status, body) = list(&router, &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        status, 200,
        "case default-page: expected 200, got {status} with {body}"
    );
    assert_eq!(
        served_accounts("default-page", &body).len(),
        MAX_PAGE_SIZE,
        "case default-page: expected {MAX_PAGE_SIZE} entries, got {body}"
    );
    let token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case default-page: expected a NextToken, got {body}"))
        .to_string();
    let (status, resumed) = list(
        &router,
        &json!({ "PolicyId": policy_id, "NextToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case default-page: resuming must answer 200, got {status} with {resumed}"
    );
    assert_eq!(
        served_accounts("default-page-resumed", &resumed),
        expected[MAX_PAGE_SIZE..].to_vec(),
        "case default-page: the token must resume exactly after the first page, got {resumed}"
    );

    // One past each published `MaxResults` bound is outside the modelled range.
    for out_of_range in [BELOW_MIN_MAX_RESULTS, ABOVE_MAX_MAX_RESULTS] {
        let (status, body) = list(
            &router,
            &json!({ "PolicyId": policy_id, "MaxResults": out_of_range }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case max-results-{out_of_range}: a MaxResults outside the modelled 1..100 range \
             must be refused, got {status} with {body}"
        );
    }

    // Tokens this service never minted for an account-keyed page: a value that is
    // not hex at all, hex that decodes to something that is no account id, and a
    // token minted for a page keyed by something else (a `PolicyId`).
    let foreign = [
        ("not-hex", "not.a.token.this.service.minted".to_string()),
        ("hex-of-a-non-account", hex::encode("not-an-account-id")),
        ("policy-keyed-token", hex::encode(&policy_id)),
    ];
    for (case, token) in &foreign {
        let (status, body) = list(
            &router,
            &json!({ "PolicyId": policy_id, "NextToken": token }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a foreign NextToken must be refused, got {status} with {body}"
        );
    }
}
