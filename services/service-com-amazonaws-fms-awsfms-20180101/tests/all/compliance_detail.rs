// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetComplianceDetail`.
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
//!    `AccessDeniedException` — whatever else the request carries, and before
//!    either identifier is looked up.
//! 2. A `PolicyId` naming no stored policy is the modelled
//!    `ResourceNotFoundException`, and so is one whose policy has been deleted.
//! 3. A `MemberAccount` the stored policy does not apply to is that same
//!    not-found: Firewall Manager evaluates a policy only against the accounts in
//!    the policy's own scope — the accounts `ListComplianceStatus` reports one
//!    entry per — so no other account has a compliance detail of this policy.
//! 4. Answered, `Violators` names the resources the store records as violating
//!    THAT policy in THAT account, in ascending resource-id order, each member
//!    read off the recorded violation. The shape's own documentation bounds the
//!    report — "A maximum of 100 entries are displayed. If more than 100
//!    resources are noncompliant, `EvaluationLimitExceeded` is set to `True`" —
//!    so at most 100 are named and the flag is set exactly when more than 100 are
//!    recorded.
//! 5. The detail and the summary are the same evaluation: `ViolatorCount` from
//!    `ListComplianceStatus` is how many the detail's own flag is about, and the
//!    two agree for every row.
//! 6. Nothing is written: repeating the read answers identically and the
//!    violation inventory is unchanged.
//!
//! The violation inventory the operation reports is seeded through the store, by
//! the ONE seam [`crate::compliance_status::seed_violations`] the compliance
//! suites share: Firewall Manager learns a violation by evaluating the member
//! accounts' resources, this emulator stands in for no such evaluator, and so the
//! only honest source of a violation is one somebody put in the store.
//!
//! Each rule is asserted over a TABLE: the rows differ in which scope map and
//! which scope key names the accounts, in account-id length and digits, and in
//! violator count (including zero, one, a few, the documented maximum and one
//! past it).

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::VIOLATION_REASON_VALUES;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the store seam and the compliance fixtures live in one place
// for this test target, so no two suites can drift in how a request is signed,
// the store is seeded or the organisation is set up.
use crate::administrator_gated_operations::with_store;
use crate::compliance_status::{associate, error_message, put_policy, put_roster, ADMIN};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// The most violating resources one `PolicyComplianceDetail` names: "A maximum of
/// 100 entries are displayed. If more than 100 resources are noncompliant,
/// `EvaluationLimitExceeded` is set to `True`"
/// (`smithy_query(query="shape", shape="PolicyComplianceDetail")`). A DOCUMENTED
/// bound, from that shape's own documentation.
const MAX_VIOLATORS: usize = 100;

/// A `PolicyId` (the model's `com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "99999999-8888-7777-6666-555555555555";

/// The instant the seeded violations are recorded at: 2026-01-01T01:00:00Z in
/// Unix seconds.
const VIOLATION_DETECTED_SECS: u64 = 1_767_229_200;

/// `secs` seconds after the Unix epoch.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Call `GetComplianceDetail` for one (policy, member account) pair.
async fn detail(router: &MultiServiceRouter, policy_id: &str, account: &str) -> (u16, Value) {
    call(
        router,
        "GetComplianceDetail",
        &json!({ "PolicyId": policy_id, "MemberAccount": account }),
    )
    .await
}

/// The `PolicyComplianceDetail` of a response, asserting the member is there at
/// all.
fn reported(case: &str, body: &Value) -> Value {
    let reported = &body["PolicyComplianceDetail"];
    assert!(
        reported.is_object(),
        "case {case}: no PolicyComplianceDetail in {body}"
    );
    reported.clone()
}

/// `PolicyComplianceDetail$Violators`. An absent member is no violator at all,
/// which is what an account with nothing recorded against it reports.
fn violators(case: &str, reported: &Value) -> Vec<Value> {
    match reported.get("Violators") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: Violators is not a list: {reported}"))
            .clone(),
    }
}

/// The `ResourceId` of each reported violator, in the order they were reported.
fn violator_ids(case: &str, reported: &Value) -> Vec<String> {
    violators(case, reported)
        .iter()
        .map(|violator| {
            violator["ResourceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a violator carries no ResourceId: {violator}")
                })
                .to_string()
        })
        .collect()
}

/// `PolicyComplianceDetail$EvaluationLimitExceeded`. An absent member is the
/// modelled default (`com.amazonaws.fms#Boolean` has `default: false`).
fn limit_exceeded(reported: &Value) -> bool {
    reported
        .get("EvaluationLimitExceeded")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The resource id [`crate::compliance_status::seed_violations`] records its
/// `n`th violation under.
fn seeded_resource_id(n: usize) -> String {
    format!("violator-{n:04}")
}

/// Rule 1: no administrator association, no organisation-scoped answer — and the
/// refusal does not depend on what else the request carries, nor on whether the
/// policy or the account exists.
#[tokio::test]
async fn get_compliance_detail_is_refused_until_an_administrator_is_associated() {
    let (router, engine) = emulator_with_engine();
    // A stored policy, so the gate is shown to run before either identifier is
    // looked up: the known and the unknown pair are refused identically. Writing
    // a policy is organisation-scoped too, so it is created with an administrator
    // associated and the association is then removed through the store seam —
    // which is the precondition rule 1 is about: a store that holds policies but
    // no administrator.
    associate(&router, ADMIN).await;
    let policy_id = put_policy(
        &router,
        "gated",
        Some(json!({ "ACCOUNT": [ADMIN, "111111111111"] })),
        None,
    )
    .await;
    with_store(&engine, |store| store.admin_account = None);

    // Request shapes spanning everything the operation accepts: a pair the policy
    // scopes, a pair it does not, an id naming nothing, and an `AWSAccountId` at
    // its minimum length (1..1024, `^[0-9]+$`).
    let shapes = [
        ("in-scope-account", policy_id.clone(), ADMIN.to_string()),
        (
            "out-of-scope-account",
            policy_id.clone(),
            "222222222222".to_string(),
        ),
        (
            "unknown-policy",
            UNKNOWN_POLICY_ID.to_string(),
            ADMIN.to_string(),
        ),
        (
            "shortest-account-id",
            policy_id.clone(),
            "7".to_string(),
        ),
    ];

    for (shape, policy, account) in &shapes {
        let (status, body) = detail(&router, policy, account).await;
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
            "GetComplianceDetail",
            &json!({ "PolicyId": policy_id, "MemberAccount": ADMIN }),
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
    // merits: only the pair the policy scopes is answered.
    associate(&router, ADMIN).await;
    for (shape, policy, account) in &shapes {
        let (status, body) = detail(&router, policy, account).await;
        if *shape == "in-scope-account" {
            assert_eq!(
                status, 200,
                "case {shape}: an associated administrator must be answered, got {status} \
                 with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {shape}: expected the modelled not-found, got {status} with {body}"
            );
        }
    }
}

/// Rule 2: a `PolicyId` naming no stored policy has no compliance detail — also
/// once the policy it named has been deleted.
#[tokio::test]
async fn an_unknown_or_deleted_policy_has_no_compliance_detail() {
    let (router, _engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let policy_id = put_policy(
        &router,
        "to-be-deleted",
        Some(json!({ "ACCOUNT": [ADMIN] })),
        None,
    )
    .await;

    // Well-formed `PolicyId`s (36 characters of `^[a-z0-9A-Z-]{36}$`) that name
    // nothing: a UUID shape, an all-digit one and an all-dash one.
    let unknown = [
        UNKNOWN_POLICY_ID,
        "000000000000000000000000000000000000",
        "------------------------------------",
    ];
    for id in unknown {
        let (status, body) = detail(&router, id, ADMIN).await;
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

    let (status, body) = detail(&router, &policy_id, ADMIN).await;
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
    let (status, body) = detail(&router, &policy_id, ADMIN).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case deleted: a deleted policy must be not found, got {status} with {body}"
    );
}

/// One row of the scope table: the policy's scope maps, the accounts the policy
/// therefore applies to, and accounts it does not.
struct ScopeRow {
    name: &'static str,
    include: Option<Value>,
    exclude: Option<Value>,
    in_scope: &'static [&'static str],
    out_of_scope: &'static [&'static str],
}

/// Rule 3 (and rule 6): a compliance detail exists for exactly the accounts the
/// stored policy applies to — the scope `ListComplianceStatus` enumerates — and
/// for no other account; and repeating the read answers identically.
#[tokio::test]
async fn only_the_accounts_the_policy_applies_to_have_a_compliance_detail() {
    // The organisation, for a row that specifies no `IncludeMap`: the
    // administrator plus the roster, plus whatever the row's own maps name.
    let rows = [
        ScopeRow {
            name: "no-scope-maps",
            include: None,
            exclude: None,
            in_scope: &["111111111111", "123456789012", "222222222222"],
            out_of_scope: &["333333333333"],
        },
        ScopeRow {
            name: "included-accounts",
            include: Some(json!({ "ACCOUNT": ["222222222222", "333333333333"] })),
            exclude: None,
            in_scope: &["222222222222", "333333333333"],
            out_of_scope: &["111111111111", "123456789012"],
        },
        ScopeRow {
            name: "included-account-at-the-minimum-length",
            // "7" is an `AWSAccountId` at its minimum length (1..1024, `^[0-9]+$`).
            include: Some(json!({ "ACCOUNT": ["7"] })),
            exclude: None,
            in_scope: &["7"],
            out_of_scope: &["111111111111", "123456789012"],
        },
        ScopeRow {
            name: "organizational-units-only",
            // An OU is "the equivalent of specifying all accounts in the OU",
            // which is membership Organizations holds and this emulator does not.
            include: Some(json!({ "ORG_UNIT": ["ou-abc1-12345678"] })),
            exclude: None,
            in_scope: &[],
            out_of_scope: &["111111111111", "123456789012", "222222222222"],
        },
        ScopeRow {
            name: "included-values-that-are-not-account-ids",
            // A `CustomerPolicyScopeId` is a free-form string, so a value that is
            // not an `AWSAccountId` names no account.
            include: Some(json!({ "ACCOUNT": ["not-an-account-id", "555555555555", "12_34"] })),
            exclude: None,
            in_scope: &["555555555555"],
            out_of_scope: &["1234", "111111111111"],
        },
        ScopeRow {
            name: "excluded-account-of-the-roster",
            include: None,
            exclude: Some(json!({ "ACCOUNT": ["111111111111"] })),
            in_scope: &["123456789012", "222222222222"],
            out_of_scope: &["111111111111"],
        },
        ScopeRow {
            name: "excluded-accounts-and-organizational-units",
            include: None,
            exclude: Some(json!({ "ACCOUNT": ["222222222222"], "ORG_UNIT": ["ou-abc1-11112222"] })),
            in_scope: &["111111111111", "123456789012"],
            out_of_scope: &["222222222222"],
        },
        ScopeRow {
            name: "include-map-that-names-nothing",
            // A map whose every id list is empty scopes the policy to nothing, so
            // it is no inclusion: the whole organisation applies.
            include: Some(json!({ "ACCOUNT": [] })),
            exclude: None,
            in_scope: &["111111111111", "123456789012", "222222222222"],
            out_of_scope: &["444444444444"],
        },
    ];

    for row in &rows {
        // A fresh emulator per row, so one row's accounts cannot join another
        // row's organisation.
        let (router, engine) = emulator_with_engine();
        associate(&router, ADMIN).await;
        put_roster(&router).await;
        let policy_id =
            put_policy(&router, row.name, row.include.clone(), row.exclude.clone()).await;

        for account in row.in_scope {
            let (status, body) = detail(&router, &policy_id, account).await;
            assert_eq!(
                status, 200,
                "case {}/{account}: an account the policy applies to must be answered, got \
                 {status} with {body}",
                row.name
            );
            let reported = reported(row.name, &body);
            assert_eq!(
                (
                    reported["PolicyOwner"].as_str(),
                    reported["PolicyId"].as_str(),
                    reported["MemberAccount"].as_str()
                ),
                (Some(ACCOUNT), Some(policy_id.as_str()), Some(*account)),
                "case {}/{account}: the detail must name the policy's owner, the policy and \
                 the account asked about, got {reported}",
                row.name
            );
            // Nothing is recorded against any account here, so the account has no
            // violating resource and the documented maximum is not in play.
            assert_eq!(
                (violator_ids(row.name, &reported), limit_exceeded(&reported)),
                (Vec::new(), false),
                "case {}/{account}: with no violation recorded the account has no violator, \
                 got {reported}",
                row.name
            );

            // A read changes nothing a later caller sees.
            let (repeat_status, repeat) = detail(&router, &policy_id, account).await;
            assert_eq!(
                (repeat_status, &repeat),
                (status, &body),
                "case {}/{account}: repeating the read must answer identically, got \
                 {repeat_status} with {repeat}",
                row.name
            );
        }

        for account in row.out_of_scope {
            let (status, body) = detail(&router, &policy_id, account).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {}/{account}: an account the policy does not apply to has no compliance \
                 detail, got {status} with {body}",
                row.name
            );
            assert_eq!(
                error_message(&body),
                state::RESOURCE_NOT_FOUND_MESSAGE,
                "case {}/{account}: expected the service's not-found text, got {body}",
                row.name
            );
        }

        let violations = with_store(&engine, |store| store.policy_violations.len());
        assert_eq!(
            violations, 0,
            "case {}: the read must record no violation, got {violations} entries",
            row.name
        );
    }
}

/// One row of the violator table: how many violations are recorded against an
/// account, and therefore how many of them the detail names.
struct ViolatorRow {
    account: &'static str,
    violations: usize,
}

/// Rules 4, 5 and 6: the violators are the resources recorded against that policy
/// and that account, bounded by the documented maximum, agreeing with the summary
/// of the same evaluation, and written by no read.
#[tokio::test]
async fn the_violators_are_the_resources_recorded_against_the_account() {
    // The model's own catalog, so the violation reasons asserted below are the
    // modelled ones.
    assert!(
        VIOLATION_REASON_VALUES.contains(&"RESOURCE_MISSING_WEB_ACL"),
        "the model's ViolationReason catalog no longer names RESOURCE_MISSING_WEB_ACL: \
         {VIOLATION_REASON_VALUES:?}"
    );

    // Violator counts spanning the class: none, one, a few, exactly the
    // documented maximum, and one past it.
    let rows = [
        ViolatorRow {
            account: "100000000001",
            violations: 0,
        },
        ViolatorRow {
            account: "100000000002",
            violations: 1,
        },
        ViolatorRow {
            account: "100000000003",
            violations: 3,
        },
        ViolatorRow {
            account: "100000000004",
            violations: MAX_VIOLATORS,
        },
        ViolatorRow {
            account: "100000000005",
            violations: MAX_VIOLATORS + 1,
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
    // policy's detail must not report another's.
    let other_policy_id = put_policy(
        &router,
        "other-evaluated",
        Some(json!({ "ACCOUNT": accounts })),
        None,
    )
    .await;

    for row in &rows {
        crate::compliance_status::seed_violations(
            &engine,
            &policy_id,
            row.account,
            row.violations,
            at(VIOLATION_DETECTED_SECS),
        );
        // The other policy records a DIFFERENT number against the same account,
        // so a detail that read the whole inventory instead of this policy's
        // range would report the wrong resources.
        crate::compliance_status::seed_violations(
            &engine,
            &other_policy_id,
            row.account,
            row.violations + 2,
            at(VIOLATION_DETECTED_SECS),
        );
    }
    let seeded = with_store(&engine, |store| store.policy_violations.len());

    for row in &rows {
        let case = row.account;
        let (status, body) = detail(&router, &policy_id, row.account).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let reported = reported(case, &body);

        // At most the documented maximum are named, in the inventory's own
        // ascending resource-id order, and they are the FIRST that many of the
        // violations recorded against this account under this policy.
        let expected_ids: Vec<String> = (0..row.violations.min(MAX_VIOLATORS))
            .map(seeded_resource_id)
            .collect();
        assert_eq!(
            violator_ids(case, &reported),
            expected_ids,
            "case {case}: expected the {} recorded violators the detail may name, got \
             {reported}",
            expected_ids.len()
        );
        assert_eq!(
            limit_exceeded(&reported),
            row.violations > MAX_VIOLATORS,
            "case {case}: EvaluationLimitExceeded must be set exactly when more than \
             {MAX_VIOLATORS} resources are noncompliant ({} are), got {reported}",
            row.violations
        );

        // Every member of a reported violator is read off the recorded violation.
        for (n, violator) in violators(case, &reported).iter().enumerate() {
            assert_eq!(
                (
                    violator["ResourceType"].as_str(),
                    violator["ViolationReason"].as_str(),
                    violator["Metadata"]["ViolationIndex"].as_str()
                ),
                (
                    Some("AWS::ElasticLoadBalancingV2::LoadBalancer"),
                    Some(VIOLATION_REASON_VALUES[n % VIOLATION_REASON_VALUES.len()]),
                    Some(n.to_string().as_str())
                ),
                "case {case}/{n}: the violator must report what the violation recorded, got \
                 {violator}"
            );
        }

        // Rule 5: the detail and the summary are the same evaluation.
        let (summary_status, summary) = call(
            &router,
            "ListComplianceStatus",
            &json!({ "PolicyId": policy_id }),
        )
        .await;
        assert_eq!(
            summary_status, 200,
            "case {case}: ListComplianceStatus must answer 200, got {summary_status} with \
             {summary}"
        );
        let entry = summary["PolicyComplianceStatusList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no PolicyComplianceStatusList in {summary}"))
            .iter()
            .find(|entry| entry["MemberAccount"].as_str() == Some(row.account))
            .unwrap_or_else(|| panic!("case {case}: the summary omits the account: {summary}"))
            .clone();
        let evaluation = entry["EvaluationResults"][0].clone();
        assert_eq!(
            (
                evaluation
                    .get("ViolatorCount")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                evaluation
                    .get("EvaluationLimitExceeded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            ),
            (row.violations as i64, row.violations > MAX_VIOLATORS),
            "case {case}: the summary must count the violations the detail names, got \
             {evaluation}"
        );

        // Rule 6: a read changes nothing a later caller sees.
        let (repeat_status, repeat) = detail(&router, &policy_id, row.account).await;
        assert_eq!(
            (repeat_status, &repeat),
            (status, &body),
            "case {case}: repeating the read must answer identically, got {repeat_status} \
             with {repeat}"
        );
    }

    let after = with_store(&engine, |store| store.policy_violations.len());
    assert_eq!(
        after, seeded,
        "the reads must leave the violation inventory as it was, expected {seeded} entries, \
         got {after}"
    );
}

/// The violation inventory is reached the way the handler reaches it, so this
/// suite's seam is the same one the store exposes: a second map of violations
/// would let the detail and the summary disagree.
#[tokio::test]
async fn the_detail_reads_the_same_inventory_the_store_records() {
    let (router, engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    let account = "100000000009";
    let policy_id = put_policy(
        &router,
        "one-violation",
        Some(json!({ "ACCOUNT": [account] })),
        None,
    )
    .await;
    crate::compliance_status::seed_violations(
        &engine,
        &policy_id,
        account,
        1,
        at(VIOLATION_DETECTED_SECS),
    );

    let (status, body) = detail(&router, &policy_id, account).await;
    assert_eq!(
        status, 200,
        "expected 200, got {status} with {body}"
    );
    let recorded: Vec<String> = with_store(&engine, |store| {
        store
            .account_violations(&policy_id, account)
            .map(|violation| violation.resource_id.clone())
            .collect()
    });
    assert_eq!(
        violator_ids("one-violation", &reported("one-violation", &body)),
        recorded,
        "the detail must name exactly the resources the store records, which are {recorded:?}, \
         got {body}"
    );
}
