// SPDX-License-Identifier: CC0-1.0

//! The wire harness and fixtures every suite in this target shares.
//!
//! One emulator behind the same router the binary boots, driven with awsJson1_0
//! `POST /` requests, plus the readers and request fixtures a test needs. Suites
//! `use crate::harness::…` rather than standing one up each.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in these tests is signed for. ARNs must carry it,
/// which is how a region other than the emulator default proves the ARN is built
/// from the request rather than from a constant.
pub const REGION: &str = "eu-west-1";

/// The account segment every emulator ARN carries, sourced from the vendored
/// constant so an ARN assertion cannot drift from it.
pub const ACCOUNT: &str = emulator_server::EMULATOR_ACCOUNT_ID;

/// One emulator: a single shared engine behind the router every test calls.
pub struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    /// Stand up the router with this service mounted on a fresh engine.
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one operation and return its HTTP status and parsed JSON body.
    pub async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://network-firewall.eu-west-1.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("NetworkFirewall_20201112.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/network-firewall/\
                     aws4_request, SignedHeaders=host, Signature=abc"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Send one operation that must succeed, and return its body.
    pub async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Create a firewall policy with the minimal valid document.
    pub async fn create_policy(&self, name: &str) -> Value {
        self.ok(
            "CreateFirewallPolicy",
            &json!({ "FirewallPolicyName": name, "FirewallPolicy": minimal_policy_document() }),
        )
        .await
    }

    /// Create a firewall policy and return its ARN.
    pub async fn shared_policy(&self, name: &str) -> String {
        let created = self.create_policy(name).await;
        string_at(&created, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string()
    }

    /// Create a rule group of `group_type` carrying one rule, and return its ARN.
    pub async fn shared_rule_group(&self, name: &str, group_type: &str, capacity: i64) -> String {
        let document = if group_type == "STATELESS" {
            json!({ "RulesSource": { "StatelessRulesAndCustomActions": {
                "StatelessRules": [{
                    "Priority": 1,
                    "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
                }],
            }}})
        } else {
            json!({ "RulesSource": { "StatefulRules": [{
                "Action": "PASS",
                "Header": {
                    "Protocol": "TCP",
                    "Source": "ANY",
                    "SourcePort": "ANY",
                    "Direction": "ANY",
                    "Destination": "ANY",
                    "DestinationPort": "ANY",
                },
                "RuleOptions": [{ "Keyword": "sid", "Settings": ["1"] }],
            }]}})
        };
        let created = self
            .ok(
                "CreateRuleGroup",
                &json!({
                    "RuleGroupName": name,
                    "Type": group_type,
                    "Capacity": capacity,
                    "RuleGroup": document,
                }),
            )
            .await;
        string_at(&created, &["RuleGroupResponse", "RuleGroupArn"]).to_string()
    }
}

/// A minimal valid `FirewallPolicy` document: the two members the model marks
/// required (`StatelessDefaultActions`, `StatelessFragmentDefaultActions`).
pub fn minimal_policy_document() -> Value {
    json!({
        "StatelessDefaultActions": ["aws:forward_to_sfe"],
        "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
    })
}

/// A resource-policy document granting exactly `actions` on `resource` to this
/// account, in the form the AWS `PutResourcePolicy` documentation shows.
pub fn resource_policy_document(actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": ACCOUNT },
            "Action": actions,
            "Resource": resource,
        }],
    })
    .to_string()
}

/// The awsJson error code (`__type`) of an error body.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Whether `value` satisfies the `ResourceId` / `UpdateToken` pattern
/// `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$` (length 36).
pub fn is_uuid_form(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// Read a required string out of a response body, naming the path on failure.
pub fn string_at<'a>(body: &'a Value, path: &[&str]) -> &'a str {
    let mut cursor = body;
    for step in path {
        cursor = cursor
            .get(step)
            .unwrap_or_else(|| panic!("expected {path:?} in {body:?}"));
    }
    cursor
        .as_str()
        .unwrap_or_else(|| panic!("expected {path:?} to be a string in {body:?}"))
}
