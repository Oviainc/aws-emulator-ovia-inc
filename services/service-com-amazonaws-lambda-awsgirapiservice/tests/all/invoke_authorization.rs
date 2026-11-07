// SPDX-License-Identifier: CC0-1.0

//! That the invoke-authorization decision is taken BEFORE anything the function's own state
//! decides — one of the two complements of the suite beside it
//! (`tests/all/invoke_identity_authorization.rs`), the other being the ceiling a permissions
//! boundary puts on the grant (`tests/all/invoke_authorization_ceiling.rs`, which drives the
//! principals through the fixtures this module owns).
//!
//! RULE: an invocation is decided at the identity layer, ahead of everything the function's
//! own state decides, so a caller whose policies do not allow the invoke action on the
//! function it addressed is refused with HTTP 403 `AccessDeniedException` — whichever
//! invocation type it asks for — where the same call by a permitted caller is refused for the
//! state the function is in.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! mounted beside Lambda on ONE engine; the principal is created through the real `CreateUser`
//! and `CreateAccessKey`, and each request is signed with the key id IAM minted, so the
//! principal the gate judges is the one the credential speaks for.
//!
//! The rows differ along what the rule generalises over: which invocation TYPE the
//! unauthorized caller asks for — the synchronous one, the asynchronous one, and the dry run,
//! which "verif[ies] that the user or role has permission to invoke the function" and runs
//! nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{element, iam, router_with_iam};
use crate::event_source_mappings::{call, call_signed_by, Answer};

/// Where `CreateFunction` and the invocation path are bound.
pub(crate) const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The region the harness's credential scope names, which is what the ARNs below carry.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The wire code of the refusal every unauthorized row is answered with.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The IAM action a synchronous invocation is authorized under. Source: the action AWS
/// publishes for invoking a function ("Actions defined by AWS Lambda", IAM Service
/// Authorization Reference), which is also the one `AddPermission` grants.
pub(crate) const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The handler every function below runs: its answer is computed from the event, so a row that
/// reports it really ran the package.
pub(crate) const HANDLER_SOURCE: &str =
    "def lambda_handler(event, context):\n    return {'invoked': event['n'] * 2}\n";

/// The event every invocation below carries.
pub(crate) fn event() -> Value {
    json!({ "n": 21 })
}

/// What the handler answers for [`event`].
fn answered() -> Value {
    json!({ "invoked": 42 })
}

/// The ARN of the function `name` of this account, in the Region the requests name.
pub(crate) fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN of the IAM user `name` of this account, at the default path IAM gives an entity
/// whose creation names none.
pub(crate) fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{name}")
}

/// One policy document carrying `statements`.
pub(crate) fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// One statement of a policy document.
pub(crate) fn statement(effect: &str, action: &str, resource: &str) -> Value {
    json!({ "Effect": effect, "Action": action, "Resource": resource })
}

/// Create the IAM user `name`, embed `policy` in it as its one inline policy when the row
/// states one, put `boundary` on it as its permissions boundary when the row states one, and
/// answer the access key id a request signs with to speak for it.
///
/// `pub(crate)` because both complements build their principals this one way, through IAM's
/// own operations.
pub(crate) async fn principal(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    policy: Option<String>,
    boundary: Option<String>,
) -> String {
    let (status, created) = iam(router, "CreateUser", &[("UserName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser {name:?} must succeed, got {status}: {created}"
    );
    if let Some(document) = &policy {
        let (status, written) = iam(
            router,
            "PutUserPolicy",
            &[
                ("UserName", name),
                ("PolicyName", "invoke-grant"),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture PutUserPolicy on {name:?} must succeed, got {status}: \
             {written}"
        );
    }
    if let Some(document) = &boundary {
        let policy_name = format!("{name}-ceiling");
        let (status, created) = iam(
            router,
            "CreatePolicy",
            &[("PolicyName", &policy_name), ("PolicyDocument", document)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreatePolicy {policy_name:?} must succeed, got \
             {status}: {created}"
        );
        let arn = element(&created, "Arn");
        let (status, put) = iam(
            router,
            "PutUserPermissionsBoundary",
            &[("UserName", name), ("PermissionsBoundary", &arn)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture PutUserPermissionsBoundary on {name:?} must succeed, \
             got {status}: {put}"
        );
    }
    let (status, minted) = iam(router, "CreateAccessKey", &[("UserName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey for {name:?} must succeed, got {status}: \
         {minted}"
    );
    element(&minted, "AccessKeyId")
}

/// `Invoke` on the function `name`, signed with the credential `access_key_id` names (the
/// harness's own when it names none), carrying the headers `headers` states.
pub(crate) async fn invoke_as(
    router: &MultiServiceRouter,
    access_key_id: Option<&str>,
    name: &str,
    headers: &[(&str, &str)],
) -> Answer {
    call_signed_by(
        router,
        access_key_id,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &event(),
        headers,
    )
    .await
}

/// The sentence an unauthorized invocation of `function` by `principal` is refused with.
pub(crate) fn refusal(principal: &str, function: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {INVOKE_ACTION} on resource: \
         {function} {reason}"
    )
}

/// The reason a refusal states when nothing the caller carries allows the invocation.
fn no_identity_grant() -> String {
    format!("because no identity-based policy allows the {INVOKE_ACTION} action")
}

/// Assert that `answer` is the run: HTTP 200 carrying what the handler returned.
pub(crate) fn assert_invoked(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the invocation to be served, got {} {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.json(),
        answered(),
        "case {case:?}: expected the handler's own answer {}, got {}",
        answered(),
        answer.text()
    );
}

/// Assert that `answer` is the authorization refusal carrying `expected` as its message.
pub(crate) fn assert_denied(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status,
        403,
        "case {case:?}: expected the invocation to be refused with 403, got {} {}",
        answer.status,
        answer.text()
    );
    // The wire code carries the shape's namespace on some protocols, so the comparison is
    // against the shape NAME alone — another 403 channel of this service
    // (`KMSAccessDeniedException`, say) must not pass as this one.
    let code = answer.error_code();
    let named = code.rsplit('#').next().unwrap_or(&code).to_string();
    assert_eq!(
        named, ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED}, got {code:?}: {}",
        answer.text()
    );
    assert_eq!(
        answer.string("Message").unwrap_or_default(),
        expected,
        "case {case:?}: expected {expected:?}, got {}",
        answer.text()
    );
}

/// RULE: the decision is taken at the identity layer, ahead of everything the function's own
/// state decides — so an unauthorized caller is refused `AccessDeniedException` where the same
/// call by a permitted caller is refused for the function's state, whichever invocation type is
/// asked for.
#[tokio::test]
async fn the_authorization_decision_precedes_the_functions_own_state() {
    let router = router_with_iam();
    let function = "still-provisioning";
    // Created and invoked at once, so the provisioning window the create documents is still
    // open: nothing here waits it out.
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": function,
            "Role": format!("arn:aws:iam::{ACCOUNT}:role/service-role/lambda-exec"),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Code": {
                "ZipFile": ::base64::Engine::encode(
                    &::base64::engine::general_purpose::STANDARD,
                    crate::synchronous_invoke_run::package(HANDLER_SOURCE),
                ),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case \"the fixture CreateFunction\": expected 201, got {} {}",
        created.status,
        created.text()
    );

    let user = "unpermitted-caller";
    let key = principal(&router, "a caller carrying no grant", user, None, None).await;
    let expected = refusal(
        &user_arn(user),
        &function_arn(function),
        &no_identity_grant(),
    );

    // (case, the headers the invocation carries)
    let invocations: [(&str, &[(&str, &str)]); 3] = [
        ("a synchronous invocation", &[]),
        (
            "an asynchronous invocation",
            &[("x-amz-invocation-type", "Event")],
        ),
        (
            "a dry run, which verifies the permission and runs nothing",
            &[("x-amz-invocation-type", "DryRun")],
        ),
    ];
    for (case, headers) in invocations {
        let refused = invoke_as(&router, Some(&key), function, headers).await;
        assert_denied(case, &refused, &expected);
    }

    // The same call by the account's own credential reaches the function, and is refused for
    // the state the function is in rather than for the caller — which is what shows the
    // authorization decision above was taken FIRST.
    let permitted = invoke_as(&router, None, function, &[]).await;
    assert_eq!(
        permitted.status,
        409,
        "case \"the account's own credential\": expected the state refusal, got {} {}",
        permitted.status,
        permitted.text()
    );
    assert!(
        permitted.error_code().ends_with("ResourceConflictException"),
        "case \"the account's own credential\": expected the state refusal, got {:?}: {}",
        permitted.error_code(),
        permitted.text()
    );
}
