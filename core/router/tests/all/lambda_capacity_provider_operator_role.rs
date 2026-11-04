// SPDX-License-Identifier: CC0-1.0

//! Lambda's OPERATOR-ROLE check for a capacity provider, across the two services it spans.
//!
//! RULE: `CreateCapacityProvider` resolves the IAM role its `PermissionsConfig` names
//! against this account's IAM records before it writes, and refuses the create with
//! `InvalidParameterValueException` (HTTP 400) — persisting nothing — when IAM ANSWERS that
//! the role is one this service cannot manage compute as: this account's IAM holds no role
//! under the name the ARN carries, or the role it does hold carries a trust policy that does
//! NOT let `lambda.amazonaws.com` perform `sts:AssumeRole` on it. The provider manages
//! compute instances AS that role, and the role is part of what the client provisions here,
//! so neither an absent record nor a trust policy that withholds the assume can be the role
//! a stored provider names.
//!
//! That ABSENT-record half is what separates this surface from a function's EXECUTION role,
//! which `all/lambda_execution_role_trust.rs` pins to the weaker reading: an execution role
//! is routinely provisioned out of band, so a name IAM holds nothing under says nothing
//! about it, while this operation is recorded refusing an operator role that does not
//! exist. That is the reading
//! `service_com_amazonaws_lambda_awsgirapiservice::state::capacity_provider::operator_role_is_unassumable`
//! implements through the shared `service_common::iam_role::role_is_unassumable`. A role of
//! ANOTHER account is served by both: that account's IAM is not this emulator's to ask, so
//! there is no answer to ground a refusal in.
//!
//! The refusal's message is wholly fixed text: it names neither the role nor which half of
//! the question failed.
//!
//! The role is resolved against the ACCOUNT's IAM records, which is why this suite lives
//! here: each role is created through the real `CreateRole` on the same engine that serves
//! Lambda, so what the check reads is the document IAM persisted and not a copy. The rows
//! span what is particular to this surface: whether IAM holds the role at all, whether the
//! trust policy admits this service, and a role of ANOTHER account — about which this
//! emulator's IAM can answer nothing, so nothing is refused.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder an `application/x-www-form-urlencoded` component needs, the reader of
// an XML answer's element, and the Lambda call harness: all already owned by neighbouring
// suites, shared rather than copied.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;
use crate::lambda_signing_name_routing::{AUTHORIZATION, REGION};
use crate::query_wire::element;

/// The service principal a capacity provider's operator role has to admit: Lambda is what
/// assumes it to manage the provider's compute.
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// Where `CreateCapacityProvider` and `GetCapacityProvider` are bound, from
/// `smithy_query(query="operation", operation="CreateCapacityProvider,GetCapacityProvider")`.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// AWS's refusal of an operator role this service cannot manage compute with.
const OPERATOR_ROLE_INVALID: &str =
    "The operator role is invalid or doesn't have sufficient permissions. Verify the role \
     and permissions and try again.";

/// One emulator carrying both services over ONE shared engine, exactly as the production
/// router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One restJson1 call to the Lambda control plane, answered with the HTTP status, the wire
/// error code (`""` on a success) and the JSON body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    (status, code, parsed)
}

/// Create the role `name` with the trust document `document`, and report its ARN.
async fn create_role(router: &MultiServiceRouter, case: &str, name: &str, document: &str) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRole to persist the role, got {status}: {created}"
    );
    element(&created, "Arn")
}

/// A trust policy with one statement naming `principal` under `effect` for `action`.
fn trust(effect: &str, action: Value, principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": principal,
            "Action": action
        }]
    })
    .to_string()
}

/// The `CreateCapacityProvider` request body for a provider of `name` managed with `role`.
fn create_body(name: &str, role: &str) -> Value {
    json!({
        "CapacityProviderName": name,
        "VpcConfig": {
            "SubnetIds": ["subnet-0123456789abcdef0"],
            "SecurityGroupIds": [],
        },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": role },
    })
}

/// How a row names the operator role it creates a provider with.
enum Role {
    /// A role IAM holds, created with this trust document.
    Held(String),
    /// A well-formed role ARN of THIS account that IAM holds no role for.
    AbsentFromIam,
    /// A well-formed role ARN of ANOTHER account.
    AnotherAccount,
}

/// One row: the operator role the create names, and whether the provider is stored.
struct Row {
    case: &'static str,
    role: Role,
    accepted: bool,
}

/// The table the rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role whose trust policy lets lambda assume it",
            role: Role::Held(trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            )),
            accepted: true,
        },
        Row {
            case: "a role trusting lambda among several services",
            role: Role::Held(trust(
                "Allow",
                json!(["sts:TagSession", "sts:AssumeRole"]),
                json!({ "Service": ["ec2.amazonaws.com", LAMBDA_PRINCIPAL] }),
            )),
            accepted: true,
        },
        Row {
            case: "a role trusting only another service",
            role: Role::Held(trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": "ec2.amazonaws.com" }),
            )),
            accepted: false,
        },
        Row {
            case: "a role whose statement DENIES lambda the assume",
            role: Role::Held(trust(
                "Deny",
                json!("sts:AssumeRole"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            )),
            accepted: false,
        },
        Row {
            case: "a role trusting lambda for an action that is not assuming it",
            role: Role::Held(trust(
                "Allow",
                json!("s3:GetObject"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            )),
            accepted: false,
        },
        Row {
            // A name this account's IAM holds no role under is evidence about this
            // emulator's STORE rather than about the role the caller named — the role a
            // provider manages compute as is normally provisioned outside the client that
            // creates the provider — so a refusal has to be grounded in a trust document
            // IAM answered with. That is the reading
            // `crate::state::capacity_provider::operator_role_is_unassumable` implements
            // and documents (the shared
            // `service_common::iam_role::held_role_is_unassumable`, the reading a
            // function's EXECUTION role takes as well), and that handler names THIS suite
            // as what pins it: so this row is served, exactly as another account's role
            // below is.
            case: "a role this account's IAM holds none of",
            role: Role::AbsentFromIam,
            accepted: true,
        },
        Row {
            // Nothing is KNOWN about another account's role — its IAM is not this
            // emulator's to ask — and a refusal has to be grounded in an answer, so this
            // one is served.
            case: "a well-formed role ARN of another account",
            role: Role::AnotherAccount,
            accepted: true,
        },
    ]
}

#[tokio::test]
async fn create_capacity_provider_refuses_an_operator_role_lambda_cannot_assume() {
    let router = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let role_arn = match &row.role {
            Role::Held(document) => {
                create_role(
                    &router,
                    row.case,
                    &format!("capacity-operator-{index}"),
                    document,
                )
                .await
            }
            Role::AbsentFromIam => {
                "arn:aws:iam::000000000000:role/capacity-operator-never-created".to_string()
            }
            Role::AnotherAccount => "arn:aws:iam::123456789012:role/capacity-operator".to_string(),
        };
        let name = format!("operator-probe-{index}");

        let (status, code, created) = lambda(
            &router,
            "POST",
            CAPACITY_PROVIDERS_URI,
            Some(create_body(&name, &role_arn)),
        )
        .await;

        if row.accepted {
            assert_eq!(
                (status, code.as_str()),
                (202, ""),
                "case {:?}: a role nothing is known against must create the provider, got \
                 {status} {code:?}: {created}",
                row.case
            );
            assert_eq!(
                created["CapacityProvider"]["PermissionsConfig"]
                    ["CapacityProviderOperatorRoleArn"]
                    .as_str(),
                Some(role_arn.as_str()),
                "case {:?}: the provider is managed with the role the request named, got \
                 {created}",
                row.case
            );
        } else {
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {:?}: a role this service cannot assume must be refused, got {status} \
                 {code:?}: {created}",
                row.case
            );
            assert_eq!(
                created["message"].as_str(),
                Some(OPERATOR_ROLE_INVALID),
                "case {:?}: expected the modelled refusal message, got {created}",
                row.case
            );
            assert!(
                !created.to_string().contains(&role_arn),
                "case {:?}: the refusal must name no role, got {created}",
                row.case
            );
        }

        // Accepted or refused, the store holds exactly what the answer claimed: a refused
        // create persists NOTHING, so the read reports the provider only for the rows that
        // were served.
        let (status, code, fetched) = lambda(
            &router,
            "GET",
            &format!("{CAPACITY_PROVIDERS_URI}/{name}"),
            None,
        )
        .await;
        let expected = if row.accepted {
            (200, "")
        } else {
            (404, "ResourceNotFoundException")
        };
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {:?}: expected HTTP {} {:?} reading the provider back, got {status} \
             {code:?}: {fetched}",
            row.case,
            expected.0,
            expected.1
        );
    }
}
