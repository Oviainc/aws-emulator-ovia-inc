// SPDX-License-Identifier: CC0-1.0

//! `UpdateMember` end to end over the router: the member configuration parameters a
//! request names are applied to the stored member, and `GetMember` reads them back.
//!
//! The table below spans the class the rule generalises over: WHICH members of
//! `MemberLogPublishingConfiguration` a request names (the `Enabled` leaf, the CloudWatch
//! structure, the log stream, the framework structure, nothing at all) crossed with the
//! configuration the member was created with, plus the boolean values `Enabled` can take.
//! The nested member names come from `smithy_query` (shapes
//! `MemberLogPublishingConfiguration` -> `Fabric`,
//! `MemberFabricLogPublishingConfiguration` -> `CaLogs`, `LogConfigurations` ->
//! `Cloudwatch`, `LogConfiguration` -> `Enabled`).
//!
//! The lookup cases span every id-shaped segment of the request that can name nothing
//! stored.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// One answer from the emulator.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

/// A router with only this service mounted, over a fresh engine.
///
/// The sibling suites' harnesses are private to their own modules (a test target's suites
/// are separate modules), so this one keeps the few calls it needs.
fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and return what the emulator answered.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://managedblockchain.{REGION}.amazonaws.com{path}"
        ))
        .header("content-type", "application/json")
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/managedblockchain/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Create one Fabric network whose founder `MemberConfiguration` carries `logs` (when
/// given), and return `(NetworkId, MemberId)`.
///
/// A member is created with the network that holds it, so one network per row is how a row
/// gets a member of its own to update.
async fn network_with_member(
    router: &MultiServiceRouter,
    token: &str,
    logs: Option<Value>,
) -> (String, String) {
    let mut member_configuration = json!({
        "Name": format!("org-{token}"),
        "FrameworkConfiguration": {
            "Fabric": { "AdminUsername": "AdminUser", "AdminPassword": "MyPassword1" }
        }
    });
    if let Some(logs) = logs {
        member_configuration["LogPublishingConfiguration"] = logs;
    }
    let body = json!({
        "ClientRequestToken": token,
        "Name": format!("net-{token}"),
        "Framework": "HYPERLEDGER_FABRIC",
        "FrameworkVersion": "2.2",
        "FrameworkConfiguration": { "Fabric": { "Edition": "STANDARD" } },
        "VotingPolicy": {
            "ApprovalThresholdPolicy": {
                "ThresholdPercentage": 50,
                "ProposalDurationInHours": 24,
                "ThresholdComparator": "GREATER_THAN"
            }
        },
        "MemberConfiguration": member_configuration,
    });
    let answer = call(router, "POST", "/networks", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case {token}: expected CreateNetwork 200, got {} {:?}",
        answer.status, answer.body
    );
    (
        answer.body["NetworkId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        answer.body["MemberId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// `GetMember`'s view of one member's `LogPublishingConfiguration` (`Value::Null` when the
/// member carries none).
async fn stored_logs(router: &MultiServiceRouter, network_id: &str, member_id: &str) -> Value {
    let answer = call(
        router,
        "GET",
        &format!("/networks/{network_id}/members/{member_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case GetMember {member_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Member"]["LogPublishingConfiguration"].clone()
}

/// A `MemberLogPublishingConfiguration` naming the member's CA log stream with the
/// CloudWatch `Enabled` given, or an empty `Fabric` structure when it names none.
fn logs(ca: Option<bool>) -> Value {
    match ca {
        Some(enabled) => json!({ "Fabric": { "CaLogs": { "Cloudwatch": { "Enabled": enabled } } } }),
        None => json!({ "Fabric": {} }),
    }
}

/// One `UpdateMember` call: `PATCH /networks/{NetworkId}/members/{MemberId}`, naming the
/// configuration given (and nothing at all when it is `None`).
async fn update_member(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    patch: Option<&Value>,
) -> Answer {
    let mut body = json!({});
    if let Some(patch) = patch {
        body["LogPublishingConfiguration"] = patch.clone();
    }
    call(
        router,
        "PATCH",
        &format!("/networks/{network_id}/members/{member_id}"),
        Some(body),
    )
    .await
}

/// One row: what the member was created with, what the update names, and what `GetMember`
/// must report afterwards.
#[derive(Debug)]
struct Case {
    name: &'static str,
    initial: Option<Value>,
    patch: Option<Value>,
    expected: Value,
}

/// `UpdateMember` applies the configuration members a request names and leaves every
/// member it does not name as it was — over a table of created and requested
/// configurations.
#[tokio::test]
async fn update_member_applies_the_parameters_it_names_and_keeps_the_rest() {
    let router = new_router();

    let cases: Vec<Case> = vec![
        Case {
            name: "the CA log stream turned on for a member created without any configuration",
            initial: None,
            patch: Some(logs(Some(true))),
            expected: logs(Some(true)),
        },
        Case {
            name: "the CA log stream turned off",
            initial: Some(logs(Some(true))),
            patch: Some(logs(Some(false))),
            expected: logs(Some(false)),
        },
        Case {
            name: "the CA log stream turned back on",
            initial: Some(logs(Some(false))),
            patch: Some(logs(Some(true))),
            expected: logs(Some(true)),
        },
        Case {
            name: "a stream the create never named is added by the update",
            initial: Some(logs(None)),
            patch: Some(logs(Some(true))),
            expected: logs(Some(true)),
        },
        Case {
            name: "an empty Fabric structure names nothing and changes nothing",
            initial: Some(logs(Some(true))),
            patch: Some(json!({ "Fabric": {} })),
            expected: logs(Some(true)),
        },
        Case {
            name: "an empty configuration names nothing and changes nothing",
            initial: Some(logs(Some(false))),
            patch: Some(json!({})),
            expected: logs(Some(false)),
        },
        Case {
            name: "a log stream named without its CloudWatch parameters keeps the stored one",
            initial: Some(logs(Some(true))),
            patch: Some(json!({ "Fabric": { "CaLogs": {} } })),
            expected: logs(Some(true)),
        },
        Case {
            name: "an omitted Enabled keeps the stored setting",
            initial: Some(logs(Some(true))),
            patch: Some(json!({ "Fabric": { "CaLogs": { "Cloudwatch": {} } } })),
            expected: logs(Some(true)),
        },
        Case {
            name: "no configuration at all changes nothing",
            initial: Some(logs(Some(false))),
            patch: None,
            expected: logs(Some(false)),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let (network_id, member_id) =
            network_with_member(&router, &format!("upd-member-{index}"), case.initial.clone()).await;

        let answer = update_member(&router, &network_id, &member_id, case.patch.as_ref()).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected UpdateMember 200, got {} {:?}",
            case.name, answer.status, answer.body
        );
        assert_eq!(
            answer.body,
            json!({}),
            "case {}: expected an empty UpdateMember body, got {:?}",
            case.name,
            answer.body
        );

        let actual = stored_logs(&router, &network_id, &member_id).await;
        assert_eq!(
            actual, case.expected,
            "case {}: expected the stored configuration {:?}, got {actual:?}",
            case.name, case.expected
        );
    }
}

/// The update is persisted, not momentary: a later `UpdateMember` builds on the record the
/// previous one left, and every read in between answers from it.
#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_member() {
    let router = new_router();
    let (network_id, member_id) = network_with_member(&router, "upd-member-seq", None).await;

    // Rows: one setting at a time, so each step's expectation includes what the earlier
    // steps wrote.
    let steps: Vec<(&str, Value, Value)> = vec![
        ("enable the CA log stream", logs(Some(true)), logs(Some(true))),
        (
            "an update that names nothing leaves it enabled",
            json!({}),
            logs(Some(true)),
        ),
        (
            "turn the CA log stream back off",
            logs(Some(false)),
            logs(Some(false)),
        ),
    ];

    for (case, patch, expected) in &steps {
        let answer = update_member(&router, &network_id, &member_id, Some(patch)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateMember 200, got {} {:?}",
            answer.status, answer.body
        );
        let actual = stored_logs(&router, &network_id, &member_id).await;
        assert_eq!(
            actual, *expected,
            "case {case}: expected the stored configuration {expected:?}, got {actual:?}"
        );
    }
}

/// `UpdateMember` addresses the member by the ids the request carries: one that names
/// nothing stored is the modelled `ResourceNotFoundException`, and nothing is written.
///
/// The path nests the member inside the network, so the labels resolve left to right: an
/// unknown network is the missing resource even when the member id is one this service
/// holds elsewhere.
#[tokio::test]
async fn an_id_that_names_nothing_stored_is_a_404_and_writes_nothing() {
    let router = new_router();
    let (network_id, member_id) =
        network_with_member(&router, "upd-member-404", Some(logs(Some(true)))).await;
    // A second network, so a member id that exists but belongs elsewhere is a row too.
    let (other_network, other_member) =
        network_with_member(&router, "upd-member-404-other", None).await;

    // Ids shaped exactly as this service mints them (prefix, hyphen, 26 base-32
    // characters, within `ResourceIdString`'s @length(1, 32)) but naming nothing stored.
    let absent_network = "n-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let absent_member = "m-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

    // Rows differ along WHICH segment of the request names nothing stored.
    let cases: Vec<(&str, String)> = vec![
        (
            "unknown network",
            format!("/networks/{absent_network}/members/{member_id}"),
        ),
        (
            "unknown member",
            format!("/networks/{network_id}/members/{absent_member}"),
        ),
        (
            "both unknown",
            format!("/networks/{absent_network}/members/{absent_member}"),
        ),
        (
            "a member of another network",
            format!("/networks/{network_id}/members/{other_member}"),
        ),
    ];

    for (case, path) in &cases {
        let answer = call(
            &router,
            "PATCH",
            path,
            Some(json!({ "LogPublishingConfiguration": logs(Some(false)) })),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // None of the refusals touched either member.
    let actual = stored_logs(&router, &network_id, &member_id).await;
    assert_eq!(
        actual,
        logs(Some(true)),
        "case after the refusals: expected the created configuration to be untouched, \
         got {actual:?}"
    );
    let untouched = stored_logs(&router, &other_network, &other_member).await;
    assert_eq!(
        untouched,
        Value::Null,
        "case after the refusals: expected the other network's member to carry no \
         configuration, got {untouched:?}"
    );
}
