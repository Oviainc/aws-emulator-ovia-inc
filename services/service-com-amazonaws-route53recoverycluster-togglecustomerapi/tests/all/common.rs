// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this target shares: one router with BOTH planes of
//! Route 53 ARC mounted over ONE engine, a request helper per plane, and the fleet the
//! suites build their fixtures from.
//!
//! The fleet is created through the CONTROL plane's own API, because that is the only
//! way a routing control comes into existence: this data-plane service models no create
//! or delete, so everything it serves originates in
//! `route53-recovery-control-config` and is reached here through the shared engine.
//!
//! It lives here so the suites cannot drift apart on how a request is signed, how a
//! fleet is created, or how a page is followed.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_route53recoverycluster_togglecustomerapi::EmulatedService;
use service_com_amazonaws_route53recoverycontrolconfig_route53recoverycontrolconfig::EmulatedService as ControlPlaneService;
use smithy_server_runtime::MultiServiceRouter;

/// The signing name the data-plane (cluster) model gives this service; it is what the
/// router keys a request on.
pub const DATA_PLANE_SIGNING_NAME: &str = "route53-recovery-cluster";

/// The signing name of the control plane that creates what this service serves.
pub const CONTROL_PLANE_SIGNING_NAME: &str = "route53-recovery-control-config";

/// The Region the tests sign for. Recovery-control resource ARNs are account-wide, so
/// it never appears in one.
pub const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is
/// how every signed SDK request routes. Request authentication is off in these tests, so
/// only the scope matters.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEMULATORTEST/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, \
         Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One emulator over its own engine, with both planes mounted: the registry the data
/// plane reads is the store the control plane writes, so a test that creates a resource
/// through one plane addresses it through the other.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ControlPlaneService::new(engine));
    router
}

/// Call one data-plane operation and return its HTTP status and parsed JSON body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{DATA_PLANE_SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("ToggleCustomerAPI.{operation}"))
        .header("authorization", authorization(DATA_PLANE_SIGNING_NAME))
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    dispatch(router, request, operation).await
}

/// Call one control-plane operation (a REST route) and return its status and body.
pub async fn control_plane(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://{CONTROL_PLANE_SIGNING_NAME}.{REGION}.amazonaws.com{path}"
        ))
        .header("content-type", "application/json")
        .header("authorization", authorization(CONTROL_PLANE_SIGNING_NAME))
        .body(payload)
        .expect("a valid request");
    dispatch(router, request, path).await
}

/// Send one request and read its status and JSON body, naming `case` if the body is not
/// JSON at all.
async fn dispatch(
    router: &MultiServiceRouter,
    request: http::Request<bytes::Bytes>,
    case: &str,
) -> (u16, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "case {case}: response body is not JSON ({e}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of a failure body: the `__type` discriminator, without any
/// `namespace#` prefix.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.rsplit('#').next().unwrap_or(t).to_string())
        .unwrap_or_default()
}

/// The `Message` of a failure body.
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A routing control of a fleet, as the control plane minted it.
#[derive(Debug, Clone)]
pub struct Control {
    pub arn: String,
    pub name: String,
    pub panel_arn: String,
    pub cluster_arn: String,
}

/// A control panel of a fleet and the routing controls created in it.
#[derive(Debug, Clone)]
pub struct Panel {
    pub arn: String,
    pub name: String,
    pub cluster_arn: String,
    pub controls: Vec<Control>,
}

/// A fleet created through the control plane: its control panels, in creation order.
#[derive(Debug, Clone)]
pub struct Fleet {
    pub panels: Vec<Panel>,
}

impl Fleet {
    /// Every routing control of the fleet, in the registry's ARN order — which is the
    /// order `ListRoutingControls` reports them in.
    pub fn controls(&self) -> Vec<Control> {
        let mut controls: Vec<Control> = self
            .panels
            .iter()
            .flat_map(|panel| panel.controls.iter().cloned())
            .collect();
        controls.sort_by(|left, right| left.arn.cmp(&right.arn));
        controls
    }

    /// Every routing control ARN of the fleet, in that same order.
    pub fn arns(&self) -> Vec<String> {
        self.controls()
            .into_iter()
            .map(|control| control.arn)
            .collect()
    }

    /// The routing controls hosted on `cluster_arn`, in ARN order.
    pub fn controls_in_cluster(&self, cluster_arn: &str) -> Vec<Control> {
        self.controls()
            .into_iter()
            .filter(|control| control.cluster_arn == cluster_arn)
            .collect()
    }
}

/// Create one cluster and return its `ClusterArn`.
pub async fn create_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) =
        control_plane(router, "POST", "/cluster", Some(json!({ "ClusterName": name }))).await;
    assert_eq!(
        status, 200,
        "case CreateCluster({name}): expected 200, got {status} with {created}"
    );
    created["Cluster"]["ClusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case CreateCluster({name}): a created cluster has an ARN, got {created}"))
        .to_string()
}

/// Create one control panel on `cluster_arn` and return it, with no controls yet.
pub async fn create_control_panel(
    router: &MultiServiceRouter,
    cluster_arn: &str,
    name: &str,
) -> Panel {
    let (status, created) = control_plane(
        router,
        "POST",
        "/controlpanel",
        Some(json!({ "ClusterArn": cluster_arn, "ControlPanelName": name })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateControlPanel({name}): expected 200, got {status} with {created}"
    );
    let arn = created["ControlPanel"]["ControlPanelArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case CreateControlPanel({name}): a created panel has an ARN, got {created}")
        })
        .to_string();
    Panel {
        arn,
        name: name.to_string(),
        cluster_arn: cluster_arn.to_string(),
        controls: Vec::new(),
    }
}

/// Create one routing control in `panel` and return it.
pub async fn create_routing_control(
    router: &MultiServiceRouter,
    panel: &Panel,
    name: &str,
) -> Control {
    let (status, created) = control_plane(
        router,
        "POST",
        "/routingcontrol",
        Some(json!({
            "ClusterArn": panel.cluster_arn,
            "ControlPanelArn": panel.arn,
            "RoutingControlName": name,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateRoutingControl({name}): expected 200, got {status} with {created}"
    );
    let arn = created["RoutingControl"]["RoutingControlArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case CreateRoutingControl({name}): a created control has an ARN, got {created}")
        })
        .to_string();
    Control {
        arn,
        name: name.to_string(),
        panel_arn: panel.arn.clone(),
        cluster_arn: panel.cluster_arn.clone(),
    }
}

/// Delete the routing control under `arn` through the control plane.
pub async fn delete_routing_control(router: &MultiServiceRouter, arn: &str) {
    let (status, body) = control_plane(
        router,
        "DELETE",
        &format!("/routingcontrol/{}", encode_arn(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteRoutingControl({arn}): expected 200, got {status} with {body}"
    );
}

/// Delete the control panel under `arn` through the control plane.
pub async fn delete_control_panel(router: &MultiServiceRouter, arn: &str) {
    let (status, body) = control_plane(
        router,
        "DELETE",
        &format!("/controlpanel/{}", encode_arn(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteControlPanel({arn}): expected 200, got {status} with {body}"
    );
}

/// An ARN of the same shape as `arn` that names no resource: its last character is
/// replaced, which keeps it inside the modelled `Arn` pattern and length.
///
/// The ONE reading of "a well-formed ARN nothing was minted for" every suite here uses,
/// so two of them cannot disagree about what an unknown ARN is.
pub fn unknown_arn_like(arn: &str) -> String {
    let last = arn.chars().next_back().unwrap_or('0');
    let mut shifted: String = arn.chars().take(arn.chars().count() - 1).collect();
    shifted.push(if last == 'f' { 'e' } else { 'f' });
    shifted
}

/// An ARN as one path segment: the characters an ARN carries that are not safe there
/// (`:` and `/`) percent-encoded, which is how an SDK sends it.
pub fn encode_arn(arn: &str) -> String {
    arn.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The standard fleet the suites run against, created through the control plane: two
/// clusters, one of them hosting two control panels, and five routing controls — so the
/// whole modelled surface is reachable (the `ControlPanelArn` filter, a batch update
/// across two panels of ONE cluster, and one spanning two clusters).
pub async fn fleet(router: &MultiServiceRouter) -> Fleet {
    let first_cluster = create_cluster(router, "fleet-one").await;
    let second_cluster = create_cluster(router, "fleet-two").await;

    let mut panels = Vec::new();
    for (cluster_arn, panel_name, control_names) in [
        (
            first_cluster.as_str(),
            "fleet-one-panel-a",
            vec!["control-a1", "control-a2", "control-a3"],
        ),
        (first_cluster.as_str(), "fleet-one-panel-b", vec!["control-b1"]),
        (
            second_cluster.as_str(),
            "fleet-two-panel-c",
            vec!["control-c1"],
        ),
    ] {
        let mut panel = create_control_panel(router, cluster_arn, panel_name).await;
        for control_name in control_names {
            let control = create_routing_control(router, &panel, control_name).await;
            panel.controls.push(control);
        }
        panels.push(panel);
    }

    Fleet { panels }
}

/// The rule configuration a safety rule is created with: the three members of the control
/// plane's modelled `RuleConfig` (`Type`, `Threshold`, `Inverted`).
#[derive(Debug, Clone, Copy)]
pub struct RuleConfigSpec {
    /// A value of the modelled `RuleType` enum: `ATLEAST`, `AND` or `OR`.
    pub rule_type: &'static str,
    pub threshold: i32,
    pub inverted: bool,
}

impl RuleConfigSpec {
    /// This configuration as the `RuleConfig` body the control plane takes.
    fn body(self) -> Value {
        json!({
            "Type": self.rule_type,
            "Threshold": self.threshold,
            "Inverted": self.inverted,
        })
    }
}

/// Create one ASSERTION safety rule over `asserted` and answer its `SafetyRuleArn`, once
/// the control plane reports the rule `DEPLOYED`.
pub async fn create_assertion_rule(
    router: &MultiServiceRouter,
    panel_arn: &str,
    name: &str,
    asserted: &[String],
    config: RuleConfigSpec,
) -> String {
    create_safety_rule(
        router,
        name,
        json!({
            "AssertionRule": {
                "AssertedControls": asserted,
                "ControlPanelArn": panel_arn,
                "Name": name,
                "RuleConfig": config.body(),
                "WaitPeriodMs": 5000,
            }
        }),
        "AssertionRule",
    )
    .await
}

/// Create one GATING safety rule whose `gating` controls gate `targets`, and answer its
/// `SafetyRuleArn` once the control plane reports it `DEPLOYED`.
pub async fn create_gating_rule(
    router: &MultiServiceRouter,
    panel_arn: &str,
    name: &str,
    gating: &[String],
    targets: &[String],
    config: RuleConfigSpec,
) -> String {
    create_safety_rule(
        router,
        name,
        json!({
            "GatingRule": {
                "ControlPanelArn": panel_arn,
                "GatingControls": gating,
                "Name": name,
                "RuleConfig": config.body(),
                "TargetControls": targets,
                "WaitPeriodMs": 5000,
            }
        }),
        "GatingRule",
    )
    .await
}

/// `CreateSafetyRule` with `body`, then poll `DescribeSafetyRule` until the rule's
/// `Status` is `DEPLOYED` — the point from which it guards state updates.
async fn create_safety_rule(
    router: &MultiServiceRouter,
    name: &str,
    body: Value,
    member: &str,
) -> String {
    let (status, created) = control_plane(router, "POST", "/safetyrule", Some(body)).await;
    assert_eq!(
        status, 200,
        "case CreateSafetyRule({name}): expected 200, got {status} with {created}"
    );
    let arn = created[member]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case CreateSafetyRule({name}): a created rule has an ARN, got {created}")
        })
        .to_string();

    // The control plane deploys a created rule asynchronously, so the rule is waited for
    // rather than assumed; the bound makes a rule that never deploys fail the test.
    for _ in 0..400 {
        let (status, described) = control_plane(
            router,
            "GET",
            &format!("/safetyrule/{}", encode_arn(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case DescribeSafetyRule({name}): expected 200, got {status} with {described}"
        );
        if described[member]["Status"].as_str() == Some("DEPLOYED") {
            return arn;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("case CreateSafetyRule({name}): the rule never reached Status DEPLOYED");
}

/// `GetRoutingControlState` for `arn`, expecting success.
pub async fn get_state(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GetRoutingControlState",
        &json!({ "RoutingControlArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetRoutingControlState({arn}): expected 200, got {status} with {body}"
    );
    body
}

/// Page through `ListRoutingControls` with `max_results`, following `NextToken`, and
/// return every `RoutingControl` reported, page by page. `bound` is one more page than
/// the set can need, so a token that never terminates fails the test instead of hanging.
pub async fn list_all_pages(
    router: &MultiServiceRouter,
    control_panel_arn: Option<&str>,
    max_results: Option<u32>,
    bound: usize,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..(bound + 2) {
        let mut request = serde_json::Map::new();
        if let Some(panel) = control_panel_arn {
            request.insert("ControlPanelArn".to_string(), json!(panel));
        }
        if let Some(size) = max_results {
            request.insert("MaxResults".to_string(), json!(size));
        }
        if let Some(token) = &next_token {
            request.insert("NextToken".to_string(), json!(token));
        }
        let case = format!("ListRoutingControls(panel={control_panel_arn:?}, max={max_results:?})");
        let (status, body) = call(router, "ListRoutingControls", &Value::Object(request)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let page = body
            .get("RoutingControls")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: RoutingControls is required, got {body}"))
            .clone();
        if let Some(size) = max_results {
            assert!(
                page.len() <= size as usize,
                "case {case}: a page must not exceed MaxResults, got {} entries",
                page.len()
            );
        }
        collected.extend(page);
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("ListRoutingControls kept issuing a NextToken past the end of the set");
}

/// The `RoutingControlArn` of every entry of a listing, in the order it reported them.
pub fn listed_arns(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|control| {
            control
                .get("RoutingControlArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}
