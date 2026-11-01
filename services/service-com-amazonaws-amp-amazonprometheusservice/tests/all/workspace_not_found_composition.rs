// SPDX-License-Identifier: CC0-1.0

//! The ONE refusal every operation addressed by `/workspaces/{workspaceId}` answers when
//! that label resolves to no workspace, end to end over HTTP.
//!
//! The rule under test: an operation whose `{workspaceId}` names no workspace this
//! account holds in the Region the request was signed for answers the modelled
//! `ResourceNotFoundException` (404) composed ONE way — that operation's constant
//! sentence in `message`, the id the caller named in `resourceId`, and the workspace's
//! AWS type in `resourceType` — whichever operation was called, whatever form the
//! unresolved id takes and whether the Region holds other workspaces or nothing at all.
//!
//! The table below carries one row per operation the model binds under
//! `/workspaces/{workspaceId}` and which resolves that parent, so every handler that
//! answers this refusal is driven through it — `CreateAnomalyDetector` and
//! `PutAnomalyDetector` among them, the two that used to compose it in a private helper
//! of their own. The two suites cross that table with the two ways a workspace fails to
//! resolve: an id this Region's store does not hold, and a Region whose store holds
//! nothing at all.
//!
//! The harness (`Amp`, `Reply`, `text`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` builds, and the detector request bodies come from
//! `tests/all/anomaly_detector_update.rs`. The sentences and the type name are LITERALS
//! here, so the wire values are what is under test: re-wording a constant in the service
//! cannot keep this suite green.

use crate::anomaly_detector_update::Spec;
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use serde_json::{json, Value};

/// `ResourceNotFoundException$resourceType` ("Type of the resource affected") for a
/// workspace: the AWS type name the model itself gives the resource —
/// `WorkspaceArn` carries `@arnReference(type: "AWS::APS::Workspace")`.
const WORKSPACE_RESOURCE_TYPE: &str = "AWS::APS::Workspace";

/// The sentence the workspace-addressed operations answer for a workspace this account
/// does not hold.
const WORKSPACE_NOT_FOUND: &str = "Workspace not found.";

/// `DescribeWorkspace`'s own recorded spelling of it, without the trailing period.
const DESCRIBE_WORKSPACE_NOT_FOUND: &str = "Workspace not found";

/// An `anomalyDetectorId` inside `AnomalyDetectorId`'s modelled pattern
/// `^ad-[0-9A-Za-z][-.0-9A-Z_a-z]*$` — the label the detector-addressed operations carry
/// beside the workspace, which no store here ever minted.
const ABSENT_DETECTOR_ID: &str = "ad-00000000-0000-4000-8000-000000000000";

/// A `name` inside `RuleGroupsNamespaceName`'s modelled pattern
/// `^[0-9A-Za-z][-.0-9A-Z_a-z]*$`, which no workspace here holds.
const ABSENT_NAMESPACE: &str = "absent-namespace";

/// A CloudWatch log group ARN inside `LogGroupArn`'s modelled pattern
/// `^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\.\-\_\#/]{1,512}\:\*$`.
fn log_group_arn(region: &str) -> String {
    format!(
        "arn:aws:logs:{region}:{}:log-group:/aws/prometheus/parent:*",
        engine::emulator_account_id()
    )
}

/// `LoggingDestinations` is modelled `len 1..1`, so one `cloudWatchLogs` destination.
fn destinations(region: &str) -> Value {
    json!([{
        "cloudWatchLogs": { "logGroupArn": log_group_arn(region) },
        "filters": { "qspThreshold": 100 },
    }])
}

/// A resource-based policy document: `policyDocument` is modelled a plain `String`, so
/// only its being well-formed JSON matters to a caller.
fn policy_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "Parent",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": ["aps:QueryMetrics"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A rules file the service accepts, as `data` carries it: a blob, so it travels
/// base64-encoded the way the wire serializer encodes one.
fn rules_file() -> String {
    aws_smithy_types::base64::encode(b"groups: []")
}

/// An alert manager definition the service accepts, in the form
/// `AlertManagerDefinitionData` documents — a YAML mapping whose `alertmanager_config`
/// entry holds the Alertmanager configuration.
fn alert_manager_definition() -> String {
    aws_smithy_types::base64::encode(
        b"alertmanager_config: |\n  route:\n    receiver: default\n  receivers:\n    - name: default\n",
    )
}

/// One operation addressed by `/workspaces/{workspaceId}`: what to call, what to send,
/// and the sentence that operation answers when the label resolves to nothing.
struct Call {
    operation: &'static str,
    method: &'static str,
    /// What follows `/workspaces/{workspaceId}` in the modelled URI.
    suffix: String,
    body: String,
    message: &'static str,
}

/// Every operation the model binds under `/workspaces/{workspaceId}` which resolves that
/// workspace — one row each, carrying a request the service accepts in every other
/// respect, so what the row asserts is the parent refusal and nothing else.
fn workspace_addressed_calls(region: &str) -> Vec<Call> {
    let definition = json!({ "data": alert_manager_definition() }).to_string();
    let logging = json!({ "logGroupArn": log_group_arn(region) }).to_string();
    let query_logging = json!({ "destinations": destinations(region) }).to_string();
    let policy = json!({ "policyDocument": policy_document() }).to_string();
    let detector_spec = Spec::new("sum(rate(requests_total[5m]))");

    let rows: Vec<(&'static str, &'static str, String, String, &'static str)> = vec![
        (
            "CreateAlertManagerDefinition",
            "POST",
            "/alertmanager/definition".to_string(),
            definition.clone(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "CreateAnomalyDetector",
            "POST",
            "/anomalydetectors".to_string(),
            detector_spec.create_body("parentless", &[]).to_string(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "CreateLoggingConfiguration",
            "POST",
            "/logging".to_string(),
            logging.clone(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "CreateQueryLoggingConfiguration",
            "POST",
            "/logging/query".to_string(),
            query_logging.clone(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "CreateRuleGroupsNamespace",
            "POST",
            "/rulegroupsnamespaces".to_string(),
            json!({ "name": ABSENT_NAMESPACE, "data": rules_file() }).to_string(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DeleteAlertManagerDefinition",
            "DELETE",
            "/alertmanager/definition".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DeleteAnomalyDetector",
            "DELETE",
            format!("/anomalydetectors/{ABSENT_DETECTOR_ID}"),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DeleteLoggingConfiguration",
            "DELETE",
            "/logging".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DeleteQueryLoggingConfiguration",
            "DELETE",
            "/logging/query".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DeleteResourcePolicy",
            "DELETE",
            "/policy".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DeleteRuleGroupsNamespace",
            "DELETE",
            format!("/rulegroupsnamespaces/{ABSENT_NAMESPACE}"),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DeleteWorkspace",
            "DELETE",
            String::new(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DescribeAlertManagerDefinition",
            "GET",
            "/alertmanager/definition".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DescribeAnomalyDetector",
            "GET",
            format!("/anomalydetectors/{ABSENT_DETECTOR_ID}"),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DescribeLoggingConfiguration",
            "GET",
            "/logging".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DescribeQueryLoggingConfiguration",
            "GET",
            "/logging/query".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DescribeResourcePolicy",
            "GET",
            "/policy".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "DescribeRuleGroupsNamespace",
            "GET",
            format!("/rulegroupsnamespaces/{ABSENT_NAMESPACE}"),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            // The one operation with a recorded sentence of its own.
            "DescribeWorkspace",
            "GET",
            String::new(),
            String::new(),
            DESCRIBE_WORKSPACE_NOT_FOUND,
        ),
        (
            "DescribeWorkspaceConfiguration",
            "GET",
            "/configuration".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "ListAnomalyDetectors",
            "GET",
            "/anomalydetectors".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "ListRuleGroupsNamespaces",
            "GET",
            "/rulegroupsnamespaces".to_string(),
            String::new(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "PutAlertManagerDefinition",
            "PUT",
            "/alertmanager/definition".to_string(),
            definition,
            WORKSPACE_NOT_FOUND,
        ),
        (
            "PutAnomalyDetector",
            "PUT",
            format!("/anomalydetectors/{ABSENT_DETECTOR_ID}"),
            detector_spec.put_body().to_string(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "PutResourcePolicy",
            "PUT",
            "/policy".to_string(),
            policy,
            WORKSPACE_NOT_FOUND,
        ),
        (
            "PutRuleGroupsNamespace",
            "PUT",
            format!("/rulegroupsnamespaces/{ABSENT_NAMESPACE}"),
            json!({ "data": rules_file() }).to_string(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "UpdateLoggingConfiguration",
            "PUT",
            "/logging".to_string(),
            logging,
            WORKSPACE_NOT_FOUND,
        ),
        (
            "UpdateQueryLoggingConfiguration",
            "PUT",
            "/logging/query".to_string(),
            query_logging,
            WORKSPACE_NOT_FOUND,
        ),
        (
            "UpdateWorkspaceAlias",
            "POST",
            "/alias".to_string(),
            json!({ "alias": "renamed" }).to_string(),
            WORKSPACE_NOT_FOUND,
        ),
        (
            "UpdateWorkspaceConfiguration",
            "PATCH",
            "/configuration".to_string(),
            json!({ "retentionPeriodInDays": 30 }).to_string(),
            WORKSPACE_NOT_FOUND,
        ),
    ];

    rows.into_iter()
        .map(|(operation, method, suffix, body, message)| Call {
            operation,
            method,
            suffix,
            body,
            message,
        })
        .collect()
}

/// Every member of the refusal, as one composition answers it: the modelled status and
/// code, the sentence this operation reports, the id the caller named in `resourceId`,
/// the workspace's AWS type in `resourceType` — and nothing of the request inside the
/// sentence.
fn assert_one_composition(case: &str, reply: &Reply, message: &str, workspace_id: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: an unresolved workspace must answer 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
    );
    let reported = text(&reply.body["message"]);
    assert_eq!(
        reported, message,
        "case {case:?}: expected {message:?}, got {reported:?}"
    );
    assert_eq!(
        text(&reply.body["resourceId"]),
        workspace_id,
        "case {case:?}: the id the caller named belongs in resourceId, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceType"]),
        WORKSPACE_RESOURCE_TYPE,
        "case {case:?}: expected resourceType {WORKSPACE_RESOURCE_TYPE:?}, got {reply:?}"
    );
    assert!(
        !reported.contains(workspace_id),
        "case {case:?}: the sentence must carry nothing from the request, got {reported:?}"
    );
}

/// An id this Region's store does not hold: every operation addressed by
/// `/workspaces/{workspaceId}` refuses it with the one composition, whatever form the id
/// takes and however it came to be unresolvable.
#[tokio::test]
async fn an_unresolved_workspace_is_refused_alike_by_every_operation() {
    let region = "us-east-2";
    let amp = Amp::new();

    // A workspace that was held and has finished deleting, beside two ids no store ever
    // minted, in different forms — so the rule is not read off one id's shape.
    let deleted = amp.active_workspace(region, "doomed").await;
    let accepted = amp.delete(region, &deleted, None).await;
    assert_eq!(
        accepted.status, 202,
        "the deletion must be accepted, got {accepted:?}"
    );
    wait_for_due_transition();

    // A workspace this Region DOES hold, so each row below is refused for the id it
    // names rather than for the Region holding nothing — that is the next suite.
    let held = amp.active_workspace(region, "held").await;
    assert_ne!(
        held, deleted,
        "the two workspaces must be different records, got {held:?}"
    );

    let absent = [
        ("a workspace that finished deleting", deleted),
        (
            "an id no store ever minted",
            "ws-00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("an id carrying no ws- prefix", "absent".to_string()),
    ];

    for (history, workspace_id) in &absent {
        for call in workspace_addressed_calls(region) {
            let case = format!("{} on {history}", call.operation);
            let reply = amp
                .call(
                    call.method,
                    &format!("/workspaces/{workspace_id}{}", call.suffix),
                    region,
                    &call.body,
                )
                .await;
            assert_one_composition(&case, &reply, call.message, workspace_id);
        }
    }
}

/// A Region this account holds nothing in holds no workspace either, so the same
/// composition answers there — including for the operations whose own Region lookup is a
/// separate branch from their workspace lookup.
#[tokio::test]
async fn a_region_holding_nothing_is_refused_alike_by_every_operation() {
    // The store of this Region is never brought into being: nothing is created in it.
    let empty_region = "ap-northeast-3";
    // One Region that DOES hold a workspace, so the emptiness under test is the
    // Region's and not the whole store's.
    let busy_region = "eu-west-1";

    let amp = Amp::new();
    let elsewhere = amp.active_workspace(busy_region, "elsewhere").await;

    // The id of a workspace held in ANOTHER Region, beside one no store ever minted:
    // neither resolves from an endpoint of the empty Region.
    let absent = [
        ("a workspace held in another Region", elsewhere),
        (
            "an id no store ever minted",
            "ws-11111111-1111-4111-8111-111111111111".to_string(),
        ),
    ];

    for (origin, workspace_id) in &absent {
        for call in workspace_addressed_calls(empty_region) {
            let case = format!("{} in an empty Region on {origin}", call.operation);
            let reply = amp
                .call(
                    call.method,
                    &format!("/workspaces/{workspace_id}{}", call.suffix),
                    empty_region,
                    &call.body,
                )
                .await;
            assert_one_composition(&case, &reply, call.message, workspace_id);
        }
    }
}
