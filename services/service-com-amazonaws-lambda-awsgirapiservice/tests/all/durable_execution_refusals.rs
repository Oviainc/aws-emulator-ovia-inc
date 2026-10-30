// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDS the durable-execution surface refuses a request with.
//!
//! The rule under test: this service answers a refusal in AWS's own words for that error
//! class, and where AWS's sentence for the class is FIXED it is the whole message — nothing
//! of the request is interpolated into it, so neither the identifier the caller sent nor the
//! form a service-minted token has is disclosed. Two refusals of this surface are such
//! sentences:
//!
//! * `ListDurableExecutionsByFunction` on a `FunctionName`/`Qualifier` pair that resolves to
//!   no function — no such function, or no such version or alias of one — is refused `404
//!   ResourceNotFoundException` with the whole message [`FUNCTION_NOT_FOUND`]. The ARN this
//!   service resolved the reference to is NOT appended, unlike the sentence the function
//!   operations of the earlier Lambda API dates carry (`refusal_message_rules.rs` pins
//!   those);
//! * each of `SendDurableExecutionCallbackSuccess`, `SendDurableExecutionCallbackFailure`
//!   and `SendDurableExecutionCallbackHeartbeat` on a `CallbackId` that cannot be used is
//!   refused `400 InvalidParameterValueException` whose body is the whole message
//!   [`INVALID_CALLBACK_ID`] and NOTHING else — not the constraint envelope, which would
//!   quote the id back and state the base64 form of a minted token, and not the fault class
//!   every other error body of this service names (`error_fault_class.rs` pins that rule
//!   over the rest of the surface): an opaque refusal states the fault and nothing about
//!   it.
//!
//! The not-found refusal, like every other error body of this service, does name the class
//! of its fault in `Type`.
//!
//! Every row drives a real HTTP request through the router, so what is asserted is the wire
//! body. The rows differ along what the rule generalises over: for the listing, the FORM the
//! function was addressed in (bare name, qualified name, partial ARN, full ARN, full ARN
//! qualified) and WHICH part of the pair named nothing (the function, a version, an alias);
//! for the callbacks, the OPERATION asked and the way the supplied id is unusable (a
//! character outside the token alphabet, padding in the wrong place, a length past the
//! member's maximum). A recorded case is one row of each table.
//!
//! What a WELL-FORMED callback id is answered with is not asserted here: no operation of
//! this emulator mints one yet, which the open stub issue for the durable-execution runtime
//! owns. The scope of the fixed callback sentence is pinned instead by a refusal of the same
//! surface that keeps the constraint envelope — a `MaxItems` outside its declared range.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, send, zip_file,
    Answer, ACCOUNT, REGION, ROLE,
};

/// Where `ListDurableExecutionsByFunction` is bound
/// (`GET /2025-12-01/functions/{FunctionName}/durable-executions`).
const DURABLE_EXECUTIONS_URI: &str = "/2025-12-01/functions";

/// Where the three callback operations are bound
/// (`POST /2025-12-01/durable-execution-callbacks/{CallbackId}/<verb>`).
const CALLBACKS_URI: &str = "/2025-12-01/durable-execution-callbacks";

/// The whole message `ListDurableExecutionsByFunction` refuses an unresolvable function
/// with.
const FUNCTION_NOT_FOUND: &str = "Function not found";

/// The whole message each callback operation refuses an unusable `CallbackId` with.
const INVALID_CALLBACK_ID: &str = "Invalid callback id";

/// The error shape a missing function is refused as, and the one an unusable callback id is
/// refused as (`smithy_query(query="operation",
/// operation="ListDurableExecutionsByFunction,SendDurableExecutionCallbackSuccess")`:
/// `ResourceNotFoundException` at `@httpError(404)`, `InvalidParameterValueException` at
/// `@httpError(400)`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The class of fault a 4xx error body of this service names in `Type`, where it names one.
const USER_FAULT: &str = "User";

/// The `message` of a wire error, under either casing this model's error shapes spell it
/// (`ResourceNotFoundException$Message`, `InvalidParameterValueException$message`).
fn reported_message(answer: &Answer) -> String {
    answer
        .string("Message")
        .or_else(|| answer.string("message"))
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Create one function configured for durable executions and bring it to `Active`,
/// publishing version 1 so a qualified reference to an existing version resolves.
async fn create_durable_function(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
            "DurableConfig": {
                "KMSKeyArn": format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/durable-executions"),
                "ExecutionTimeout": 3_600,
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateFunction must succeed, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// `ListDurableExecutionsByFunction` on the function `reference` names, with `query`
/// appended (`""` names no filter at all).
async fn list(
    router: &smithy_server_runtime::MultiServiceRouter,
    reference: &str,
    query: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{DURABLE_EXECUTIONS_URI}/{reference}/durable-executions{query}"),
        &Value::Null,
    )
    .await
}

/// One row of the listing's refusal table: how the function was addressed, and which part
/// of the pair named nothing.
struct Unresolvable {
    /// What the row is called in a failure message.
    case: &'static str,
    /// The `FunctionName` the request binds to the operation's path label.
    reference: String,
    /// The query string the request carries, `""` for none.
    query: &'static str,
}

/// Every way a `FunctionName`/`Qualifier` pair can resolve to no function is refused with
/// ONE fixed sentence, which names nothing of the request.
#[tokio::test]
async fn an_unresolvable_function_is_refused_with_one_fixed_sentence() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let held = "durable-refusals";
    create_durable_function(&router, &engine, held).await;

    // The absent function is addressed in each documented form of a `FunctionName` ("You
    // can specify a function name, a partial ARN, or a full ARN"), and the HELD function is
    // addressed through a version and an alias it does not have — one resolution failure,
    // whichever half of the pair names nothing.
    let absent = "durable-refusals-absent";
    let cases = [
        Unresolvable {
            case: "a bare name nothing was created under",
            reference: absent.to_string(),
            query: "",
        },
        Unresolvable {
            case: "that name qualified by the unpublished version",
            reference: format!("{absent}:$LATEST"),
            query: "",
        },
        Unresolvable {
            case: "the same name as a partial ARN",
            reference: format!("function:{absent}"),
            query: "",
        },
        Unresolvable {
            case: "the same name as an account-qualified partial ARN",
            reference: format!("{ACCOUNT}:function:{absent}"),
            query: "",
        },
        Unresolvable {
            case: "the same name as a full ARN",
            reference: function_arn(absent),
            query: "",
        },
        Unresolvable {
            case: "the same full ARN qualified by a version",
            reference: format!("{}:1", function_arn(absent)),
            query: "",
        },
        Unresolvable {
            case: "a held function at a version it has not published",
            reference: format!("{held}:7"),
            query: "",
        },
        Unresolvable {
            case: "a held function at an alias it does not have",
            reference: held.to_string(),
            query: "?Qualifier=prod",
        },
        Unresolvable {
            case: "a held function at a version the Qualifier names",
            reference: held.to_string(),
            query: "?Qualifier=9",
        },
    ];

    for case in &cases {
        let answer = list(&router, &case.reference, case.query).await;
        let body = String::from_utf8_lossy(&answer.body).to_string();
        assert_eq!(
            (answer.status, answer.error_code().contains(RESOURCE_NOT_FOUND)),
            (404, true),
            "case {:?}: expected 404 {RESOURCE_NOT_FOUND}, got {} {body}",
            case.case,
            answer.status
        );
        let reported = reported_message(&answer);
        assert_eq!(
            reported, FUNCTION_NOT_FOUND,
            "case {:?}: expected the whole message {FUNCTION_NOT_FOUND:?}, got {reported:?}",
            case.case
        );
        assert_eq!(
            answer.string("Type").as_deref(),
            Some(USER_FAULT),
            "case {:?}: expected the fault class {USER_FAULT:?}, got {body}",
            case.case
        );
        // Nothing of the request: not the reference the caller sent, and not the ARN this
        // service resolved it to.
        assert!(
            !reported.contains(absent) && !reported.contains(held) && !reported.contains("arn:"),
            "case {:?}: the message must name nothing of the request, got {reported:?}",
            case.case
        );
    }

    // The same operation on the function it HOLDS reports that function's executions, so
    // the refusal above is the resolution failing rather than the operation refusing
    // everything.
    let held_answer = list(&router, held, "").await;
    assert_eq!(
        held_answer.status,
        200,
        "case \"the function this emulator holds\": expected 200, got {} {}",
        held_answer.status,
        String::from_utf8_lossy(&held_answer.body)
    );

    // And a member of the same request that is NOT an opaque token keeps the constraint
    // envelope: the refusal quotes the value and states the requirement, which is what lets
    // a caller correct it.
    let over_range = list(&router, held, "?MaxItems=5000").await;
    let stated = reported_message(&over_range);
    assert_eq!(
        over_range.status,
        400,
        "case \"MaxItems outside its range\": expected 400, got {} {}",
        over_range.status,
        String::from_utf8_lossy(&over_range.body)
    );
    assert!(
        stated.contains("maxItems") && stated.contains("5000") && stated.contains("1000"),
        "case \"MaxItems outside its range\": expected the constraint envelope quoting the \
         value and its bound, got {stated:?}"
    );
}

/// One callback operation: the verb its URI ends in, the content type a request to it
/// carries, and the body.
struct CallbackOperation {
    /// What the row is called in a failure message.
    case: &'static str,
    /// The last URI segment (`succeed`, `fail`, `heartbeat`).
    verb: &'static str,
    /// The `content-type` of the request.
    content_type: &'static str,
    /// The request body, which is each operation's own `@httpPayload` (or none).
    body: Vec<u8>,
}

/// The three operations a callback is completed, failed or kept alive through.
fn callback_operations() -> Vec<CallbackOperation> {
    vec![
        CallbackOperation {
            case: "SendDurableExecutionCallbackSuccess",
            verb: "succeed",
            // `Result` is a `BinaryOperationPayload` bound as `@httpPayload`, so the body
            // IS the blob.
            content_type: "application/octet-stream",
            body: b"{\"done\":true}".to_vec(),
        },
        CallbackOperation {
            case: "SendDurableExecutionCallbackFailure",
            verb: "fail",
            // `Error` is an `ErrorObject` bound as `@httpPayload`, so the body IS that
            // structure.
            content_type: "application/json",
            body: serde_json::to_vec(&json!({
                "ErrorMessage": "the external system could not complete the work",
                "ErrorType": "States.TaskFailed",
            }))
            .expect("the test request body must serialize"),
        },
        CallbackOperation {
            case: "SendDurableExecutionCallbackHeartbeat",
            verb: "heartbeat",
            content_type: "application/json",
            body: Vec::new(),
        },
    ]
}

/// One row of the callback refusal table: a `CallbackId` that cannot be used, and why.
struct UnusableId {
    /// What the row is called in a failure message.
    case: &'static str,
    /// The id the request binds to the operation's path label.
    id: String,
}

/// The ids no minted callback token can be, one row per constraint the member declares and
/// per way of breaking it. The member is `@length(min: 1, max: 1024)` with
/// `@pattern("^[A-Za-z0-9+/]+={0,2}$")` (`smithy_query(query="shape",
/// shape="CallbackId")`), so each row is outside the alphabet, outside the padding rule, or
/// past the maximum length.
fn unusable_ids() -> Vec<UnusableId> {
    vec![
        UnusableId {
            case: "a hyphen, outside the token alphabet",
            id: "not-a-token".to_string(),
        },
        UnusableId {
            case: "an underscore, outside the token alphabet",
            id: "not_a_token".to_string(),
        },
        UnusableId {
            case: "a dot, outside the token alphabet",
            id: "not.a.token".to_string(),
        },
        UnusableId {
            case: "a sub-delimiter, outside the token alphabet",
            id: "not!a!token".to_string(),
        },
        UnusableId {
            case: "padding before the end of the token",
            id: "cG9z=dGVk".to_string(),
        },
        UnusableId {
            case: "more padding than base64 admits",
            id: "cG9zdGVk===".to_string(),
        },
        UnusableId {
            case: "longer than the member's maximum",
            id: "A".repeat(1_100),
        },
    ]
}

/// Every callback operation refuses an unusable `CallbackId` with ONE fixed sentence, under
/// the modelled 400, disclosing neither the id nor the form a usable one has.
#[tokio::test]
async fn an_unusable_callback_id_is_refused_with_one_fixed_sentence() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for operation in callback_operations() {
        for unusable in unusable_ids() {
            let answer = send(
                &router,
                "POST",
                &format!("{CALLBACKS_URI}/{}/{}", unusable.id, operation.verb),
                operation.content_type,
                operation.body.clone(),
                &[],
            )
            .await;
            let body = String::from_utf8_lossy(&answer.body).to_string();
            let case = format!("{} with {}", operation.case, unusable.case);
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected the modelled 400, got {} {body}",
                answer.status
            );
            assert_eq!(
                answer.header("x-amzn-errortype").as_deref(),
                Some(INVALID_PARAMETER_VALUE),
                "case {case:?}: expected the error shape {INVALID_PARAMETER_VALUE:?}, got {body}"
            );
            let reported = reported_message(&answer);
            assert_eq!(
                reported, INVALID_CALLBACK_ID,
                "case {case:?}: expected the whole message {INVALID_CALLBACK_ID:?}, got \
                 {reported:?}"
            );
            assert_eq!(
                answer.string("Type"),
                None,
                "case {case:?}: an opaque refusal names no fault class, got {body}"
            );
            // The id the caller sent, the alphabet a minted one is drawn from, and the
            // constraint envelope's own lead-in are all absent: an opaque token's refusal
            // says only that it cannot be used.
            for leaked in [
                unusable.id.as_str(),
                "A-Za-z0-9",
                "regular expression",
                "validation error",
            ] {
                assert!(
                    !body.contains(leaked),
                    "case {case:?}: the refusal must not disclose {leaked:?}, got {body}"
                );
            }
        }
    }
}
