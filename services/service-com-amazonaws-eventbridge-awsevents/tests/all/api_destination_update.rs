// SPDX-License-Identifier: CC0-1.0

//! `UpdateApiDestination` and `DeleteApiDestination` end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter`, read back through `DescribeApiDestination`.
//!
//! The RULES asserted here:
//!
//! * an update of an API destination is applied IN PLACE: every member the
//!   request carries replaces the stored one, every member it leaves out keeps
//!   its prior value, and the destination's identity does not move — the
//!   `ApiDestinationArn` and the `CreationTime` the create minted are the ones
//!   the update answers and a later read reports, while `LastModifiedTime`
//!   advances;
//! * the `ApiDestinationState` an update answers is COMPUTED from the connection
//!   the destination now invokes through, not stored, so pointing a destination
//!   at a connection that holds no authorization reports it as `INACTIVE`;
//! * a request naming a resource this account does not hold is refused with the
//!   modelled `ResourceNotFoundException` (HTTP 400) whose message names the CALL
//!   that failed and the destination the REQUEST asked for, and every refused
//!   call leaves the stored destination exactly as it was;
//! * a delete removes the destination, so every following call on that name is
//!   that same refusal.
//!
//! Each table spans the class rather than one recorded case: the rows differ
//! along which members an update submits (one, several, all, none), the value and
//! form of each, every value of the `HttpMethod` enum, both published bounds of
//! the rate limit and one past the account maximum, and which member of a request
//! names something that does not exist.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// Every value of the modelled `ApiDestinationHttpMethod` enum, written out here
/// rather than imported from the implementation.
///
/// Source: `smithy_query(query="shape",shape="ApiDestinationHttpMethod")` —
/// `DELETE | GET | HEAD | OPTIONS | PATCH | POST | PUT`.
const HTTP_METHODS: [&str; 7] = [
    "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT",
];

/// The smallest `InvocationRateLimitPerSecond` the model admits
/// (`ApiDestinationInvocationRateLimitPerSecond`, `range min 1`).
const MIN_RATE_LIMIT: i32 = 1;

/// The members every destination in this suite is created with, so a row only has
/// to say what its update CHANGES.
const BASELINE_DESCRIPTION: &str = "the baseline destination";
const BASELINE_ENDPOINT: &str = "https://api.example.com/events";
const BASELINE_METHOD: &str = "POST";
const BASELINE_RATE: i32 = 10;

/// How long a test waits before reading a resource whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not make
/// the wait the flaky part of the assertion.
fn past_the_transition(span: Duration) -> Duration {
    span + Duration::from_millis(250)
}

/// One `awsJson1_1` call to EventBridge over the router.
pub(crate) async fn events(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The `message` an error body carries.
pub(crate) fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What a call on an API destination this account does not hold must read,
/// composed here from the failing call's own verb and the name the request
/// carried — not imported from the implementation.
fn expected_missing_wording(verb: &str, name: &str) -> String {
    format!("Failed to {verb} the api-destination(s). An api-destination '{name}' does not exist.")
}

/// An API-key connection called `name`, already settled into `AUTHORIZED` when
/// `settled`; answers its ARN.
pub(crate) async fn given_connection(
    router: &MultiServiceRouter,
    name: &str,
    settled: bool,
) -> String {
    let (status, body) = events(
        router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit" },
            },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateConnection answered {status} {body}");
    if settled {
        tokio::time::sleep(past_the_transition(state::CONNECTION_TRANSITION)).await;
    }
    body["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no ConnectionArn in {body}"))
        .to_string()
}

/// A destination called `name` on `connection`, created with the baseline
/// members; answers the whole `CreateApiDestination` response.
pub(crate) async fn given_destination(
    router: &MultiServiceRouter,
    name: &str,
    connection: &str,
) -> Value {
    let (status, created) = events(
        router,
        "CreateApiDestination",
        &json!({
            "Name": name,
            "Description": BASELINE_DESCRIPTION,
            "ConnectionArn": connection,
            "InvocationEndpoint": BASELINE_ENDPOINT,
            "HttpMethod": BASELINE_METHOD,
            "InvocationRateLimitPerSecond": BASELINE_RATE,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApiDestination answered {status} {created}"
    );
    created
}

/// One `UpdateApiDestination` call on `name` carrying `members` beside it.
pub(crate) async fn update(
    router: &MultiServiceRouter,
    name: &str,
    members: &Value,
) -> (u16, Value) {
    let mut request = json!({ "Name": name });
    for (member, value) in members.as_object().expect("the update members are an object") {
        request[member] = value.clone();
    }
    events(router, "UpdateApiDestination", &request).await
}

/// One `DescribeApiDestination` call that must succeed.
async fn described(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = events(router, "DescribeApiDestination", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApiDestination answered {status} {body}"
    );
    body
}

/// A row of the in-place-update table.
struct UpdateCase {
    case: &'static str,
    /// The members this update submits beside the required `Name`.
    update: Value,
    /// What a read must report afterwards for the members this update touched;
    /// every member absent from here keeps its baseline value. A `null` means the
    /// read must report the member not at all.
    expected: Value,
    /// Whether this update also points the destination at a SECOND connection,
    /// whose ARN only the running test knows.
    repoint: bool,
}

/// An update applies exactly the members it submits, keeps every other one, and
/// leaves the destination's ARN and creation time where they were.
#[tokio::test]
async fn an_update_applies_its_submitted_members_and_keeps_the_destinations_identity() {
    let cases = [
        UpdateCase {
            case: "nothing but the required name",
            update: json!({}),
            expected: json!({}),
            repoint: false,
        },
        UpdateCase {
            case: "only the HTTP method",
            update: json!({ "HttpMethod": "PUT" }),
            expected: json!({ "HttpMethod": "PUT" }),
            repoint: false,
        },
        UpdateCase {
            case: "only the rate limit",
            update: json!({ "InvocationRateLimitPerSecond": 5 }),
            expected: json!({ "InvocationRateLimitPerSecond": 5 }),
            repoint: false,
        },
        UpdateCase {
            case: "the method and the rate at the published minimum together",
            update: json!({ "HttpMethod": "DELETE", "InvocationRateLimitPerSecond": MIN_RATE_LIMIT }),
            expected: json!({ "HttpMethod": "DELETE", "InvocationRateLimitPerSecond": MIN_RATE_LIMIT }),
            repoint: false,
        },
        UpdateCase {
            case: "the rate at the account maximum",
            update: json!({ "InvocationRateLimitPerSecond": state::MAX_API_DESTINATION_RATE_LIMIT }),
            expected: json!({
                "InvocationRateLimitPerSecond": state::MAX_API_DESTINATION_RATE_LIMIT,
            }),
            repoint: false,
        },
        UpdateCase {
            case: "only the invocation endpoint, to a longer path with a query",
            update: json!({ "InvocationEndpoint": "https://api.example.com/v2/webhook?retry=1" }),
            expected: json!({ "InvocationEndpoint": "https://api.example.com/v2/webhook?retry=1" }),
            repoint: false,
        },
        UpdateCase {
            case: "only the description",
            update: json!({ "Description": "moved to the v2 webhook" }),
            expected: json!({ "Description": "moved to the v2 webhook" }),
            repoint: false,
        },
        UpdateCase {
            case: "the description cleared with the empty value the model admits",
            update: json!({ "Description": "" }),
            expected: json!({ "Description": Value::Null }),
            repoint: false,
        },
        UpdateCase {
            case: "only the connection it invokes through",
            update: json!({}),
            expected: json!({}),
            repoint: true,
        },
        UpdateCase {
            case: "every member at once",
            update: json!({
                "Description": "everything at once",
                "InvocationEndpoint": "https://hooks.example.org/ingest",
                "HttpMethod": "PATCH",
                "InvocationRateLimitPerSecond": 42,
            }),
            expected: json!({
                "Description": "everything at once",
                "InvocationEndpoint": "https://hooks.example.org/ingest",
                "HttpMethod": "PATCH",
                "InvocationRateLimitPerSecond": 42,
            }),
            repoint: true,
        },
    ];

    for UpdateCase {
        case,
        update: members,
        expected,
        repoint,
    } in cases
    {
        let router = emulator();
        let first = given_connection(&router, "primary-connection", true).await;
        let second = given_connection(&router, "secondary-connection", true).await;
        let name = "mutable-destination";
        let created = given_destination(&router, name, &first).await;
        let arn = created["ApiDestinationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ApiDestinationArn in {created}"))
            .to_string();

        let mut members = members;
        if repoint {
            members["ConnectionArn"] = json!(second);
        }
        let (status, updated) = update(&router, name, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateApiDestination answered {status} {updated}"
        );
        assert_eq!(
            updated["ApiDestinationArn"],
            json!(arn),
            "case {case:?}: expected the ARN the create minted ({arn}), got {updated}"
        );
        assert_eq!(
            updated["CreationTime"], created["CreationTime"],
            "case {case:?}: expected the creation time to stay at {}, got {updated}",
            created["CreationTime"]
        );
        let created_at = created["LastModifiedTime"].as_f64().unwrap_or_default();
        let modified_at = updated["LastModifiedTime"].as_f64().unwrap_or_default();
        assert!(
            modified_at >= created_at,
            "case {case:?}: expected a last-modified time at or after {created_at}, \
             got {modified_at}"
        );
        assert_eq!(
            updated["ApiDestinationState"],
            json!(state::API_DESTINATION_STATE_ACTIVE),
            "case {case:?}: both connections are authorized, so the destination is {}, \
             got {updated}",
            state::API_DESTINATION_STATE_ACTIVE
        );

        // The round trip: a read reports the merged record, not the request.
        let read = described(&router, case, name).await;
        let mut wanted = json!({
            "ApiDestinationArn": arn,
            "Name": name,
            "Description": BASELINE_DESCRIPTION,
            "ConnectionArn": if repoint { second.clone() } else { first.clone() },
            "InvocationEndpoint": BASELINE_ENDPOINT,
            "HttpMethod": BASELINE_METHOD,
            "InvocationRateLimitPerSecond": BASELINE_RATE,
            "CreationTime": created["CreationTime"].clone(),
        });
        for (member, value) in expected.as_object().expect("the expectation is an object") {
            wanted[member] = value.clone();
        }
        for (member, value) in wanted.as_object().expect("the expectation is an object") {
            assert_eq!(
                &read[member.as_str()],
                value,
                "case {case:?}: expected {member} to read back as {value}, got {read}"
            );
        }
        assert_eq!(
            read["LastModifiedTime"], updated["LastModifiedTime"],
            "case {case:?}: the read must report the time the update answered ({}), got {read}",
            updated["LastModifiedTime"]
        );
    }
}

/// Every value of the modelled HTTP-method enum can be moved to, and a method the
/// enum does not have is refused rather than stored.
#[tokio::test]
async fn an_update_moves_the_destination_to_any_modelled_http_method_and_no_other() {
    let router = emulator();
    let connection = given_connection(&router, "method-connection", true).await;
    let name = "method-destination";
    given_destination(&router, name, &connection).await;

    for method in HTTP_METHODS {
        let (status, updated) = update(&router, name, &json!({ "HttpMethod": method })).await;
        assert_eq!(
            status, 200,
            "case {method:?}: UpdateApiDestination answered {status} {updated}"
        );
        let read = described(&router, method, name).await;
        assert_eq!(
            read["HttpMethod"],
            json!(method),
            "case {method:?}: expected the read to report {method}, got {read}"
        );
    }

    // One step past the enum: refused, and the last accepted method survives.
    let (status, body) = update(&router, name, &json!({ "HttpMethod": "TRACE" })).await;
    assert_eq!(
        status, 400,
        "case \"TRACE\": expected a refusal, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"TRACE\": expected ValidationException, got {body}"
    );
    let read = described(&router, "TRACE", name).await;
    assert_eq!(
        read["HttpMethod"],
        json!(HTTP_METHODS[HTTP_METHODS.len() - 1]),
        "case \"TRACE\": a refused update must change nothing, got {read}"
    );
}

/// A call on an API destination this account does not hold is the modelled
/// missing-resource refusal, naming the call that failed and the name the request
/// carried — before any destination exists, and again after a delete.
#[tokio::test]
async fn a_destination_that_does_not_exist_is_refused_by_name_for_every_call() {
    let router = emulator();

    // The rows differ along the FORM of the name: a plain one, one carrying each
    // character class `ApiDestinationName`'s pattern admits, and one at the
    // modelled maximum length of 64.
    let longest = "n".repeat(64);
    let names = ["absent", "dots.and-dashes_1", longest.as_str()];
    // (the operation, the verb its refusal names)
    let calls = [
        ("UpdateApiDestination", "update"),
        ("DescribeApiDestination", "describe"),
        ("DeleteApiDestination", "delete"),
    ];

    for name in names {
        for (operation, verb) in calls {
            let case = format!("{operation} on {name:?}");
            let (status, body) = events(&router, operation, &json!({ "Name": name })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                message(&body),
                expected_missing_wording(verb, name),
                "case {case:?}: expected {:?}, got {body}",
                expected_missing_wording(verb, name)
            );
        }
    }

    // The same three refusals once a destination that DID exist is deleted.
    let connection = given_connection(&router, "doomed-connection", true).await;
    let name = "doomed-destination";
    given_destination(&router, name, &connection).await;
    let (status, body) = events(&router, "DeleteApiDestination", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "DeleteApiDestination answered {status} {body}"
    );
    for (operation, verb) in calls {
        let case = format!("{operation} after the delete");
        let (status, body) = events(&router, operation, &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            message(&body),
            expected_missing_wording(verb, name),
            "case {case:?}: expected {:?}, got {body}",
            expected_missing_wording(verb, name)
        );
    }
    // A delete also removes the destination from the LISTING.
    let (status, listed) = events(
        &router,
        "ListApiDestinations",
        &json!({ "NamePrefix": name }),
    )
    .await;
    assert_eq!(status, 200, "ListApiDestinations answered {status} {listed}");
    assert_eq!(
        listed["ApiDestinations"],
        json!([]),
        "a deleted destination is in no listing, got {listed}"
    );
}

/// What an update refuses, with which modelled error — and that each refused
/// update leaves the stored destination exactly as it was.
#[tokio::test]
async fn a_refused_update_changes_nothing_about_the_stored_destination() {
    let router = emulator();
    let connection = given_connection(&router, "kept-connection", true).await;
    let name = "kept-destination";
    given_destination(&router, name, &connection).await;
    let before = described(&router, "before any refusal", name).await;

    let absent_connection = format!(
        "arn:aws:events:{REGION}:{}:connection/no-such-connection/\
         00000000-0000-0000-0000-000000000000",
        engine::emulator_account_id()
    );
    // (case, the members the update submits, the expected wire code)
    let rows: [(&str, Value, &str); 4] = [
        (
            "a connection ARN this account holds no connection for",
            json!({ "ConnectionArn": absent_connection }),
            "ResourceNotFoundException",
        ),
        (
            "a rate limit one past the account maximum",
            json!({
                "InvocationRateLimitPerSecond": state::MAX_API_DESTINATION_RATE_LIMIT + 1,
            }),
            "LimitExceededException",
        ),
        (
            "a rate limit far past the account maximum",
            json!({
                "InvocationRateLimitPerSecond": state::MAX_API_DESTINATION_RATE_LIMIT * 10,
            }),
            "LimitExceededException",
        ),
        (
            "an HTTP method the enum does not have",
            json!({ "HttpMethod": "CONNECT" }),
            "ValidationException",
        ),
    ];

    for (case, members, expected_code) in rows {
        let (status, body) = update(&router, name, &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let after = described(&router, case, name).await;
        for member in [
            "ApiDestinationArn",
            "Name",
            "Description",
            "ConnectionArn",
            "InvocationEndpoint",
            "HttpMethod",
            "InvocationRateLimitPerSecond",
            "CreationTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: a refused update must leave {member} at {}, got {after}",
                before[member]
            );
        }
    }
}

/// The state an update answers is COMPUTED from the connection the destination
/// now invokes through: pointing it at a connection that holds no authorization
/// reports `INACTIVE`, and pointing it back at one that does reports `ACTIVE`.
#[tokio::test]
async fn the_state_an_update_answers_follows_the_connection_it_repoints_to() {
    let router = emulator();
    let authorized = given_connection(&router, "settled-connection", true).await;
    let name = "repointed-destination";
    let created = given_destination(&router, name, &authorized).await;
    assert_eq!(
        created["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_ACTIVE),
        "a destination on an authorized connection is {}, got {created}",
        state::API_DESTINATION_STATE_ACTIVE
    );

    // The second connection has had its authorization parameters stripped, so it
    // holds nothing to authorize an invocation with.
    let stripped = given_connection(&router, "stripped-connection", true).await;
    let (status, body) = events(
        &router,
        "DeauthorizeConnection",
        &json!({ "Name": "stripped-connection" }),
    )
    .await;
    assert_eq!(status, 200, "DeauthorizeConnection answered {status} {body}");
    let (status, updated) = update(&router, name, &json!({ "ConnectionArn": stripped })).await;
    assert_eq!(
        status, 200,
        "UpdateApiDestination answered {status} {updated}"
    );
    assert_eq!(
        updated["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_INACTIVE),
        "a destination repointed at a deauthorized connection is {}, got {updated}",
        state::API_DESTINATION_STATE_INACTIVE
    );

    // The deauthorization settling changes nothing: the state was already read
    // off where that connection was headed.
    tokio::time::sleep(past_the_transition(state::CONNECTION_TRANSITION)).await;
    let read = described(&router, "once the deauthorization has settled", name).await;
    assert_eq!(
        read["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_INACTIVE),
        "a deauthorized connection keeps the destination {}, got {read}",
        state::API_DESTINATION_STATE_INACTIVE
    );

    let (status, updated) = update(&router, name, &json!({ "ConnectionArn": authorized })).await;
    assert_eq!(
        status, 200,
        "UpdateApiDestination answered {status} {updated}"
    );
    assert_eq!(
        updated["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_ACTIVE),
        "repointed back at an authorized connection the destination is {}, got {updated}",
        state::API_DESTINATION_STATE_ACTIVE
    );
    let read = described(&router, "repointed back", name).await;
    assert_eq!(
        read["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_ACTIVE),
        "an authorized connection makes the destination {}, got {read}",
        state::API_DESTINATION_STATE_ACTIVE
    );
    assert_eq!(
        read["ApiDestinationArn"], created["ApiDestinationArn"],
        "the ARN must survive the update, got {read}"
    );
}
