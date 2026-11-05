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
//! implements through the shared `service_common::iam_role::role_is_unassumable`.
//!
//! SECOND RULE, asked AHEAD of that one: a well-formed role ARN of ANOTHER account is
//! refused `AccessDeniedException` (HTTP 403) with the fixed sentence `Cross-account pass
//! role is not allowed.`, persisting nothing. Creating the provider HANDS the role to this
//! service, which is an `iam:PassRole`, and a pass is granted by the account that HOLDS the
//! role — so a principal of this account may pass a role of its own account and no role of
//! another one, whatever that other role trusts. It is decided on the request's own account
//! field and asks the IAM beside it nothing, which is why it also stands where no IAM is
//! mounted; it too is the reading
//! `…::state::capacity_provider::operator_role_is_of_another_account` implements.
//!
//! Both refusals' messages are wholly fixed text: neither names the role, the account, nor
//! which half of the question failed.
//!
//! The role is resolved against the ACCOUNT's IAM records, which is why this suite lives
//! here: each role is created through the real `CreateRole` on the same engine that serves
//! Lambda, so what the check reads is the document IAM persisted and not a copy. The rows
//! span what is particular to this surface: whether IAM holds the role at all, whether the
//! trust policy admits this service, and whose account the ARN names — this one, or another
//! one whose role no principal here may pass.

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

/// AWS's refusal of a pass of another account's role, which only that account can grant.
const CROSS_ACCOUNT_PASS_ROLE: &str = "Cross-account pass role is not allowed.";

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
    /// A well-formed role ARN of ANOTHER account, in the partition the row names.
    AnotherAccount(&'static str),
}

/// What the create answers for the role a row names.
enum Outcome {
    /// Served: 202, and the provider is read back.
    Served,
    /// Refused the modelled 400: IAM answered that this service cannot manage compute as
    /// the role.
    OperatorRoleInvalid,
    /// Refused 403: the role belongs to another account, so this account cannot pass it.
    CrossAccountPassRole,
}

impl Outcome {
    /// The HTTP status and wire error code the answer carries (`""` on a success).
    fn status_and_code(&self) -> (u16, &'static str) {
        match self {
            Self::Served => (202, ""),
            Self::OperatorRoleInvalid => (400, "InvalidParameterValueException"),
            Self::CrossAccountPassRole => (403, "AccessDeniedException"),
        }
    }

    /// The refusal's fixed sentence, for a row that is refused.
    fn message(&self) -> Option<&'static str> {
        match self {
            Self::Served => None,
            Self::OperatorRoleInvalid => Some(OPERATOR_ROLE_INVALID),
            Self::CrossAccountPassRole => Some(CROSS_ACCOUNT_PASS_ROLE),
        }
    }

    /// The member the refusal's sentence arrives on: the modelled errors of this service
    /// spell it `message`, and the synthetic 403 spells it `Message`.
    fn message_member(&self) -> &'static str {
        match self {
            Self::Served | Self::OperatorRoleInvalid => "message",
            Self::CrossAccountPassRole => "Message",
        }
    }
}

/// One row: the operator role the create names, and what the create answers for it.
struct Row {
    case: &'static str,
    role: Role,
    outcome: Outcome,
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
            outcome: Outcome::Served,
        },
        Row {
            case: "a role trusting lambda among several services",
            role: Role::Held(trust(
                "Allow",
                json!(["sts:TagSession", "sts:AssumeRole"]),
                json!({ "Service": ["ec2.amazonaws.com", LAMBDA_PRINCIPAL] }),
            )),
            outcome: Outcome::Served,
        },
        Row {
            case: "a role trusting only another service",
            role: Role::Held(trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": "ec2.amazonaws.com" }),
            )),
            outcome: Outcome::OperatorRoleInvalid,
        },
        Row {
            case: "a role whose statement DENIES lambda the assume",
            role: Role::Held(trust(
                "Deny",
                json!("sts:AssumeRole"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            )),
            outcome: Outcome::OperatorRoleInvalid,
        },
        Row {
            case: "a role trusting lambda for an action that is not assuming it",
            role: Role::Held(trust(
                "Allow",
                json!("s3:GetObject"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            )),
            outcome: Outcome::OperatorRoleInvalid,
        },
        Row {
            // "This account's IAM holds no role under that name" IS an answer about the
            // role the caller named here: the role a provider manages compute as is part of
            // what the client provisions in this account, and this operation is recorded
            // refusing an operator role that does not exist — the ABSENT-record half this
            // suite's own RULE states above. That is the reading
            // `crate::state::capacity_provider::operator_role_is_unassumable` implements
            // (the shared `service_common::iam_role::role_is_unassumable`), and what
            // separates it from a function's EXECUTION role, which is routinely provisioned
            // out of band and so serves such a create. This answer is AWS's RECORDED one
            // for this operation, so a fix for another issue must not flip it: if this row
            // fails in a gate, the change under review is what to fix.
            case: "a role this account's IAM holds none of",
            role: Role::AbsentFromIam,
            outcome: Outcome::OperatorRoleInvalid,
        },
        Row {
            // Creating the provider HANDS the role to this service, which is an
            // `iam:PassRole`, and a pass is granted by the account that HOLDS the role —
            // so no principal of this account may pass another account's role, whatever
            // that role trusts, and this emulator's IAM is never asked about it. That is
            // the reading
            // `crate::state::capacity_provider::operator_role_is_of_another_account`
            // implements. This answer too is AWS's RECORDED one for this operation (403
            // `AccessDeniedException`, "Cross-account pass role is not allowed."), and it
            // differs from the EXECUTION-role reading, which serves another account's role,
            // on purpose. A fix for another issue must not flip it: if this row fails in a
            // gate, the change under review is what to fix.
            case: "a well-formed role ARN of another account",
            role: Role::AnotherAccount("aws"),
            outcome: Outcome::CrossAccountPassRole,
        },
        Row {
            // The account field is what decides it, in any partition the member's pattern
            // admits.
            case: "another account's role in another partition",
            role: Role::AnotherAccount("aws-us-gov"),
            outcome: Outcome::CrossAccountPassRole,
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
            Role::AnotherAccount(partition) => {
                format!("arn:{partition}:iam::123456789012:role/capacity-operator")
            }
        };
        let name = format!("operator-probe-{index}");

        let (status, code, created) = lambda(
            &router,
            "POST",
            CAPACITY_PROVIDERS_URI,
            Some(create_body(&name, &role_arn)),
        )
        .await;

        let expected = row.outcome.status_and_code();
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {:?}: expected HTTP {} {:?} from the create, got {status} {code:?}: \
             {created}",
            row.case,
            expected.0,
            expected.1
        );
        match row.outcome.message() {
            None => assert_eq!(
                created["CapacityProvider"]["PermissionsConfig"]
                    ["CapacityProviderOperatorRoleArn"]
                    .as_str(),
                Some(role_arn.as_str()),
                "case {:?}: the provider is managed with the role the request named, got \
                 {created}",
                row.case
            ),
            Some(message) => {
                assert_eq!(
                    created[row.outcome.message_member()].as_str(),
                    Some(message),
                    "case {:?}: expected the refusal's fixed sentence, got {created}",
                    row.case
                );
                assert!(
                    !created.to_string().contains(&role_arn),
                    "case {:?}: the refusal must name no role, got {created}",
                    row.case
                );
            }
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
        let expected = if row.outcome.message().is_none() {
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
