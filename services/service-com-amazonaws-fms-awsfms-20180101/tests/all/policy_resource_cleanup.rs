// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the cleanup `DeletePolicy` selects with
//! `DeleteAllPolicyResources`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`], with the
//! `X-Amz-Target` header and a SigV4 credential scope, so the whole
//! deserialize → handle → serialize walk runs.
//!
//! The rule under test in one sentence: a delete that carries a cleanup selector
//! always removes the named resource itself, and removes the objects the service
//! holds on that resource's BEHALF only when the selector asks for the cleanup —
//! so the two modes leave different state behind, and neither of them reaches
//! another resource's objects.
//!
//! For a Firewall Manager policy, the objects held on its behalf are what its
//! protections put in this store: the violations an evaluation of that policy
//! recorded (reported by `ListComplianceStatus`) and the attacks recorded against
//! the resources it protects (reported by `GetProtectionStatus`). Both
//! inventories are seeded through the store, as the `compliance_status` and
//! `protection_status` suites seed them: Firewall Manager learns a violation by
//! evaluating member accounts and Shield Advanced detects the attacks, and this
//! emulator stands in for neither, so the only honest source of such a record is
//! one somebody put in the store. Each test asserts first, over the WIRE, that
//! the seeded records are what the API reports for the policy — so what the
//! cleanup removes is the state a reader could see, not bookkeeping nothing
//! reaches.
//!
//! The table in [`cases`] is the class the rule ranges over: the rows differ in
//! how the selector arrives on the wire (absent — the member's modelled default
//! — explicitly false, explicitly true), in the policy's security-service type,
//! in which kinds of record the policy holds (violations only, attacks only,
//! both, none), in how many of each, and in how many member accounts they are
//! spread over.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::SECURITY_SERVICE_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the store seam and the two inventory seeders live in one
// place for this test target, so no two suites can drift in how a request is
// signed or a record is written.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::compliance_status::seed_violations;
use crate::policy_lifecycle::{call, emulator_with_engine, error_code};
use crate::protection_status::{seed_attack, Attack};

/// The `SecurityServiceType` whose policies `GetProtectionStatus` serves, so a
/// row of this type can be read back over the wire.
const SHIELD_ADVANCED: &str = "SHIELD_ADVANCED";

/// A second `SecurityServiceType`, so the rule is asserted on more than the one
/// type whose attack inventory exists.
const WAFV2: &str = "WAFV2";

/// The member accounts the policies under test are scoped to, and the accounts
/// the seeded violations are recorded against. They differ in their digits so no
/// assertion can pass by matching one account id for another.
const ACCOUNTS: &[&str] = &["111111111111", "222222222222", "333333333333"];

/// Attack ids for the policy under test. Taken in order, so a row that records
/// fewer attacks records a PREFIX of them.
const SUBJECT_ATTACK_IDS: &[&str] = &["cleanup-a1", "cleanup-a2", "cleanup-a3"];

/// Attack ids for the second policy, which no row ever deletes: its records must
/// survive both modes of the delete.
const DECOY_ATTACK_IDS: &[&str] = &["decoy-a1"];

/// How many violations the second policy holds, in [`ACCOUNTS`]`[0]`.
const DECOY_VIOLATIONS: usize = 2;

/// How many attacks the second policy holds.
const DECOY_ATTACKS: usize = 1;

/// One row of the delete table.
struct Case {
    name: &'static str,
    /// How the cleanup selector arrives on the wire: absent, so the member's
    /// modelled default applies, or explicitly either way.
    delete_all: Option<bool>,
    /// The policy's `SecurityServicePolicyData.Type`.
    service_type: &'static str,
    /// The violations recorded against the policy: one entry per member account,
    /// with how many of that account's resources violate it.
    violations: &'static [(&'static str, usize)],
    /// How many attacks are recorded against the policy. Only a Shield Advanced
    /// policy protects resources Shield Advanced reports attacks on, so this is
    /// zero for a row of any other type.
    attacks: usize,
}

impl Case {
    /// How many violations the row records in total.
    fn violation_count(&self) -> usize {
        self.violations.iter().map(|(_, count)| count).sum()
    }

    /// Whether the row's request asks for the cleanup. The member's documented
    /// default is `false` ("DeleteAllPolicyResources (Boolean, default false)"),
    /// so a request that leaves it out asks for no cleanup.
    fn cleanup_requested(&self) -> bool {
        self.delete_all.unwrap_or(false)
    }

    /// The `DeletePolicy` request body for this row.
    fn request(&self, policy_id: &str) -> Value {
        let mut request = json!({ "PolicyId": policy_id });
        if let Some(flag) = self.delete_all {
            request["DeleteAllPolicyResources"] = json!(flag);
        }
        request
    }
}

/// The delete class the rule ranges over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "selector absent: the member's modelled default",
            delete_all: None,
            service_type: SHIELD_ADVANCED,
            violations: &[("111111111111", 1)],
            attacks: 2,
        },
        Case {
            name: "cleanup declined, violations in two accounts",
            delete_all: Some(false),
            service_type: WAFV2,
            violations: &[("111111111111", 3), ("222222222222", 1)],
            attacks: 0,
        },
        Case {
            name: "cleanup declined, attacks only",
            delete_all: Some(false),
            service_type: SHIELD_ADVANCED,
            violations: &[],
            attacks: 1,
        },
        Case {
            name: "cleanup requested, records of both kinds in three accounts",
            delete_all: Some(true),
            service_type: SHIELD_ADVANCED,
            violations: &[
                ("111111111111", 2),
                ("222222222222", 2),
                ("333333333333", 1),
            ],
            attacks: 3,
        },
        Case {
            name: "cleanup requested, one record of each kind",
            delete_all: Some(true),
            service_type: SHIELD_ADVANCED,
            violations: &[("333333333333", 1)],
            attacks: 1,
        },
        Case {
            name: "cleanup requested, nothing recorded to clean up",
            delete_all: Some(true),
            service_type: WAFV2,
            violations: &[],
            attacks: 0,
        },
    ]
}

/// A policy document `PutPolicy` accepts, of the given `SecurityServiceType`.
///
/// The resource type is the v2 load balancer, which the `Policy$ResourceType`
/// documentation's "valid resource types for each Firewall Manager policy type"
/// lists for both WAF and Shield Advanced policies.
fn policy_document(name: &str, service_type: &str) -> Value {
    json!({
        "PolicyName": name,
        "SecurityServicePolicyData": { "Type": service_type },
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "IncludeMap": { "ACCOUNT": ACCOUNTS },
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    })
}

/// Store one tagged policy and return the `PolicyId` and `PolicyArn` the service
/// minted for it.
async fn put_policy(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    service_type: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        "PutPolicy",
        &json!({
            "Policy": policy_document(name, service_type),
            "TagList": [{ "Key": "owner", "Value": name }]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutPolicy({name}) to answer 200, got {status} with {body}"
    );
    let policy_id = body["Policy"]["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: PutPolicy reported no PolicyId: {body}"))
        .to_string();
    let policy_arn = body["PolicyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: PutPolicy reported no PolicyArn: {body}"))
        .to_string();
    (policy_id, policy_arn)
}

/// `count` attacks recorded against one policy, their ids taken in order from
/// `ids`. The rows differ in account, start instant and whether the attack is
/// still ongoing, so no two records of one policy are the same record twice.
fn attack_rows(ids: &'static [&'static str], count: usize) -> Vec<Attack> {
    assert!(
        count <= ids.len(),
        "the attack-id catalog holds {} ids, {count} were asked for",
        ids.len()
    );
    (0..count)
        .map(|n| Attack {
            attack_id: ids[n],
            account: ACCOUNTS[n % ACCOUNTS.len()],
            start_secs: 600 * (n as u64 + 1),
            start_millis: 0,
            duration_secs: if n % 2 == 0 { Some(60) } else { None },
            vectors: &["REQUEST_FLOOD"],
        })
        .collect()
}

/// What the store still holds on `policy_id`'s behalf: how many violations and
/// how many attacks are recorded against it.
///
/// Each record names its own policy, so the count is read off the records rather
/// than off the shape of the keys they are stored under.
fn held(engine: &Arc<engine::InMemoryEngine>, policy_id: &str) -> (usize, usize) {
    with_store(engine, |store| {
        (
            store
                .policy_violations
                .values()
                .filter(|violation| violation.policy_id == policy_id)
                .count(),
            store
                .shield_attacks
                .values()
                .filter(|attack| attack.policy_id == policy_id)
                .count(),
        )
    })
}

/// How many violating resources `ListComplianceStatus` reports for a policy,
/// across every account it serves an entry for.
async fn reported_violators(router: &MultiServiceRouter, case: &str, policy_id: &str) -> usize {
    let (status, body) = call(
        router,
        "ListComplianceStatus",
        &json!({ "PolicyId": policy_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListComplianceStatus({policy_id}) to answer 200, \
         got {status} with {body}"
    );
    body["PolicyComplianceStatusList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyComplianceStatusList in {body}"))
        .iter()
        .map(|entry| {
            let count = entry["EvaluationResults"][0]
                .get("ViolatorCount")
                .and_then(Value::as_i64)
                // An absent member is the modelled default:
                // `com.amazonaws.fms#ResourceCount` declares `default: 0`.
                .unwrap_or(0);
            usize::try_from(count).unwrap_or_else(|_| {
                panic!("case {case:?}: a negative ViolatorCount in {entry}")
            })
        })
        .sum()
}

/// How many attacks `GetProtectionStatus` reports for a Shield Advanced policy.
async fn reported_attacks(router: &MultiServiceRouter, case: &str, policy_id: &str) -> usize {
    let (status, body) = call(
        router,
        "GetProtectionStatus",
        &json!({ "PolicyId": policy_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetProtectionStatus({policy_id}) to answer 200, \
         got {status} with {body}"
    );
    let data = body["Data"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the response carried no Data string: {body}"));
    match serde_json::from_str::<Value>(data) {
        Ok(Value::Array(entries)) => entries.len(),
        other => panic!("case {case:?}: Data must be a JSON array, got {data:?} ({other:?})"),
    }
}

/// The emulator one row is asserted against: an administrator association, the
/// policy under test with the row's records, and a SECOND policy with records of
/// its own that no delete in this suite names.
struct Seeded {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    policy_id: String,
    policy_arn: String,
    other_policy_id: String,
}

async fn seeded(case: &Case) -> Seeded {
    assert!(
        SECURITY_SERVICE_TYPE_VALUES.contains(&case.service_type),
        "case {:?}: {} is not a SecurityServiceType the model declares, which are {:?}",
        case.name,
        case.service_type,
        SECURITY_SERVICE_TYPE_VALUES
    );

    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let (policy_id, policy_arn) =
        put_policy(&router, case.name, "policy-under-test", case.service_type).await;
    let (other_policy_id, _) =
        put_policy(&router, case.name, "the-other-policy", SHIELD_ADVANCED).await;

    let detected_at = SystemTime::now();
    for (account, count) in case.violations {
        seed_violations(&engine, &policy_id, account, *count, detected_at);
    }
    seed_violations(
        &engine,
        &other_policy_id,
        ACCOUNTS[0],
        DECOY_VIOLATIONS,
        detected_at,
    );
    with_store(&engine, |store| {
        for attack in attack_rows(SUBJECT_ATTACK_IDS, case.attacks) {
            seed_attack(store, &policy_id, &attack);
        }
        for attack in attack_rows(DECOY_ATTACK_IDS, DECOY_ATTACKS) {
            seed_attack(store, &other_policy_id, &attack);
        }
    });

    Seeded {
        router,
        engine,
        policy_id,
        policy_arn,
        other_policy_id,
    }
}

/// `DeleteAllPolicyResources` decides what becomes of the records the store
/// holds on the deleted policy's behalf: asked for, the cleanup takes them with
/// the policy; declined — explicitly or by leaving the member out — they are
/// left in place. Another policy's records are never reached either way.
#[tokio::test]
async fn delete_policy_cleans_up_the_policys_own_records_only_when_asked() {
    for case in cases() {
        let name = case.name;
        let seeded = seeded(&case).await;
        let router = &seeded.router;

        // What the cleanup is about is state a reader can see: before the delete
        // the API reports exactly the seeded records for this policy.
        let violations = case.violation_count();
        assert_eq!(
            reported_violators(router, name, &seeded.policy_id).await,
            violations,
            "case {name:?}: ListComplianceStatus must report the {violations} seeded violations \
             before the delete"
        );
        if case.service_type == SHIELD_ADVANCED {
            assert_eq!(
                reported_attacks(router, name, &seeded.policy_id).await,
                case.attacks,
                "case {name:?}: GetProtectionStatus must report the {} seeded attacks before the \
                 delete",
                case.attacks
            );
        }

        let request = case.request(&seeded.policy_id);
        let (status, body) = call(router, "DeletePolicy", &request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeletePolicy({request}) to answer 200, got {status} with \
             {body}"
        );

        let expected = if case.cleanup_requested() {
            (0, 0)
        } else {
            (violations, case.attacks)
        };
        let actual = held(&seeded.engine, &seeded.policy_id);
        assert_eq!(
            actual, expected,
            "case {name:?}: with DeleteAllPolicyResources={:?} the policy's own \
             (violations, attacks) must be {expected:?}, got {actual:?}",
            case.delete_all
        );

        // The cleanup is scoped to the deleted policy: every record of the other
        // policy is still held, and still reported.
        let other = held(&seeded.engine, &seeded.other_policy_id);
        assert_eq!(
            other,
            (DECOY_VIOLATIONS, DECOY_ATTACKS),
            "case {name:?}: another policy's (violations, attacks) must be untouched, got \
             {other:?}"
        );
        assert_eq!(
            reported_violators(router, name, &seeded.other_policy_id).await,
            DECOY_VIOLATIONS,
            "case {name:?}: another policy's violations must still be reported"
        );
        assert_eq!(
            reported_attacks(router, name, &seeded.other_policy_id).await,
            DECOY_ATTACKS,
            "case {name:?}: another policy's attacks must still be reported"
        );
    }
}

/// The other half of the rule: the POLICY itself — its record and its entry in
/// the ARN-keyed tag index — goes on every delete, whatever the cleanup selector
/// says, so a second delete of the same id finds nothing and the policy's ARN
/// names no taggable resource afterwards.
#[tokio::test]
async fn delete_policy_removes_the_policy_itself_whatever_the_selector_says() {
    for case in cases() {
        let name = case.name;
        let seeded = seeded(&case).await;
        let router = &seeded.router;

        let request = case.request(&seeded.policy_id);
        let (status, body) = call(router, "DeletePolicy", &request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeletePolicy({request}) to answer 200, got {status} with \
             {body}"
        );

        let (status, body) = call(
            router,
            "GetPolicy",
            &json!({ "PolicyId": &seeded.policy_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {name:?}: GetPolicy after the delete must not find the policy, got {status} \
             with {body}"
        );

        let (status, body) = call(
            router,
            "ListTagsForResource",
            &json!({ "ResourceArn": &seeded.policy_arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {name:?}: the deleted policy's ARN must name no taggable resource, got \
             {status} with {body}"
        );

        let (status, body) = call(router, "DeletePolicy", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {name:?}: deleting the same id twice must not find it, got {status} with {body}"
        );

        // The delete reached one policy: the other one is still readable.
        let (status, body) = call(
            router,
            "GetPolicy",
            &json!({ "PolicyId": &seeded.other_policy_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the other policy must still be readable, got {status} with {body}"
        );
    }
}
