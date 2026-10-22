// SPDX-License-Identifier: CC0-1.0

//! What a Domain response reports BESIDE its modelled members, and what a deletion
//! reports.
//!
//! Three rules, each asserted over a TABLE of domains that differ along what the rule
//! generalises over — which optional members the create named, how many tags it carried,
//! how many subscriptions the domain holds, and the region the request was signed for.
//! The recorded case (a minimal `{"name": …}` create) is one row of each table.
//!
//! 1. Every Domain-record response projects the domain's own owning account and its
//!    onboarding generation out of state, and every later read reports the same values.
//! 2. A response that reports a resource's tags always carries the map — EMPTY for an
//!    untagged resource, never absent.
//! 3. `DeleteDomain` reports the status the domain actually reached and the instant the
//!    deletion was recorded, which is stable under a repeated delete.
//!
//! The harness (`Emulator`, `error_code`, `string_at`) is the sibling suite's: one way
//! of driving this service through signed restJson1 requests, not two.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The onboarding generations this service records — one per provisioning path it
/// implements (`crate::state::OnboardingGeneration`). A domain's generation is always
/// one of these.
const ONBOARDING_GENERATIONS: [&str; 1] = ["LEGACY"];

/// `DomainStatus`'s terminal and non-terminal deletion values
/// (`smithy_query(query="shape", shape="DomainStatus")`: ACTIVE | DELETING | DELETED).
const DELETED: &str = "DELETED";
const DELETING: &str = "DELETING";

/// Long enough for the `DELETING` -> purged transition `state::DOMAIN_DELETION_SPAN`
/// owes to have fallen due.
const PAST_THE_DELETION_SPAN: Duration = Duration::from_millis(900);

/// A `CreateWebAppConfiguration` (`ehrRole`, `idcInstanceId`, `idcRegion`, all required
/// when the member is supplied — `smithy_query(query="shape",
/// shape="CreateWebAppConfiguration")`): what a create names to ask for a web
/// application, the one resource a domain owns outside this service.
fn web_app_setup() -> Value {
    json!({
        "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
        "idcInstanceId": "ssoins-72230ed02aa4b4ea",
        "idcRegion": "us-east-1",
    })
}

/// A `tags` map of `count` distinct entries.
fn tags(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), Value::String(format!("value-{index}")));
    }
    Value::Object(map)
}

/// The account segment of an ARN — the fifth field of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
fn arn_account(arn: &str) -> String {
    arn.split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("an ARN with an account segment, got {arn:?}"))
        .to_string()
}

/// A required number member of a response, as epoch seconds.
fn seconds_at(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("member {member} must be a number in {body:?}"))
}

/// Now, as epoch seconds — the clock the handlers stamp their timestamps from.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// Assert that `body` carries this domain's own account and a known onboarding
/// generation, and answer the pair so a later read can be compared with it.
fn account_and_generation(case: &str, label: &str, body: &Value) -> (String, String) {
    let arn = string_at(body, "arn");
    let account_id = string_at(body, "accountId");
    assert!(
        account_id.len() == 12 && account_id.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?} ({label}): accountId must be a 12-digit account, got {account_id:?}"
    );
    assert_eq!(
        account_id,
        arn_account(&arn),
        "case {case:?} ({label}): accountId must be the account that owns the domain, \
         the one its ARN names, got {body:?}"
    );
    let generation = string_at(body, "onboardingGeneration");
    assert!(
        ONBOARDING_GENERATIONS.contains(&generation.as_str()),
        "case {case:?} ({label}): onboardingGeneration must be one of \
         {ONBOARDING_GENERATIONS:?}, got {generation:?}"
    );
    (account_id, generation)
}

/// The create requests the account / generation rule is asserted over: the optional
/// members present, the size of `name`, the number of tags, and the signing region all
/// vary; row 1 is the recorded minimal create.
fn account_projection_cases() -> Vec<(&'static str, &'static str, Value)> {
    let kms_key = "arn:aws:kms:us-west-2:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789";
    vec![
        ("minimal", "us-east-1", json!({ "name": "clinic-a" })),
        (
            "name at the modelled maximum (64)",
            "eu-west-2",
            json!({ "name": "c".repeat(64) }),
        ),
        (
            "customer-managed key",
            "us-west-2",
            json!({ "name": "clinic-kms", "kmsKeyArn": kms_key }),
        ),
        (
            "one tag",
            "ap-southeast-2",
            json!({ "name": "clinic-tag", "tags": tags(1) }),
        ),
        (
            "web app set up",
            "us-west-2",
            json!({ "name": "clinic-web", "webAppSetupConfiguration": web_app_setup() }),
        ),
        (
            "every optional member at once",
            "eu-central-1",
            json!({
                "name": "clinic-everything",
                "kmsKeyArn": kms_key,
                "tags": tags(3),
                "webAppSetupConfiguration": web_app_setup(),
            }),
        ),
    ]
}

#[tokio::test]
async fn domain_responses_project_the_owning_account_and_the_onboarding_generation() {
    for (case, region, request) in account_projection_cases() {
        let emulator = Emulator::new(region);
        let created = emulator.create_domain(&request).await;
        let (account_id, generation) = account_and_generation(case, "CreateDomain", &created);
        assert!(
            created.get("deletedAt").is_none(),
            "case {case:?}: a live domain carries no deletion instant, got {created:?}"
        );

        // The read-back path reports the SAME values: they are the domain's own, held
        // in state, not recomputed per response.
        let domain_id = string_at(&created, "domainId");
        let path = format!("/domain/{domain_id}");
        for read in 1..=2 {
            let (status, fetched) = emulator.send("GET", &path, None).await;
            assert_eq!(status, 200, "case {case:?}: GetDomain got {fetched:?}");
            assert_eq!(
                account_and_generation(case, "GetDomain", &fetched),
                (account_id.clone(), generation.clone()),
                "case {case:?}: read {read} of the domain must report the account and \
                 generation the create recorded, got {fetched:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_read_always_reports_the_tag_map_and_reports_it_empty_when_there_are_none() {
    // (case, the `tags` the create supplies, the map the read must report)
    let cases: Vec<(&str, Option<Value>, Value)> = vec![
        ("no tags member at all", None, json!({})),
        ("an empty tags member", Some(json!({})), json!({})),
        ("one tag", Some(tags(1)), tags(1)),
        ("three tags", Some(tags(3)), tags(3)),
        ("fifty tags (the per-resource maximum)", Some(tags(50)), tags(50)),
    ];

    let emulator = Emulator::new("us-east-1");
    for (case, supplied, expected) in cases {
        let mut request = json!({ "name": "clinic-tags" });
        if let Some(supplied) = supplied {
            request["tags"] = supplied;
        }
        let created = emulator.create_domain(&request).await;
        let domain_id = string_at(&created, "domainId");
        let (status, fetched) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(status, 200, "case {case:?}: GetDomain got {fetched:?}");
        assert_eq!(
            fetched.get("tags"),
            Some(&expected),
            "case {case:?}: the tag map must be present and hold exactly the stored \
             tags, got {fetched:?}"
        );
    }
}

/// One row of the deletion table: the case, the create request, how many subscriptions
/// the domain holds when it is deleted, and the status the delete must report.
struct DeletionCase {
    case: &'static str,
    request: Value,
    subscriptions: usize,
    status: &'static str,
}

#[tokio::test]
async fn delete_domain_reports_the_status_it_reached_and_when_the_deletion_was_recorded() {
    // The rows differ along what the rule generalises over: what the domain owns. Only
    // a web application is provisioned OUTSIDE this service, so only a domain that has
    // one passes through the non-terminal DELETING; everything else a domain holds —
    // its tags, its key, its subscriptions — are records of this store, removed by the
    // deleting call itself, so its deletion is complete when the call answers.
    let kms_key = "arn:aws:kms:us-east-1:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789";
    let cases = vec![
        DeletionCase {
            case: "minimal",
            request: json!({ "name": "clinic-a" }),
            subscriptions: 0,
            status: DELETED,
        },
        DeletionCase {
            case: "tagged",
            request: json!({ "name": "clinic-tagged", "tags": tags(2) }),
            subscriptions: 0,
            status: DELETED,
        },
        DeletionCase {
            case: "customer-managed key",
            request: json!({ "name": "clinic-kms", "kmsKeyArn": kms_key }),
            subscriptions: 0,
            status: DELETED,
        },
        DeletionCase {
            case: "two subscriptions",
            request: json!({ "name": "clinic-subs" }),
            subscriptions: 2,
            status: DELETED,
        },
        DeletionCase {
            case: "web app set up",
            request: json!({
                "name": "clinic-web",
                "webAppSetupConfiguration": web_app_setup(),
            }),
            subscriptions: 0,
            status: DELETING,
        },
        DeletionCase {
            case: "web app and a subscription",
            request: json!({
                "name": "clinic-web-subs",
                "webAppSetupConfiguration": web_app_setup(),
            }),
            subscriptions: 1,
            status: DELETING,
        },
    ];

    for DeletionCase {
        case,
        request,
        subscriptions,
        status: expected_status,
    } in cases
    {
        let emulator = Emulator::new("us-east-1");
        let created = emulator.create_domain(&request).await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");
        let created_at = seconds_at(&created, "createdAt");
        let path = format!("/domain/{domain_id}");
        for index in 0..subscriptions {
            let (status, body) = emulator
                .send(
                    "POST",
                    &format!("/domains/{domain_id}/subscriptions"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?}: subscription {index} got {status} {body:?}"
            );
        }

        let before = epoch_now();
        let (status, deleted) = emulator.send("DELETE", &path, None).await;
        let after = epoch_now();
        assert_eq!(status, 200, "case {case:?}: DeleteDomain got {deleted:?}");
        assert_eq!(
            (
                deleted.get("domainId").and_then(Value::as_str),
                deleted.get("arn").and_then(Value::as_str),
                deleted.get("status").and_then(Value::as_str),
            ),
            (
                Some(domain_id.as_str()),
                Some(arn.as_str()),
                Some(expected_status)
            ),
            "case {case:?}: DeleteDomain must report the domain it deleted and the \
             status that deletion reached, got {deleted:?}"
        );

        // The deletion instant is when this call recorded the deletion: after the
        // domain was created, and inside the window the call was served in.
        let deleted_at = seconds_at(&deleted, "deletedAt");
        assert!(
            deleted_at >= created_at,
            "case {case:?}: deletedAt {deleted_at} must not precede createdAt \
             {created_at}"
        );
        assert!(
            (before..=after).contains(&deleted_at),
            "case {case:?}: deletedAt must be the instant the delete was served, \
             in [{before}, {after}], got {deleted_at}"
        );

        if expected_status == DELETED {
            // A deletion that completed is terminal: the domain is gone, so a read of
            // it and a second delete both answer the modelled not-found error.
            for (label, method) in [("GetDomain", "GET"), ("DeleteDomain", "DELETE")] {
                let (status, body) = emulator.send(method, &path, None).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: {label} on a domain whose deletion completed got \
                     {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: {label} on a deleted domain got {body:?}"
                );
            }
            continue;
        }

        // A deletion still in flight is observable, and `@idempotent`: repeating the
        // delete reports the same status and the same instant it first recorded.
        let (status, fetched) = emulator.send("GET", &path, None).await;
        assert_eq!(status, 200, "case {case:?}: GetDomain got {fetched:?}");
        assert_eq!(
            fetched.get("status").and_then(Value::as_str),
            Some(DELETING),
            "case {case:?}: GetDomain must report the deletion in flight, got {fetched:?}"
        );
        let (status, repeated) = emulator.send("DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated DeleteDomain got {repeated:?}"
        );
        assert_eq!(
            repeated, deleted,
            "case {case:?}: a repeated DeleteDomain must answer exactly as the first \
             did, got {repeated:?}"
        );

        // Once the span the clock owes has elapsed the deletion is applied, and the
        // domain is gone. Real elapsed time is required (the handlers read
        // `SystemTime::now()`) and the workspace `tokio` dependency does not enable the
        // `time` feature, so this is a plain blocking sleep.
        std::thread::sleep(PAST_THE_DELETION_SPAN);
        let (status, body) = emulator.send("GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: GetDomain after the deletion fell due got {status} {body:?}"
        );
    }
}
