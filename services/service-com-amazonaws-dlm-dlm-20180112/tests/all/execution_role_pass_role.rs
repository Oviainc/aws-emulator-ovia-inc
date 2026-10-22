// SPDX-License-Identifier: CC0-1.0

//! `iam:PassRole` for an `ExecutionRoleArn`, end to end over HTTP.
//!
//! Amazon Data Lifecycle Manager assumes the role a policy names, so every
//! operation that takes an `ExecutionRoleArn` authorizes `iam:PassRole` for it
//! before the operation runs. The rule under test:
//!
//! > an `ExecutionRoleArn` that denotes an IAM principal the caller may not pass
//! > — one in another account, or one that is not a role — is refused with the
//! > error code `AccessDeniedException` (HTTP 403) and changes no state, while a
//! > value that is not a well-formed IAM ARN at all names no principal to
//! > authorize and stays the member's own shape refusal (this service's modelled
//! > `InvalidRequestException`, HTTP 400).
//!
//! Every row goes through the router, so each case exercises routing, the
//! authorization step, the member-constraint walk and the handler — the same path
//! a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the requests in this file are signed for.
const REGION: &str = "eu-west-1";

/// The error code an AWS authorization failure carries, and the status that goes
/// with it.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The error code this service refuses a value outside a modelled constraint
/// with, and the status its `@httpError` declares.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_EXCEPTION_STATUS: u16 = 400;

/// What the service must answer for one `ExecutionRoleArn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// `iam:PassRole` is granted: the call succeeds and the ARN is stored.
    Passed,
    /// `iam:PassRole` is denied: `AccessDeniedException`, and no state changes.
    Denied,
    /// Not an IAM ARN, so there is no principal to authorize: the member's own
    /// `@pattern` refuses the request.
    Malformed,
}

/// Every `ExecutionRoleArn` class the rule spans, with the answer it is owed.
///
/// The rows differ along what the rule generalises over: the account the ARN
/// names, its partition (`^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/` admits
/// `aws`, `aws-cn`, `aws-us-gov`, from
/// `smithy_query(query="operation", operation="CreateLifecyclePolicy")`), the IAM
/// resource type it names, whether it names a resource at all, how deep the role
/// path is, and whether it parses as an IAM ARN. The issue cited only the first
/// row.
fn cases() -> Vec<(&'static str, String, Expected)> {
    // The one account this emulator presents, so the only account whose roles it
    // can pass. Read from the framework rather than written out, which is also
    // what makes the "another account" rows below genuinely foreign.
    let own = engine::emulator_account_id();
    vec![
        (
            "an IAM user in another account",
            "arn:aws:iam::111122223333:user/not-a-role".to_string(),
            Expected::Denied,
        ),
        (
            "a role in another account",
            "arn:aws:iam::111122223333:role/CrossAccountDlmRole".to_string(),
            Expected::Denied,
        ),
        (
            "a role in another account, in another partition",
            "arn:aws-cn:iam::111122223333:role/CrossAccountDlmRole".to_string(),
            Expected::Denied,
        ),
        (
            "an IAM user in this account",
            format!("arn:aws:iam::{own}:user/not-a-role"),
            Expected::Denied,
        ),
        (
            "an IAM group in this account",
            format!("arn:aws:iam::{own}:group/not-a-role"),
            Expected::Denied,
        ),
        (
            "an instance profile in this account",
            format!("arn:aws:iam::{own}:instance-profile/not-a-role"),
            Expected::Denied,
        ),
        (
            "the role resource type naming no role",
            format!("arn:aws:iam::{own}:role/"),
            Expected::Denied,
        ),
        (
            "a role in this account",
            format!("arn:aws:iam::{own}:role/AWSDataLifecycleManagerDefaultRole"),
            Expected::Passed,
        ),
        (
            "a role in this account under a path",
            format!("arn:aws:iam::{own}:role/service-role/nested/dlm-role"),
            Expected::Passed,
        ),
        (
            "a role in this account in the aws-cn partition",
            format!("arn:aws-cn:iam::{own}:role/dlm-role"),
            Expected::Passed,
        ),
        (
            "a role in this account in the aws-us-gov partition",
            format!("arn:aws-us-gov:iam::{own}:role/dlm-role"),
            Expected::Passed,
        ),
        (
            "not an ARN at all",
            "not-an-arn".to_string(),
            Expected::Malformed,
        ),
        (
            "an ARN of another service",
            format!("arn:aws:sts::{own}:role/NotAnIamRole"),
            Expected::Malformed,
        ),
        (
            "an IAM ARN carrying a region, which IAM ARNs do not",
            format!("arn:aws:iam:{REGION}:{own}:role/Regional"),
            Expected::Malformed,
        ),
        (
            "an IAM ARN whose account is not digits",
            "arn:aws:iam::not-an-account:role/dlm-role".to_string(),
            Expected::Malformed,
        ),
    ]
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, however the envelope spells the member.
    fn error_message(&self) -> &str {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
    }

    /// The `PolicyId` `CreateLifecyclePolicy` returned.
    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A SigV4 credential scope naming this service in `REGION`; it is what routes a
/// request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!("{method} {uri} answered a body that is not JSON ({error}): {bytes:?}")
        })
    };
    Answer { status, body }
}

/// A `CreateLifecyclePolicy` request naming `role` and `description`, otherwise
/// the custom EBS snapshot policy the issue's request carried.
fn create_request(role: &str, description: &str) -> Value {
    json!({
        "ExecutionRoleArn": role,
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": {
            "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
            "ResourceTypes": ["VOLUME"],
            "TargetTags": [{"Key": "backup", "Value": "daily"}],
            "Schedules": [{
                "Name": "daily",
                "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS", "Times": ["03:00"]},
                "RetainRule": {"Count": 1},
            }],
        },
    })
}

/// Every policy id the store holds, as `GetLifecyclePolicies` reports them.
async fn stored_ids(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(router, "GET", "/policies", None).await;
    answer.body["Policies"]
        .as_array()
        .unwrap_or_else(|| panic!("GetLifecyclePolicies must return a list: {:?}", answer.body))
        .iter()
        .map(|summary| summary["PolicyId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `ExecutionRoleArn` the policy `id` names has stored.
async fn stored_role(router: &MultiServiceRouter, id: &str) -> String {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    answer.body["Policy"]["ExecutionRoleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `CreateLifecyclePolicy` authorizes the `ExecutionRoleArn` it is given, and a
/// denied one leaves the store exactly as it was.
#[tokio::test]
async fn create_authorizes_the_execution_role_it_is_given() {
    let router = emulator();

    for (index, (case, role, expected)) in cases().into_iter().enumerate() {
        let description = format!("pass role case {index}");
        let before = stored_ids(&router).await;
        let answer = call(
            &router,
            "POST",
            "/policies",
            Some(create_request(&role, &description)),
        )
        .await;
        let after = stored_ids(&router).await;

        match expected {
            Expected::Passed => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} ({role}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let id = answer.policy_id();
                assert!(
                    after.contains(&id),
                    "case {case:?} ({role}): expected the created policy {id} in the store, \
                     got {after:?}"
                );
                assert_eq!(
                    stored_role(&router, &id).await,
                    role,
                    "case {case:?}: expected the stored ExecutionRoleArn to be the created one, \
                     got {:?}",
                    stored_role(&router, &id).await
                );
            }
            Expected::Denied => {
                assert_eq!(
                    answer.status, ACCESS_DENIED_STATUS,
                    "case {case:?} ({role}): expected HTTP {ACCESS_DENIED_STATUS}, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    ACCESS_DENIED,
                    "case {case:?} ({role}): expected {ACCESS_DENIED}, got {:?}",
                    answer.error_code()
                );
                assert!(
                    !answer.error_message().is_empty(),
                    "case {case:?} ({role}): a denial must say why, got {:?}",
                    answer.body
                );
                assert_eq!(
                    after, before,
                    "case {case:?} ({role}): a denied create must leave no policy behind, \
                     got {after:?}"
                );
            }
            Expected::Malformed => {
                assert_eq!(
                    answer.status, INVALID_REQUEST_EXCEPTION_STATUS,
                    "case {case:?} ({role}): expected HTTP {INVALID_REQUEST_EXCEPTION_STATUS}, \
                     got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    INVALID_REQUEST_EXCEPTION,
                    "case {case:?} ({role}): expected {INVALID_REQUEST_EXCEPTION}, got {:?}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.body["Code"],
                    json!("InvalidParameter"),
                    "case {case:?} ({role}): the refusal must carry the modelled Code, got {:?}",
                    answer.body
                );
                assert_eq!(
                    after, before,
                    "case {case:?} ({role}): a refused create must leave no policy behind, \
                     got {after:?}"
                );
            }
        }
    }
}

/// `UpdateLifecyclePolicy` authorizes the `ExecutionRoleArn` it is given by the
/// same rule, and a denied one leaves the stored policy as it was.
#[tokio::test]
async fn update_authorizes_the_execution_role_it_is_given() {
    let router = emulator();
    let original = format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    );
    let created = call(
        &router,
        "POST",
        "/policies",
        Some(create_request(&original, "pass role update subject")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the subject policy must be created: {:?}",
        created.body
    );
    let id = created.policy_id();
    let mut current = original.clone();

    for (case, role, expected) in cases() {
        let answer = call(
            &router,
            "PATCH",
            &format!("/policies/{id}"),
            Some(json!({"ExecutionRoleArn": role})),
        )
        .await;
        let (status, code) = match expected {
            Expected::Passed => (200, ""),
            Expected::Denied => (ACCESS_DENIED_STATUS, ACCESS_DENIED),
            Expected::Malformed => (
                INVALID_REQUEST_EXCEPTION_STATUS,
                INVALID_REQUEST_EXCEPTION,
            ),
        };
        assert_eq!(
            answer.status, status,
            "case {case:?} ({role}): expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?} ({role}): expected error code {code:?}, got {:?}",
            answer.error_code()
        );
        if expected == Expected::Passed {
            current = role.clone();
        }
        assert_eq!(
            stored_role(&router, &id).await,
            current,
            "case {case:?} ({role}): expected the stored ExecutionRoleArn to be {current}, got {:?}",
            stored_role(&router, &id).await
        );
    }
}

/// An update that carries no `ExecutionRoleArn` has no role to authorize, so the
/// rule does not touch it: it still updates what it carries.
#[tokio::test]
async fn an_update_naming_no_role_is_unaffected() {
    let router = emulator();
    let role = format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    );
    let created = call(
        &router,
        "POST",
        "/policies",
        Some(create_request(&role, "pass role untouched subject")),
    )
    .await;
    let id = created.policy_id();

    let answer = call(
        &router,
        "PATCH",
        &format!("/policies/{id}"),
        Some(json!({"State": "DISABLED"})),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "an update naming no ExecutionRoleArn must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        stored_role(&router, &id).await,
        role,
        "the stored ExecutionRoleArn must be unchanged, got {:?}",
        stored_role(&router, &id).await
    );
}
