// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's recursive loop detection configuration:
//! `PutFunctionRecursionConfig` "Sets your function's recursive loop detection
//! configuration" and `GetFunctionRecursionConfig` "Returns your function's recursive
//! loop detection configuration".
//!
//! The rule under test: a function's `RecursiveLoop` setting is one function-level value
//! it always has — the documented `Terminate` until a `PutFunctionRecursionConfig` call
//! stores another modelled value, and from then on the value of the most recent such
//! call on THAT function — and `GetFunctionRecursionConfig` reports that stored value for
//! every documented `FunctionName` form naming the function, while a name no function
//! holds is refused as not found.
//!
//! Every row drives real HTTP requests through the router and reads the setting back
//! through `GetFunctionRecursionConfig`, so a value that was answered but never stored
//! fails here. The rows differ along what the rule generalises over: the modelled value
//! set (`Allow | Terminate`), how many calls have been made (none, one, several —
//! including a repeat of the value already held and a change back to the default), the
//! documented `FunctionName` form the requests address the function by, the length of
//! that name (each bound the model's `UnqualifiedFunctionName` admits, and one past it),
//! and how many versions the function has published — while a value outside the modelled
//! set, a name carrying a `:qualifier` the modelled pattern excludes, and a name longer
//! than the modelled maximum are each refused and store nothing.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The ARN convention and the account id come from the shared harness in
// `function_lifecycle`, so this suite cannot disagree with it about what a function's ARN
// looks like.
use crate::function_lifecycle::{function_arn, ACCOUNT};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The whole `RecursiveLoop` value set, from
/// `smithy_query(query="shape", shape="RecursiveLoop")`: `Allow | Terminate`.
const RECURSIVE_LOOP_VALUES: &[&str] = &["Allow", "Terminate"];

/// The value a function's configuration starts at, which the model's own `RecursiveLoop`
/// member documentation states: "By default, Lambda sets your function's configuration
/// to Terminate."
const DEFAULT_RECURSIVE_LOOP: &str = "Terminate";

/// The `RecursiveLoop` value set as a refusal reports it, in the order the shape declares
/// its members (`smithy_query(query="shape", shape="RecursiveLoop")` -> `Allow`,
/// `Terminate`).
const REPORTED_VALUE_SET: &str = "Allow, Terminate";

/// The longest `FunctionName` the model admits, from
/// `smithy_query(query="operation", operation="PutFunctionRecursionConfig")`:
/// `UnqualifiedFunctionName` is `@length(min: 1, max: 140)`.
const FUNCTION_NAME_MAX: usize = 140;

/// The longest bare function name the member's own documentation admits: "The length
/// constraint applies only to the full ARN. If you specify only the function name, it is
/// limited to 64 characters in length."
const BARE_NAME_MAX: usize = 64;


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

/// Where both recursion-config operations are bound, for the function `reference` names.
fn recursion_config_uri(reference: &str) -> String {
    format!("/2024-08-31/functions/{reference}/recursion-config")
}

/// `GetFunctionRecursionConfig` on the function `reference` names.
async fn get_recursion_config(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(
        router,
        "GET",
        &recursion_config_uri(reference),
        &Value::Null,
    )
    .await
}

/// `PutFunctionRecursionConfig` on the function `reference` names.
async fn put_recursion_config(
    router: &MultiServiceRouter,
    reference: &str,
    recursive_loop: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &recursion_config_uri(reference),
        &json!({ "RecursiveLoop": recursive_loop }),
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

/// The `RecursiveLoop` a successful answer reports.
fn reported(case: &str, operation: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from {operation}, got {} {}",
        answer.status,
        answer.text()
    );
    answer.string("RecursiveLoop").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a RecursiveLoop member from {operation}, got {}",
            answer.text()
        )
    })
}

/// A request whose `FunctionName` the model does not admit — one carrying a `:qualifier`
/// the `UnqualifiedFunctionName` pattern excludes, or one past its length maximum — is
/// refused at the boundary before any function is looked up: a 400 `ValidationException`
/// whose message names the member the violated constraint sits on, rather than a 404
/// about a function that is missing or a 400 from some unrelated cause.
fn assert_refused_on_the_name(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected 400 from {operation} for a name the model does not admit, \
         got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains("ValidationException"),
        "case {case:?}: expected ValidationException from {operation}, got {:?}",
        answer.error_code()
    );
    let message = answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a message member from {operation}, got {}",
                answer.text()
            )
        });
    assert!(
        message.contains("'functionName'"),
        "case {case:?}: expected the message from {operation} to name the member the constraint \
         sits on, got {message:?}"
    );
}

/// One row of the recursion-config table: the function, the `FunctionName` form the
/// requests address it by, the `RecursiveLoop` values set on it in order, and the value
/// it must report once they have all been applied.
struct RecursionCase {
    name: &'static str,
    function: &'static str,
    /// The documented `FunctionName` form the requests use; `None` means the bare name.
    reference: Option<String>,
    /// The `RecursiveLoop` values `PutFunctionRecursionConfig` sets, in order; empty
    /// means the configuration is never set at all.
    puts: &'static [&'static str],
    expected: &'static str,
}

/// The rows. Each differs from the others in the modelled value it ends on, how many
/// calls have been made and the `FunctionName` form it is addressed by — the dimensions
/// the rule generalises over. Between them they cover the whole `RecursiveLoop` value
/// set (see [`RECURSIVE_LOOP_VALUES`]) and each documented `FunctionName` form.
fn recursion_cases() -> Vec<RecursionCase> {
    vec![
        // Nothing has been set, so the function must report the documented default.
        RecursionCase {
            name: "a function nobody has configured reports the default",
            function: "recursion-default",
            reference: None,
            puts: &[],
            expected: DEFAULT_RECURSIVE_LOOP,
        },
        // One call, moving the function off the default.
        RecursionCase {
            name: "one call sets Allow",
            function: "recursion-allow",
            reference: None,
            puts: &["Allow"],
            expected: "Allow",
        },
        // One call naming the value the function already holds: the setting is a value,
        // not a toggle, so it stays where it was.
        RecursionCase {
            name: "one call sets the value the function already holds",
            function: "recursion-terminate",
            reference: Some("function:recursion-terminate".to_string()),
            puts: &["Terminate"],
            expected: "Terminate",
        },
        // Several calls, addressed by the documented partial ARN: the last one wins.
        RecursionCase {
            name: "several calls, addressed by a partial arn, end on Allow",
            function: "recursion-partial-arn",
            reference: Some(format!("{ACCOUNT}:function:recursion-partial-arn")),
            puts: &["Terminate", "Allow", "Allow"],
            expected: "Allow",
        },
        // A change back to the default, addressed by the full function ARN: the default
        // is a value a caller may set, not only the one a function starts at.
        RecursionCase {
            name: "a change back to the default, addressed by a function arn",
            function: "recursion-full-arn",
            reference: Some(function_arn("recursion-full-arn")),
            puts: &["Allow", "Terminate"],
            expected: DEFAULT_RECURSIVE_LOOP,
        },
    ]
}

#[tokio::test]
async fn a_function_reports_the_recursion_config_it_holds() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in recursion_cases() {
        let label = case.name;
        create_function(&router, case.function).await;
        let reference = case
            .reference
            .clone()
            .unwrap_or_else(|| case.function.to_string());

        // Before anything is set, every function reports the documented default.
        let initial = get_recursion_config(&router, &reference).await;
        let initial = reported(label, "GetFunctionRecursionConfig", &initial);
        assert_eq!(
            initial, DEFAULT_RECURSIVE_LOOP,
            "case {label:?}: a function starts at {DEFAULT_RECURSIVE_LOOP:?}, got {initial:?}"
        );

        for value in case.puts {
            let put = put_recursion_config(&router, &reference, value).await;
            let put = reported(label, "PutFunctionRecursionConfig", &put);
            assert_eq!(
                put, *value,
                "case {label:?}: PutFunctionRecursionConfig must answer the value it stored \
                 ({value:?}), got {put:?}"
            );

            // Read it back after every call: a value that was answered but never
            // persisted fails here.
            let fetched = get_recursion_config(&router, &reference).await;
            let fetched = reported(label, "GetFunctionRecursionConfig", &fetched);
            assert_eq!(
                fetched, *value,
                "case {label:?}: after setting {value:?} the function must report it, got \
                 {fetched:?}"
            );
        }

        // The final value, read by the bare name whichever form the calls used.
        let fetched = get_recursion_config(&router, case.function).await;
        let fetched = reported(label, "GetFunctionRecursionConfig", &fetched);
        assert_eq!(
            fetched, case.expected,
            "case {label:?}: expected {:?}, got {fetched:?}",
            case.expected
        );

        // Reading it does not change what the next caller sees.
        let again = get_recursion_config(&router, case.function).await;
        let again = reported(label, "GetFunctionRecursionConfig", &again);
        assert_eq!(
            again, case.expected,
            "case {label:?}: a second read must report {:?} unchanged, got {again:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn the_recursion_config_is_held_per_function() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One function per modelled value, plus one left at the default, so setting any of
    // them cannot be reported by any of the others.
    let mut expected: Vec<(String, &str)> =
        vec![("recursion-untouched".to_string(), DEFAULT_RECURSIVE_LOOP)];
    for value in RECURSIVE_LOOP_VALUES {
        expected.push((format!("recursion-scoped-{}", value.to_lowercase()), value));
    }

    for (name, _) in &expected {
        create_function(&router, name).await;
    }
    for (name, value) in &expected {
        if *name == "recursion-untouched" {
            continue;
        }
        let put = put_recursion_config(&router, name, value).await;
        let put = reported(name, "PutFunctionRecursionConfig", &put);
        assert_eq!(
            put, *value,
            "case {name:?}: expected {value:?} back from PutFunctionRecursionConfig, got {put:?}"
        );
    }

    for (name, value) in &expected {
        let fetched = get_recursion_config(&router, name).await;
        let fetched = reported(name, "GetFunctionRecursionConfig", &fetched);
        assert_eq!(
            fetched, *value,
            "case {name:?}: each function holds its own configuration, expected {value:?}, got \
             {fetched:?}"
        );
    }
}

#[tokio::test]
async fn a_function_that_does_not_exist_has_no_recursion_config() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "recursion-present").await;

    // Every form that names no stored function, including one that names a function the
    // store does hold under a DIFFERENT name.
    let missing = [
        "recursion-absent".to_string(),
        format!("{ACCOUNT}:function:recursion-absent"),
        function_arn("recursion-absent"),
    ];

    for reference in &missing {
        let fetched = get_recursion_config(&router, reference).await;
        assert_eq!(
            fetched.status,
            404,
            "case {reference:?}: expected 404 from GetFunctionRecursionConfig, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert!(
            fetched.error_code().contains("ResourceNotFoundException"),
            "case {reference:?}: expected ResourceNotFoundException, got {:?}",
            fetched.error_code()
        );

        let put = put_recursion_config(&router, reference, "Allow").await;
        assert_eq!(
            put.status,
            404,
            "case {reference:?}: expected 404 from PutFunctionRecursionConfig, got {} {}",
            put.status,
            put.text()
        );
        assert!(
            put.error_code().contains("ResourceNotFoundException"),
            "case {reference:?}: expected ResourceNotFoundException, got {:?}",
            put.error_code()
        );
    }

    // The refusals stored nothing: the function that does exist is untouched.
    let fetched = get_recursion_config(&router, "recursion-present").await;
    let fetched = reported("recursion-present", "GetFunctionRecursionConfig", &fetched);
    assert_eq!(
        fetched, DEFAULT_RECURSIVE_LOOP,
        "a refused call must leave every function at {DEFAULT_RECURSIVE_LOOP:?}, got {fetched:?}"
    );
}

#[tokio::test]
async fn a_value_outside_the_modelled_set_is_refused_and_stores_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "recursion-bad-value").await;
    // Seed a value that is NOT the default, so a refusal that reset the setting, or that
    // stored the value it was handed, both fail below.
    let seeded = put_recursion_config(&router, "recursion-bad-value", "Allow").await;
    let seeded = reported("seed", "PutFunctionRecursionConfig", &seeded);
    assert_eq!(
        seeded, "Allow",
        "the seeded configuration must be stored, got {seeded:?}"
    );

    // One row per form a value outside the `RecursiveLoop` set takes: a word that is no
    // setting at all, each modelled value differing only in case, one that merely
    // contains a modelled value, and the empty string.
    for value in [
        "Deny",
        "allow",
        "TERMINATE",
        "Terminate ",
        "TerminateAll",
        "",
    ] {
        let answer = put_recursion_config(&router, "recursion-bad-value", value).await;
        assert_eq!(
            answer.status,
            400,
            "case {value:?}: expected 400 from PutFunctionRecursionConfig, got {} {}",
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
                "1 validation error detected: Value '{value}' at 'recursiveLoop' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{REPORTED_VALUE_SET}]"
            ),
            "case {value:?}: expected the envelope naming the member, the value and the value \
             set, got {message:?}"
        );

        // The refusal stored nothing: the seeded value still stands.
        let fetched = get_recursion_config(&router, "recursion-bad-value").await;
        let fetched = reported(value, "GetFunctionRecursionConfig", &fetched);
        assert_eq!(
            fetched, "Allow",
            "case {value:?}: a refused call must leave the stored value at \"Allow\", got \
             {fetched:?}"
        );
    }
}

#[tokio::test]
async fn the_setting_is_function_level_whatever_versions_exist() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "recursion-versioned").await;

    // The number of published versions is a dimension the rule generalises over: the
    // setting is one function-level value, so it survives publishing and keeps answering
    // the same whether the function has none, one or several versions.
    for versions in 0..3u32 {
        if versions > 0 {
            let published = call(
                &router,
                "POST",
                "/2015-03-31/functions/recursion-versioned/versions",
                &json!({}),
            )
            .await;
            assert_eq!(
                published.status,
                201,
                "case {versions} version(s): PublishVersion must succeed, got {} {}",
                published.status,
                published.text()
            );
        }

        for value in RECURSIVE_LOOP_VALUES {
            let case = format!("{versions} version(s), {value}");
            let put = put_recursion_config(&router, "recursion-versioned", value).await;
            let put = reported(&case, "PutFunctionRecursionConfig", &put);
            assert_eq!(
                put, *value,
                "case {case:?}: PutFunctionRecursionConfig must answer the value it stored, got \
                 {put:?}"
            );
            let fetched = get_recursion_config(&router, "recursion-versioned").await;
            let fetched = reported(&case, "GetFunctionRecursionConfig", &fetched);
            assert_eq!(
                fetched, *value,
                "case {case:?}: the function-level setting must report {value:?}, got {fetched:?}"
            );
        }
    }

    // Park the function on the value that is NOT the default, so a refusal below that
    // reset the setting (or stored the value it was handed) cannot pass unnoticed.
    let parked = RECURSIVE_LOOP_VALUES
        .iter()
        .find(|value| **value != DEFAULT_RECURSIVE_LOOP)
        .expect("the modelled value set must hold a value other than the default");
    let put = put_recursion_config(&router, "recursion-versioned", parked).await;
    let put = reported("parked on the non-default value", "PutFunctionRecursionConfig", &put);
    assert_eq!(
        put, *parked,
        "the parked value must be stored, got {put:?}"
    );

    // `FunctionName` is an `UnqualifiedFunctionName`, whose pattern admits no
    // `:qualifier`, so a version or alias is not a resource this setting lives on — and
    // the version named below does exist, so the refusal is about the qualifier.
    let qualified = [
        "recursion-versioned:1".to_string(),
        "recursion-versioned:$LATEST".to_string(),
        "recursion-versioned:live".to_string(),
        format!("{ACCOUNT}:function:recursion-versioned:1"),
        format!("{}:2", function_arn("recursion-versioned")),
    ];
    for reference in &qualified {
        for (operation, answer) in [
            (
                "GetFunctionRecursionConfig",
                get_recursion_config(&router, reference).await,
            ),
            (
                "PutFunctionRecursionConfig",
                put_recursion_config(&router, reference, parked).await,
            ),
        ] {
            assert_refused_on_the_name(reference, operation, &answer);
        }
    }

    // None of those refusals moved the function-level value off the one last stored.
    let fetched = get_recursion_config(&router, "recursion-versioned").await;
    let fetched = reported("after the refusals", "GetFunctionRecursionConfig", &fetched);
    assert_eq!(
        fetched, *parked,
        "a refused call must leave the stored value at {parked:?}, got {fetched:?}"
    );
}

#[tokio::test]
async fn every_name_length_the_model_admits_round_trips() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The bounds of the name the rule generalises over: the shortest name the pattern
    // admits, the longest BARE name the member documents, and the longest reference a
    // stored function can be addressed by at all — that same name as a full ARN, the form
    // the model's 140 applies to ("The length constraint applies only to the full ARN").
    let shortest = "r".to_string();
    let longest_bare = "r".repeat(BARE_NAME_MAX);
    // A second function of the same maximum length, addressed by its full ARN, so the ARN
    // row stands on its own record rather than overwriting the bare row's.
    let longest_bare_other = "s".repeat(BARE_NAME_MAX);
    let longest_arn = function_arn(&longest_bare_other);
    assert!(
        longest_arn.chars().count() <= FUNCTION_NAME_MAX,
        "the longest-ARN row must stay within the modelled maximum of {FUNCTION_NAME_MAX}, got {}",
        longest_arn.chars().count()
    );

    let rows: [(&str, &str, String, &str); 3] = [
        (
            "the shortest admitted name",
            &shortest,
            shortest.clone(),
            "Allow",
        ),
        (
            "the longest documented bare name",
            &longest_bare,
            longest_bare.clone(),
            "Terminate",
        ),
        (
            "the longest documented name as a full arn",
            &longest_bare_other,
            longest_arn.clone(),
            "Allow",
        ),
    ];

    for (case, function, reference, value) in &rows {
        create_function(&router, function).await;
        let put = put_recursion_config(&router, reference, value).await;
        let put = reported(case, "PutFunctionRecursionConfig", &put);
        assert_eq!(
            put, *value,
            "case {case:?}: PutFunctionRecursionConfig must answer the value it stored, got \
             {put:?}"
        );
        let fetched = get_recursion_config(&router, reference).await;
        let fetched = reported(case, "GetFunctionRecursionConfig", &fetched);
        assert_eq!(
            fetched, *value,
            "case {case:?}: expected {value:?} read back, got {fetched:?}"
        );
    }

    // One step past the modelled maximum: refused by both operations, nothing stored.
    let too_long = "r".repeat(FUNCTION_NAME_MAX + 1);
    for (operation, answer) in [
        (
            "GetFunctionRecursionConfig",
            get_recursion_config(&router, &too_long).await,
        ),
        (
            "PutFunctionRecursionConfig",
            put_recursion_config(&router, &too_long, "Allow").await,
        ),
    ] {
        assert_refused_on_the_name("one past the modelled maximum", operation, &answer);
    }

    // The names that do fit still report what they stored, so the refusals wrote nothing.
    for (case, _, reference, value) in &rows {
        let fetched = get_recursion_config(&router, reference).await;
        let fetched = reported(case, "GetFunctionRecursionConfig", &fetched);
        assert_eq!(
            fetched, *value,
            "case {case:?}: a refused call must leave {value:?} in place, got {fetched:?}"
        );
    }
}
