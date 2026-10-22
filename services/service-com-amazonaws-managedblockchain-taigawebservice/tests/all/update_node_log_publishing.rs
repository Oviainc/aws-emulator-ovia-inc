// SPDX-License-Identifier: CC0-1.0

//! `UpdateNode` end to end over the router: the node configuration parameters a request
//! names are applied to the stored node, and `GetNode` reads them back.
//!
//! The table below spans the class the rule generalises over: WHICH members of
//! `NodeLogPublishingConfiguration` a request names (both log streams, one of them, the
//! `Enabled` leaf alone, an empty nested structure, no configuration at all) crossed with
//! the configuration the node was created with, plus the boolean values `Enabled` can
//! take. The nested member names come from `smithy_query` (shapes
//! `NodeLogPublishingConfiguration` -> `Fabric`, `NodeFabricLogPublishingConfiguration` ->
//! `ChaincodeLogs` / `PeerLogs`, `LogConfigurations` -> `Cloudwatch`, `LogConfiguration` ->
//! `Enabled`).
//!
//! The lookup cases span every id-shaped segment of the request that can name nothing
//! stored, and the member scope that owns no such node.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// Comfortably longer than the service's `TRANSITION_SPAN`, so a network that has to be
/// `AVAILABLE` before it can take a node has settled by the time the create is made.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// One answer from the emulator.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

/// A router with only this service mounted, over a fresh engine.
///
/// The sibling suite's harness is private to its own module (a test target's suites are
/// separate modules), so this one keeps the few calls it needs.
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

/// Create one Fabric network, wait out its `CREATING` span, and return
/// `(NetworkId, MemberId)`.
async fn available_network(router: &MultiServiceRouter, token: &str) -> (String, String) {
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
        "MemberConfiguration": {
            "Name": "org1",
            "FrameworkConfiguration": {
                "Fabric": { "AdminUsername": "AdminUser", "AdminPassword": "MyPassword1" }
            }
        }
    });
    let answer = call(router, "POST", "/networks", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case {token}: expected CreateNetwork 200, got {} {:?}",
        answer.status, answer.body
    );
    let ids = (
        answer.body["NetworkId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        answer.body["MemberId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    ids
}

/// Create one node whose `NodeConfiguration` carries `logs` (when given), and return its
/// `NodeId`.
async fn create_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
    logs: Option<Value>,
) -> String {
    let mut configuration = json!({ "InstanceType": "bc.t3.small" });
    if let Some(logs) = logs {
        configuration["LogPublishingConfiguration"] = logs;
    }
    let body = json!({
        "ClientRequestToken": token,
        "MemberId": member_id,
        "NodeConfiguration": configuration,
    });
    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {body:?}: expected CreateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["NodeId"].as_str().unwrap_or_default().to_string()
}

/// `GetNode`'s view of one node's `LogPublishingConfiguration` (`Value::Null` when the
/// node carries none).
async fn stored_logs(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> Value {
    let answer = call(
        router,
        "GET",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case GetNode {node_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Node"]["LogPublishingConfiguration"].clone()
}

/// A `NodeLogPublishingConfiguration` naming the log streams given, each with its
/// CloudWatch `Enabled`.
fn logs(chaincode: Option<bool>, peer: Option<bool>) -> Value {
    let mut fabric = json!({});
    if let Some(enabled) = chaincode {
        fabric["ChaincodeLogs"] = json!({ "Cloudwatch": { "Enabled": enabled } });
    }
    if let Some(enabled) = peer {
        fabric["PeerLogs"] = json!({ "Cloudwatch": { "Enabled": enabled } });
    }
    json!({ "Fabric": fabric })
}

/// One row: what the node was created with, what the update names, and what `GetNode`
/// must report afterwards.
#[derive(Debug)]
struct Case {
    name: &'static str,
    initial: Option<Value>,
    patch: Option<Value>,
    expected: Value,
}

/// `UpdateNode` applies the configuration members a request names and leaves every member
/// it does not name as it was — over a table of created and requested configurations.
#[tokio::test]
async fn update_node_applies_the_parameters_it_names_and_keeps_the_rest() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "upd-logs").await;

    let cases: Vec<Case> = vec![
        Case {
            name: "both streams from a node created without any configuration",
            initial: None,
            patch: Some(logs(Some(true), Some(true))),
            expected: logs(Some(true), Some(true)),
        },
        Case {
            name: "both streams turned off",
            initial: Some(logs(Some(true), Some(true))),
            patch: Some(logs(Some(false), Some(false))),
            expected: logs(Some(false), Some(false)),
        },
        Case {
            name: "only the chaincode stream named leaves the peer stream alone",
            initial: Some(logs(Some(true), Some(true))),
            patch: Some(logs(Some(false), None)),
            expected: logs(Some(false), Some(true)),
        },
        Case {
            name: "only the peer stream named leaves the chaincode stream alone",
            initial: Some(logs(Some(false), Some(false))),
            patch: Some(logs(None, Some(true))),
            expected: logs(Some(false), Some(true)),
        },
        Case {
            name: "a stream the create never named is added by the update",
            initial: Some(logs(None, Some(false))),
            patch: Some(logs(Some(true), None)),
            expected: logs(Some(true), Some(false)),
        },
        Case {
            name: "an empty Fabric structure names nothing and changes nothing",
            initial: Some(logs(Some(true), Some(false))),
            patch: Some(json!({ "Fabric": {} })),
            expected: logs(Some(true), Some(false)),
        },
        Case {
            name: "an empty configuration names nothing and changes nothing",
            initial: Some(logs(Some(true), Some(false))),
            patch: Some(json!({})),
            expected: logs(Some(true), Some(false)),
        },
        Case {
            name: "an omitted Enabled keeps the stored setting",
            initial: Some(logs(Some(true), Some(true))),
            patch: Some(json!({ "Fabric": { "PeerLogs": { "Cloudwatch": {} } } })),
            expected: logs(Some(true), Some(true)),
        },
        Case {
            name: "no configuration at all changes nothing",
            initial: Some(logs(Some(false), Some(true))),
            patch: None,
            expected: logs(Some(false), Some(true)),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let node_id = create_node(
            &router,
            &network_id,
            &member_id,
            &format!("upd-node-{index}"),
            case.initial.clone(),
        )
        .await;

        let mut body = json!({ "MemberId": member_id });
        if let Some(patch) = &case.patch {
            body["LogPublishingConfiguration"] = patch.clone();
        }
        let answer = call(
            &router,
            "PATCH",
            &format!("/networks/{network_id}/nodes/{node_id}"),
            Some(body),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected UpdateNode 200, got {} {:?}",
            case.name, answer.status, answer.body
        );

        let actual = stored_logs(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            actual, case.expected,
            "case {}: expected the stored configuration {:?}, got {actual:?}",
            case.name, case.expected
        );
    }
}

/// The update is persisted, not momentary: a later `UpdateNode` builds on the record the
/// previous one left, and every read in between answers from it.
#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_node() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "upd-seq").await;
    let node_id = create_node(&router, &network_id, &member_id, "upd-seq-node", None).await;

    // Rows: one stream at a time, so each step's expectation includes what the earlier
    // steps wrote.
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "enable the peer stream",
            logs(None, Some(true)),
            json!({ "Fabric": { "PeerLogs": { "Cloudwatch": { "Enabled": true } } } }),
        ),
        (
            "enable the chaincode stream too",
            logs(Some(true), None),
            logs(Some(true), Some(true)),
        ),
        (
            "turn the peer stream back off",
            logs(None, Some(false)),
            logs(Some(true), Some(false)),
        ),
    ];

    for (case, patch, expected) in &steps {
        let answer = call(
            &router,
            "PATCH",
            &format!("/networks/{network_id}/nodes/{node_id}"),
            Some(json!({ "MemberId": member_id, "LogPublishingConfiguration": patch })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateNode 200, got {} {:?}",
            answer.status, answer.body
        );
        let actual = stored_logs(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            actual, *expected,
            "case {case}: expected the stored configuration {expected:?}, got {actual:?}"
        );
    }
}

/// `UpdateNode` addresses the node by the ids the request carries: one that names nothing
/// stored — or a member that does not own the node — is the modelled
/// `ResourceNotFoundException`, and nothing is written.
#[tokio::test]
async fn an_id_that_names_nothing_stored_is_a_404_and_writes_nothing() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "upd-404").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "upd-404-node",
        Some(logs(Some(true), Some(true))),
    )
    .await;

    // Ids shaped exactly as this service mints them (prefix, hyphen, 26 base-32
    // characters, within `ResourceIdString`'s @length(1, 32)) but naming nothing stored.
    let absent_network = "n-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let absent_node = "nd-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let absent_member = "m-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

    // Rows differ along WHICH segment of the request names nothing stored.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "unknown network",
            format!("/networks/{absent_network}/nodes/{node_id}"),
            json!({ "MemberId": member_id, "LogPublishingConfiguration": logs(Some(false), None) }),
        ),
        (
            "unknown node",
            format!("/networks/{network_id}/nodes/{absent_node}"),
            json!({ "MemberId": member_id, "LogPublishingConfiguration": logs(Some(false), None) }),
        ),
        (
            "a member that owns no such node",
            format!("/networks/{network_id}/nodes/{node_id}"),
            json!({
                "MemberId": absent_member,
                "LogPublishingConfiguration": logs(Some(false), None)
            }),
        ),
    ];

    for (case, path, body) in &cases {
        let answer = call(&router, "PATCH", path, Some(body.clone())).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // None of the refusals touched the node.
    let actual = stored_logs(&router, &network_id, &member_id, &node_id).await;
    assert_eq!(
        actual,
        logs(Some(true), Some(true)),
        "case after the refusals: expected the created configuration to be untouched, \
         got {actual:?}"
    );
}

/// `UpdateNodeInput$MemberId` is documented "Applies only to Hyperledger Fabric" WITHOUT
/// the "and is required for Hyperledger Fabric" clause `GetNodeInput$MemberId`,
/// `ListNodesInput$MemberId` and `DeleteNodeInput$MemberId` carry, so a member named on an
/// update only scopes the lookup: an update that names none still addresses the node.
#[tokio::test]
async fn the_owning_member_is_optional_on_an_update() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "upd-scope").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "upd-scope-node",
        Some(logs(Some(false), Some(false))),
    )
    .await;

    // Rows: the update with the owning member named, and the same update with no member
    // at all. Both apply.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "scoped to the owning member",
            json!({
                "MemberId": member_id,
                "LogPublishingConfiguration": logs(Some(true), None)
            }),
            logs(Some(true), Some(false)),
        ),
        (
            "no member named",
            json!({ "LogPublishingConfiguration": logs(None, Some(true)) }),
            logs(Some(true), Some(true)),
        ),
    ];

    for (case, body, expected) in &cases {
        let answer = call(
            &router,
            "PATCH",
            &format!("/networks/{network_id}/nodes/{node_id}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateNode 200, got {} {:?}",
            answer.status, answer.body
        );
        let actual = stored_logs(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            actual, *expected,
            "case {case}: expected the stored configuration {expected:?}, got {actual:?}"
        );
    }
}
