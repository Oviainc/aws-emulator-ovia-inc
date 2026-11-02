// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function version's runtime management configuration:
//! `PutRuntimeManagementConfig` "Sets the runtime management configuration for a
//! function's version" and `GetRuntimeManagementConfig`, which reports it.
//!
//! The rule under test: the runtime management configuration is one value every version
//! of a function always has — the documented `Auto` with no runtime version ARN until a
//! `PutRuntimeManagementConfig` call stores another for THAT version, and from then on
//! the mode of the most recent such call, carrying a `RuntimeVersionArn` exactly when
//! the mode is `Manual` ("If the runtime update mode is manual, the ARN is returned,
//! otherwise null is returned") — so a request pairing `Manual` with no ARN, or any
//! other mode with one, is refused `InvalidParameterValueException` (HTTP 400) and
//! stores nothing, and a name or qualifier that names no version is refused
//! `ResourceNotFoundException` (HTTP 404).
//!
//! Every row drives real HTTP requests through the router and reads the configuration
//! back through `GetRuntimeManagementConfig`, so a value that was answered but never
//! stored fails here. The rows differ along what the rule generalises over: the modelled
//! mode (the whole [`UPDATE_RUNTIME_ON_VALUES`] set), the form of the runtime version
//! ARN (both branches of the member's own pattern, see [`RUNTIME_VERSION_ARNS`]), the
//! version the configuration is addressed on (`$LATEST` implicitly, `$LATEST`
//! explicitly, a published version number, a qualified `FunctionName`, an alias), the
//! documented `FunctionName` form, and how many calls have been made (none, one,
//! several).
//!
//! The reporting side has rules of its own, pinned by the last three suites below:
//! `GetRuntimeManagementConfig` resolves the `FunctionName`/`Qualifier` pair to ONE
//! version of one stored function, so every documented way of naming that version reads
//! the same stored value and reports the same canonical `FunctionArn`; a pair whose two
//! halves name two DIFFERENT versions names no single resource and is refused
//! `InvalidParameterValueException` (HTTP 400) without storing anything; and because the
//! configuration belongs to the version, a qualifier naming an alias reads (and writes)
//! the configuration of the version that alias invokes.
//!
//! Storing a configuration is an update of the function, and Lambda refuses a second
//! modification while that update is still being applied (`function_update_window_conflict.rs`
//! pins that refusal), so a suite that writes twice settles the first update in between by
//! moving its stored due instant into the past — what waiting would do, without waiting.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The whole `UpdateRuntimeOn` value set, from
/// `smithy_query(query="shape", shape="UpdateRuntimeOn")`: `Auto | Manual |
/// FunctionUpdate`.
const UPDATE_RUNTIME_ON_VALUES: &[&str] = &["Auto", "Manual", "FunctionUpdate"];

/// The mode a version carries before anyone sets one, which the model's own
/// `UpdateRuntimeOn` member documentation states: "Auto (default) - Automatically
/// update to the most recent and secure runtime version".
const DEFAULT_UPDATE_RUNTIME_ON: &str = "Auto";

/// The only mode that pins a runtime version: "You specify a runtime version in your
/// function configuration."
const MANUAL: &str = "Manual";

/// `UpdateRuntimeOn`'s value set as this service REPORTS it in a constraint refusal —
/// the same values, in the order AWS lists them (`src/service.rs`'s
/// `REPORTED_ENUM_ORDERS`, recorded from the AWS answer the corpus carries), which is
/// neither the model's declaration order nor ascending order.
const REPORTED_VALUE_SET: &str = "FunctionUpdate, Manual, Auto";

/// One runtime version ARN per branch of the `RuntimeVersionArn` pattern, from
/// `smithy_query(query="shape", shape="RuntimeVersionArn")`:
/// `arn:<partition>:lambda:<region>::runtime:<id>` and
/// `arn:<partition>:lambda:<region>:<account>:runtime/<name>/version/<version>`.
const RUNTIME_VERSION_ARNS: &[&str] = &[
    "arn:aws:lambda:us-east-1::runtime:8eeff65f6809a3ce81507fe733fe09b835899b99481ba22fd75b5a7338290ec1",
    "arn:aws:lambda:us-east-1:000000000000:runtime/python3.12/version/a1b2c3d4e5",
];

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Where both runtime-management operations are bound, for the function `reference`
/// names, optionally with the `Qualifier` query parameter.
fn runtime_management_uri(reference: &str, qualifier: Option<&str>) -> String {
    let base = format!("/2021-07-20/functions/{reference}/runtime-management-config");
    match qualifier {
        Some(qualifier) => format!("{base}?Qualifier={qualifier}"),
        None => base,
    }
}

/// `GetRuntimeManagementConfig` on the version `reference` and `qualifier` name.
async fn get_runtime_management_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(
        router,
        "GET",
        &runtime_management_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// `PutRuntimeManagementConfig` on the version `reference` and `qualifier` name.
async fn put_runtime_management_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    update_runtime_on: &str,
    runtime_version_arn: Option<&str>,
) -> Answer {
    let mut body = json!({ "UpdateRuntimeOn": update_runtime_on });
    if let Some(arn) = runtime_version_arn {
        body["RuntimeVersionArn"] = json!(arn);
    }
    call(
        router,
        "PUT",
        &runtime_management_uri(reference, qualifier),
        &body,
    )
    .await
}

/// Create a function to configure.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Publish one version of `name` and report the version label it was given.
async fn publish_version(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/versions"),
        &json!({}),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "PublishVersion {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .string("Version")
        .unwrap_or_else(|| panic!("PublishVersion {name:?} must report a Version: {}", answer.text()))
}

/// `CreateAlias`: one alias of `function`, named `alias`, invoking `version`.
///
/// Built on this suite's own `call`, like every other request helper here, so the whole
/// suite drives one harness; `function_aliases.rs` reaches the same binding through its
/// harness and pins `CreateAlias`'s own rules, which this helper only sets up for.
async fn create_alias(router: &MultiServiceRouter, function: &str, alias: &str, version: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        &json!({ "Name": alias, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {alias:?} -> {version:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Move every in-flight update's settle instant into the past, so the next call sees an
/// update the clock has already finished applying rather than one still in flight. This
/// is what waiting would do, without waiting.
fn make_update_settled(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// The unqualified ARN of the function `name`.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The `(UpdateRuntimeOn, RuntimeVersionArn, FunctionArn)` a successful answer reports.
fn reported(case: &str, operation: &str, answer: &Answer) -> (String, Option<String>, String) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from {operation}, got {} {}",
        answer.status,
        answer.text()
    );
    let mode = answer.string("UpdateRuntimeOn").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected an UpdateRuntimeOn member from {operation}, got {}",
            answer.text()
        )
    });
    let arn = answer.string("FunctionArn").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a FunctionArn member from {operation}, got {}",
            answer.text()
        )
    });
    (mode, answer.string("RuntimeVersionArn"), arn)
}

/// Assert that `answer` reports exactly the configuration `mode`/`arn` describe, and
/// that it names `function_arn`.
fn assert_reports(
    case: &str,
    operation: &str,
    answer: &Answer,
    mode: &str,
    arn: Option<&str>,
    function_arn: &str,
) {
    let (reported_mode, reported_arn, reported_function_arn) = reported(case, operation, answer);
    assert_eq!(
        reported_mode, mode,
        "case {case:?}: expected UpdateRuntimeOn {mode:?} from {operation}, got {reported_mode:?}"
    );
    assert_eq!(
        reported_arn.as_deref(),
        arn,
        "case {case:?}: expected RuntimeVersionArn {arn:?} from {operation}, got {reported_arn:?}"
    );
    assert_eq!(
        reported_function_arn, function_arn,
        "case {case:?}: expected FunctionArn {function_arn:?} from {operation}, got \
         {reported_function_arn:?}"
    );
}

/// One `PutRuntimeManagementConfig` call: the mode it names, and the runtime version ARN
/// it pins, which is `Some` exactly for [`MANUAL`].
struct Put {
    update_runtime_on: &'static str,
    runtime_version_arn: Option<&'static str>,
}

/// One row of the runtime-management table.
struct Case {
    name: &'static str,
    /// The function the row creates.
    function: &'static str,
    /// Whether the row publishes a version to address.
    publish: bool,
    /// The `FunctionName` the requests carry, built from the function name.
    reference: fn(&str) -> String,
    /// The `Qualifier` query parameter, when the row sends one.
    qualifier: Option<&'static str>,
    /// The version label the configuration is expected to be stored against, which is
    /// also the qualifier the reported `FunctionArn` carries; `None` means `$LATEST`,
    /// whose ARN carries no qualifier.
    arn_qualifier: Option<&'static str>,
    /// The calls the row makes, in order; empty means nothing is ever set.
    puts: Vec<Put>,
    /// The configuration the version must report once they have all been applied.
    expected: (&'static str, Option<&'static str>),
}

/// The rows. Between them they cover the whole [`UPDATE_RUNTIME_ON_VALUES`] set, both
/// [`RUNTIME_VERSION_ARNS`] forms, the documented `FunctionName` forms, addressing a
/// version implicitly / by `Qualifier` / by a qualified name, and one, several or no
/// calls at all.
fn cases() -> Vec<Case> {
    vec![
        // Nothing has been set, so the version reports the documented default.
        Case {
            name: "a version nobody has configured reports the default",
            function: "runtime-default",
            publish: false,
            reference: |name| name.to_string(),
            qualifier: None,
            arn_qualifier: None,
            puts: Vec::new(),
            expected: (DEFAULT_UPDATE_RUNTIME_ON, None),
        },
        // One call naming the mode the version already holds: the configuration is a
        // value, not a toggle, so it stays where it was.
        Case {
            name: "one call sets the default mode the version already holds",
            function: "runtime-auto",
            publish: false,
            reference: |name| name.to_string(),
            qualifier: None,
            arn_qualifier: None,
            puts: vec![Put {
                update_runtime_on: "Auto",
                runtime_version_arn: None,
            }],
            expected: ("Auto", None),
        },
        // The third modelled mode, addressed by the bare `function:` prefix form.
        Case {
            name: "one call sets FunctionUpdate, addressed by the function: prefix",
            function: "runtime-function-update",
            publish: false,
            reference: |name| format!("function:{name}"),
            qualifier: None,
            arn_qualifier: None,
            puts: vec![Put {
                update_runtime_on: "FunctionUpdate",
                runtime_version_arn: None,
            }],
            expected: ("FunctionUpdate", None),
        },
        // Manual pins a runtime version, in the first pattern branch, addressed by the
        // documented partial ARN.
        Case {
            name: "Manual pins a runtime version ARN, addressed by a partial arn",
            function: "runtime-manual-runtime-arn",
            publish: false,
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            qualifier: None,
            arn_qualifier: None,
            puts: vec![Put {
                update_runtime_on: MANUAL,
                runtime_version_arn: Some(RUNTIME_VERSION_ARNS[0]),
            }],
            expected: (MANUAL, Some(RUNTIME_VERSION_ARNS[0])),
        },
        // Several calls, ending on the second pattern branch: the last one wins, and
        // the ARN it pinned replaces the one before it.
        Case {
            name: "several calls end on the latest Manual runtime version",
            function: "runtime-manual-repinned",
            publish: false,
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            qualifier: None,
            arn_qualifier: None,
            puts: vec![
                Put {
                    update_runtime_on: MANUAL,
                    runtime_version_arn: Some(RUNTIME_VERSION_ARNS[0]),
                },
                Put {
                    update_runtime_on: "FunctionUpdate",
                    runtime_version_arn: None,
                },
                Put {
                    update_runtime_on: MANUAL,
                    runtime_version_arn: Some(RUNTIME_VERSION_ARNS[1]),
                },
            ],
            expected: (MANUAL, Some(RUNTIME_VERSION_ARNS[1])),
        },
        // Leaving Manual drops the pinned ARN: "if the runtime update mode is manual,
        // the ARN is returned, otherwise null is returned".
        Case {
            name: "a move off Manual drops the pinned runtime version",
            function: "runtime-manual-then-auto",
            publish: false,
            reference: |name| name.to_string(),
            qualifier: None,
            arn_qualifier: None,
            puts: vec![
                Put {
                    update_runtime_on: MANUAL,
                    runtime_version_arn: Some(RUNTIME_VERSION_ARNS[1]),
                },
                Put {
                    update_runtime_on: "Auto",
                    runtime_version_arn: None,
                },
            ],
            expected: ("Auto", None),
        },
        // The same version, addressed by an explicit `$LATEST` qualifier rather than by
        // leaving it out.
        Case {
            name: "the $LATEST version addressed by an explicit qualifier",
            function: "runtime-explicit-latest",
            publish: false,
            reference: |name| name.to_string(),
            qualifier: Some("$LATEST"),
            arn_qualifier: None,
            puts: vec![Put {
                update_runtime_on: MANUAL,
                runtime_version_arn: Some(RUNTIME_VERSION_ARNS[0]),
            }],
            expected: (MANUAL, Some(RUNTIME_VERSION_ARNS[0])),
        },
        // A published version, addressed by the `Qualifier` parameter.
        Case {
            name: "a published version addressed by the Qualifier parameter",
            function: "runtime-published-qualifier",
            publish: true,
            reference: |name| name.to_string(),
            qualifier: Some("1"),
            arn_qualifier: Some("1"),
            puts: vec![Put {
                update_runtime_on: "FunctionUpdate",
                runtime_version_arn: None,
            }],
            expected: ("FunctionUpdate", None),
        },
        // The same published version, addressed by a qualified `FunctionName` instead.
        Case {
            name: "a published version addressed by a qualified function name",
            function: "runtime-published-name",
            publish: true,
            reference: |name| format!("{name}:1"),
            qualifier: None,
            arn_qualifier: Some("1"),
            puts: vec![Put {
                update_runtime_on: MANUAL,
                runtime_version_arn: Some(RUNTIME_VERSION_ARNS[1]),
            }],
            expected: (MANUAL, Some(RUNTIME_VERSION_ARNS[1])),
        },
    ]
}

#[tokio::test]
async fn a_version_reports_the_runtime_management_config_it_holds() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Every modelled mode is exercised by the rows below.
    for mode in UPDATE_RUNTIME_ON_VALUES {
        assert!(
            cases().iter().any(|case| case
                .puts
                .iter()
                .any(|put| put.update_runtime_on == *mode)),
            "case {mode:?}: the table must cover every modelled UpdateRuntimeOn value"
        );
    }

    for case in cases() {
        let label = case.name;
        create_function(&router, case.function).await;
        if case.publish {
            let published = publish_version(&router, case.function).await;
            assert_eq!(
                published, "1",
                "case {label:?}: the first published version must be 1, got {published:?}"
            );
        }
        let reference = (case.reference)(case.function);
        let expected_function_arn = match case.arn_qualifier {
            Some(qualifier) => format!("{}:{qualifier}", function_arn(case.function)),
            None => function_arn(case.function),
        };

        // Before anything is set, every version reports the documented default with no
        // runtime version ARN.
        let initial =
            get_runtime_management_config(&router, &reference, case.qualifier).await;
        assert_reports(
            label,
            "GetRuntimeManagementConfig",
            &initial,
            DEFAULT_UPDATE_RUNTIME_ON,
            None,
            &expected_function_arn,
        );

        for put in &case.puts {
            let answer = put_runtime_management_config(
                &router,
                &reference,
                case.qualifier,
                put.update_runtime_on,
                put.runtime_version_arn,
            )
            .await;
            assert_reports(
                label,
                "PutRuntimeManagementConfig",
                &answer,
                put.update_runtime_on,
                put.runtime_version_arn,
                &expected_function_arn,
            );

            // Read it back after every call: a value that was answered but never
            // persisted fails here.
            let fetched =
                get_runtime_management_config(&router, &reference, case.qualifier).await;
            assert_reports(
                label,
                "GetRuntimeManagementConfig",
                &fetched,
                put.update_runtime_on,
                put.runtime_version_arn,
                &expected_function_arn,
            );

            // Storing it updated the function, so the next call in this row waits for
            // that update the way a client does.
            make_update_settled(&engine);
        }

        let (mode, arn) = case.expected;
        for round in 0..2 {
            // A read does not change what the next caller sees, so both rounds report
            // the same configuration.
            let fetched =
                get_runtime_management_config(&router, &reference, case.qualifier).await;
            assert_reports(
                &format!("{label} (read {round})"),
                "GetRuntimeManagementConfig",
                &fetched,
                mode,
                arn,
                &expected_function_arn,
            );
        }
    }
}

#[tokio::test]
async fn the_runtime_management_config_is_held_per_version() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "runtime-per-version").await;
    let first = publish_version(&router, "runtime-per-version").await;
    // A second publish needs a change to snapshot, which the configuration update
    // below provides; publishing twice in a row reports the version already published.
    let unqualified_put = put_runtime_management_config(
        &router,
        "runtime-per-version",
        None,
        MANUAL,
        Some(RUNTIME_VERSION_ARNS[0]),
    )
    .await;
    assert_eq!(
        unqualified_put.status,
        200,
        "setting $LATEST must succeed, got {} {}",
        unqualified_put.status,
        unqualified_put.text()
    );
    make_update_settled(&engine);

    // Each version holds its own configuration: `$LATEST` is Manual, version 1 is
    // FunctionUpdate, and a version nobody configured stays at the default.
    let version_one_put = put_runtime_management_config(
        &router,
        "runtime-per-version",
        Some(&first),
        "FunctionUpdate",
        None,
    )
    .await;
    assert_eq!(
        version_one_put.status,
        200,
        "setting version {first} must succeed, got {} {}",
        version_one_put.status,
        version_one_put.text()
    );

    create_function(&router, "runtime-untouched").await;

    let expected: Vec<(&str, Option<&str>, &str, Option<&str>)> = vec![
        (
            "runtime-per-version",
            None,
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[0]),
        ),
        (
            "runtime-per-version",
            Some(first.as_str()),
            "FunctionUpdate",
            None,
        ),
        (
            "runtime-untouched",
            None,
            DEFAULT_UPDATE_RUNTIME_ON,
            None,
        ),
    ];

    for (function, qualifier, mode, arn) in expected {
        let label = format!("{function}:{}", qualifier.unwrap_or("$LATEST"));
        let fetched = get_runtime_management_config(&router, function, qualifier).await;
        let expected_function_arn = match qualifier {
            Some(qualifier) => format!("{}:{qualifier}", function_arn(function)),
            None => function_arn(function),
        };
        assert_reports(
            &label,
            "GetRuntimeManagementConfig",
            &fetched,
            mode,
            arn,
            &expected_function_arn,
        );
    }
}

#[tokio::test]
async fn a_mode_and_runtime_version_arn_that_cannot_hold_together_are_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "runtime-refusals").await;
    // The version starts configured, so a refusal that stored anything would show up as
    // a change to this configuration.
    let seed = put_runtime_management_config(
        &router,
        "runtime-refusals",
        None,
        MANUAL,
        Some(RUNTIME_VERSION_ARNS[1]),
    )
    .await;
    assert_eq!(
        seed.status,
        200,
        "the seed call must succeed, got {} {}",
        seed.status,
        seed.text()
    );
    make_update_settled(&engine);

    // One row per way the pair cannot hold together: `Manual` names no runtime version,
    // and each of the other modelled modes names one.
    let mut refusals: Vec<(String, &str, Option<&str>)> = vec![(
        "Manual without a runtime version ARN".to_string(),
        MANUAL,
        None,
    )];
    for mode in UPDATE_RUNTIME_ON_VALUES {
        if *mode == MANUAL {
            continue;
        }
        for arn in RUNTIME_VERSION_ARNS {
            refusals.push((format!("{mode} with a runtime version ARN"), mode, Some(arn)));
        }
    }

    for (label, mode, arn) in &refusals {
        let answer =
            put_runtime_management_config(&router, "runtime-refusals", None, mode, *arn).await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?}: expected 400 from PutRuntimeManagementConfig, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("InvalidParameterValueException"),
            "case {label:?}: expected InvalidParameterValueException, got {:?}",
            answer.error_code()
        );

        // The refusal stored nothing: the seeded configuration is untouched.
        let fetched = get_runtime_management_config(&router, "runtime-refusals", None).await;
        assert_reports(
            label,
            "GetRuntimeManagementConfig",
            &fetched,
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[1]),
            &function_arn("runtime-refusals"),
        );
    }
}

#[tokio::test]
async fn a_mode_outside_the_modelled_value_set_is_refused_with_the_aws_envelope() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "runtime-bad-mode").await;

    // One row per form a value outside `UpdateRuntimeOn`'s value set takes: a word that
    // is no mode, one that differs from a modelled value only in case, and one that
    // merely contains a modelled value. None of them is a member of the set, so each is
    // refused alike, and the clause echoes the value the request carried.
    for value in ["Bogus", "auto", "Manual-ish", ""] {
        let answer =
            put_runtime_management_config(&router, "runtime-bad-mode", None, value, None).await;
        assert_eq!(
            answer.status,
            400,
            "case {value:?}: expected 400 from PutRuntimeManagementConfig, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ValidationException"),
            "case {value:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        let message = answer
            .string("message")
            .or_else(|| answer.string("Message"))
            .unwrap_or_else(|| {
                panic!(
                    "case {value:?}: expected a message member, got {}",
                    answer.text()
                )
            });
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value '{value}' at 'updateRuntimeOn' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{REPORTED_VALUE_SET}]"
            ),
            "case {value:?}: expected the AWS envelope naming the member, the value and the \
             value set, got {message:?}"
        );

        // The refusal stored nothing: the version is still at the documented default.
        let fetched = get_runtime_management_config(&router, "runtime-bad-mode", None).await;
        assert_reports(
            value,
            "GetRuntimeManagementConfig",
            &fetched,
            DEFAULT_UPDATE_RUNTIME_ON,
            None,
            &function_arn("runtime-bad-mode"),
        );
    }
}

#[tokio::test]
async fn a_version_that_does_not_exist_has_no_runtime_management_config() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "runtime-present").await;

    // Every form that names no stored version: a function the store does not hold, in
    // each documented `FunctionName` form, and a version of one it does hold that was
    // never published.
    let missing: Vec<(String, Option<&str>)> = vec![
        ("runtime-absent".to_string(), None),
        (format!("{ACCOUNT}:function:runtime-absent"), None),
        (
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:runtime-absent"),
            None,
        ),
        ("runtime-present".to_string(), Some("7")),
        ("runtime-present:7".to_string(), None),
    ];

    for (reference, qualifier) in &missing {
        let label = format!("{reference}?Qualifier={qualifier:?}");
        let fetched = get_runtime_management_config(&router, reference, *qualifier).await;
        assert_eq!(
            fetched.status,
            404,
            "case {label:?}: expected 404 from GetRuntimeManagementConfig, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert!(
            fetched.error_code().contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            fetched.error_code()
        );

        let put =
            put_runtime_management_config(&router, reference, *qualifier, "Auto", None).await;
        assert_eq!(
            put.status,
            404,
            "case {label:?}: expected 404 from PutRuntimeManagementConfig, got {} {}",
            put.status,
            put.text()
        );
        assert!(
            put.error_code().contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            put.error_code()
        );
    }

    // The refusals stored nothing: the version that does exist is at the default.
    let fetched = get_runtime_management_config(&router, "runtime-present", None).await;
    assert_reports(
        "runtime-present",
        "GetRuntimeManagementConfig",
        &fetched,
        DEFAULT_UPDATE_RUNTIME_ON,
        None,
        &function_arn("runtime-present"),
    );
}

/// One way of naming a version: the case name, how the `FunctionName` is built from the
/// function's name, and the `Qualifier` parameter the request carries (if any).
type Reference = (&'static str, fn(&str) -> String, Option<&'static str>);

/// Every way of naming ONE version, with the configuration and the `FunctionArn` that
/// version must report: `(version, references, mode, runtime version ARN, FunctionArn)`.
type Group = (
    &'static str,
    Vec<Reference>,
    &'static str,
    Option<&'static str>,
    String,
);

/// A `FunctionName`/`Qualifier` pair whose halves name two different versions: the case
/// name, how the name is built, and the `Qualifier` it disagrees with.
type Conflict = (&'static str, fn(&str) -> String, &'static str);

/// One read of a configuration: `(FunctionName, Qualifier, mode, runtime version ARN,
/// FunctionArn)`.
type Read = (
    String,
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
    String,
);

/// Every documented way of naming the `$LATEST` version, from the `FunctionName`
/// member's own pattern (`smithy_query(query="shape", shape="NamespacedFunctionName")`:
/// an optional ARN prefix, region and account id, an optional `function:` literal, the
/// name, and an optional `:$LATEST(.PUBLISHED)|:<qualifier>`) combined with the
/// `Qualifier` parameter, which "If no value is specified, the configuration for the
/// $LATEST version is returned".
fn latest_references() -> Vec<Reference> {
    vec![
        ("a bare name, no Qualifier", |name| name.to_string(), None),
        (
            "a bare name and an explicit Qualifier",
            |name| name.to_string(),
            Some("$LATEST"),
        ),
        (
            "a name qualified with $LATEST",
            |name| format!("{name}:$LATEST"),
            None,
        ),
        (
            "a name and a Qualifier naming the same version",
            |name| format!("{name}:$LATEST"),
            Some("$LATEST"),
        ),
        (
            "the bare function: prefix",
            |name| format!("function:{name}"),
            None,
        ),
        (
            "a partial ARN and a Qualifier",
            |name| format!("{ACCOUNT}:function:{name}"),
            Some("$LATEST"),
        ),
        (
            "a qualified partial ARN",
            |name| format!("{ACCOUNT}:function:{name}:$LATEST"),
            None,
        ),
        (
            "a function ARN",
            |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            None,
        ),
        (
            "a qualified function ARN",
            |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}:$LATEST"),
            None,
        ),
    ]
}

/// The same forms, naming published version `1` instead: by the `Qualifier` parameter,
/// by the name's own qualifier, or by both at once.
fn version_one_references() -> Vec<Reference> {
    vec![
        ("a bare name and a Qualifier", |name| name.to_string(), Some("1")),
        (
            "a name qualified with the version",
            |name| format!("{name}:1"),
            None,
        ),
        (
            "a name and a Qualifier naming the same version",
            |name| format!("{name}:1"),
            Some("1"),
        ),
        (
            "the bare function: prefix and a Qualifier",
            |name| format!("function:{name}"),
            Some("1"),
        ),
        (
            "a partial ARN and a Qualifier",
            |name| format!("{ACCOUNT}:function:{name}"),
            Some("1"),
        ),
        (
            "a qualified partial ARN",
            |name| format!("{ACCOUNT}:function:{name}:1"),
            None,
        ),
        (
            "a function ARN and a Qualifier",
            |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            Some("1"),
        ),
        (
            "a qualified function ARN",
            |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}:1"),
            None,
        ),
    ]
}

#[tokio::test]
async fn every_way_of_naming_one_version_reads_the_same_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "runtime-reference-forms";
    create_function(&router, function).await;
    let published = publish_version(&router, function).await;
    assert_eq!(
        published, "1",
        "the first published version must be 1, got {published:?}"
    );

    // The two versions hold DIFFERENT configurations, so a row that resolved to the
    // wrong version reports the other one's values rather than passing by accident.
    for (qualifier, mode, arn) in [
        (None, MANUAL, Some(RUNTIME_VERSION_ARNS[0])),
        (Some("1"), "FunctionUpdate", None),
    ] {
        let seeded =
            put_runtime_management_config(&router, function, qualifier, mode, arn).await;
        assert_eq!(
            seeded.status,
            200,
            "seeding {:?} must succeed, got {} {}",
            qualifier.unwrap_or("$LATEST"),
            seeded.status,
            seeded.text()
        );
        // Seeding updated the function, so the next write waits for that update the way
        // a client does.
        make_update_settled(&engine);
    }

    // Each group names ONE version, so every row in it reads the same stored value and
    // reports the same canonical FunctionArn — whatever form the request used to get
    // there.
    let groups: Vec<Group> = vec![
        (
            "$LATEST",
            latest_references(),
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[0]),
            function_arn(function),
        ),
        (
            "version 1",
            version_one_references(),
            "FunctionUpdate",
            None,
            format!("{}:1", function_arn(function)),
        ),
    ];

    for (version, references, mode, arn, expected_function_arn) in groups {
        for (case, build, qualifier) in references {
            let reference = build(function);
            let label = format!("{version} as {case} ({reference}?Qualifier={qualifier:?})");
            // Read twice: a report does not change what the next caller sees, so the
            // second read of every form answers exactly what the first one did.
            for round in 0..2 {
                let fetched =
                    get_runtime_management_config(&router, &reference, qualifier).await;
                assert_reports(
                    &format!("{label} (read {round})"),
                    "GetRuntimeManagementConfig",
                    &fetched,
                    mode,
                    arn,
                    &expected_function_arn,
                );
            }
        }
    }
}

#[tokio::test]
async fn a_name_and_qualifier_that_name_two_versions_are_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "runtime-conflicting-qualifier";
    create_function(&router, function).await;
    let published = publish_version(&router, function).await;
    assert_eq!(
        published, "1",
        "the first published version must be 1, got {published:?}"
    );
    // Both versions start configured, so a refusal that stored anything shows up as a
    // change to one of these two configurations.
    for (qualifier, mode, arn) in [
        (None, MANUAL, Some(RUNTIME_VERSION_ARNS[1])),
        (Some("1"), "FunctionUpdate", None),
    ] {
        let seeded =
            put_runtime_management_config(&router, function, qualifier, mode, arn).await;
        assert_eq!(
            seeded.status,
            200,
            "seeding {:?} must succeed, got {} {}",
            qualifier.unwrap_or("$LATEST"),
            seeded.status,
            seeded.text()
        );
        // Seeding updated the function, so the next write waits for that update the way
        // a client does.
        make_update_settled(&engine);
    }

    // One row per way the two halves can disagree: the name's qualifier against the
    // parameter, in each direction, in each documented name form, and naming a version
    // that exists or one that does not — the pair names no single resource either way,
    // so the disagreement is what is reported rather than a 404.
    let conflicts: Vec<Conflict> = vec![
        (
            "a published version in the name against $LATEST",
            |name| format!("{name}:1"),
            "$LATEST",
        ),
        (
            "$LATEST in the name against a published version",
            |name| format!("{name}:$LATEST"),
            "1",
        ),
        (
            "a partial ARN qualified with a version nobody published",
            |name| format!("{ACCOUNT}:function:{name}:9"),
            "1",
        ),
        (
            "a function ARN against an alias name",
            |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}:1"),
            "live",
        ),
    ];

    for (case, build, qualifier) in conflicts {
        let reference = build(function);
        let label = format!("{case} ({reference}?Qualifier={qualifier})");
        for (operation, answer) in [
            (
                "GetRuntimeManagementConfig",
                get_runtime_management_config(&router, &reference, Some(qualifier)).await,
            ),
            (
                "PutRuntimeManagementConfig",
                put_runtime_management_config(&router, &reference, Some(qualifier), "Auto", None)
                    .await,
            ),
        ] {
            assert_eq!(
                answer.status,
                400,
                "case {label:?}: expected 400 from {operation}, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("InvalidParameterValueException"),
                "case {label:?}: expected InvalidParameterValueException from {operation}, got \
                 {:?}",
                answer.error_code()
            );
        }
    }

    // Neither refusal stored anything: both versions still hold what was seeded.
    for (qualifier, mode, arn, expected_function_arn) in [
        (
            None,
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[1]),
            function_arn(function),
        ),
        (
            Some("1"),
            "FunctionUpdate",
            None,
            format!("{}:1", function_arn(function)),
        ),
    ] {
        let fetched = get_runtime_management_config(&router, function, qualifier).await;
        assert_reports(
            qualifier.unwrap_or("$LATEST"),
            "GetRuntimeManagementConfig",
            &fetched,
            mode,
            arn,
            &expected_function_arn,
        );
    }
}

#[tokio::test]
async fn an_alias_reads_the_configuration_of_the_version_it_invokes() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "runtime-aliased";
    create_function(&router, function).await;
    let published = publish_version(&router, function).await;
    assert_eq!(
        published, "1",
        "the first published version must be 1, got {published:?}"
    );
    for (qualifier, mode, arn) in [
        (None, MANUAL, Some(RUNTIME_VERSION_ARNS[0])),
        (Some("1"), "FunctionUpdate", None),
    ] {
        let seeded =
            put_runtime_management_config(&router, function, qualifier, mode, arn).await;
        assert_eq!(
            seeded.status,
            200,
            "seeding {:?} must succeed, got {} {}",
            qualifier.unwrap_or("$LATEST"),
            seeded.status,
            seeded.text()
        );
        // Seeding updated the function, so the next write waits for that update the way
        // a client does.
        make_update_settled(&engine);
    }
    // Two aliases over the same function, invoking the two different versions.
    create_alias(&router, function, "live", "1").await;
    create_alias(&router, function, "edge", "$LATEST").await;

    // The configuration belongs to the VERSION, so each alias reports the configuration
    // of the version it invokes, whether the alias arrives as the `Qualifier` parameter
    // or as the name's own qualifier, while the reported `FunctionArn` names what the
    // request addressed.
    let reads: Vec<Read> = vec![
        (
            function.to_string(),
            Some("live"),
            "FunctionUpdate",
            None,
            format!("{}:live", function_arn(function)),
        ),
        (
            format!("{function}:live"),
            None,
            "FunctionUpdate",
            None,
            format!("{}:live", function_arn(function)),
        ),
        (
            function.to_string(),
            Some("edge"),
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[0]),
            format!("{}:edge", function_arn(function)),
        ),
        (
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:edge"),
            None,
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[0]),
            format!("{}:edge", function_arn(function)),
        ),
    ];

    for (reference, qualifier, mode, arn, expected_function_arn) in &reads {
        let label = format!("{reference}?Qualifier={qualifier:?}");
        let fetched = get_runtime_management_config(&router, reference, *qualifier).await;
        assert_reports(
            &label,
            "GetRuntimeManagementConfig",
            &fetched,
            mode,
            *arn,
            expected_function_arn,
        );
    }

    // A call addressed by an alias and one addressed by the version that alias invokes
    // reach the SAME configuration: setting it through `live` is what version 1 reports,
    // and `$LATEST` (and therefore `edge`) is untouched.
    make_update_settled(&engine);
    let through_alias =
        put_runtime_management_config(&router, function, Some("live"), "Auto", None).await;
    assert_reports(
        "through the alias",
        "PutRuntimeManagementConfig",
        &through_alias,
        "Auto",
        None,
        &format!("{}:live", function_arn(function)),
    );
    for (qualifier, mode, arn, expected_function_arn) in [
        ("1", "Auto", None, format!("{}:1", function_arn(function))),
        (
            "live",
            "Auto",
            None,
            format!("{}:live", function_arn(function)),
        ),
        (
            "$LATEST",
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[0]),
            function_arn(function),
        ),
        (
            "edge",
            MANUAL,
            Some(RUNTIME_VERSION_ARNS[0]),
            format!("{}:edge", function_arn(function)),
        ),
    ] {
        let fetched = get_runtime_management_config(&router, function, Some(qualifier)).await;
        assert_reports(
            qualifier,
            "GetRuntimeManagementConfig",
            &fetched,
            mode,
            arn,
            &expected_function_arn,
        );
    }
}
