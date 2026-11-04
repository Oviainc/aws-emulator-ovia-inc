// SPDX-License-Identifier: CC0-1.0

//! Who may act on the resource an ARN names.
//!
//! The rule under test: every operation that takes a caller-supplied ARN for one of this
//! service's resources decides authorization from the REQUEST alone, before it resolves
//! the ARN — so an ARN whose account id is not the caller's is refused with
//! `AccessDeniedException` (403), naming the principal that signed, the IAM action it
//! asked for and the ARN it asked for, and the answer never says whether a resource is
//! stored there. An ARN of the caller's own account is left to that lookup, which is what
//! answers `ResourceNotFoundException` (404) when it holds nothing
//! (`tests/all/not_found_message.rs`).
//!
//! The tables span what that rule generalises over: every implemented operation that
//! carries such an ARN — read, delete, update, list and the two creates that reference a
//! parent — all three resource levels, both ARN members of the create that carries two,
//! an ARN bound to the URI and one carried in the body or the query string, and three
//! foreign accounts (including the account swapped on an ARN this service really minted,
//! so the refusal is demonstrably not about the resource being absent).

use serde_json::{json, Value};

use super::common::{call, create_cluster, enc, error_code, router};

/// The account the emulator's own credential belongs to: the caller of every request in
/// this suite.
const CALLER_ACCOUNT: &str = "000000000000";

/// The IAM user that credential speaks for, as AWS names a principal in an authorization
/// refusal: `arn:aws:iam::<account>:user/<name>`.
const CALLER_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The namespace this service's IAM actions are published under: the model's
/// `arnNamespace` (`smithy_query(query="service_info")`).
const ACTION_NAMESPACE: &str = "route53-recovery-control-config";

/// Accounts that are not the caller's, so an ARN built for one of them is another
/// account's resource.
const FOREIGN_ACCOUNTS: &[&str] = &["123456789012", "999999999999", "210987654321"];

/// The sentence AWS refuses an unauthorized request with, restated here independently of
/// the service's own code: the principal, the fully qualified action and the resource, all
/// as the request carried them.
fn not_authorized(action: &str, resource: &str) -> String {
    format!(
        "User: {CALLER_PRINCIPAL} is not authorized to perform: {ACTION_NAMESPACE}:{action} \
         on resource: {resource} because no resource-based policy allows the \
         {ACTION_NAMESPACE}:{action} action"
    )
}

/// A recovery-control ARN: AWS writes them with an empty Region segment.
fn arn(account: &str, tail: &str) -> String {
    format!("arn:aws:route53-recovery-control::{account}:{tail}")
}

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="AccessDeniedException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One request to make, with the action and resource its refusal must name.
struct Case {
    label: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    action: &'static str,
    resource: String,
}

/// Every implemented request that carries a cluster ARN, for the cluster ARN `cluster_arn`.
///
/// `own_cluster_arn` is a cluster of the CALLER's account, used by the row that puts a
/// foreign ARN in the create's OTHER ARN member.
fn cases_for(cluster_arn: &str, own_cluster_arn: &str) -> Vec<Case> {
    let panel_arn = cluster_arn.replace("cluster/", "controlpanel/");
    let control_arn = format!("{panel_arn}/routingcontrol/control");
    vec![
        Case {
            label: format!("DescribeCluster({cluster_arn})"),
            method: "GET",
            path: format!("/cluster/{}", enc(cluster_arn)),
            body: None,
            action: "DescribeCluster",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("DeleteCluster({cluster_arn})"),
            method: "DELETE",
            path: format!("/cluster/{}", enc(cluster_arn)),
            body: None,
            action: "DeleteCluster",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("UpdateCluster({cluster_arn})"),
            method: "PUT",
            path: "/cluster".to_string(),
            body: Some(json!({ "ClusterArn": cluster_arn, "NetworkType": "DUALSTACK" })),
            action: "UpdateCluster",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("ListControlPanels(ClusterArn={cluster_arn})"),
            method: "GET",
            path: format!("/controlpanels?ClusterArn={}", enc(cluster_arn)),
            body: None,
            action: "ListControlPanels",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("CreateControlPanel(ClusterArn={cluster_arn})"),
            method: "POST",
            path: "/controlpanel".to_string(),
            body: Some(json!({ "ClusterArn": cluster_arn, "ControlPanelName": "panel" })),
            action: "CreateControlPanel",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("CreateRoutingControl(ClusterArn={cluster_arn})"),
            method: "POST",
            path: "/routingcontrol".to_string(),
            body: Some(json!({ "ClusterArn": cluster_arn, "RoutingControlName": "control" })),
            action: "CreateRoutingControl",
            resource: cluster_arn.to_string(),
        },
        Case {
            // The create's OTHER ARN member, with a cluster the caller does own: every
            // ARN the request carries is authorized, not just the first.
            label: format!("CreateRoutingControl(ControlPanelArn={panel_arn})"),
            method: "POST",
            path: "/routingcontrol".to_string(),
            body: Some(json!({
                "ClusterArn": own_cluster_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": "control"
            })),
            action: "CreateRoutingControl",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("DescribeControlPanel({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}", enc(&panel_arn)),
            body: None,
            action: "DescribeControlPanel",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("DeleteControlPanel({panel_arn})"),
            method: "DELETE",
            path: format!("/controlpanel/{}", enc(&panel_arn)),
            body: None,
            action: "DeleteControlPanel",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("ListRoutingControls({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}/routingcontrols", enc(&panel_arn)),
            body: None,
            action: "ListRoutingControls",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("ListSafetyRules({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}/safetyrules", enc(&panel_arn)),
            body: None,
            action: "ListSafetyRules",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("UpdateControlPanel({panel_arn})"),
            method: "PUT",
            path: "/controlpanel".to_string(),
            body: Some(json!({ "ControlPanelArn": panel_arn, "ControlPanelName": "renamed" })),
            action: "UpdateControlPanel",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("DescribeRoutingControl({control_arn})"),
            method: "GET",
            path: format!("/routingcontrol/{}", enc(&control_arn)),
            body: None,
            action: "DescribeRoutingControl",
            resource: control_arn.clone(),
        },
        Case {
            label: format!("DeleteRoutingControl({control_arn})"),
            method: "DELETE",
            path: format!("/routingcontrol/{}", enc(&control_arn)),
            body: None,
            action: "DeleteRoutingControl",
            resource: control_arn,
        },
    ]
}

/// RULE: an ARN of another account is refused with `AccessDeniedException` (403) naming
/// the principal, the action and the ARN — on every operation that carries one.
#[tokio::test]
async fn an_arn_of_another_account_is_refused_with_access_denied() {
    let router = router();

    // A cluster of the caller's own account exists throughout: a 403 below is about the
    // ARN the request names and not about an empty store, and the rows that need a
    // second, valid ARN use this one.
    let own = create_cluster(&router, json!({ "ClusterName": "owned" })).await;
    let own_arn = own["ClusterArn"].as_str().unwrap_or_default().to_string();

    let mut cases: Vec<Case> = Vec::new();
    for account in FOREIGN_ACCOUNTS {
        cases.extend(cases_for(
            &arn(account, "cluster/7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4"),
            &own_arn,
        ));
    }
    // The ARN this service really minted, with only its account swapped: the resource
    // behind it exists, and the answer is the same refusal, so the refusal cannot be
    // reporting absence.
    cases.extend(cases_for(
        &own_arn.replace(CALLER_ACCOUNT, "123456789012"),
        &own_arn,
    ));

    for case in &cases {
        let (status, answer) = call(&router, case.method, &case.path, case.body.clone()).await;
        assert_eq!(
            status, 403,
            "case {}: expected 403, got {status} {answer:?}",
            case.label
        );
        assert_eq!(
            error_code(&answer),
            "AccessDeniedException",
            "case {}: expected AccessDeniedException, got {answer:?}",
            case.label
        );
        let actual = message(&answer);
        let expected = not_authorized(case.action, &case.resource);
        assert_eq!(
            actual, expected,
            "case {}: expected {expected:?}, got {actual:?}",
            case.label
        );
    }

    // And none of them created anything: the refusal happens before any handler writes.
    let (status, listed) = call(&router, "GET", "/cluster?MaxResults=1000", None).await;
    assert_eq!(
        status, 200,
        "case list clusters: expected 200, got {status} {listed:?}"
    );
    let arns: Vec<&str> = listed["Clusters"]
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .map(|cluster| cluster["ClusterArn"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![own_arn.as_str()],
        "case after {} refused requests: expected only the cluster created up front, got \
         {arns:?}",
        cases.len()
    );
}

/// RULE, at its boundary: the check refuses an ARN that names ANOTHER account and nothing
/// else, so a value no account can be read out of — and one of the caller's own account —
/// reaches the lookup, which answers for it.
#[tokio::test]
async fn an_arn_naming_no_other_account_reaches_the_lookup() {
    let router = router();

    let own = create_cluster(&router, json!({ "ClusterName": "boundary" })).await;
    let own_arn = own["ClusterArn"].as_str().unwrap_or_default().to_string();

    // (case, the ClusterArn to ask for, the status expected, the code expected)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "the caller's own stored cluster",
            own_arn.clone(),
            200,
            "",
        ),
        (
            "an unstored ARN of the caller's own account",
            arn(CALLER_ACCOUNT, "cluster/00000000-0000-0000-0000-000000000000"),
            404,
            "ResourceNotFoundException",
        ),
        (
            // Nothing to compare an account against, so authorization refuses nothing and
            // the lookup answers.
            "a value that is not an ARN at all",
            "not-an-arn".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN whose account segment is empty",
            arn("", "cluster/00000000-0000-0000-0000-000000000000"),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, cluster_arn, expected_status, expected_code) in &rows {
        let (status, answer) = call(
            &router,
            "GET",
            &format!("/cluster/{}", enc(cluster_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case}: expected {expected_status}, got {status} {answer:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&answer),
                *expected_code,
                "case {case}: expected {expected_code}, got {answer:?}"
            );
        }
    }
}
