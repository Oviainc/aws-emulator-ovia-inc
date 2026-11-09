// SPDX-License-Identifier: CC0-1.0

//! How every flow operation words the `NotFoundException` for a `{FlowArn}` label that
//! resolved no flow, end to end over HTTP.
//!
//! The subject is one rule: a label that satisfies the `@pattern` the flow operations'
//! `FlowArn` members declare (`^arn:.+:mediaconnect.+:flow:.+$`,
//! `smithy_query(query="shape",shape="DescribeFlowRequest")`) reached the flow lookup and
//! resolved nothing, so the refusal names THAT ARN; a label that does not satisfy it is no
//! flow ARN, reached no lookup, and the refusal carries the service's fixed reason for a
//! path that resolved nothing instead of echoing the caller's own text back.
//!
//! The rows therefore vary what the rule generalises over: how a label fails to be a flow
//! ARN (no `arn:` prefix at all, an empty partition field, another service's ARN, another
//! resource type of this service, a truncated flow ARN, a flow ARN with nothing after
//! `:flow:`), how a flow ARN can resolve nothing (never minted, another partition, an
//! account field outside the narrow shape pattern, a flow that was deleted), and which
//! operation the label travels to — every operation that takes a flow ARN as a path label.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! door uses, so what these tests assert is what a MediaConnect client sees.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// The canonical placeholder account id every ARN this emulator mints carries.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// One operation that takes a flow ARN as a path label: its name, method, and URI template
/// with `{}` where the label goes, plus the body it needs to reach its flow lookup.
///
/// A sub-resource label in the same URI (`OutputArn`, `SourceArn`, `EntitlementArn`)
/// carries its own modelled `@pattern` that the boundary walk enforces, so each row below
/// supplies one of that form: the request must get PAST the boundary for the flow lookup to
/// be what refuses it.
struct FlowCall {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation whose `{FlowArn}` label this service resolves itself.
///
/// DescribeFlowSourceMetadata and DescribeFlowSourceThumbnail are NOT here: their `FlowArn`
/// members declare no pattern of their own, so the narrow `FlowArn` shape pattern is the
/// only one the model states for them and the boundary walk refuses a label outside it
/// before any lookup (see the note on `DescribeFlowInput` in the crate's `ops::types`).
fn flow_calls() -> Vec<FlowCall> {
    let output_arn = format!("arn:aws:mediaconnect:{REGION}:{}:output:1-0000beef:out", account());
    let source_arn = format!("arn:aws:mediaconnect:{REGION}:{}:source:1-0000beef:src", account());
    let entitlement_arn =
        format!("arn:aws:mediaconnect:{REGION}:{}:entitlement:1-0000beef:ent", account());
    vec![
        FlowCall {
            operation: "DescribeFlow",
            method: "GET",
            uri: "/v1/flows/{}".to_string(),
            body: None,
        },
        FlowCall {
            operation: "DeleteFlow",
            method: "DELETE",
            uri: "/v1/flows/{}".to_string(),
            body: None,
        },
        FlowCall {
            operation: "StartFlow",
            method: "POST",
            uri: "/v1/flows/start/{}".to_string(),
            body: None,
        },
        FlowCall {
            operation: "StopFlow",
            method: "POST",
            uri: "/v1/flows/stop/{}".to_string(),
            body: None,
        },
        FlowCall {
            operation: "UpdateFlow",
            method: "PUT",
            uri: "/v1/flows/{}".to_string(),
            body: Some(json!({ "flowSize": "MEDIUM" })),
        },
        FlowCall {
            operation: "AddFlowOutputs",
            method: "POST",
            uri: "/v1/flows/{}/outputs".to_string(),
            body: Some(json!({ "outputs": [
                { "name": "out", "protocol": "rtp", "destination": "203.0.113.5", "port": 5000 }
            ] })),
        },
        FlowCall {
            operation: "AddFlowSources",
            method: "POST",
            uri: "/v1/flows/{}/source".to_string(),
            body: Some(json!({ "sources": [
                { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            ] })),
        },
        FlowCall {
            operation: "AddFlowMediaStreams",
            method: "POST",
            uri: "/v1/flows/{}/mediaStreams".to_string(),
            body: Some(json!({ "mediaStreams": [
                { "mediaStreamId": 1, "mediaStreamName": "ms", "mediaStreamType": "video" }
            ] })),
        },
        FlowCall {
            operation: "AddFlowVpcInterfaces",
            method: "POST",
            uri: "/v1/flows/{}/vpcInterfaces".to_string(),
            body: Some(json!({ "vpcInterfaces": [{
                "name": "vpc",
                "roleArn": format!("arn:aws:iam::{}:role/MediaConnect", account()),
                "securityGroupIds": ["sg-00000000"],
                "subnetId": "subnet-00000000"
            }] })),
        },
        FlowCall {
            operation: "GrantFlowEntitlements",
            method: "POST",
            uri: "/v1/flows/{}/entitlements".to_string(),
            body: Some(json!({ "entitlements": [
                { "name": "ent", "subscribers": ["111122223333"] }
            ] })),
        },
        FlowCall {
            operation: "RemoveFlowOutput",
            method: "DELETE",
            uri: format!("/v1/flows/{{}}/outputs/{output_arn}"),
            body: None,
        },
        FlowCall {
            operation: "RemoveFlowSource",
            method: "DELETE",
            uri: format!("/v1/flows/{{}}/source/{source_arn}"),
            body: None,
        },
        FlowCall {
            operation: "RemoveFlowMediaStream",
            method: "DELETE",
            uri: "/v1/flows/{}/mediaStreams/ms".to_string(),
            body: None,
        },
        FlowCall {
            operation: "RemoveFlowVpcInterface",
            method: "DELETE",
            uri: "/v1/flows/{}/vpcInterfaces/vpc".to_string(),
            body: None,
        },
        FlowCall {
            operation: "RevokeFlowEntitlement",
            method: "DELETE",
            uri: format!("/v1/flows/{{}}/entitlements/{entitlement_arn}"),
            body: None,
        },
        FlowCall {
            operation: "UpdateFlowOutput",
            method: "PUT",
            uri: format!("/v1/flows/{{}}/outputs/{output_arn}"),
            body: Some(json!({ "description": "d" })),
        },
        FlowCall {
            operation: "UpdateFlowSource",
            method: "PUT",
            uri: format!("/v1/flows/{{}}/source/{source_arn}"),
            body: Some(json!({ "description": "d" })),
        },
        FlowCall {
            operation: "UpdateFlowMediaStream",
            method: "PUT",
            uri: "/v1/flows/{}/mediaStreams/ms".to_string(),
            body: Some(json!({ "description": "d" })),
        },
        FlowCall {
            operation: "UpdateFlowEntitlement",
            method: "PUT",
            uri: format!("/v1/flows/{{}}/entitlements/{entitlement_arn}"),
            body: Some(json!({ "description": "d" })),
        },
    ]
}

/// One `{FlowArn}` label that resolves no flow, and the identifier the refusal must name.
struct LabelCase {
    case: &'static str,
    label: String,
    /// `true` when the label is a flow ARN (so the refusal names it), `false` when it is
    /// not one at all (so the refusal carries the fixed reason).
    is_flow_arn: bool,
}

/// The ways a label can fail to be a flow ARN, and the ways a flow ARN can resolve nothing.
fn label_cases() -> Vec<LabelCase> {
    vec![
        LabelCase {
            case: "a label that is not an ARN at all",
            label: "not-a-valid-flow-arn".to_string(),
            is_flow_arn: false,
        },
        LabelCase {
            case: "a bare flow name",
            label: "my-contribution-feed".to_string(),
            is_flow_arn: false,
        },
        LabelCase {
            case: "an ARN prefix with nothing after it",
            label: "arn:".to_string(),
            is_flow_arn: false,
        },
        LabelCase {
            case: "an ARN with an empty partition field",
            label: format!("arn::mediaconnect:{REGION}:{}:flow:1-0000beef:f", account()),
            is_flow_arn: false,
        },
        LabelCase {
            case: "another service's ARN",
            label: format!("arn:aws:medialive:{REGION}:{}:channel:1234567", account()),
            is_flow_arn: false,
        },
        LabelCase {
            case: "another resource type of this service",
            label: format!("arn:aws:mediaconnect:{REGION}:{}:gateway:1-0000beef:gw", account()),
            is_flow_arn: false,
        },
        LabelCase {
            case: "a flow ARN truncated before its resource type",
            label: format!("arn:aws:mediaconnect:{REGION}:{}", account()),
            is_flow_arn: false,
        },
        LabelCase {
            case: "a flow ARN with nothing after its resource type",
            label: format!("arn:aws:mediaconnect:{REGION}:{}:flow:", account()),
            is_flow_arn: false,
        },
        LabelCase {
            case: "a flow ARN of this service's own form that names no flow",
            label: format!("arn:aws:mediaconnect:{REGION}:{}:flow:1-0000dead:never-minted", account()),
            is_flow_arn: true,
        },
        LabelCase {
            case: "a flow ARN in another partition",
            label: "arn:aws-us-gov:mediaconnect:us-gov-west-1:000000000000:flow:1-0000dead:gov"
                .to_string(),
            is_flow_arn: true,
        },
        LabelCase {
            case: "a flow ARN whose account field is not twelve digits",
            label: format!("arn:aws:mediaconnect:{REGION}:acct:flow:1-0000dead:f"),
            is_flow_arn: true,
        },
        LabelCase {
            case: "a flow ARN in another Region",
            label: format!("arn:aws:mediaconnect:eu-west-2:{}:flow:1-0000dead:elsewhere", account()),
            is_flow_arn: true,
        },
    ]
}

/// The message the service's not-found envelope carries for `case`: the ARN when the label
/// is a flow ARN, and the fixed reason for a path that resolved nothing when it is not.
fn expected_message(case: &LabelCase) -> String {
    if case.is_flow_arn {
        format!("{}{}", mc::NOT_FOUND_PREFIX, case.label)
    } else {
        format!(
            "{}{}",
            mc::NOT_FOUND_PREFIX,
            mc::UNRESOLVED_PATH_NOT_FOUND_REASON
        )
    }
}

/// Every flow operation refuses an unresolvable `{FlowArn}` label with 404
/// `NotFoundException`, and the message is the ARN that resolved nothing — or, for a label
/// that is no flow ARN at all, the fixed reason, with none of the caller's own text in it.
#[tokio::test]
async fn an_unresolvable_flow_label_is_refused_in_one_wording() {
    let emulator = Emulator::new();

    for label_case in label_cases() {
        let expected = expected_message(&label_case);
        let case = label_case.case;
        if !label_case.is_flow_arn {
            assert!(
                !expected.contains(&label_case.label),
                "case {case:?}: a label that is no flow ARN must not be echoed, expected \
                 {expected:?}"
            );
        }
        for FlowCall {
            operation,
            method,
            uri,
            body,
        } in flow_calls()
        {
            let uri = uri.replace("{}", &label_case.label);
            let (status, code, response) = emulator.send(method, &uri, body).await;
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?} / {operation}: expected 404 NotFoundException, got {status} \
                 {code:?}: {response}"
            );
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?} / {operation}: expected {expected:?}, got {response}"
            );
        }
    }
}

/// The positive control for the same rule, through the operations the cases above drive:
/// a label the store DOES hold is served, and the flow it names is still there afterwards —
/// so the wording rule did not come at the cost of resolving a real label.
#[tokio::test]
async fn a_label_the_store_holds_is_still_served() {
    let emulator = Emulator::new();
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "resolvable-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture flow must be created, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a label the store holds is served, got {status} {code:?}: {described}"
    );
    assert_eq!(
        described["flow"]["flowArn"].as_str(),
        Some(flow_arn.as_str()),
        "the served flow is the one the label named, got {described}"
    );

    // A flow that EXISTED and does not any more is an ARN that resolved nothing, so the
    // refusal names it: the deleted flow's own ARN, not the generic reason.
    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "deleting the fixture flow expected 202, got {status} {code:?}: {deleted}"
    );
    let (status, code, gone) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "a deleted flow's ARN resolves nothing, got {status} {code:?}: {gone}"
    );
    assert_eq!(
        gone["message"].as_str(),
        Some(format!("{}{flow_arn}", mc::NOT_FOUND_PREFIX).as_str()),
        "the refusal names the ARN that resolved nothing, got {gone}"
    );
}
