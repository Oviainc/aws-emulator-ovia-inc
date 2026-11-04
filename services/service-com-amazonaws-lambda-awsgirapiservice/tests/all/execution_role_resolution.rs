// SPDX-License-Identifier: CC0-1.0

//! What the execution-role check does when there is no IAM to ask.
//!
//! RULE: an operation that STORES the execution role a function runs as refuses the write
//! only on a TRUST DOCUMENT this account's IAM answered with. Nothing is KNOWN about a role
//! this emulator cannot ask about — an ARN naming another account's IAM, or an emulator with
//! no IAM service mounted at all — nor about a name this account's IAM holds no role under,
//! so the write goes ahead and the role is stored as supplied. The refusal grounded in a
//! stored trust document is asserted where both services can be mounted on one engine
//! (`core/router/tests/all/lambda_execution_role_trust.rs`); this suite pins the complement,
//! which is the behaviour of this crate ON ITS OWN.
//!
//! The rows differ along the form of the role ARN: the path the role lives under, how many
//! path segments it has, which characters the role name holds, which account the ARN names
//! and which partition — every one of them well-formed under the model's `RoleArn` pattern
//! (`smithy_query(query="shape", shape="RoleArn")`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, zip_file,
};

/// Bring an IN-FLIGHT configuration update's completion instant forward, the way
/// [`make_activation_due`] brings a creation's forward: "you can't modify the function"
/// while one is in progress, so each row below has to start from a settled function.
fn make_update_due(engine: &Arc<engine::InMemoryEngine>) {
    make_activation_due(engine);
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// Role ARNs the model's `RoleArn` pattern admits, spanning the forms one takes. None of
/// them is resolvable here: this target mounts Lambda alone, so there is no IAM to ask.
fn role_arns() -> Vec<(&'static str, String)> {
    vec![
        ("a role at the default path", "arn:aws:iam::000000000000:role/lambda-exec".to_string()),
        (
            "a role under a path",
            "arn:aws:iam::000000000000:role/service-role/lambda-exec".to_string(),
        ),
        (
            "a role under a nested path",
            "arn:aws:iam::000000000000:role/a/b/c/lambda-exec".to_string(),
        ),
        (
            // The characters IAM admits in a role name, per the pattern above.
            "a role name holding every admitted character class",
            "arn:aws:iam::000000000000:role/a_b-1+2=3,4.5@6".to_string(),
        ),
        (
            "another account's role",
            "arn:aws:iam::123456789012:role/remote-exec".to_string(),
        ),
        (
            "another partition's role",
            "arn:aws-us-gov:iam::000000000000:role/lambda-exec".to_string(),
        ),
        (
            "a role of the China partition",
            "arn:aws-cn:iam::000000000000:role/lambda-exec".to_string(),
        ),
    ]
}

/// RULE: with no IAM answer to ground a refusal on, `CreateFunction` stores the function
/// and reports the role the request named, in every form the member admits.
#[tokio::test]
async fn create_function_stores_a_role_no_iam_answered_about() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let code = package_bytes(64);

    for (index, (case, role)) in role_arns().iter().enumerate() {
        let name = format!("unresolvable-role-{index}");
        let answer = create_function(
            &router,
            &json!({
                "FunctionName": name,
                "Role": role,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&code) },
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: a role no IAM answered about must be accepted, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        // The stored role is what a later read reports, not what the create echoed.
        make_activation_due(&engine);
        let read = json_call(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the configuration to read back, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.string("Role").as_deref(),
            Some(role.as_str()),
            "case {case:?}: expected the stored role to be {role}, got {}",
            read.json()
        );
    }
}

/// RULE (the same, on the other write path): `UpdateFunctionConfiguration` replaces the
/// stored role with one no IAM answered about, and the replacement is what a later read
/// reports.
#[tokio::test]
async fn update_function_configuration_stores_a_role_no_iam_answered_about() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let code = package_bytes(64);
    let name = "unresolvable-role-update";

    let answer = create_function(
        &router,
        &json!({
            "FunctionName": name,
            "Role": "arn:aws:iam::000000000000:role/first-exec",
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&code) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case \"the function's first role\": expected 201 from CreateFunction, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );

    for (case, role) in role_arns() {
        make_update_due(&engine);
        let updated = json_call(
            &router,
            "PUT",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &json!({ "Role": role }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: a role no IAM answered about must be accepted, got {} {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );

        make_update_due(&engine);
        let read = json_call(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            read.string("Role").as_deref(),
            Some(role.as_str()),
            "case {case:?}: expected the stored role to be {role}, got {}",
            read.json()
        );
    }
}
