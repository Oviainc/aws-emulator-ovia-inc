// SPDX-License-Identifier: CC0-1.0

//! The authorization gate on every implemented operation of this service.
//!
//! The rule under test: a request is authorized at the IAM layer BEFORE the resource it
//! names is evaluated, so a caller whose identity policy explicitly denies the action is
//! refused with `AccessDeniedException` — naming the principal, the action and the
//! resource — rather than being told whether the resource exists. Amazon EVS declares no
//! `AccessDeniedException` of its own: the refusal is the AWS-common one, which is why
//! the error code is asserted on the wire body rather than against a modelled shape.
//!
//! What does NOT refuse here is as much of the rule as what does. The IAM emulator does
//! not model a principal's whole permission set (group policies, permissions boundaries,
//! service control policies), so an ABSENT `Allow` is not evidence that the caller lacks
//! the permission and only an EXPLICIT `Deny` refuses — the reading
//! `crate::state::authorization` documents. Each row below therefore pairs a denied
//! caller with a caller the same request is not refused for.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::environment_lifecycle::{
    call, call_as, create, emulator, error_type, Emulator, ACCOUNT, DEFAULT_ACCESS_KEY_ID, REGION,
};

/// The wire error code the AWS authorization layer refuses with.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// How long the sessions these tests register last — long enough that none expires
/// while the test runs.
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

/// Register an assumed-role session in STS's OWN store under `access_key_id`, so a
/// request signed with that key speaks for the role, and answer the session ARN.
///
/// Written through the STS store rather than by calling `sts:AssumeRole`, because what
/// is under test is what this service does with the principal a credential names, not
/// how the credential was minted.
fn assume_role(engine: &engine::InMemoryEngine, access_key_id: &str, role_name: &str) -> String {
    let arn = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/session-1");
    let now = SystemTime::now();
    let vend = sts::NewSession {
        role_arn: Some(format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: format!("{}:session-1", sts::role_id(&arn)),
                account: ACCOUNT.to_string(),
                arn: arn.clone(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_LIFETIME.as_secs() as i32,
        )
    };
    let state = sts::service_state(engine).expect("the in-memory engine carries the STS store");
    state
        .lock()
        .insert(vend.into_session(access_key_id, now))
        .expect("the session registry has room for one session");
    arn
}

/// Embed an inline identity policy on a role in IAM's OWN store, the way
/// `iam:PutRolePolicy` does.
fn put_role_policy(engine: &engine::InMemoryEngine, role_name: &str, document: &str) {
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    let mut store = state.lock();
    store
        .role_inline_policies
        .entry(iam::name_key(role_name))
        .or_default()
        .insert(
            iam::name_key("evs-access"),
            iam::InlinePolicy {
                policy_name: "evs-access".to_string(),
                policy_document: document.to_string(),
            },
        );
}

/// A caller: the access key id a request signs with, prepared by registering a session
/// for a role and giving that role one inline policy.
fn caller(
    router: &Emulator,
    access_key_id: &str,
    role_name: &str,
    document: &str,
) -> String {
    assume_role(&router.engine, access_key_id, role_name);
    put_role_policy(&router.engine, role_name, document);
    access_key_id.to_string()
}

/// Every implemented operation of this service, with a request that names `environment`
/// (or its ARN), so the gate is exercised on each of them.
///
/// The requests are ones the model's own constraints accept, because the framework
/// validates a request's SHAPE before any handler — and so before this gate — runs: a
/// malformed request is refused as invalid whoever sends it.
fn every_operation(
    router: &Emulator,
    environment: &str,
    environment_arn: &str,
) -> Vec<(&'static str, Value)> {
    vec![
        ("CreateEnvironment", router.create_request(None)),
        ("ListEnvironments", json!({})),
        (
            "GetEnvironment",
            json!({ "environmentId": environment }),
        ),
        (
            "DeleteEnvironment",
            json!({ "environmentId": environment }),
        ),
        (
            "ListEnvironmentVlans",
            json!({ "environmentId": environment }),
        ),
        (
            "AssociateEipToVlan",
            json!({
                "environmentId": environment,
                "vlanName": "hcx",
                "allocationId": "eipalloc-0123456789abcdef0",
            }),
        ),
        (
            "DisassociateEipFromVlan",
            json!({
                "environmentId": environment,
                "vlanName": "hcx",
                "associationId": "eipassoc-0123456789abcdef0",
            }),
        ),
        (
            "CreateEnvironmentConnector",
            json!({
                "environmentId": environment,
                "type": "VCENTER",
                "applianceFqdn": "vcenter.example.invalid",
                "secretIdentifier": "evs/appliance",
            }),
        ),
        (
            "ListEnvironmentConnectors",
            json!({ "environmentId": environment }),
        ),
        (
            "UpdateEnvironmentConnector",
            json!({
                "environmentId": environment,
                "connectorId": "cnctr-0123456789",
                "applianceFqdn": "vcenter.example.invalid",
            }),
        ),
        (
            "CreateEnvironmentHost",
            json!({
                "environmentId": environment,
                "host": {
                    "hostName": "esx-1",
                    "keyName": "evs-key",
                    "instanceType": "i4i.metal",
                },
            }),
        ),
        (
            "ListEnvironmentHosts",
            json!({ "environmentId": environment }),
        ),
        (
            "CreateEntitlement",
            json!({
                "environmentId": environment,
                "connectorId": "cnctr-0123456789",
                "vmIds": ["vm-1"],
                "entitlementType": "WINDOWS_SERVER",
            }),
        ),
        (
            "ListVmEntitlements",
            json!({
                "environmentId": environment,
                "connectorId": "cnctr-0123456789",
                "entitlementType": "WINDOWS_SERVER",
            }),
        ),
        (
            "DeleteEntitlement",
            json!({
                "environmentId": environment,
                "connectorId": "cnctr-0123456789",
                "vmIds": ["vm-1"],
                "entitlementType": "WINDOWS_SERVER",
            }),
        ),
        (
            "TagResource",
            json!({ "resourceArn": environment_arn, "tags": { "owner": "team" } }),
        ),
        (
            "ListTagsForResource",
            json!({ "resourceArn": environment_arn }),
        ),
        (
            "UntagResource",
            json!({ "resourceArn": environment_arn, "tagKeys": ["owner"] }),
        ),
        // Account-level or service-level, so these three name no resource of their own:
        // only a statement covering every resource decides them.
        ("GetAccountSettings", json!({})),
        ("GetVersions", json!({})),
        (
            "PutAccountSettings",
            json!({ "settings": [{ "name": "vcfPortedCoreCount", "value": "100" }] }),
        ),
    ]
}

#[tokio::test]
async fn an_explicit_deny_of_the_service_refuses_every_operation() {
    let router = emulator();
    let environment = create(&router, Some("denied-env")).await;
    let environment_arn = format!("arn:aws:evs:{REGION}:{ACCOUNT}:environment/{environment}");

    // The recorded case: a role whose inline identity policy denies the whole service.
    let denied = caller(
        &router,
        "ASIADENIEDTESTONLY00",
        "evs-denied",
        r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Deny","Action":"evs:*","Resource":"*"}
        ]}"#,
    );
    // A role that is only ALLOWED the service: nothing denies it, so nothing here
    // refuses it either.
    let allowed = caller(
        &router,
        "ASIAALLOWEDTESTONLY0",
        "evs-allowed",
        r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"evs:*","Resource":"*"}
        ]}"#,
    );

    for (operation, request) in every_operation(&router, &environment, &environment_arn) {
        let (status, body) = call_as(&router, &denied, REGION, operation, &request).await;
        // Every CLIENT error of this awsJson1_0 service is reported with HTTP 400 — see
        // `tests/all/error_wire_status.rs`, which owns that rule.
        assert_eq!(
            status, 400,
            "case {operation:?} denied: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            ACCESS_DENIED,
            "case {operation:?} denied: expected {ACCESS_DENIED}, got {} in {body}",
            error_type(&body)
        );
        let message = body["message"].as_str().unwrap_or_default();
        for part in [
            "evs-denied/session-1",
            &format!("evs:{operation}"),
            "with an explicit deny in an identity-based policy",
        ] {
            assert!(
                message.contains(part),
                "case {operation:?} denied: the refusal must name {part:?}, got {message:?}"
            );
        }

        // The same request from a caller nothing denies is NOT refused at the gate: it
        // reaches this service's own handling, whatever that answers.
        for (case, access_key_id) in [
            ("an allowed role", allowed.as_str()),
            ("the account's own credential", DEFAULT_ACCESS_KEY_ID),
        ] {
            let (_, body) = call_as(&router, access_key_id, REGION, operation, &request).await;
            assert_ne!(
                error_type(&body),
                ACCESS_DENIED,
                "case {operation:?} as {case}: nothing denies this caller, so the gate \
                 must not refuse it, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_deny_refuses_only_the_actions_and_resources_it_names() {
    let router = emulator();
    let environment = create(&router, Some("scoped-env")).await;
    let other_arn = format!(
        "arn:aws:evs:{REGION}:{ACCOUNT}:environment/{}",
        "env-0000000000"
    );

    // One action only.
    let one_action = caller(
        &router,
        "ASIAONEACTIONTESTONL",
        "evs-one-action",
        r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"evs:*","Resource":"*"},
            {"Effect":"Deny","Action":"evs:GetEnvironment","Resource":"*"}
        ]}"#,
    );
    // One resource only: another environment's ARN.
    let one_resource = caller(
        &router,
        "ASIAONERESOURCETESTO",
        "evs-one-resource",
        &format!(
            r#"{{"Version":"2012-10-17","Statement":[
                {{"Effect":"Deny","Action":"evs:*","Resource":"{other_arn}"}}
            ]}}"#
        ),
    );

    // Each row: the case, the caller, the operation, the request, and whether the gate
    // refuses it.
    let rows: Vec<(&str, &str, &str, Value, bool)> = vec![
        (
            "the denied action",
            one_action.as_str(),
            "GetEnvironment",
            json!({ "environmentId": environment }),
            true,
        ),
        (
            "an action the deny does not name",
            one_action.as_str(),
            "ListEnvironmentVlans",
            json!({ "environmentId": environment }),
            false,
        ),
        (
            "a listing the deny does not name",
            one_action.as_str(),
            "ListEnvironments",
            json!({}),
            false,
        ),
        (
            "the denied resource",
            one_resource.as_str(),
            "GetEnvironment",
            json!({ "environmentId": "env-0000000000" }),
            true,
        ),
        (
            "another resource of the same action",
            one_resource.as_str(),
            "GetEnvironment",
            json!({ "environmentId": environment }),
            false,
        ),
        (
            "a request that names no resource at all",
            one_resource.as_str(),
            "ListEnvironments",
            json!({}),
            false,
        ),
    ];

    for (case, access_key_id, operation, request, refused) in rows {
        let (status, body) = call_as(&router, access_key_id, REGION, operation, &request).await;
        if refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {} in {body}",
                error_type(&body)
            );
        } else {
            assert_ne!(
                error_type(&body),
                ACCESS_DENIED,
                "case {case:?}: the deny does not cover this request, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_gate_refuses_before_the_resource_is_evaluated() {
    // The two answers a client must be able to tell apart: "denied" and "absent". The
    // same call, one caller denied and one not, against an environment that does not
    // exist — the denied caller learns nothing about whether it exists.
    let router = emulator();
    let denied = caller(
        &router,
        "ASIABEFORERESOURCE00",
        "evs-before-resource",
        r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Deny","Action":"evs:*","Resource":"*"}
        ]}"#,
    );
    let request = json!({ "environmentId": "env-0000000000" });

    let (_, denied_body) = call_as(&router, &denied, REGION, "GetEnvironment", &request).await;
    assert_eq!(
        error_type(&denied_body),
        ACCESS_DENIED,
        "the denied caller must be refused at the gate, got {denied_body}"
    );
    assert!(
        !denied_body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("was not found"),
        "the refusal must not report whether the environment exists, got {denied_body}"
    );

    let (_, allowed_body) = call(&router, "GetEnvironment", &request).await;
    assert!(
        error_type(&allowed_body).contains("ResourceNotFoundException"),
        "the caller nothing denies reaches this service's own handling, got {allowed_body}"
    );
}
