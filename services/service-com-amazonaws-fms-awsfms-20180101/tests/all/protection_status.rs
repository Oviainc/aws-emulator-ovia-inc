// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetProtectionStatus`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: `GetProtectionStatus` reports, for the
//! Shield Advanced policy the request names, exactly the recorded attacks of
//! THAT policy whose account matches the requested member account and whose
//! lifetime overlaps the requested period — in chronological order, bounded by
//! `MaxResults` and resumable from `NextToken` — beside the administrator
//! association's account id and the policy's own `SecurityServiceType`; a
//! policy of any other type is refused, and nothing is written.
//!
//! The attack inventory the operation reports is seeded through the store (as
//! the `ListDiscoveredResources` suite seeds its inventory): Shield Advanced
//! detects attacks and this emulator stands in for no attack detector, so the
//! only honest source of an attack is one somebody put in the store.
//!
//! The tables below are the class the rule ranges over: the attack rows differ
//! in account, in start instant (including two within the same second), in
//! duration, in whether the attack is still ongoing, in attack-vector count and
//! in which policy they belong to; the query rows differ in which bounds are
//! present, in where each bound falls relative to an attack's start and end
//! (including exactly ON a boundary, and an inverted period), and in the member
//! account selected; the page rows span both published `MaxResults` bounds and
//! one past each.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::SECURITY_SERVICE_TYPE_VALUES;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the store helper live in one place for this test target,
// so no two suites can drift in how a request is signed or the store is seeded.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// The largest page the operation serves: the maximum the model's
/// `com.amazonaws.fms#PaginationMaxResults` declares for `MaxResults`
/// (range 1..100).
const MAX_PAGE_SIZE: usize = 100;

/// A `MaxResults` one past each published bound of that range.
const BELOW_MIN_MAX_RESULTS: i64 = 0;
const ABOVE_MAX_MAX_RESULTS: i64 = 101;

/// The instant the seeded attacks are placed around: 2026-01-01T00:00:00Z in
/// Unix seconds, which is the form Firewall Manager documents as the default for
/// its `TimeStamp` members ("the default used by Firewall Manager is Unix time
/// in seconds").
const BASE_SECS: u64 = 1_767_225_600;

/// A second Region, to assert the administrator gate's answer does not depend on
/// which Region the request addresses.
const OTHER_REGION: &str = "eu-central-1";

/// A `PolicyId` (the model's `com.amazonaws.fms#PolicyId`: 36 characters
/// matching `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "99999999-8888-7777-6666-555555555555";

/// `BASE_SECS` plus `offset_secs` seconds and `offset_millis` milliseconds.
fn at(offset_secs: u64, offset_millis: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(BASE_SECS + offset_secs)
        + Duration::from_millis(offset_millis)
}

/// The same instant as a request bound carries it on the wire: Unix seconds.
fn stamp(offset_secs: u64) -> Value {
    json!(BASE_SECS + offset_secs)
}

/// One attack row: what the store holds about an attack Shield Advanced
/// recorded.
struct Attack {
    attack_id: &'static str,
    account: &'static str,
    /// When the attack started, as an offset from [`BASE_SECS`].
    start_secs: u64,
    /// The sub-second part of the start instant, so two attacks can start within
    /// the same second and still have a total chronological order.
    start_millis: u64,
    /// How long the attack lasted, or `None` while it is still ongoing.
    duration_secs: Option<u64>,
    vectors: &'static [&'static str],
}

impl Attack {
    fn started_at(&self) -> SystemTime {
        at(self.start_secs, self.start_millis)
    }

    fn ended_at(&self) -> Option<SystemTime> {
        self.duration_secs
            .map(|lasted| self.started_at() + Duration::from_secs(lasted))
    }
}

/// The attacks seeded against the policy under test, deliberately NOT inserted
/// in chronological order, so the chronological order the operation serves is
/// the store's doing and not the seeding's.
///
/// `a5` and `a6` start in the same second, half a second apart.
fn attacks() -> Vec<Attack> {
    vec![
        Attack {
            attack_id: "a3-ongoing",
            account: "111111111111",
            start_secs: 3_600,
            start_millis: 0,
            duration_secs: None,
            vectors: &["REQUEST_FLOOD", "HTTP_REFLECTION"],
        },
        Attack {
            attack_id: "a1-first",
            account: "111111111111",
            start_secs: 0,
            start_millis: 0,
            duration_secs: Some(600),
            vectors: &["UDP_TRAFFIC"],
        },
        Attack {
            attack_id: "a6-same-second",
            account: "222222222222",
            start_secs: 10_800,
            start_millis: 500,
            duration_secs: Some(60),
            vectors: &[],
        },
        Attack {
            attack_id: "a2-other-account",
            account: "222222222222",
            start_secs: 1_800,
            start_millis: 0,
            duration_secs: Some(600),
            vectors: &["SYN_FLOOD"],
        },
        Attack {
            attack_id: "a5-long",
            account: "111111111111",
            start_secs: 10_800,
            start_millis: 0,
            duration_secs: Some(3_600),
            vectors: &["REQUEST_FLOOD", "UDP_REFLECTION", "DNS_REFLECTION"],
        },
        Attack {
            attack_id: "a4-brief",
            account: "333333333333",
            start_secs: 7_200,
            start_millis: 0,
            duration_secs: Some(60),
            vectors: &["UDP_REFLECTION"],
        },
    ]
}

/// Every seeded attack of the policy under test, in the chronological order the
/// operation serves them.
const CHRONOLOGICAL: &[&str] = &[
    "a1-first",
    "a2-other-account",
    "a3-ongoing",
    "a4-brief",
    "a5-long",
    "a6-same-second",
];

/// The attack seeded against a SECOND Shield Advanced policy: it must never
/// appear in the first policy's page, whatever the query.
const DECOY_ATTACK_ID: &str = "decoy-of-the-other-policy";

/// A resource type the model lists as valid for a policy of `service_type`, from
/// the `Policy$ResourceType` documentation's "valid resource types for each
/// Firewall Manager policy type" — so the document below is one `PutPolicy`
/// accepts for every type the model declares, not only for Shield Advanced.
fn resource_type_for(service_type: &str) -> &'static str {
    match service_type {
        // WAF Classic (`WAF`), WAF (`WAFV2`) and Shield Advanced all list the
        // v2 load balancer.
        "WAF" | "WAFV2" | "SHIELD_ADVANCED" => "AWS::ElasticLoadBalancingV2::LoadBalancer",
        // "Network ACL"
        "NETWORK_ACL_COMMON" => "AWS::EC2::Subnet",
        // The security-group policies ("usage audit", "content audit", and the
        // common policy the doc's list does not constrain).
        "SECURITY_GROUPS_USAGE_AUDIT" | "SECURITY_GROUPS_CONTENT_AUDIT"
        | "SECURITY_GROUPS_COMMON" => "AWS::EC2::SecurityGroup",
        // "DNS Firewall, Network Firewall, and third-party firewall", and the
        // Network Firewall import variant the doc's list does not constrain.
        _ => "AWS::EC2::VPC",
    }
}

/// A policy document `PutPolicy` accepts, of the given `SecurityServiceType`.
fn policy_of_type(name: &str, service_type: &str) -> Value {
    json!({
        "PolicyName": name,
        "SecurityServicePolicyData": { "Type": service_type },
        "ResourceType": resource_type_for(service_type),
        "IncludeMap": { "ACCOUNT": ["111111111111", "222222222222", "333333333333"] },
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    })
}

/// Store one policy and return the `PolicyId` the service minted for it.
async fn put_policy(router: &MultiServiceRouter, name: &str, service_type: &str) -> String {
    let (status, body) = call(
        router,
        "PutPolicy",
        &json!({ "Policy": policy_of_type(name, service_type) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected PutPolicy to answer 200, got {status} with {body}"
    );
    body["Policy"]["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: PutPolicy reported no PolicyId: {body}"))
        .to_string()
}

/// Put one attack in the store's inventory, under the key the service derives
/// from the record itself.
fn seed_attack(
    store: &mut state::ComAmazonawsFmsAwsfms20180101State,
    policy_id: &str,
    attack: &Attack,
) {
    let record = state::ShieldAttackRecord {
        policy_id: policy_id.to_string(),
        attack_id: attack.attack_id.to_string(),
        account_id: attack.account.to_string(),
        resource_arn: format!(
            "arn:aws:elasticloadbalancing:{REGION}:{}:loadbalancer/app/{}/1",
            attack.account, attack.attack_id
        ),
        attack_vectors: attack.vectors.iter().map(|v| (*v).to_string()).collect(),
        started_at: attack.started_at(),
        ended_at: attack.ended_at(),
    };
    store.shield_attacks.insert(record.key(), record);
}

/// A fresh emulator with an administrator associated, two Shield Advanced
/// policies stored, the attack table seeded against the first and a decoy attack
/// against the second.
///
/// Returns the router, the engine (so a test can read the store back) and the
/// two `PolicyId`s.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String, String) {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let policy_id = put_policy(&router, "shield-under-test", "SHIELD_ADVANCED").await;
    let other_policy_id = put_policy(&router, "shield-other", "SHIELD_ADVANCED").await;
    with_store(&engine, |store| {
        for attack in &attacks() {
            seed_attack(store, &policy_id, attack);
        }
        seed_attack(
            store,
            &other_policy_id,
            &Attack {
                attack_id: DECOY_ATTACK_ID,
                account: "111111111111",
                start_secs: 1_800,
                start_millis: 0,
                duration_secs: Some(60),
                vectors: &["REQUEST_FLOOD"],
            },
        );
    });
    (router, engine, policy_id, other_policy_id)
}

/// The `Data` member parsed as the JSON document it is documented to be.
fn data_entries(case: &str, body: &Value) -> Vec<Value> {
    let data = body["Data"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the response carried no Data string: {body}"));
    match serde_json::from_str::<Value>(data) {
        Ok(Value::Array(entries)) => entries,
        other => panic!("case {case:?}: Data must be a JSON array, got {data:?} ({other:?})"),
    }
}

/// The `AttackId`s of a response's `Data` document, in the order served.
fn reported_ids(case: &str, body: &Value) -> Vec<String> {
    data_entries(case, body)
        .iter()
        .map(|entry| {
            entry["AttackId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a Data entry carried no AttackId: {entry}")
                })
                .to_string()
        })
        .collect()
}

/// One query row: a request's filters, and the attacks they select.
struct Query {
    case: &'static str,
    member_account: Option<&'static str>,
    start: Option<u64>,
    end: Option<u64>,
    expected: &'static [&'static str],
}

/// The query class the filtering rule ranges over.
fn queries() -> Vec<Query> {
    vec![
        Query {
            case: "no filter at all",
            member_account: None,
            start: None,
            end: None,
            expected: CHRONOLOGICAL,
        },
        Query {
            case: "one member account with three attacks",
            member_account: Some("111111111111"),
            start: None,
            end: None,
            expected: &["a1-first", "a3-ongoing", "a5-long"],
        },
        Query {
            case: "a member account with two attacks",
            member_account: Some("222222222222"),
            start: None,
            end: None,
            expected: &["a2-other-account", "a6-same-second"],
        },
        Query {
            case: "a member account no attack names",
            member_account: Some("999999999999"),
            start: None,
            end: None,
            expected: &[],
        },
        Query {
            case: "a lower bound only: an ended attack before it drops out, an ongoing one stays",
            member_account: None,
            start: Some(3_000),
            end: None,
            expected: &["a3-ongoing", "a4-brief", "a5-long", "a6-same-second"],
        },
        Query {
            case: "an upper bound only",
            member_account: None,
            start: None,
            end: Some(1_800),
            expected: &["a1-first", "a2-other-account"],
        },
        Query {
            case: "a one-instant period exactly on an attack's end",
            member_account: None,
            start: Some(600),
            end: Some(600),
            expected: &["a1-first"],
        },
        Query {
            case: "a period bounded exactly by one attack's end and another's start",
            member_account: None,
            start: Some(2_400),
            end: Some(3_600),
            expected: &["a2-other-account", "a3-ongoing"],
        },
        Query {
            case: "a member account and a lower bound together",
            member_account: Some("111111111111"),
            start: Some(3_600),
            end: None,
            expected: &["a3-ongoing", "a5-long"],
        },
        Query {
            case: "an inverted period selects nothing",
            member_account: None,
            start: Some(7_200),
            end: Some(0),
            expected: &[],
        },
    ]
}

/// A `GetProtectionStatus` request for one policy and query row.
fn request(policy_id: &str, query: &Query) -> Value {
    let mut body = json!({ "PolicyId": policy_id });
    if let Some(account) = query.member_account {
        body["MemberAccountId"] = json!(account);
    }
    if let Some(start) = query.start {
        body["StartTime"] = stamp(start);
    }
    if let Some(end) = query.end {
        body["EndTime"] = stamp(end);
    }
    body
}

/// Every query row reports exactly the attacks of the named policy that match
/// its member account and overlap its period, in chronological order, beside the
/// administrator account and the policy's own service type.
#[tokio::test]
async fn get_protection_status_reports_the_attacks_the_query_selects() {
    let (router, _engine, policy_id, _other) = seeded().await;

    for query in queries() {
        let case = query.case;
        let (status, body) = call(&router, "GetProtectionStatus", &request(&policy_id, &query)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            reported_ids(case, &body),
            query.expected,
            "case {case:?}: expected {:?}, got {body}",
            query.expected
        );
        assert_eq!(
            body["AdminAccountId"].as_str(),
            Some(ACCOUNT),
            "case {case:?}: expected the associated administrator {ACCOUNT}, got {body}"
        );
        assert_eq!(
            body["ServiceType"].as_str(),
            Some("SHIELD_ADVANCED"),
            "case {case:?}: expected the stored policy's service type, got {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case:?}: an unbounded page that served everything must carry no NextToken, \
             got {body}"
        );
        assert!(
            !reported_ids(case, &body).iter().any(|id| id == DECOY_ATTACK_ID),
            "case {case:?}: another policy's attack must never be reported, got {body}"
        );
    }
}

/// Each `Data` entry carries the members Firewall Manager documents, read off
/// the stored attack — and an ongoing attack carries no end time at all.
#[tokio::test]
async fn get_protection_status_projects_every_documented_member_of_an_attack() {
    let (router, _engine, policy_id, _other) = seeded().await;

    let (status, body) = call(
        &router,
        "GetProtectionStatus",
        &json!({ "PolicyId": policy_id }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    let entries = data_entries("documented members", &body);
    assert_eq!(
        entries.len(),
        CHRONOLOGICAL.len(),
        "expected one entry per seeded attack, got {body}"
    );

    for attack in attacks() {
        let case = attack.attack_id;
        let entry = entries
            .iter()
            .find(|entry| entry["AttackId"].as_str() == Some(case))
            .unwrap_or_else(|| panic!("case {case:?}: no entry for this attack in {body}"));
        assert_eq!(
            entry["AccountId"].as_str(),
            Some(attack.account),
            "case {case:?}: expected the attacked account {}, got {entry}",
            attack.account
        );
        assert_eq!(
            entry["ResourceArn"].as_str(),
            Some(
                format!(
                    "arn:aws:elasticloadbalancing:{REGION}:{}:loadbalancer/app/{case}/1",
                    attack.account
                )
                .as_str()
            ),
            "case {case:?}: expected the attacked resource's ARN, got {entry}"
        );
        assert_eq!(
            entry["AttackVectors"],
            json!(attack.vectors),
            "case {case:?}: expected the attack vectors {:?}, got {entry}",
            attack.vectors
        );
        assert_eq!(
            entry["StartTime"].as_i64(),
            Some(i64::try_from(BASE_SECS + attack.start_secs).expect("an in-range instant")),
            "case {case:?}: expected the start instant in Unix seconds, got {entry}"
        );
        match attack.duration_secs {
            Some(lasted) => assert_eq!(
                entry["EndTime"].as_i64(),
                Some(
                    i64::try_from(BASE_SECS + attack.start_secs + lasted)
                        .expect("an in-range instant")
                ),
                "case {case:?}: expected the end instant in Unix seconds, got {entry}"
            ),
            None => assert!(
                entry.get("EndTime").is_none(),
                "case {case:?}: an ongoing attack must carry no end time, got {entry}"
            ),
        }
    }
}

/// Read every page of `GetProtectionStatus`, following `NextToken`, and return
/// the attacks in the order they were served.
///
/// Asserts the page invariants on every page: a page is never larger than the
/// effective bound, a page that carries a token is exactly that bound long, and
/// a token appears only while attacks remain.
async fn page_through(
    router: &MultiServiceRouter,
    case: &str,
    policy_id: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let bound = max_results.unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE);
    let mut served: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // Enough iterations for the table at the smallest page size, so a token that
    // never clears fails here instead of looping forever.
    for page in 0..100 {
        let mut body = json!({ "PolicyId": policy_id });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "GetProtectionStatus", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page} must answer 200, got {status} with {response}"
        );
        let ids = reported_ids(case, &response);
        assert!(
            ids.len() <= bound,
            "case {case:?}: page {page} served {} attacks, past the bound of {bound}: {response}",
            ids.len()
        );
        served.extend(ids.iter().cloned());
        next_token = response["NextToken"].as_str().map(str::to_string);
        if next_token.is_some() {
            assert_eq!(
                ids.len(),
                bound,
                "case {case:?}: page {page} carried a NextToken, so it must be a full page of \
                 {bound}, got {response}"
            );
        } else {
            return served;
        }
    }
    panic!("case {case:?}: NextToken never cleared after 100 pages, having served {served:?}");
}

/// Every page size serves the same attacks in the same chronological order, in
/// pages no larger than the bound.
#[tokio::test]
async fn get_protection_status_pages_the_attacks_chronologically() {
    let (router, _engine, policy_id, _other) = seeded().await;

    // Both published bounds of the model's `PaginationMaxResults` range (1 and
    // 100), the sizes around the table's own length, and no bound at all.
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(6), Some(7), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let served = page_through(&router, &case, &policy_id, max_results).await;
        assert_eq!(
            served, CHRONOLOGICAL,
            "case {case:?}: paging must serve every attack once, in chronological order"
        );
    }
}

/// A `MaxResults` past either published bound of the modelled range is refused.
#[tokio::test]
async fn get_protection_status_refuses_a_max_results_outside_the_modelled_range() {
    let (router, _engine, policy_id, _other) = seeded().await;

    for max_results in [BELOW_MIN_MAX_RESULTS, ABOVE_MAX_MAX_RESULTS] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = call(
            &router,
            "GetProtectionStatus",
            &json!({ "PolicyId": policy_id, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside the modelled 1..100 range must be refused with \
             400, got {status} with {body}"
        );
    }
}

/// A policy of any other security-service type is refused: Firewall Manager
/// reports attack data only for a Shield Advanced policy.
#[tokio::test]
async fn get_protection_status_refuses_a_policy_that_is_not_shield_advanced() {
    let (router, _engine, _policy_id, _other) = seeded().await;

    // The catalog is the model's own `SecurityServiceType` enum, as
    // `ops::types::SECURITY_SERVICE_TYPE_VALUES` lists it — so every type the
    // service models is covered, not only the ones an issue cited.
    assert!(
        SECURITY_SERVICE_TYPE_VALUES.contains(&"SHIELD_ADVANCED"),
        "SHIELD_ADVANCED must be one of the modelled service types, got \
         {SECURITY_SERVICE_TYPE_VALUES:?}"
    );
    for service_type in SECURITY_SERVICE_TYPE_VALUES
        .iter()
        .filter(|service_type| **service_type != "SHIELD_ADVANCED")
    {
        let policy_id = put_policy(&router, service_type, service_type).await;
        let (status, body) = call(
            &router,
            "GetProtectionStatus",
            &json!({ "PolicyId": policy_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {service_type:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {service_type:?}: expected the modelled InvalidInputException, got {body}"
        );
    }
}

/// A `PolicyId` that names no policy is the modelled not-found, and a token this
/// operation did not mint for THIS policy is a modelled bad input.
#[tokio::test]
async fn get_protection_status_refuses_an_unknown_policy_and_a_foreign_token() {
    let (router, _engine, policy_id, other_policy_id) = seeded().await;

    let (status, body) = call(
        &router,
        "GetProtectionStatus",
        &json!({ "PolicyId": UNKNOWN_POLICY_ID }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown PolicyId must be refused with 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "an unknown PolicyId must be the modelled not-found, got {body}"
    );

    // The token the OTHER policy's first page hands out: a well-formed token of
    // this service that names no boundary in this policy's attacks.
    let (status, other_page) = call(
        &router,
        "GetProtectionStatus",
        &json!({ "PolicyId": other_policy_id, "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other policy's first page must answer 200, got {status} with {other_page}"
    );
    assert_eq!(
        reported_ids("the other policy's own page", &other_page),
        vec![DECOY_ATTACK_ID.to_string()],
        "the other policy must report its OWN attack, got {other_page}"
    );

    for (case, token) in [
        ("not a token of this service", "not-a-token".to_string()),
        (
            "a token of another policy's page",
            // That page served the decoy and nothing else, so its own key is the
            // only well-formed token of that policy.
            hex_token(&other_policy_id),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetProtectionStatus",
            &json!({ "PolicyId": policy_id, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token that names no boundary of this policy must be refused with \
             400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: expected the modelled InvalidInputException, got {body}"
        );
    }
}

/// A pagination token of this service naming a key of `policy_id`'s range —
/// built through the service's own codec, so the test cannot disagree with it
/// about the encoding.
fn hex_token(policy_id: &str) -> String {
    state::encode_key_token(&state::shield_attack_key(
        policy_id,
        at(1_800, 0),
        DECOY_ATTACK_ID,
    ))
}

/// The read is organisation-scoped: with no administrator association it is
/// refused before any identifier is looked up, in every Region.
#[tokio::test]
async fn get_protection_status_is_gated_on_the_administrator_association() {
    for region in [REGION, OTHER_REGION] {
        let (router, _engine) = emulator_with_engine();
        let (status, body) = call_in(
            &router,
            "GetProtectionStatus",
            &json!({ "PolicyId": UNKNOWN_POLICY_ID }),
            region,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region:?}: with no administrator the read must be refused with 400, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {region:?}: expected the authorization refusal, got {body}"
        );
    }
}

/// The read changes nothing a later caller sees: the same request answers the
/// same page twice, and the store still holds exactly the attacks that were
/// seeded.
#[tokio::test]
async fn get_protection_status_leaves_the_store_untouched() {
    let (router, engine, policy_id, _other) = seeded().await;
    let seeded_keys = with_store(&engine, |store| {
        store.shield_attacks.keys().cloned().collect::<Vec<String>>()
    });

    let mut answers: Vec<Value> = Vec::new();
    for attempt in 0..2 {
        let (status, body) = call(
            &router,
            "GetProtectionStatus",
            &json!({ "PolicyId": policy_id, "MaxResults": 3 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: expected 200, got {status} with {body}"
        );
        answers.push(body);
    }
    assert_eq!(
        answers[0], answers[1],
        "a reporting operation must answer the same page twice, got {:?} then {:?}",
        answers[0], answers[1]
    );
    assert_eq!(
        with_store(&engine, |store| store
            .shield_attacks
            .keys()
            .cloned()
            .collect::<Vec<String>>()),
        seeded_keys,
        "the read must neither add nor drop an attack"
    );
}
