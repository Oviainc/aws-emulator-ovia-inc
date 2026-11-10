// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Operator App slice of one AgentSpace:
//! `EnableOperatorApp` (POST `/v1/agentspaces/{agentSpaceId}/operator`) and
//! `GetOperatorApp` (GET `/v2/agentspaces/{agentSpaceId}/operator`).
//!
//! Every case drives the service through the same router the emulator serves, so the
//! routes, status codes, protocol envelope and state are all exercised.
//!
//! The rule under test: an AgentSpace's Operator App is a persisted configuration of
//! independent authentication flows, one per `AuthFlow` value; an enablement stores the
//! flow it names from the members that flow carries and leaves the others as they were,
//! and `GetOperatorApp` reports exactly the stored flows plus the URL operators reach
//! the app at. An AgentSpace that exists but has no flow enabled has nothing configured,
//! which the read reports as the empty configuration; an `agentSpaceId` naming no
//! AgentSpace names no Operator App either, which the read refuses as not found with
//! the wording this service's control plane uses for a lookup that resolved nothing:
//! HTTP 404 `ResourceNotFoundException` naming both the identifier the request carried
//! and the account the lookup ran in, each double-quoted.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_space_not_found::{assert_not_found, missing_identifiers};

/// The `AuthFlow` value set, from the model's enum
/// (`smithy_query(query="shape", shape="AuthFlow")`). Every value is enabled by some
/// row of [`enablement_rows`].
const AUTH_FLOWS: &[&str] = &["iam", "idc", "idp"];

/// The output members `GetOperatorApp` and `EnableOperatorApp` report a flow under,
/// from `smithy_query(query="operation", operation="GetOperatorApp")`.
const FLOW_MEMBERS: &[&str] = &["iam", "idc", "idp"];

/// The region a case signs for when it only needs one.
const REGION: &str = "us-west-2";

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which is
/// how the router resolves which service claims a request, and the region the request
/// is signed for. Signature verification is switched off for these tests, so the
/// signature is a placeholder.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/aidevops/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine),
    );
    router
}

/// Send one request signed for `region` and return its status and parsed body
/// (`Value::Null` for an empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    region: &str,
) -> (u16, Value) {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", authorization(region));
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create an agent space in `region` and return its id.
async fn create_space(router: &MultiServiceRouter, name: &str, region: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/v1/agentspaces",
        Some(json!({"name": name})),
        region,
    )
    .await;
    assert_eq!(status, 201, "case {name:?}: expected 201, got {status}");
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

/// An IAM role ARN satisfying the modelled `RoleArn` pattern
/// (`^arn:(aws|aws-cn|aws-us-gov):iam::\d{12}:role/[a-zA-Z0-9+=,.@_/-]+$`).
fn role_arn(partition: &str, account: &str, name: &str) -> String {
    format!("arn:{partition}:iam::{account}:role/{name}")
}

/// An AWS Identity Center instance ARN, the form `idcInstanceArn` carries.
fn instance_arn(partition: &str, instance_id: &str) -> String {
    format!("arn:{partition}:sso:::instance/{instance_id}")
}

/// `GET /v2/agentspaces/{id}/operator`.
async fn get_operator_app(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    region: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v2/agentspaces/{agent_space_id}/operator"),
        None,
        region,
    )
    .await
}

/// `POST /v1/agentspaces/{id}/operator`.
async fn enable_operator_app(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    body: Value,
    region: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/agentspaces/{agent_space_id}/operator"),
        Some(body),
        region,
    )
    .await
}

/// Whether `body` carries no value for `member` — an absent member and an explicit
/// null are the same absence on the wire.
fn absent(body: &Value, member: &str) -> bool {
    body.get(member).is_none_or(Value::is_null)
}

/// The `createdAt` / `updatedAt` instant of a reported flow, asserted to be present.
///
/// Returned as the raw JSON value: a timestamp renders as restJson1's default
/// epoch-seconds here, so what matters to a case is that the instant is there and
/// that a later read reports the same one.
fn timestamp(case: &str, flow: &Value, member: &str) -> Value {
    let value = flow.get(member).cloned().unwrap_or(Value::Null);
    assert!(
        !value.is_null(),
        "case {case:?}: expected a {member} instant, got {flow}"
    );
    value
}

/// One row of the enablement table: what is enabled, and what the service must then
/// report for it.
struct Row {
    case: &'static str,
    /// The region the row signs for, which is the region the app URL names.
    region: &'static str,
    auth_flow: &'static str,
    /// The `operatorAppRoleArn` every flow carries, as (partition, account, name).
    role: (&'static str, &'static str, &'static str),
    /// `idcInstanceArn`, as (partition, instance id), for the `idc` flow.
    instance: Option<(&'static str, &'static str)>,
    /// `issuerUrl`, `idpClientId`, `idpClientSecret` and `provider`, for the `idp` flow.
    idp: Option<(&'static str, &'static str, &'static str, &'static str)>,
}

impl Row {
    /// The request body this row sends.
    fn body(&self) -> Value {
        let (partition, account, name) = self.role;
        let mut body = json!({
            "authFlow": self.auth_flow,
            "operatorAppRoleArn": role_arn(partition, account, name),
        });
        if let Some((instance_partition, instance_id)) = self.instance {
            body["idcInstanceArn"] = json!(instance_arn(instance_partition, instance_id));
        }
        if let Some((issuer_url, client_id, client_secret, provider)) = self.idp {
            body["issuerUrl"] = json!(issuer_url);
            body["idpClientId"] = json!(client_id);
            body["idpClientSecret"] = json!(client_secret);
            body["provider"] = json!(provider);
        }
        body
    }

    /// The member the flow is reported under.
    fn member(&self) -> &'static str {
        self.auth_flow
    }

    /// Assert that `reported` — the object the service reports this row's flow as —
    /// carries exactly the configuration the row enabled.
    fn assert_reported(&self, reported: &Value) {
        let case = self.case;
        let (partition, account, name) = self.role;
        assert_eq!(
            reported.get("operatorAppRoleArn").and_then(Value::as_str),
            Some(role_arn(partition, account, name).as_str()),
            "case {case:?}: expected the enabled role, got {reported}"
        );
        if let Some((instance_partition, instance_id)) = self.instance {
            assert_eq!(
                reported.get("idcInstanceArn").and_then(Value::as_str),
                Some(instance_arn(instance_partition, instance_id).as_str()),
                "case {case:?}: expected the enabled IdC instance, got {reported}"
            );
            // The IdC application the enablement created lives in that instance,
            // under the partition and account of the operator role.
            let application = reported
                .get("idcApplicationArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no idcApplicationArn in {reported}"));
            let prefix = format!("arn:{partition}:sso::{account}:application/{instance_id}/");
            let application_id = application.strip_prefix(&prefix).unwrap_or_else(|| {
                panic!("case {case:?}: expected an application ARN under {prefix:?}, got {application:?}")
            });
            assert!(
                !application_id.is_empty(),
                "case {case:?}: expected an application id, got {application:?}"
            );
        }
        if let Some((issuer_url, client_id, _, provider)) = self.idp {
            assert_eq!(
                reported.get("issuerUrl").and_then(Value::as_str),
                Some(issuer_url),
                "case {case:?}: expected the enabled issuer, got {reported}"
            );
            assert_eq!(
                reported.get("clientId").and_then(Value::as_str),
                Some(client_id),
                "case {case:?}: expected the enabled client id, got {reported}"
            );
            assert_eq!(
                reported.get("provider").and_then(Value::as_str),
                Some(provider),
                "case {case:?}: expected the enabled provider, got {reported}"
            );
        }
        timestamp(case, reported, "createdAt");
        assert!(
            absent(reported, "updatedAt"),
            "case {case:?}: a first enablement has nothing to update, got {reported}"
        );
    }
}

/// The enablement table: every `AuthFlow` value, and rows differing in partition,
/// account, region, IdC instance and IdP provider.
fn enablement_rows() -> Vec<Row> {
    vec![
        Row {
            case: "iam in the aws partition",
            region: REGION,
            auth_flow: "iam",
            role: ("aws", "123456789012", "OperatorAppRole"),
            instance: None,
            idp: None,
        },
        Row {
            case: "iam in the aws-us-gov partition, signed for another region",
            region: "us-gov-west-1",
            auth_flow: "iam",
            role: ("aws-us-gov", "210987654321", "devops/OperatorApp-Role_1"),
            instance: None,
            idp: None,
        },
        Row {
            case: "idc in the aws partition",
            region: REGION,
            auth_flow: "idc",
            role: ("aws", "123456789012", "OperatorAppRole"),
            instance: Some(("aws", "ssoins-7223f7ba9b71f4c2")),
            idp: None,
        },
        Row {
            case: "idc in the aws-cn partition, signed for another region",
            region: "cn-north-1",
            auth_flow: "idc",
            role: ("aws-cn", "345678901234", "OperatorAppRole"),
            instance: Some(("aws-cn", "ssoins-0123456789abcdef")),
            idp: None,
        },
        Row {
            case: "idp with an Okta provider",
            region: REGION,
            auth_flow: "idp",
            role: ("aws", "123456789012", "OperatorAppRole"),
            instance: None,
            idp: Some((
                "https://example.okta.com/oauth2/default",
                "0oa1b2c3d4e5f6g7h8i9",
                "okta-client-secret",
                "Okta",
            )),
        },
        Row {
            case: "idp with an Entra provider, signed for another region",
            region: "eu-west-1",
            auth_flow: "idp",
            role: ("aws", "456789012345", "OperatorAppRole"),
            instance: None,
            idp: Some((
                "https://login.microsoftonline.com/tenant/v2.0",
                "11111111-2222-3333-4444-555555555555",
                "entra-client-secret",
                "Entra",
            )),
        },
    ]
}

/// The URL operators reach the Operator App of `agent_space_id` at, in `region`: one
/// host per AgentSpace under this service's own namespace.
fn expected_url(agent_space_id: &str, region: &str) -> String {
    format!("https://{agent_space_id}.operator.aidevops.{region}.on.aws")
}

/// Assert that the Operator App of `agent_space_id` is reported as the empty
/// configuration: the AgentSpace exists, so nothing is refused, and no flow is
/// enabled, so there is no configuration and no URL to report.
async fn assert_empty_configuration(
    router: &MultiServiceRouter,
    case: &str,
    agent_space_id: &str,
) {
    let (status, body) = get_operator_app(router, agent_space_id, REGION).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} ({body})"
    );
    for member in FLOW_MEMBERS.iter().chain(["operatorAppUrl"].iter()) {
        assert!(
            absent(&body, member),
            "case {case:?}: expected no {member}, got {body}"
        );
    }
}

#[tokio::test]
async fn an_agent_space_with_no_enabled_flow_reports_the_empty_configuration() {
    let router = new_router();
    let agent_space_id = create_space(&router, "no-operator-app", REGION).await;

    assert_empty_configuration(&router, "an agent space with no enablement", &agent_space_id)
        .await;
}

#[tokio::test]
async fn reading_the_operator_app_of_an_agent_space_that_does_not_exist_is_refused() {
    let router = new_router();
    // A stored AgentSpace, so each refusal below is decided from what the store holds
    // rather than from the store being empty.
    let present = create_space(&router, "a-space-that-exists", REGION).await;

    for agent_space_id in missing_identifiers() {
        assert_ne!(
            agent_space_id, present,
            "the table must not name the stored AgentSpace"
        );
        let case = format!("GetOperatorApp {agent_space_id:?}");
        let (status, refused) = get_operator_app(&router, &agent_space_id, REGION).await;
        assert_not_found(&case, &agent_space_id, status, &refused);
    }
}

#[tokio::test]
async fn each_enabled_flow_is_reported_as_the_enablement_stored_it() {
    let rows = enablement_rows();
    for flow in AUTH_FLOWS {
        assert!(
            rows.iter().any(|row| row.auth_flow == *flow),
            "the table must enable every AuthFlow value; {flow:?} is missing"
        );
    }

    for row in &rows {
        let case = row.case;
        let router = new_router();
        let agent_space_id = create_space(&router, case, row.region).await;

        let (status, enabled) =
            enable_operator_app(&router, &agent_space_id, row.body(), row.region).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} ({enabled})");
        assert_eq!(
            enabled.get("agentSpaceId").and_then(Value::as_str),
            Some(agent_space_id.as_str()),
            "case {case:?}: expected the enabled agent space, got {enabled}"
        );

        // What the enablement answered is what a later read reports: one stored
        // configuration, rendered the same way by both operations.
        let (status, reported) = get_operator_app(&router, &agent_space_id, row.region).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");
        for member in FLOW_MEMBERS.iter().chain(["operatorAppUrl"].iter()) {
            assert_eq!(
                reported.get(member),
                enabled.get(member),
                "case {case:?}: {member} differs between the enablement and the read: \
                 {enabled} vs {reported}"
            );
        }

        assert_eq!(
            reported.get("operatorAppUrl").and_then(Value::as_str),
            Some(expected_url(&agent_space_id, row.region).as_str()),
            "case {case:?}: expected the app URL of this agent space in this region, \
             got {reported}"
        );
        row.assert_reported(&reported[row.member()]);

        // Only the flow that was enabled is configured.
        for member in FLOW_MEMBERS.iter().filter(|m| **m != row.member()) {
            assert!(
                absent(&reported, member),
                "case {case:?}: {member} was never enabled, got {reported}"
            );
        }

        // The OIDC client secret is `@sensitive` and no operation reports it.
        if let Some((_, _, client_secret, _)) = row.idp {
            assert!(
                !reported.to_string().contains(client_secret),
                "case {case:?}: the client secret must not be reported, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn flows_accumulate_and_re_enabling_one_reconfigures_only_that_flow() {
    let router = new_router();
    let agent_space_id = create_space(&router, "accumulating-flows", REGION).await;
    let rows: Vec<Row> = enablement_rows()
        .into_iter()
        .filter(|row| row.region == REGION)
        .collect();

    // Each enablement adds its own flow and leaves the flows already enabled, and
    // their enablement instants, exactly as they were.
    let mut created: Vec<(&'static str, Value)> = Vec::new();
    for row in &rows {
        let case = row.case;
        let (status, _) =
            enable_operator_app(&router, &agent_space_id, row.body(), REGION).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");

        let (_, reported) = get_operator_app(&router, &agent_space_id, REGION).await;
        created.push((row.member(), timestamp(case, &reported[row.member()], "createdAt")));
        for (member, created_at) in &created {
            assert_eq!(
                &reported[*member]["createdAt"], created_at,
                "case {case:?}: {member} was enabled earlier and must be untouched, \
                 got {reported}"
            );
            assert!(
                absent(&reported[*member], "updatedAt"),
                "case {case:?}: {member} was never reconfigured, got {reported}"
            );
        }
    }

    // Re-enabling one flow reconfigures that flow: its enablement instant stays, its
    // update instant appears, and the other flows are untouched.
    let reconfigured = rows
        .iter()
        .find(|row| row.auth_flow == "iam")
        .expect("the table enables the iam flow in this region");
    let new_role = role_arn("aws", "123456789012", "SecondOperatorAppRole");
    let mut body = reconfigured.body();
    body["operatorAppRoleArn"] = json!(new_role);
    let (status, _) = enable_operator_app(&router, &agent_space_id, body, REGION).await;
    assert_eq!(status, 200, "re-enabling iam: expected 200, got {status}");

    let (_, reported) = get_operator_app(&router, &agent_space_id, REGION).await;
    assert_eq!(
        reported["iam"].get("operatorAppRoleArn").and_then(Value::as_str),
        Some(new_role.as_str()),
        "re-enabling iam: expected the reconfigured role, got {reported}"
    );
    timestamp("re-enabling iam", &reported["iam"], "updatedAt");
    for (member, created_at) in &created {
        assert_eq!(
            &reported[*member]["createdAt"], created_at,
            "re-enabling iam: {member} keeps its enablement instant, got {reported}"
        );
        if *member != "iam" {
            assert!(
                absent(&reported[*member], "updatedAt"),
                "re-enabling iam: {member} must be untouched, got {reported}"
            );
        }
    }

    // The IdC application is the one already created for that instance, so
    // re-enabling the same instance does not leave a second application behind.
    let idc = rows
        .iter()
        .find(|row| row.auth_flow == "idc")
        .expect("the table enables the idc flow in this region");
    let application = reported["idc"]["idcApplicationArn"].clone();
    let (status, again) = enable_operator_app(&router, &agent_space_id, idc.body(), REGION).await;
    assert_eq!(status, 200, "re-enabling idc: expected 200, got {status}");
    assert_eq!(
        again["idc"]["idcApplicationArn"], application,
        "re-enabling the same IdC instance keeps its application, got {again}"
    );

    // A different instance is a different application.
    let mut moved = idc.body();
    moved["idcInstanceArn"] = json!(instance_arn("aws", "ssoins-9999999999999999"));
    let (status, moved) = enable_operator_app(&router, &agent_space_id, moved, REGION).await;
    assert_eq!(status, 200, "moving the IdC instance: expected 200, got {status}");
    assert_ne!(
        moved["idc"]["idcApplicationArn"], application,
        "a different IdC instance holds a different application, got {moved}"
    );
}

#[tokio::test]
async fn an_enablement_whose_flow_is_not_configured_is_refused_and_stores_nothing() {
    let router = new_router();
    let agent_space_id = create_space(&router, "refused-enablements", REGION).await;
    let role = role_arn("aws", "123456789012", "OperatorAppRole");

    // Every flow's own configuration, member by member: a flow is reported with all
    // of its members, so it cannot be enabled without them.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "idc without an instance",
            json!({"authFlow": "idc", "operatorAppRoleArn": role}),
            vec!["idcInstanceArn"],
        ),
        (
            "idp without any OIDC configuration",
            json!({"authFlow": "idp", "operatorAppRoleArn": role}),
            vec!["issuerUrl", "idpClientId", "idpClientSecret", "provider"],
        ),
        (
            "idp without its client secret",
            json!({
                "authFlow": "idp",
                "operatorAppRoleArn": role,
                "issuerUrl": "https://example.okta.com/oauth2/default",
                "idpClientId": "0oa1b2c3d4e5f6g7h8i9",
                "provider": "Okta",
            }),
            vec!["idpClientSecret"],
        ),
        (
            "idc with an instance ARN that names no IdC instance",
            json!({
                "authFlow": "idc",
                "operatorAppRoleArn": role,
                "idcInstanceArn": role,
            }),
            vec!["idcInstanceArn"],
        ),
    ];

    for (case, body, paths) in cases {
        let (status, refused) = enable_operator_app(&router, &agent_space_id, body, REGION).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} ({refused})");
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        let reported_paths: Vec<String> = refused["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {refused}"))
            .iter()
            .filter_map(|field| field["path"].as_str().map(str::to_string))
            .collect();
        for path in &paths {
            assert!(
                reported_paths.iter().any(|reported| reported == path),
                "case {case:?}: expected {path:?} among the refused fields, \
                 got {reported_paths:?}"
            );
        }
    }

    // A refused enablement is not a partial one: nothing was stored, so the read
    // still reports the empty configuration.
    assert_empty_configuration(&router, "after the refusals", &agent_space_id).await;
}

#[tokio::test]
async fn enabling_the_operator_app_of_an_agent_space_that_does_not_exist_is_refused() {
    let router = new_router();
    // A stored AgentSpace, so each refusal below is decided from what the store holds
    // rather than from the store being empty.
    let present = create_space(&router, "a-space-that-exists", REGION).await;
    let body = json!({
        "authFlow": "iam",
        "operatorAppRoleArn": role_arn("aws", "123456789012", "OperatorAppRole"),
    });

    for agent_space_id in missing_identifiers() {
        assert_ne!(
            agent_space_id, present,
            "the table must not name the stored AgentSpace"
        );
        let case = format!("EnableOperatorApp {agent_space_id:?}");
        let (status, refused) =
            enable_operator_app(&router, &agent_space_id, body.clone(), REGION).await;
        assert_not_found(&case, &agent_space_id, status, &refused);
    }
}

#[tokio::test]
async fn the_stored_idp_client_secret_is_never_rendered() {
    let router = new_router();
    let agent_space_id = create_space(&router, "idp-secret", REGION).await;
    let secret = "okta-client-secret-value";
    let body = json!({
        "authFlow": "idp",
        "operatorAppRoleArn": role_arn("aws", "123456789012", "OperatorAppRole"),
        "issuerUrl": "https://example.okta.com/oauth2/default",
        "idpClientId": "0oa1b2c3d4e5f6g7h8i9",
        "idpClientSecret": secret,
        "provider": "Okta",
    });

    let (status, enabled) = enable_operator_app(&router, &agent_space_id, body, REGION).await;
    assert_eq!(status, 200, "enabling idp: expected 200, got {status}");
    let (_, reported) = get_operator_app(&router, &agent_space_id, REGION).await;
    for (case, answered) in [("the enablement", &enabled), ("the read", &reported)] {
        assert!(
            !answered.to_string().contains(secret),
            "case {case:?}: the client secret must not be reported, got {answered}"
        );
    }

    // It is not rendered by the store's `Debug` either, which is how a secret
    // reaches a log without anyone meaning to.
    let record = service_com_amazonaws_devopsagent_devopsagent::state::IdpFlowRecord {
        operator_app_role_arn: role_arn("aws", "123456789012", "OperatorAppRole"),
        issuer_url: "https://example.okta.com/oauth2/default".to_string(),
        client_id: "0oa1b2c3d4e5f6g7h8i9".to_string(),
        client_secret: secret.to_string(),
        provider: "Okta".to_string(),
        created_at: std::time::SystemTime::now(),
        updated_at: None,
    };
    let rendered = format!("{record:?}");
    assert!(
        !rendered.contains(secret),
        "the stored client secret must not be rendered, got {rendered}"
    );
}
