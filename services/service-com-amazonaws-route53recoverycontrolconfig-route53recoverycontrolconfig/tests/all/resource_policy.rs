// SPDX-License-Identifier: CC0-1.0

//! What `GetResourcePolicy` reports for the resource an ARN names.
//!
//! The rule under test: `GetResourcePolicy` reports the policy document ATTACHED to the
//! resource its `ResourceArn` names, and reports the EMPTY policy document (`{}`, the
//! shortest value the member's `length 2..10240` admits) for a stored resource that
//! carries none — the read answers for the resource, so it is an ARN this service holds no
//! resource under that is refused with `ResourceNotFoundException` (404), in the one
//! sentence every unresolvable ARN of this service is refused with. An ARN of another
//! account is refused with `AccessDeniedException` (403) before the ARN is resolved, like
//! every other operation of this service that carries one. Reading a policy is a READ: it
//! attaches nothing, and what a later caller sees is unchanged.
//!
//! A recovery-control resource policy is attached OUTSIDE this API — sharing a cluster
//! with another account is what creates one — so this suite attaches the documents it
//! reads back through the store directly (`state::Store::attach_resource_policy`), which
//! is the only writer of that state, and drives every read over HTTP through the router.
//!
//! The tables span what the rule generalises over: every kind of resource this service
//! stores (cluster, default and created control panel, routing control, safety rule),
//! documents differing in size and in which characters the member admits (the empty
//! document at the length minimum, a realistic share policy, one carrying the tab,
//! newline, carriage return and Latin-1 characters its pattern admits, and one of exactly
//! the published maximum length), ARNs of each kind that resolve to nothing, a value that
//! is not an ARN at all, a deleted resource's ARN, and three foreign accounts.

use serde_json::{json, Value};
use service_com_amazonaws_route53recoverycontrolconfig_route53recoverycontrolconfig::state;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, create_control_panel, create_routing_control, enc, error_code, router,
    router_with_engine,
};

/// The account the emulator's own credential belongs to: the caller of every request here.
const CALLER_ACCOUNT: &str = "000000000000";

/// The IAM user that credential speaks for, as AWS names a principal in an authorization
/// refusal.
const CALLER_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The namespace this service's IAM actions are published under: the model's `arnNamespace`
/// (`smithy_query(query="service_info")`).
const ACTION_NAMESPACE: &str = "route53-recovery-control-config";

/// The empty JSON policy document, restated here independently of the service's own code:
/// what a resource with no policy attached to it reports, and the shortest document
/// `GetResourcePolicyResponse$Policy` can carry (`length 2..10240`,
/// `smithy_query(query="operation",operation="GetResourcePolicy")`).
const EMPTY_POLICY: &str = "{}";

/// The published length bounds of that member, from the same model query.
const POLICY_MIN_LENGTH: usize = 2;
const POLICY_MAX_LENGTH: usize = 10_240;

/// Accounts that are not the caller's, so an ARN built for one of them is another
/// account's resource.
const FOREIGN_ACCOUNTS: &[&str] = &["123456789012", "999999999999", "210987654321"];

/// A recovery-control ARN: AWS writes them with an empty Region segment.
fn arn(account: &str, tail: &str) -> String {
    format!("arn:aws:route53-recovery-control::{account}:{tail}")
}

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The sentence an ARN that resolves to no resource of this service is refused with,
/// restated here independently of the service's own code.
fn not_found(noun: &str, arn: &str) -> String {
    format!("No record corresponding to {noun} Arn({arn})")
}

/// The sentence AWS refuses an unauthorized request with: the principal, the fully
/// qualified action and the resource, all as the request carried them.
fn not_authorized(resource: &str) -> String {
    format!(
        "User: {CALLER_PRINCIPAL} is not authorized to perform: \
         {ACTION_NAMESPACE}:GetResourcePolicy on resource: {resource} because no \
         resource-based policy allows the {ACTION_NAMESPACE}:GetResourcePolicy action"
    )
}

/// One `GetResourcePolicy` call: `GET /resourcePolicy/{ResourceArn}`, the ARN sent as one
/// path segment the way an SDK encodes it.
async fn get_resource_policy(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/resourcePolicy/{}", enc(resource_arn)),
        None,
    )
    .await
}

/// The `Policy` a successful read reports, asserting the status and the member's published
/// bounds on the way.
async fn policy_of(router: &MultiServiceRouter, case: &str, resource_arn: &str) -> String {
    let (status, body) = get_resource_policy(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "case {case}: expected GetResourcePolicy 200, got {status} with {body:?}"
    );
    let policy = body["Policy"].as_str().unwrap_or_default().to_string();
    let length = policy.chars().count();
    assert!(
        (POLICY_MIN_LENGTH..=POLICY_MAX_LENGTH).contains(&length),
        "case {case}: a reported policy is within the member's published length bounds \
         {POLICY_MIN_LENGTH}..{POLICY_MAX_LENGTH}, got {length} characters: {policy:?}"
    );
    policy
}

/// Attach `policy` to the resource `resource_arn` names, through the only writer of that
/// state, and answer whether the store accepted it.
fn attach(engine: &engine::InMemoryEngine, resource_arn: &str, policy: &str) -> bool {
    let store = state::service_state(engine).expect("the test engine carries this service's state");
    let mut guard = store.lock();
    guard.attach_resource_policy(resource_arn, policy)
}

/// Create one cluster through the API and answer `(ClusterArn, the default ControlPanelArn)`.
async fn cluster_with_default_panel(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, created) = call(router, "POST", "/cluster", Some(json!({ "ClusterName": name }))).await;
    assert_eq!(
        status, 200,
        "case create {name}: expected CreateCluster 200, got {status} with {created:?}"
    );
    let cluster_arn = created["Cluster"]["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, panels) = call(
        router,
        "GET",
        &format!("/controlpanels?ClusterArn={}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list panels of {name}: expected 200, got {status} with {panels:?}"
    );
    let default_panel = panels["ControlPanels"]
        .as_array()
        .and_then(|panels| panels.first())
        .map(|panel| panel["ControlPanelArn"].as_str().unwrap_or_default())
        .unwrap_or_default()
        .to_string();
    (cluster_arn, default_panel)
}

/// A document of exactly `length` characters that is still a JSON policy: the padding sits
/// in a statement id, so the value is a document and not a run of filler.
fn policy_of_length(length: usize) -> String {
    let head = r#"{"Version":"2012-10-17","Statement":[{"Sid":""#;
    let tail = r#"","Effect":"Allow","Principal":{"AWS":"123456789012"},"Action":"route53-recovery-control-config:*"}]}"#;
    let padding = length - head.len() - tail.len();
    format!("{head}{}{tail}", "A".repeat(padding))
}

/// RULE: every stored resource that carries no attached policy reports the EMPTY policy
/// document — whatever kind of resource it is.
#[tokio::test]
async fn a_resource_with_no_attached_policy_reports_the_empty_document() {
    let router = router();

    let (cluster_arn, default_panel_arn) = cluster_with_default_panel(&router, "policy-kinds").await;
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "policy-kinds-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let control = create_routing_control(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": "policy-kinds-control"
        }),
    )
    .await;
    let control_arn = control["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, rule) = call(
        &router,
        "POST",
        "/safetyrule",
        Some(json!({
            "AssertionRule": {
                "AssertedControls": [control_arn],
                "ControlPanelArn": panel_arn,
                "Name": "policy-kinds-rule",
                "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": "ATLEAST" },
                "WaitPeriodMs": 5000
            }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create safety rule: expected 200, got {status} with {rule:?}"
    );
    let rule_arn = rule["AssertionRule"]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // One row per kind of resource this service stores, the cluster's default control
    // panel beside a created one.
    let rows: Vec<(&str, &str)> = vec![
        ("cluster", cluster_arn.as_str()),
        ("default control panel", default_panel_arn.as_str()),
        ("created control panel", panel_arn.as_str()),
        ("routing control", control_arn.as_str()),
        ("safety rule", rule_arn.as_str()),
    ];

    for (case, resource_arn) in &rows {
        let reported = policy_of(&router, case, resource_arn).await;
        assert_eq!(
            reported, EMPTY_POLICY,
            "case {case}: a resource with no attached policy reports {EMPTY_POLICY:?}, got \
             {reported:?}"
        );
        // Reading is a read: the second call reports exactly what the first did.
        let again = policy_of(&router, case, resource_arn).await;
        assert_eq!(
            again, reported,
            "case {case}: a second read must report {reported:?}, got {again:?}"
        );
    }

    // And no read attached anything or disturbed the resources it addressed: the cluster
    // still describes, and its control panel still holds its routing control.
    let (status, described) = call(
        &router,
        "GET",
        &format!("/cluster/{}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case after {} reads: expected DescribeCluster 200, got {status} with {described:?}",
        rows.len() * 2
    );
    assert_eq!(
        described["Cluster"]["ClusterArn"].as_str().unwrap_or_default(),
        cluster_arn,
        "case after reads: DescribeCluster must still report the cluster, got {described:?}"
    );
}

/// RULE: the document a resource carries is what the read reports, byte for byte — for
/// every document the member can hold.
#[tokio::test]
async fn an_attached_policy_is_reported_as_it_was_attached() {
    let (router, engine) = router_with_engine();

    let (cluster_arn, _) = cluster_with_default_panel(&router, "policy-documents").await;
    let (other_arn, _) = cluster_with_default_panel(&router, "policy-documents-other").await;

    // Rows differing along what the member admits: the empty document at its length
    // minimum, a realistic cross-account share policy, every character class its pattern
    // admits beyond printable ASCII (tab, newline, carriage return, U+00A1..U+00FF), and a
    // document of exactly the published maximum length.
    let documents: Vec<(&str, String)> = vec![
        ("the empty document", EMPTY_POLICY.to_string()),
        (
            "a cross-account share policy",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "ShareCluster",
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
                    "Action": [
                        "route53-recovery-control-config:DescribeCluster",
                        "route53-recovery-control-config:ListRoutingControls"
                    ],
                    "Resource": cluster_arn.clone()
                }]
            })
            .to_string(),
        ),
        (
            "a document carrying the whitespace and Latin-1 characters the pattern admits",
            "{\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [{\"Sid\": \"ñéÿ¡£¥\"}]\n}"
                .to_string(),
        ),
        (
            "a document of exactly the published maximum length",
            policy_of_length(POLICY_MAX_LENGTH),
        ),
    ];

    for (case, document) in &documents {
        assert!(
            attach(&engine, &cluster_arn, document),
            "case {case}: attaching to a stored cluster must be accepted"
        );

        let reported = policy_of(&router, case, &cluster_arn).await;
        assert_eq!(
            &reported, document,
            "case {case}: expected the document as attached, got {reported:?}"
        );
        // Each row replaces the one before it, and a second read still reports it.
        let again = policy_of(&router, case, &cluster_arn).await;
        assert_eq!(
            again, reported,
            "case {case}: a second read must report {reported:?}, got {again:?}"
        );

        // A policy belongs to the resource it was attached to and to no other.
        let untouched = policy_of(&router, case, &other_arn).await;
        assert_eq!(
            untouched, EMPTY_POLICY,
            "case {case}: another cluster carries no policy, so it reports \
             {EMPTY_POLICY:?}, got {untouched:?}"
        );
    }

    // A policy does not outlive the resource it is attached to: the delete takes it with
    // the cluster, and the ARN then resolves to nothing at all.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete the cluster holding a policy: expected 200, got {status} with {deleted:?}"
    );
    let (status, answer) = get_resource_policy(&router, &cluster_arn).await;
    assert_eq!(
        status, 404,
        "case read a deleted cluster's policy: expected 404, got {status} with {answer:?}"
    );
    assert_eq!(
        message(&answer),
        not_found("cluster", &cluster_arn),
        "case read a deleted cluster's policy: expected the shared not-found sentence, got \
         {answer:?}"
    );
    assert!(
        !attach(&engine, &cluster_arn, EMPTY_POLICY),
        "case attach to a deleted cluster: the store holds no resource under that ARN, so \
         the attach must be refused"
    );
}

/// RULE: an ARN this service holds no resource under is refused with
/// `ResourceNotFoundException` (404), naming the kind the ARN itself states.
#[tokio::test]
async fn an_arn_resolving_to_nothing_is_refused_with_not_found() {
    let router = router();

    // A stored cluster exists throughout, so a 404 below is about the ARN the request
    // names and not about an empty store.
    let (stored_arn, _) = cluster_with_default_panel(&router, "policy-not-found").await;

    let missing = "00000000-0000-0000-0000-000000000000";
    // One row per kind an ARN of this service can state, plus the values no kind can be
    // read out of at all.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "an unstored cluster ARN",
            arn(CALLER_ACCOUNT, &format!("cluster/{missing}")),
            "cluster",
        ),
        (
            "an unstored control panel ARN",
            arn(CALLER_ACCOUNT, &format!("controlpanel/{missing}")),
            "control panel",
        ),
        (
            "an unstored routing control ARN",
            arn(
                CALLER_ACCOUNT,
                &format!("controlpanel/{missing}/routingcontrol/{missing}"),
            ),
            "routing control",
        ),
        (
            "an unstored safety rule ARN",
            arn(
                CALLER_ACCOUNT,
                &format!("controlpanel/{missing}/safetyrule/{missing}"),
            ),
            "safety rule",
        ),
        (
            "an ARN of a resource kind this service does not store",
            arn(CALLER_ACCOUNT, &format!("widget/{missing}")),
            "resource",
        ),
        ("a value that is not an ARN at all", "not-an-arn".to_string(), "resource"),
        (
            "an ARN whose account segment is empty",
            arn("", &format!("cluster/{missing}")),
            "cluster",
        ),
    ];

    for (case, resource_arn, noun) in &rows {
        let (status, answer) = get_resource_policy(&router, resource_arn).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
        let expected = not_found(noun, resource_arn);
        let actual = message(&answer);
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // The stored cluster is still readable: none of those refusals touched the store.
    let reported = policy_of(&router, "the stored cluster", &stored_arn).await;
    assert_eq!(
        reported, EMPTY_POLICY,
        "case the stored cluster: expected {EMPTY_POLICY:?}, got {reported:?}"
    );
}

/// RULE: an ARN of another account is refused with `AccessDeniedException` (403) before the
/// ARN is resolved, so the answer never says whether a resource is stored there.
#[tokio::test]
async fn an_arn_of_another_account_is_refused_with_access_denied() {
    let (router, engine) = router_with_engine();

    let (own_arn, _) = cluster_with_default_panel(&router, "policy-authorization").await;
    assert!(
        attach(&engine, &own_arn, EMPTY_POLICY),
        "case fixture: attaching to the stored cluster must be accepted"
    );

    let mut rows: Vec<(String, String)> = Vec::new();
    for account in FOREIGN_ACCOUNTS {
        rows.push((
            format!("a cluster ARN of {account}"),
            arn(account, "cluster/7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4"),
        ));
        rows.push((
            format!("a control panel ARN of {account}"),
            arn(account, "controlpanel/7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4"),
        ));
    }
    // The ARN this service really minted, with only its account swapped: the resource
    // behind it exists and carries a policy, and the answer is the same refusal, so the
    // refusal cannot be reporting absence.
    rows.push((
        "the stored cluster's ARN with a foreign account".to_string(),
        own_arn.replace(CALLER_ACCOUNT, FOREIGN_ACCOUNTS[0]),
    ));

    for (case, resource_arn) in &rows {
        let (status, answer) = get_resource_policy(&router, resource_arn).await;
        assert_eq!(
            status, 403,
            "case {case}: expected 403, got {status} with {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "AccessDeniedException",
            "case {case}: expected AccessDeniedException, got {answer:?}"
        );
        let expected = not_authorized(resource_arn);
        let actual = message(&answer);
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // The caller's own cluster still reports its policy: the refusals changed nothing.
    let reported = policy_of(&router, "the caller's own cluster", &own_arn).await;
    assert_eq!(
        reported, EMPTY_POLICY,
        "case the caller's own cluster: expected {EMPTY_POLICY:?}, got {reported:?}"
    );
}
