// SPDX-License-Identifier: CC0-1.0

//! The OPERATOR ROLE a capacity provider manages its compute as, judged against the IAM
//! records of the account the create was made in.
//!
//! RULE: `CreateCapacityProvider` is refused with the modelled
//! `InvalidParameterValueException` (HTTP 400), persisting nothing, when IAM ANSWERED that
//! the `CapacityProviderOperatorRoleArn` the request names is a role this service cannot
//! manage compute as — this account's IAM holds no role under the name the ARN carries, or
//! the trust policy of the role it does hold does not let this service assume it. Wherever
//! nothing is KNOWN about a role of THIS account the create is SERVED: an emulator with no
//! IAM mounted to ask at all, and an answer this emulator could not read.
//!
//! SECOND RULE, ahead of the first: a role ARN of ANOTHER account is refused
//! `AccessDeniedException` (HTTP 403) with the fixed sentence `Cross-account pass role is
//! not allowed.`, persisting nothing. The create HANDS the role to this service, which is an
//! `iam:PassRole`, and a pass is granted by the account that HOLDS the role — so a principal
//! of this account may pass a role of its own account and no role of another one, whatever
//! that other role trusts. It is decided on the request's own account field and asks IAM
//! nothing, so it stands with or without an IAM mounted.
//!
//! The role a provider manages compute as is part of what the client provisions in this
//! account — unlike a function's EXECUTION role, which is routinely provisioned out of band —
//! so "IAM holds no role under that name" is an answer about the role the request named. That
//! is the reading `state::capacity_provider::operator_role_is_unassumable` takes
//! (`service_common::iam_role::role_is_unassumable`), beside
//! `state::capacity_provider::operator_role_is_of_another_account` for the pass, and this
//! suite is what pins both from THIS crate: the cross-service suite
//! `core/router/tests/all/lambda_capacity_provider_operator_role.rs` pins the same rules over
//! its own table, and a change to either reading must fail here too rather than only there.
//!
//! The rows differ along what the rules generalise over: whether this account's IAM holds the
//! role at all, what its trust policy says (this service admitted, another principal
//! admitted, the grant denied), and whose account the ARN names, in this partition and in
//! another. The last test is the complement: with no IAM mounted nothing is known about a
//! role of this account, so such a create is served — while the pass of another account's
//! role, which needs no IAM to decide, is refused there too.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::capacity_provider::{
    CROSS_ACCOUNT_PASS_ROLE_MESSAGE, OPERATOR_ROLE_INVALID_MESSAGE,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::form_encode;

/// Where `CreateCapacityProvider` is bound.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// A credential scope naming this service, which is what routes a request to it.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/lambda/aws4_request, SignedHeaders=host, \
     Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The wire code of the modelled 400 the refusal is carried by.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The wire code of the 403 a pass of another account's role is refused with. It is in no
/// operation's modelled error list: the service answers it for a hand-off this account may
/// not make.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The service principal the operator role has to let assume it — the one this service
/// assumes a function's execution role as, since this service is what assumes both.
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The subnet every row's provider is created in, in the published `subnet-` id form.
const SUBNET: &str = "subnet-0123456789abcdef0";

/// Lambda alone, on its own engine: nothing is known about any role, which is the
/// complement this suite's last test pins.
fn router_without_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Lambda over the SAME engine as IAM, so the create resolves the operator role in the
/// service that owns it — exactly what `core/router` assembles.
fn router_with_iam() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Create one role through IAM's own `CreateRole`, and answer the ARN IAM persisted.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    trust: &Value,
) -> String {
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(name),
        form_encode(&trust.to_string()),
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: the role must be created in IAM first, got {status} {xml}"
    );
    let start = xml.find("<Arn>").map(|at| at + 5).unwrap_or_else(|| {
        panic!("case {case:?}: CreateRole must answer the role's ARN, got {xml}")
    });
    let end = xml[start..]
        .find("</Arn>")
        .map(|at| at + start)
        .unwrap_or_else(|| panic!("case {case:?}: the ARN is never closed in {xml}"));
    xml[start..end].to_string()
}

/// A trust policy with one statement: `effect` on `sts:AssumeRole` for `principal`.
fn trust(effect: &str, principal: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// `CreateCapacityProvider` naming `role_arn`: the status, the wire error code and the body.
async fn create_provider(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
) -> (u16, String, Value) {
    let body = json!({
        "CapacityProviderName": name,
        "VpcConfig": { "SubnetIds": [SUBNET], "SecurityGroupIds": [] },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": role_arn },
    });
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://lambda.us-east-1.amazonaws.com{CAPACITY_PROVIDERS_URI}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body must serialize"),
        ))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, payload) = response.into_parts();
    let answered: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    let code = answered
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            parts
                .headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    (parts.status.as_u16(), code, answered)
}

/// Whether the store holds a provider under `name`, which is what says a refusal persisted
/// nothing.
fn holds_provider(engine: &Arc<engine::InMemoryEngine>, name: &str) -> bool {
    engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .capacity_providers
        .contains_key(name)
}

/// What one row says about the operator role the create names.
enum Role {
    /// This account's IAM holds the role, with this trust policy.
    Held(Value),
    /// The ARN names a role of this account that IAM holds nothing under.
    AbsentFromIam,
    /// A well-formed role ARN of another account, in the partition the row names and under
    /// the path (`""` for none) it names.
    AnotherAccount {
        partition: &'static str,
        path: &'static str,
    },
}

/// What the create answers for the role a row names.
enum Outcome {
    /// Served: 202, and the provider is persisted.
    Served,
    /// Refused the modelled 400: IAM answered that this service cannot manage compute as
    /// the role.
    OperatorRoleInvalid,
    /// Refused 403: the role is another account's, so this account may not pass it.
    CrossAccountPassRole,
}

impl Outcome {
    /// The HTTP status and the wire error code the answer carries (`""` on a success).
    fn status_and_code(&self) -> (u16, &'static str) {
        match self {
            Self::Served => (202, ""),
            Self::OperatorRoleInvalid => (400, INVALID_PARAMETER),
            Self::CrossAccountPassRole => (403, ACCESS_DENIED),
        }
    }

    /// The refusal's fixed sentence and the member it arrives on — the modelled errors of
    /// this service spell it `message`, the synthetic 403 spells it `Message` — or `None`
    /// for a served create.
    fn refusal(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Served => None,
            Self::OperatorRoleInvalid => Some(("message", OPERATOR_ROLE_INVALID_MESSAGE)),
            Self::CrossAccountPassRole => Some(("Message", CROSS_ACCOUNT_PASS_ROLE_MESSAGE)),
        }
    }
}

/// Every row: an operator role IAM ANSWERED about and this service cannot assume is refused
/// the modelled 400, a role of another account is refused the 403 pass denial, and a role
/// nothing is known against is served. Both refusals persist nothing.
#[tokio::test]
async fn create_capacity_provider_judges_the_operator_role_iam_answered_about() {
    // (case, the role, what the create answers)
    let rows: Vec<(&str, Role, Outcome)> = vec![
        (
            "a held role whose trust policy admits this service",
            Role::Held(trust("Allow", LAMBDA_PRINCIPAL)),
            Outcome::Served,
        ),
        (
            "a held role whose trust policy admits another service",
            Role::Held(trust("Allow", "ecs-tasks.amazonaws.com")),
            Outcome::OperatorRoleInvalid,
        ),
        (
            "a held role whose trust policy DENIES this service",
            Role::Held(trust("Deny", LAMBDA_PRINCIPAL)),
            Outcome::OperatorRoleInvalid,
        ),
        (
            // The ABSENT-record half of the rule: the role a provider manages compute as is
            // part of what the client provisions here, so this IS an answer about it. The
            // answer is AWS's recorded one for this operation, and it differs from the
            // EXECUTION-role reading on purpose: a fix for another issue must not flip it.
            "a role this account's IAM holds none of",
            Role::AbsentFromIam,
            Outcome::OperatorRoleInvalid,
        ),
        (
            // The create HANDS the role over, which only the account HOLDING the role can
            // grant, so another account's role is one no principal here may pass — AWS's
            // recorded answer for this operation, and again deliberately unlike the
            // EXECUTION-role reading, which serves such a role. A fix for another issue
            // must not flip it.
            "a well-formed role ARN of another account",
            Role::AnotherAccount {
                partition: "aws",
                path: "",
            },
            Outcome::CrossAccountPassRole,
        ),
        (
            // The ACCOUNT field is what decides it: a path before the role's name and
            // another partition change nothing.
            "another account's role under a path in another partition",
            Role::AnotherAccount {
                partition: "aws-us-gov",
                path: "service-role/",
            },
            Outcome::CrossAccountPassRole,
        ),
    ];

    for (index, (case, role, outcome)) in rows.into_iter().enumerate() {
        let (router, engine) = router_with_iam();
        let role_arn = match &role {
            Role::Held(document) => {
                given_role(&router, case, &format!("capacity-operator-{index}"), document).await
            }
            Role::AbsentFromIam => {
                "arn:aws:iam::000000000000:role/capacity-operator-never-created".to_string()
            }
            Role::AnotherAccount { partition, path } => {
                format!("arn:{partition}:iam::123456789012:role/{path}capacity-operator")
            }
        };
        let name = format!("operator-probe-{index}");
        let (status, code, answered) = create_provider(&router, &name, &role_arn).await;

        let expected = outcome.status_and_code();
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {case:?}: expected HTTP {} {:?} from the create, got {status} {code:?}: \
             {answered}",
            expected.0,
            expected.1
        );
        match outcome.refusal() {
            None => {
                assert_eq!(
                    answered["CapacityProvider"]["PermissionsConfig"]
                        ["CapacityProviderOperatorRoleArn"]
                        .as_str(),
                    Some(role_arn.as_str()),
                    "case {case:?}: the provider is managed with the role the request named, \
                     got {answered}"
                );
                assert!(
                    holds_provider(&engine, &name),
                    "case {case:?}: a served create must persist the provider"
                );
            }
            Some((member, message)) => {
                assert_eq!(
                    answered[member].as_str(),
                    Some(message),
                    "case {case:?}: the refusal carries this surface's own sentence, got \
                     {answered}"
                );
                assert!(
                    !answered.to_string().contains(&role_arn),
                    "case {case:?}: the refusal must name no role, got {answered}"
                );
                assert!(
                    !holds_provider(&engine, &name),
                    "case {case:?}: a refused create must persist nothing"
                );
            }
        }
    }
}

/// The complement: with no IAM mounted there is nothing to ask, so nothing IAM would have
/// ANSWERED about a role of this account is refused — while the pass of another account's
/// role, which is decided on the request alone, is refused all the same.
#[tokio::test]
async fn an_emulator_with_no_iam_refuses_only_what_needs_no_answer() {
    // (case, the role, what the create answers)
    let roles: Vec<(&str, &str, Outcome)> = vec![
        (
            "a role of this account no IAM can be asked about",
            "arn:aws:iam::000000000000:role/capacity-operator-never-created",
            Outcome::Served,
        ),
        (
            "another account's role with no IAM mounted",
            "arn:aws:iam::123456789012:role/capacity-operator",
            Outcome::CrossAccountPassRole,
        ),
    ];
    for (index, (case, role_arn, outcome)) in roles.into_iter().enumerate() {
        let router = router_without_iam();
        let (status, code, answered) =
            create_provider(&router, &format!("no-iam-probe-{index}"), role_arn).await;
        let expected = outcome.status_and_code();
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {case:?}: expected HTTP {} {:?} with no IAM mounted, got {status} \
             {code:?}: {answered}",
            expected.0,
            expected.1
        );
        if let Some((member, message)) = outcome.refusal() {
            assert_eq!(
                answered[member].as_str(),
                Some(message),
                "case {case:?}: the refusal carries its own fixed sentence, got {answered}"
            );
        }
    }
}
