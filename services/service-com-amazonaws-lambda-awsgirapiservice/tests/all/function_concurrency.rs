// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's reserved concurrency: `PutFunctionConcurrency`
//! "Sets the maximum number of simultaneous executions for a function, and reserves
//! capacity for that concurrency level" and `GetFunctionConcurrency` "Returns details
//! about the reserved concurrency configuration for a function".
//!
//! The rule under test: a function's reserved concurrency is ONE function-level value —
//! absent until a `PutFunctionConcurrency` call stores one, and from then on the value of
//! the most recent such call on THAT function, replacing rather than accumulating — which
//! every reader of the function reports identically (`GetFunctionConcurrency`,
//! `GetFunction`'s `Concurrency`, and `GetAccountSettings`'
//! `UnreservedConcurrentExecutions`, which is the account's concurrency limit less what
//! every function reserves); and a call that would leave the account less than the
//! documented minimum unreserved capacity, or that names no function, is refused and
//! stores nothing.
//!
//! Every row drives real HTTP requests through the router and reads the value back, so a
//! reservation that was answered but never stored fails here. The rows differ along what
//! the rule generalises over: the value reserved (each published bound, one past it, and
//! values in between), how many calls have been made (none, one, several — including a
//! repeat of the value already held and a decrease), the documented `FunctionName` form
//! the requests address the function by, and how much of the account other functions
//! have already reserved.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `GetAccountSettings` is bound (`GET /2016-08-19/account-settings`).
const ACCOUNT_SETTINGS_URI: &str = "/2016-08-19/account-settings";

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

/// `AccountLimit$ConcurrentExecutions`: the published default concurrency quota of an
/// account in a Region (AWS Lambda quotas, "Concurrent executions … 1,000"). Written as
/// the published number rather than as the crate's constant, so a wrong constant fails
/// here.
const CONCURRENT_EXECUTIONS_LIMIT: i64 = 1_000;

/// The capacity an account must leave unreserved, from `PutFunctionConcurrency`'s own
/// documentation: "You can reserve concurrency for as many functions as you like, as long
/// as you leave at least 100 simultaneous executions unreserved for functions that aren't
/// configured with a per-function limit."
const MIN_UNRESERVED: i64 = 100;

/// The largest reservation the rule above admits on an account nothing else reserves on.
const MAX_RESERVABLE: i64 = CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED;

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

    /// The integer at `pointer`, when the body carries one there.
    fn number(&self, pointer: &str) -> Option<i64> {
        self.json().pointer(pointer).and_then(Value::as_i64)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
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

/// `PutFunctionConcurrency` (`PUT /2017-10-31/functions/{FunctionName}/concurrency`).
async fn put_concurrency(router: &MultiServiceRouter, reference: &str, reserved: i64) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2017-10-31/functions/{reference}/concurrency"),
        &json!({ "ReservedConcurrentExecutions": reserved }),
    )
    .await
}

/// `GetFunctionConcurrency` (`GET /2019-09-30/functions/{FunctionName}/concurrency`).
async fn get_concurrency(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2019-09-30/functions/{reference}/concurrency"),
        &Value::Null,
    )
    .await
}

/// `GetFunction` (`GET /2015-03-31/functions/{FunctionName}`), whose `Concurrency` member
/// is "The function's reserved concurrency".
async fn get_function(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2015-03-31/functions/{reference}"),
        &Value::Null,
    )
    .await
}

/// The account's `UnreservedConcurrentExecutions`, through `GetAccountSettings`.
async fn unreserved(router: &MultiServiceRouter) -> i64 {
    let answer = call(router, "GET", ACCOUNT_SETTINGS_URI, &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "GetAccountSettings must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .number("/AccountLimit/UnreservedConcurrentExecutions")
        .unwrap_or_else(|| {
            panic!(
                "GetAccountSettings must report UnreservedConcurrentExecutions, got {}",
                answer.text()
            )
        })
}

/// Create a function to reserve capacity for.
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

/// Every reader's view of one function's reserved concurrency, which must agree.
async fn reported(case: &str, router: &MultiServiceRouter, reference: &str) -> Option<i64> {
    let concurrency = get_concurrency(router, reference).await;
    assert_eq!(
        concurrency.status,
        200,
        "case {case:?}: expected 200 from GetFunctionConcurrency, got {} {}",
        concurrency.status,
        concurrency.text()
    );
    let from_get = concurrency.number("/ReservedConcurrentExecutions");

    let function = get_function(router, reference).await;
    assert_eq!(
        function.status,
        200,
        "case {case:?}: expected 200 from GetFunction, got {} {}",
        function.status,
        function.text()
    );
    let from_function = function.number("/Concurrency/ReservedConcurrentExecutions");
    assert_eq!(
        from_function, from_get,
        "case {case:?}: GetFunction must report the same reservation as \
         GetFunctionConcurrency ({from_get:?}), got {from_function:?}"
    );

    from_get
}

/// One row of the round-trip table: the function, the `FunctionName` form the requests
/// address it by, the values reserved on it in order, and what it must then report.
struct Case {
    name: &'static str,
    function: &'static str,
    /// The documented `FunctionName` form the requests use; `None` means the bare name.
    reference: Option<String>,
    /// The values `PutFunctionConcurrency` reserves, in order; empty means the function
    /// is never configured at all.
    puts: &'static [i64],
    expected: Option<i64>,
}

/// The rows. Between them they cover the published bounds of
/// `ReservedConcurrentExecutions` on an otherwise empty account (its modelled minimum
/// `0`, the maximum the documented 100-unreserved rule admits, and values between), each
/// documented `FunctionName` form, and a call count of none, one and several.
fn cases() -> Vec<Case> {
    vec![
        // Nothing reserved: the member is absent rather than zero, because zero is a
        // reservation a caller may make and this function has made none.
        Case {
            name: "a function nobody has configured reserves nothing",
            function: "concurrency-unset",
            reference: None,
            puts: &[],
            expected: None,
        },
        // The modelled minimum (`@range(min: 0)`): a function throttled to no
        // simultaneous executions at all.
        Case {
            name: "the modelled minimum reservation of zero",
            function: "concurrency-zero",
            reference: None,
            puts: &[0],
            expected: Some(0),
        },
        // One unit above the minimum, addressed by the `function:` prefixed form.
        Case {
            name: "one unit reserved, addressed by the function: form",
            function: "concurrency-one",
            reference: Some("function:concurrency-one".to_string()),
            puts: &[1],
            expected: Some(1),
        },
        // The largest reservation the documented rule admits here, addressed by the
        // partial ARN.
        Case {
            name: "the largest reservation the unreserved-capacity rule admits",
            function: "concurrency-max",
            reference: Some(format!("{ACCOUNT}:function:concurrency-max")),
            puts: &[MAX_RESERVABLE],
            expected: Some(MAX_RESERVABLE),
        },
        // The same value twice: the reservation REPLACES the one held rather than adding
        // to it, so a repeat at the ceiling is still inside the rule.
        Case {
            name: "the ceiling reserved twice replaces rather than accumulates",
            function: "concurrency-repeat",
            reference: Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:concurrency-repeat"
            )),
            puts: &[MAX_RESERVABLE, MAX_RESERVABLE],
            expected: Some(MAX_RESERVABLE),
        },
        // Several calls up and down: the last one wins.
        Case {
            name: "several calls end on the last value",
            function: "concurrency-sequence",
            reference: None,
            puts: &[10, 512, 7],
            expected: Some(7),
        },
    ]
}

#[tokio::test]
async fn a_function_reports_the_reserved_concurrency_it_holds() {
    for case in cases() {
        // One account per row, so each row's reservations are the only ones the
        // account-wide rule has to leave room for.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        create_function(&router, case.function).await;
        let reference = case
            .reference
            .clone()
            .unwrap_or_else(|| case.function.to_string());

        // Before anything is reserved, no reader reports a reservation and the whole
        // account limit is unreserved.
        let initial = reported(label, &router, &reference).await;
        assert_eq!(
            initial, None,
            "case {label:?}: a function starts with no reservation, got {initial:?}"
        );
        let free = unreserved(&router).await;
        assert_eq!(
            free, CONCURRENT_EXECUTIONS_LIMIT,
            "case {label:?}: an account reserving nothing leaves \
             {CONCURRENT_EXECUTIONS_LIMIT} unreserved, got {free:?}"
        );

        for value in case.puts {
            let put = put_concurrency(&router, &reference, *value).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: expected 200 reserving {value}, got {} {}",
                put.status,
                put.text()
            );
            let answered = put.number("/ReservedConcurrentExecutions");
            assert_eq!(
                answered,
                Some(*value),
                "case {label:?}: PutFunctionConcurrency must answer the value it stored \
                 ({value}), got {answered:?}"
            );

            // Read it back after every call: a value that was answered but never
            // persisted fails here.
            let fetched = reported(label, &router, &reference).await;
            assert_eq!(
                fetched,
                Some(*value),
                "case {label:?}: after reserving {value} the function must report it, got \
                 {fetched:?}"
            );
            let free = unreserved(&router).await;
            assert_eq!(
                free,
                CONCURRENT_EXECUTIONS_LIMIT - *value,
                "case {label:?}: reserving {value} must leave {} unreserved, got {free:?}",
                CONCURRENT_EXECUTIONS_LIMIT - *value
            );
        }

        // The final value, read by the bare name whichever form the calls used.
        let fetched = reported(label, &router, case.function).await;
        assert_eq!(
            fetched, case.expected,
            "case {label:?}: expected {:?}, got {fetched:?}",
            case.expected
        );

        // Reading it does not change what the next caller sees.
        let again = reported(label, &router, case.function).await;
        assert_eq!(
            again, case.expected,
            "case {label:?}: a second read must report {:?} unchanged, got {again:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn each_function_holds_its_own_reservation() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Three functions sharing one account: two reserving different amounts and one left
    // unconfigured, so neither reservation can be reported by any other function and the
    // account's unreserved capacity is what the two of them leave together.
    let configured: &[(&str, i64)] = &[("concurrency-scoped-a", 250), ("concurrency-scoped-b", 60)];
    for (name, _) in configured {
        create_function(&router, name).await;
    }
    create_function(&router, "concurrency-scoped-none").await;

    for (name, value) in configured {
        let put = put_concurrency(&router, name, *value).await;
        assert_eq!(
            put.status,
            200,
            "case {name:?}: expected 200 reserving {value}, got {} {}",
            put.status,
            put.text()
        );
    }

    for (name, value) in configured {
        let fetched = reported(name, &router, name).await;
        assert_eq!(
            fetched,
            Some(*value),
            "case {name:?}: each function holds its own reservation, expected {value}, got \
             {fetched:?}"
        );
    }
    let untouched = reported("concurrency-scoped-none", &router, "concurrency-scoped-none").await;
    assert_eq!(
        untouched, None,
        "a function nobody configured must report no reservation, got {untouched:?}"
    );

    let total: i64 = configured.iter().map(|(_, value)| *value).sum();
    let free = unreserved(&router).await;
    assert_eq!(
        free,
        CONCURRENT_EXECUTIONS_LIMIT - total,
        "the account's unreserved capacity is the limit less every reservation ({}), got \
         {free:?}",
        CONCURRENT_EXECUTIONS_LIMIT - total
    );
}

/// One row of the account-rule table: what another function already reserves, the value
/// this call asks for, and whether the documented 100-unreserved rule admits it.
struct LimitCase {
    name: &'static str,
    /// What a second function reserves before the call under test.
    reserved_elsewhere: Option<i64>,
    requested: i64,
    admitted: bool,
}

/// The rows: each published bound of the rule, and one past it — on an empty account and
/// on one where another function already holds part of the limit.
const LIMIT_CASES: &[LimitCase] = &[
    LimitCase {
        name: "the whole reservable capacity, leaving exactly the minimum unreserved",
        reserved_elsewhere: None,
        requested: MAX_RESERVABLE,
        admitted: true,
    },
    LimitCase {
        name: "one past the reservable capacity",
        reserved_elsewhere: None,
        requested: MAX_RESERVABLE + 1,
        admitted: false,
    },
    LimitCase {
        name: "the whole account concurrency limit",
        reserved_elsewhere: None,
        requested: CONCURRENT_EXECUTIONS_LIMIT,
        admitted: false,
    },
    LimitCase {
        name: "far past the account concurrency limit",
        reserved_elsewhere: None,
        requested: i32::MAX as i64,
        admitted: false,
    },
    LimitCase {
        name: "all that is left beside another function's reservation",
        reserved_elsewhere: Some(500),
        requested: MAX_RESERVABLE - 500,
        admitted: true,
    },
    LimitCase {
        name: "one past what is left beside another function's reservation",
        reserved_elsewhere: Some(500),
        requested: MAX_RESERVABLE - 500 + 1,
        admitted: false,
    },
    LimitCase {
        name: "nothing is left beside a function reserving the whole capacity",
        reserved_elsewhere: Some(MAX_RESERVABLE),
        requested: 1,
        admitted: false,
    },
    LimitCase {
        name: "zero is admitted even when nothing is left to reserve",
        reserved_elsewhere: Some(MAX_RESERVABLE),
        requested: 0,
        admitted: true,
    },
];

#[tokio::test]
async fn a_reservation_must_leave_the_documented_minimum_unreserved() {
    for case in LIMIT_CASES {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        create_function(&router, "concurrency-subject").await;
        if let Some(elsewhere) = case.reserved_elsewhere {
            create_function(&router, "concurrency-neighbour").await;
            let put = put_concurrency(&router, "concurrency-neighbour", elsewhere).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: the neighbour's reservation of {elsewhere} must succeed, got \
                 {} {}",
                put.status,
                put.text()
            );
        }

        let put = put_concurrency(&router, "concurrency-subject", case.requested).await;
        if case.admitted {
            assert_eq!(
                put.status,
                200,
                "case {label:?}: expected 200 reserving {}, got {} {}",
                case.requested,
                put.status,
                put.text()
            );
            let stored = reported(label, &router, "concurrency-subject").await;
            assert_eq!(
                stored,
                Some(case.requested),
                "case {label:?}: an admitted reservation of {} must be stored, got {stored:?}",
                case.requested
            );
        } else {
            assert_eq!(
                put.status,
                400,
                "case {label:?}: expected 400 reserving {}, got {} {}",
                case.requested,
                put.status,
                put.text()
            );
            assert!(
                put.error_code().contains("InvalidParameterValueException"),
                "case {label:?}: expected InvalidParameterValueException, got {:?}",
                put.error_code()
            );
            // A refused call stores nothing: the subject still reserves nothing and the
            // account still reports only the neighbour's reservation.
            let stored = reported(label, &router, "concurrency-subject").await;
            assert_eq!(
                stored, None,
                "case {label:?}: a refused reservation must store nothing, got {stored:?}"
            );
            let free = unreserved(&router).await;
            let expected = CONCURRENT_EXECUTIONS_LIMIT - case.reserved_elsewhere.unwrap_or(0);
            assert_eq!(
                free, expected,
                "case {label:?}: a refused reservation must leave {expected} unreserved, got \
                 {free:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_that_holds_no_reservation_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "concurrency-present").await;
    let put = put_concurrency(&router, "concurrency-present", 42).await;
    assert_eq!(
        put.status,
        200,
        "the reservation under test must succeed, got {} {}",
        put.status,
        put.text()
    );
    // Publishing a version gives the function a qualifier that does exist, so the refusal
    // below is about the setting being function-level and not about a missing version.
    let published = call(
        &router,
        "POST",
        "/2015-03-31/functions/concurrency-present/versions",
        &json!({}),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "PublishVersion must succeed, got {} {}",
        published.status,
        published.text()
    );

    // Every `FunctionName` naming no function, in each documented form.
    let missing = [
        "concurrency-absent".to_string(),
        format!("{ACCOUNT}:function:concurrency-absent"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:concurrency-absent"),
    ];
    for reference in &missing {
        for answer in [
            get_concurrency(&router, reference).await,
            put_concurrency(&router, reference, 1).await,
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {reference:?}: expected 404 for a function that does not exist, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {reference:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }

    // A qualified name: the reservation "appl[ies] to the function as a whole, including
    // all published versions and the unpublished version", so a version or alias is not a
    // resource it can be read from or set on — including the version just published and an
    // alias-shaped qualifier.
    let qualified = [
        "concurrency-present:1".to_string(),
        "concurrency-present:live".to_string(),
        format!("{ACCOUNT}:function:concurrency-present:1"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:concurrency-present:1"),
    ];
    for reference in &qualified {
        for answer in [
            get_concurrency(&router, reference).await,
            put_concurrency(&router, reference, 1).await,
        ] {
            assert_eq!(
                answer.status,
                400,
                "case {reference:?}: expected 400 for a qualified name, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer
                    .error_code()
                    .contains("InvalidParameterValueException"),
                "case {reference:?}: expected InvalidParameterValueException, got {:?}",
                answer.error_code()
            );
        }
    }

    // None of the refusals changed the reservation the function does hold.
    let stored = reported("concurrency-present", &router, "concurrency-present").await;
    assert_eq!(
        stored,
        Some(42),
        "a refused call must leave the stored reservation alone, got {stored:?}"
    );
}
