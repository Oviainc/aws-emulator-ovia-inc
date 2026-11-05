// SPDX-License-Identifier: CC0-1.0

//! The OPERATOR ROLE a capacity provider manages its compute as, judged against the IAM
//! records of the account the create was made in.
//!
//! RULE: `CreateCapacityProvider` is refused with the modelled
//! `InvalidParameterValueException` (HTTP 400), persisting nothing, when IAM ANSWERED that
//! the `CapacityProviderOperatorRoleArn` the request names is a role this service cannot
//! manage compute as — this account's IAM holds no role under the name the ARN carries, or
//! the trust policy of the role it does hold does not let this service assume it. Wherever
//! nothing is KNOWN the create is SERVED: a role ARN of another account (whose IAM is not
//! this emulator's to ask), and an emulator with no IAM mounted to ask at all.
//!
//! The role a provider manages compute as is part of what the client provisions in this
//! account — unlike a function's EXECUTION role, which is routinely provisioned out of band —
//! so "IAM holds no role under that name" is an answer about the role the request named. That
//! is the reading `state::capacity_provider::operator_role_is_unassumable` takes
//! (`service_common::iam_role::role_is_unassumable`), and this suite is what pins it from
//! THIS crate: the cross-service suite
//! `core/router/tests/all/lambda_capacity_provider_operator_role.rs` pins the same rule over
//! its own table, and a change to the reading must fail here too rather than only there.
//!
//! The rows differ along what the rule generalises over: whether this account's IAM holds the
//! role at all, what its trust policy says (this service admitted, another principal
//! admitted, the grant denied), and whose account the ARN names. The last test is the
//! complement: with no IAM mounted nothing is known, so every one of those roles is served.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::capacity_provider::OPERATOR_ROLE_INVALID_MESSAGE;
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
    /// A well-formed role ARN of another account.
    AnotherAccount,
}

/// Every row: an operator role IAM ANSWERED about and this service cannot assume is refused
/// and stores nothing; one nothing is known against is served.
#[tokio::test]
async fn create_capacity_provider_judges_the_operator_role_iam_answered_about() {
    // (case, the role, whether the create is served)
    let rows: Vec<(&str, Role, bool)> = vec![
        (
            "a held role whose trust policy admits this service",
            Role::Held(trust("Allow", LAMBDA_PRINCIPAL)),
            true,
        ),
        (
            "a held role whose trust policy admits another service",
            Role::Held(trust("Allow", "ecs-tasks.amazonaws.com")),
            false,
        ),
        (
            "a held role whose trust policy DENIES this service",
            Role::Held(trust("Deny", LAMBDA_PRINCIPAL)),
            false,
        ),
        (
            // The ABSENT-record half of the rule: the role a provider manages compute as is
            // part of what the client provisions here, so this IS an answer about it.
            "a role this account's IAM holds none of",
            Role::AbsentFromIam,
            false,
        ),
        (
            // Another account's IAM is not this emulator's to ask, so nothing is known.
            "a well-formed role ARN of another account",
            Role::AnotherAccount,
            true,
        ),
    ];

    for (index, (case, role, served)) in rows.into_iter().enumerate() {
        let (router, engine) = router_with_iam();
        let role_arn = match &role {
            Role::Held(document) => {
                given_role(&router, case, &format!("capacity-operator-{index}"), document).await
            }
            Role::AbsentFromIam => {
                "arn:aws:iam::000000000000:role/capacity-operator-never-created".to_string()
            }
            Role::AnotherAccount => "arn:aws:iam::123456789012:role/capacity-operator".to_string(),
        };
        let name = format!("operator-probe-{index}");
        let (status, code, answered) = create_provider(&router, &name, &role_arn).await;

        if served {
            assert_eq!(
                (status, code.as_str()),
                (202, ""),
                "case {case:?}: a role nothing is KNOWN against must create the provider, got \
                 {status} {code:?}: {answered}"
            );
            assert_eq!(
                answered["CapacityProvider"]["PermissionsConfig"]
                    ["CapacityProviderOperatorRoleArn"]
                    .as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the provider is managed with the role the request named, got \
                 {answered}"
            );
            assert!(
                holds_provider(&engine, &name),
                "case {case:?}: a served create must persist the provider"
            );
            continue;
        }
        assert_eq!(
            (status, code.as_str()),
            (400, INVALID_PARAMETER),
            "case {case:?}: a role this service cannot assume must be refused, got {status} \
             {code:?}: {answered}"
        );
        assert_eq!(
            answered["message"].as_str(),
            Some(OPERATOR_ROLE_INVALID_MESSAGE),
            "case {case:?}: the refusal carries this surface's own sentence, got {answered}"
        );
        assert!(
            !holds_provider(&engine, &name),
            "case {case:?}: a refused create must persist nothing"
        );
    }
}

/// The complement: with no IAM mounted there is nothing to ask, so NOTHING is refused —
/// including the role a mounted IAM would have answered about.
#[tokio::test]
async fn an_emulator_with_no_iam_knows_nothing_and_refuses_nothing() {
    let roles: [(&str, &str); 2] = [
        (
            "a role no IAM can be asked about",
            "arn:aws:iam::000000000000:role/capacity-operator-never-created",
        ),
        (
            "another account's role with no IAM mounted",
            "arn:aws:iam::123456789012:role/capacity-operator",
        ),
    ];
    for (index, (case, role_arn)) in roles.into_iter().enumerate() {
        let router = router_without_iam();
        let (status, code, answered) =
            create_provider(&router, &format!("no-iam-probe-{index}"), role_arn).await;
        assert_eq!(
            (status, code.as_str()),
            (202, ""),
            "case {case:?}: a refusal has to be grounded in what IAM said, and nothing was \
             asked, got {status} {code:?}: {answered}"
        );
    }
}
