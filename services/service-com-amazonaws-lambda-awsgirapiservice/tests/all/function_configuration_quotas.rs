// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACCOUNT QUOTAS on a function's `Timeout` and `MemorySize`.
//!
//! The rule under test: every operation that accepts a function's `Timeout` or
//! `MemorySize` judges the requested value against Lambda's published account quota —
//! 900 seconds and 10,240 MB — which is NARROWER than the range this model declares, and
//! refuses a value past it with `InvalidParameterValueException` (400) carrying the
//! service's own sentence for that member, before anything is stored. A refused request
//! changes nothing: no function is created, no stored setting moves, and no update window
//! opens, so the next request is answered as if the refused one had never been sent.
//!
//! The rows differ along what the rule generalises over: the MEMBER carrying the value
//! (`Timeout`, `MemorySize`, both at once), the VALUE relative to the quota (the modelled
//! minimum, a mid-range value, the quota itself, one past it, and the model's own maximum,
//! which is far past it), and the OPERATION that judges it (`CreateFunction` on a function
//! that does not exist yet, `UpdateFunctionConfiguration` on one that does).
//!
//! The bounds are the published quotas ("Function timeout: 900 seconds (15 minutes)",
//! "Function memory allocation: 128 MB to 10,240 MB"); the wider modelled ranges the rows
//! also walk come from the model (`smithy_query(query="shape", shape="Timeout")` gives
//! `@range(min: 1, max: 5400)` and `shape="MemorySize"` gives `@range(min: 128, max:
//! 32768)`), so each member is exercised at its published bound, one past it, and at the
//! modelled maximum the boundary validator still admits.
//!
//! Every row drives real HTTP requests through the router and reads what it persisted back
//! through the real `GetFunctionConfiguration`, so a hardcoded answer fails. Nothing here
//! sleeps: the clock-owed `Pending` -> `Active` and `InProgress` -> `Successful` moves are
//! brought forward by moving the stored due instants into the past.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, function_arn, json_call, make_activation_due, package_bytes, router_on,
    zip_file, Answer, ROLE,
};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// AWS's published Lambda quota "Function timeout: 900 seconds (15 minutes)", in seconds.
const QUOTA_TIMEOUT_SECONDS: i64 = 900;

/// AWS's published Lambda quota "Function memory allocation: 128 MB to 10,240 MB", in MB.
const QUOTA_MEMORY_SIZE_MB: i64 = 10_240;

/// The error a value outside the bound its member admits is refused with, the same one the
/// framework's own constraint walk answers (400): the channel both quota clauses are
/// answered through, because each states a bound on the value of a member.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `Timeout` the baseline function is created with, which every refused update must
/// leave in place. Inside the quota, and no default produces it.
const BASELINE_TIMEOUT: i64 = 7;

/// The `MemorySize` the baseline function is created with, under the same rule.
const BASELINE_MEMORY_SIZE: i64 = 256;

/// The `LastUpdateStatus` a settled function carries, from the model's own enum.
const SETTLED: &str = "Successful";

/// `GetFunctionConfiguration` on a function's unpublished configuration.
async fn get_configuration(router: &MultiServiceRouter, reference: &str) -> Answer {
    let uri = format!("{FUNCTIONS_URI}/{reference}/configuration");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `UpdateFunctionConfiguration` on a function's unpublished configuration.
async fn update_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    patch: &Value,
) -> Answer {
    let uri = format!("{FUNCTIONS_URI}/{reference}/configuration");
    json_call(router, "PUT", &uri, patch).await
}

/// Move every in-flight update's settle instant into the past, so the next read
/// materialises the `InProgress` -> `Successful` move the clock owes. This is what waiting
/// out the window would do, without waiting.
fn make_update_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// The sentence the service answers a `Timeout` past the quota with: the rejected value,
/// the member path and the quota it failed.
fn timeout_refusal(value: i64) -> String {
    format!(
        "Value '{value}' at 'timeout' failed to satisfy constraint: Member must have value \
         less than or equal to {QUOTA_TIMEOUT_SECONDS}"
    )
}

/// The sentence the service answers a `MemorySize` past the quota with: the member and the
/// quota it failed.
fn memory_size_refusal() -> String {
    format!(
        "'MemorySize' value failed to satisfy constraint: Member must have value less than \
         or equal to {QUOTA_MEMORY_SIZE_MB}"
    )
}

/// One row: the settings a request names, and what the service must answer.
struct Case {
    /// Names the row, and names its function, so rows never share state.
    case: &'static str,
    /// The `Timeout` and/or `MemorySize` the request carries.
    settings: Value,
    /// `None` when the service must accept the row; otherwise the exact sentence its
    /// refusal must carry.
    refusal: Option<String>,
}

/// The table: each member at its modelled minimum, mid-range, at the published quota, one
/// past the quota and at the modelled maximum, plus the rows that name both members at
/// once (so the row that breaks one quota while satisfying the other is covered both ways
/// round).
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "timeout-at-the-modelled-minimum",
            settings: json!({ "Timeout": 1 }),
            refusal: None,
        },
        Case {
            case: "timeout-mid-range",
            settings: json!({ "Timeout": 300 }),
            refusal: None,
        },
        Case {
            case: "timeout-at-the-quota",
            settings: json!({ "Timeout": QUOTA_TIMEOUT_SECONDS }),
            refusal: None,
        },
        Case {
            case: "timeout-one-second-past-the-quota",
            settings: json!({ "Timeout": QUOTA_TIMEOUT_SECONDS + 1 }),
            refusal: Some(timeout_refusal(QUOTA_TIMEOUT_SECONDS + 1)),
        },
        Case {
            case: "timeout-at-the-modelled-maximum",
            settings: json!({ "Timeout": 5400 }),
            refusal: Some(timeout_refusal(5400)),
        },
        Case {
            case: "memory-at-the-modelled-minimum",
            settings: json!({ "MemorySize": 128 }),
            refusal: None,
        },
        Case {
            case: "memory-mid-range",
            settings: json!({ "MemorySize": 3008 }),
            refusal: None,
        },
        Case {
            case: "memory-at-the-quota",
            settings: json!({ "MemorySize": QUOTA_MEMORY_SIZE_MB }),
            refusal: None,
        },
        Case {
            case: "memory-one-megabyte-past-the-quota",
            settings: json!({ "MemorySize": QUOTA_MEMORY_SIZE_MB + 1 }),
            refusal: Some(memory_size_refusal()),
        },
        Case {
            case: "memory-far-past-the-quota",
            settings: json!({ "MemorySize": 20480 }),
            refusal: Some(memory_size_refusal()),
        },
        Case {
            case: "memory-at-the-modelled-maximum",
            settings: json!({ "MemorySize": 32768 }),
            refusal: Some(memory_size_refusal()),
        },
        Case {
            case: "both-members-at-their-quota",
            settings: json!({ "Timeout": QUOTA_TIMEOUT_SECONDS, "MemorySize": QUOTA_MEMORY_SIZE_MB }),
            refusal: None,
        },
        Case {
            case: "memory-past-the-quota-with-a-timeout-inside-it",
            settings: json!({ "Timeout": 30, "MemorySize": QUOTA_MEMORY_SIZE_MB + 1 }),
            refusal: Some(memory_size_refusal()),
        },
        Case {
            case: "timeout-past-the-quota-with-a-memory-inside-it",
            settings: json!({ "Timeout": QUOTA_TIMEOUT_SECONDS + 1, "MemorySize": 512 }),
            refusal: Some(timeout_refusal(QUOTA_TIMEOUT_SECONDS + 1)),
        },
    ]
}

/// The members of `settings`, as (name, value) pairs.
fn settings_of(settings: &Value) -> Vec<(String, Value)> {
    settings
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new)
        .into_iter()
        .collect()
}

/// The `message` of a refusal body, however the protocol spells the member.
fn refusal_message(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// Assert that `answer` is the 400 the constraint channel answers, carrying exactly
/// `sentence`.
fn assert_refused(case: &str, answer: &Answer, sentence: &str) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected 400, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains(VALIDATION_EXCEPTION),
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        refusal_message(answer),
        sentence,
        "case {case:?}: expected the message {sentence:?}, got {:?}",
        refusal_message(answer)
    );
}

#[tokio::test]
async fn create_function_enforces_the_timeout_and_memory_quotas() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let code = package_bytes(48);

    for case in cases() {
        let name = format!("create-{}", case.case);
        let mut body = json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&code) },
        });
        for (member, value) in settings_of(&case.settings) {
            body[member] = value;
        }

        let answer = create_function(&router, &body).await;

        match case.refusal.as_ref() {
            None => {
                assert_eq!(
                    answer.status,
                    201,
                    "case {:?}: a configuration inside the quotas is created, got {} {}",
                    case.case,
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                // What it stored is what a later read reports.
                let read = get_configuration(&router, &name).await;
                assert_eq!(
                    read.status,
                    200,
                    "case {:?}: expected 200 from GetFunctionConfiguration, got {} {}",
                    case.case,
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                for (member, value) in settings_of(&case.settings) {
                    assert_eq!(
                        read.json()[&member],
                        value,
                        "case {:?}: expected {member} {value} to be stored, got {:?}",
                        case.case,
                        read.json()[&member]
                    );
                }
            }
            Some(sentence) => {
                assert_refused(case.case, &answer, sentence);
                // A refused create stores nothing, so the name is still free.
                let read = get_configuration(&router, &name).await;
                assert_eq!(
                    read.status,
                    404,
                    "case {:?}: a refused CreateFunction must store nothing, got {} {}",
                    case.case,
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                // ... and the same name then takes a configuration inside the quotas.
                let mut control = body.clone();
                control["Timeout"] = json!(QUOTA_TIMEOUT_SECONDS);
                control["MemorySize"] = json!(3008);
                let created = create_function(&router, &control).await;
                assert_eq!(
                    created.status,
                    201,
                    "case {:?}: the refused name must still be free, got {} {}",
                    case.case,
                    created.status,
                    String::from_utf8_lossy(&created.body)
                );
                let read = get_configuration(&router, &name).await;
                assert_eq!(
                    (read.json()["Timeout"].clone(), read.json()["MemorySize"].clone()),
                    (json!(QUOTA_TIMEOUT_SECONDS), json!(3008)),
                    "case {:?}: expected the control's configuration to be stored, got {:?}",
                    case.case,
                    read.json()
                );
            }
        }
    }
}

#[tokio::test]
async fn update_function_configuration_enforces_the_timeout_and_memory_quotas() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let code = package_bytes(48);

    for case in cases() {
        let name = format!("update-{}", case.case);
        let created = create_function(
            &router,
            &json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Description": "as created",
                "Timeout": BASELINE_TIMEOUT,
                "MemorySize": BASELINE_MEMORY_SIZE,
                "Code": { "ZipFile": zip_file(&code) },
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: expected 201 from CreateFunction, got {} {}",
            case.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        // "During this time, you can't invoke or modify the function" — the function is
        // brought to `Active` first, so a row is never answered the creation conflict.
        make_activation_due(&engine);

        let answer = update_configuration(&router, &name, &case.settings).await;

        match case.refusal.as_ref() {
            None => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {:?}: a configuration inside the quotas is applied, got {} {}",
                    case.case,
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                make_update_due(&engine);
                let read = get_configuration(&router, &name).await;
                for (member, value) in settings_of(&case.settings) {
                    assert_eq!(
                        read.json()[&member],
                        value,
                        "case {:?}: expected {member} {value} to be stored, got {:?}",
                        case.case,
                        read.json()[&member]
                    );
                }
            }
            Some(sentence) => {
                assert_refused(case.case, &answer, sentence);
                // A refused update applies nothing and starts no update window: the
                // stored configuration is the created one and the function is settled.
                let read = get_configuration(&router, &name).await;
                assert_eq!(
                    (
                        read.json()["Timeout"].clone(),
                        read.json()["MemorySize"].clone(),
                        read.json()["LastUpdateStatus"].clone()
                    ),
                    (
                        json!(BASELINE_TIMEOUT),
                        json!(BASELINE_MEMORY_SIZE),
                        json!(SETTLED)
                    ),
                    "case {:?}: expected the created configuration, settled, got {:?}",
                    case.case,
                    read.json()
                );
                // ... so the very next update, inside the quotas, is answered rather than
                // refused for an update that is still in flight.
                let applied = update_configuration(
                    &router,
                    &name,
                    &json!({ "Timeout": QUOTA_TIMEOUT_SECONDS, "MemorySize": 3008 }),
                )
                .await;
                assert_eq!(
                    applied.status,
                    200,
                    "case {:?}: expected the following update to be applied, got {} {}",
                    case.case,
                    applied.status,
                    String::from_utf8_lossy(&applied.body)
                );
                make_update_due(&engine);
                let read = get_configuration(&router, &name).await;
                assert_eq!(
                    (
                        read.json()["Timeout"].clone(),
                        read.json()["MemorySize"].clone()
                    ),
                    (json!(QUOTA_TIMEOUT_SECONDS), json!(3008)),
                    "case {:?}: expected the applied configuration, got {:?}",
                    case.case,
                    read.json()
                );
            }
        }
    }
}

#[tokio::test]
async fn an_over_quota_update_leaves_a_function_addressable_by_every_name_form() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let code = package_bytes(32);
    let name = "quota-name-forms";

    let created = create_function(
        &router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Timeout": BASELINE_TIMEOUT,
            "MemorySize": BASELINE_MEMORY_SIZE,
            "Code": { "ZipFile": zip_file(&code) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "expected 201 from CreateFunction, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(&engine);

    // The same rule however the function is addressed: a bare name, the unpublished
    // qualifier and the full ARN all reach the same record, so all three are refused.
    let arn = function_arn(name);
    let references = [
        ("bare name", name.to_string()),
        ("the unpublished qualifier", format!("{name}:$LATEST")),
        ("the function ARN", arn),
    ];
    for (case, reference) in references {
        let answer = update_configuration(
            &router,
            &reference,
            &json!({ "MemorySize": QUOTA_MEMORY_SIZE_MB + 1 }),
        )
        .await;
        assert_refused(case, &answer, &memory_size_refusal());
    }

    let read = get_configuration(&router, name).await;
    assert_eq!(
        read.json()["MemorySize"],
        json!(BASELINE_MEMORY_SIZE),
        "expected the created MemorySize {BASELINE_MEMORY_SIZE} to stand, got {:?}",
        read.json()["MemorySize"]
    );
}
