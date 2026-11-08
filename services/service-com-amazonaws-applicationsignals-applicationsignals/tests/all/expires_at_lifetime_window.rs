// SPDX-License-Identifier: CC0-1.0

//! The lifetime a create may ask a dynamic instrumentation configuration to be kept for, over
//! HTTP: which lifetimes are STORED, which are refused, and the one `ValidationException`
//! wording every refusal of them carries.
//!
//! `ExpiresAt`'s own documentation states the rule
//! (`smithy_query(query="doc",shape="CreateInstrumentationConfigurationRequest",member="ExpiresAt")`):
//! "For BREAKPOINT: optional, defaults to 24 hours, must be between 5 min and 24 hours. For
//! PROBE: not supported. PROBE configurations are permanent and persist until explicitly
//! deleted." Neither half can be stated in the model — a window measured from the time of the
//! request, and a member only one value of a sibling supports — so the create enforces both
//! itself, and a request outside them is refused with 400 through the ONE form this service
//! words a constraint violation in: the `N validation error(s) detected:` envelope over a
//! `Value at '<memberPath>' failed to satisfy constraint: <requirement>` clause that names the
//! member in the lower-camel spelling the boundary validator uses, states the bound it
//! violated, and carries nothing of the rejected instant.
//!
//! The rows SWEEP the lifetime a request asks for rather than naming the boundary alone: an
//! instant a year behind the request, the instant of the request itself, each published bound
//! and a second either side of it, the middle of the window, a week and a century ahead — under
//! both modelled `InstrumentationType` values
//! (`smithy_query(query="shape",shape="InstrumentationType")`), and with the member omitted.
//! Each row states which side of the rule it falls on, so the table pins which lifetimes are
//! served as well as how a refusal reads, and every stored lifetime is read back through
//! `GetInstrumentationConfiguration`. The last test's control is a constraint the model DOES
//! state, refused by the boundary validator before the handler runs: the wording every refusal
//! above has to match.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    BREAKPOINT_MAX_EXPIRY, BREAKPOINT_MIN_EXPIRY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values: the temporary one, which "expires automatically
/// (default 24 hours)", and the permanent one, which "persists until explicitly deleted".
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The `ProgrammingLanguage` the locations below are written in, with the `ClassName` its own
/// doc requires for it, so no row is refused for the location instead of the lifetime.
const JAVA: &str = "Java";

/// The scope every row is created in.
const SERVICE: &str = "checkout";
const ENVIRONMENT: &str = "eks:cluster-prod/namespace";

/// The published maximum of the create's `Service` member
/// (`smithy_query(query="operation",operation="CreateInstrumentationConfiguration")`: `Service`
/// has `length 1..255` and a pattern a name of plain letters satisfies, so a name one character
/// over that maximum violates exactly one constraint).
const SERVICE_NAME_MAX: usize = 255;

/// The member path a refusal of the lifetime rule names, in the lower-camel spelling this
/// service's `ValidationException` messages state a member with.
const EXPIRES_AT_PATH: &str = "expiresAt";

/// The member path the control row's refusal names.
const SERVICE_PATH: &str = "service";

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let payload = serde_json::to_vec(body).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `POST /create-instrumentation-configuration`.
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "/create-instrumentation-configuration", body).await
}

/// `POST /get-instrumentation-configuration`.
async fn get(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "/get-instrumentation-configuration", body).await
}

/// The epoch second the clock reads, which every lifetime below is stated relative to. The
/// window is measured from the instant the create is SERVED, so each row reads the clock
/// immediately before its own request goes out: an instant computed once for the whole table
/// would drift by however long the rows before it took, and a row a second inside a bound would
/// land outside it.
///
/// The read is rounded UP to the next whole second. The service compares the lifetime at
/// nanosecond precision while the member carries whole seconds, so a read rounded DOWN leaves a
/// row one second inside a bound with only the remainder of the current second as slack, which
/// a loaded machine spends before the request is served. The rounding is strictly less than one
/// second, so a row one second OUTSIDE a bound stays outside it.
fn now_seconds() -> i64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch");
    elapsed.as_secs() as i64 + i64::from(elapsed.subsec_nanos() > 0)
}

/// The create body for one row: the instrumentation type under test, a location of its own (so
/// a row is never refused as a conflict with an earlier one) and the `ExpiresAt` the row asks
/// for, omitted when it names none.
fn create_body(instrumentation_type: &str, line_number: i64, expires_at: Option<i64>) -> Value {
    let mut body = json!({
        "InstrumentationType": instrumentation_type,
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SNAPSHOT,
        "Location": { "CodeLocation": {
            "Language": JAVA,
            "ClassName": "com.example.CartService",
            "FilePath": "src/main/java/com/example/CartService.java",
            "LineNumber": line_number,
        } },
        "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } },
    });
    if let Some(expires_at) = expires_at {
        body["ExpiresAt"] = json!(expires_at);
    }
    body
}

/// The read of the configuration a create stored, by the `LocationHash` it answered.
fn get_body(create_body: &Value, location_hash: &str) -> Value {
    json!({
        "InstrumentationType": create_body["InstrumentationType"],
        "Service": create_body["Service"],
        "Environment": create_body["Environment"],
        "SignalType": create_body["SignalType"],
        "LocationIdentifier": { "LocationHash": location_hash },
    })
}

/// The message a refusal of `member_path` for `requirement` must carry: the envelope over one
/// clause, the form the boundary validator's own refusals are rendered in.
fn one_clause(member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The error code a refusal reports, with any namespace prefix the envelope carries dropped.
fn error_code(answer: &Value) -> Option<String> {
    answer["__type"]
        .as_str()
        .map(|kind| kind.rsplit('#').next().unwrap_or(kind).to_string())
}

/// The member path and the requirement a message states, or `None` when it is not one clause
/// under the envelope at all.
fn clause_parts(message: &str) -> Option<(&str, &str)> {
    let (_, after_envelope) = message.split_once("1 validation error detected: Value at '")?;
    let (member_path, after_path) = after_envelope.split_once('\'')?;
    let requirement = after_path.strip_prefix(" failed to satisfy constraint: ")?;
    Some((member_path, requirement))
}

/// One row of the sweep: the lifetime the create asks for, and the side of the rule it falls
/// on.
struct Row {
    label: &'static str,
    instrumentation_type: &'static str,
    /// `ExpiresAt`, as a number of seconds after the time of the request; `None` omits the
    /// member.
    expires_in: Option<i64>,
    /// The requirement the refusal must state, or `None` for a lifetime the create serves.
    requirement: Option<String>,
}

/// The lifetime window `ExpiresAt` publishes for a BREAKPOINT configuration, taken from the
/// crate's own constants so the bound a message states and the bound it is measured against
/// cannot differ.
fn window() -> (i64, i64) {
    (
        BREAKPOINT_MIN_EXPIRY.as_secs() as i64,
        BREAKPOINT_MAX_EXPIRY.as_secs() as i64,
    )
}

/// The requirement a lifetime shorter than the published floor violates.
fn below_floor() -> Option<String> {
    let (floor, _) = window();
    Some(format!(
        "Member must have value greater than or equal to {floor} seconds after the time of the \
         request"
    ))
}

/// The requirement a lifetime longer than the published ceiling violates.
fn above_ceiling() -> Option<String> {
    let (_, ceiling) = window();
    Some(format!(
        "Member must have value less than or equal to {ceiling} seconds after the time of the \
         request"
    ))
}

/// The requirement an expiry on a configuration type that does not expire violates.
fn unsupported(instrumentation_type: &str) -> Option<String> {
    Some(format!(
        "Member must be null for a {instrumentation_type} instrumentation configuration"
    ))
}

/// The swept lifetimes: below the floor, inside the window, above the ceiling, and — for the
/// type that supports no expiry at all — every one of those.
fn rows() -> Vec<Row> {
    let (floor, ceiling) = window();
    vec![
        Row {
            label: "breakpoint, an instant a year behind the request",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(-365 * 24 * 3_600),
            requirement: below_floor(),
        },
        Row {
            label: "breakpoint, an instant a minute behind the request",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(-60),
            requirement: below_floor(),
        },
        Row {
            label: "breakpoint, the instant of the request itself",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(0),
            requirement: below_floor(),
        },
        Row {
            label: "breakpoint, a minute under the floor",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(floor - 60),
            requirement: below_floor(),
        },
        Row {
            label: "breakpoint, one second under the floor",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(floor - 1),
            requirement: below_floor(),
        },
        Row {
            label: "breakpoint, one second inside the floor",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(floor + 1),
            requirement: None,
        },
        Row {
            label: "breakpoint, a minute inside the floor",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(floor + 60),
            requirement: None,
        },
        Row {
            label: "breakpoint, the middle of the window",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(ceiling / 2),
            requirement: None,
        },
        Row {
            label: "breakpoint, a minute inside the ceiling",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(ceiling - 60),
            requirement: None,
        },
        Row {
            label: "breakpoint, one second inside the ceiling",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(ceiling - 1),
            requirement: None,
        },
        Row {
            label: "breakpoint, no expiry at all, which takes the documented default",
            instrumentation_type: BREAKPOINT,
            expires_in: None,
            requirement: None,
        },
        Row {
            label: "breakpoint, one second over the ceiling",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(ceiling + 1),
            requirement: above_ceiling(),
        },
        Row {
            label: "breakpoint, a minute over the ceiling",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(ceiling + 60),
            requirement: above_ceiling(),
        },
        Row {
            label: "breakpoint, a week ahead of the request",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(7 * 24 * 3_600),
            requirement: above_ceiling(),
        },
        Row {
            label: "breakpoint, a century ahead of the request",
            instrumentation_type: BREAKPOINT,
            expires_in: Some(100 * 365 * 24 * 3_600),
            requirement: above_ceiling(),
        },
        Row {
            label: "probe, an instant behind the request",
            instrumentation_type: PROBE,
            expires_in: Some(-60),
            requirement: unsupported(PROBE),
        },
        Row {
            label: "probe, the instant of the request itself",
            instrumentation_type: PROBE,
            expires_in: Some(0),
            requirement: unsupported(PROBE),
        },
        Row {
            label: "probe, a lifetime inside the breakpoint window",
            instrumentation_type: PROBE,
            expires_in: Some(floor + 1),
            requirement: unsupported(PROBE),
        },
        Row {
            label: "probe, the breakpoint ceiling",
            instrumentation_type: PROBE,
            expires_in: Some(ceiling),
            requirement: unsupported(PROBE),
        },
        Row {
            label: "probe, a week ahead of the request",
            instrumentation_type: PROBE,
            expires_in: Some(7 * 24 * 3_600),
            requirement: unsupported(PROBE),
        },
        Row {
            label: "probe, no expiry at all",
            instrumentation_type: PROBE,
            expires_in: None,
            requirement: None,
        },
    ]
}

/// Every swept lifetime is either stored as the request named it or refused with the member
/// path and the bound it violated, in the one wording this service words a constraint
/// violation in — and a stored one is the lifetime a later read reports.
#[tokio::test]
async fn every_requested_lifetime_is_served_or_refused_by_the_documented_window() {
    let router = router();
    let (_, ceiling) = window();

    for (index, row) in rows().into_iter().enumerate() {
        let label = row.label;
        // The clock is read per row, immediately before the request: see [`now_seconds`].
        let expires_at = row.expires_in.map(|offset| now_seconds() + offset);
        let body = create_body(row.instrumentation_type, 1_000 + index as i64, expires_at);
        let (status, answer) = create(&router, &body).await;

        let Some(requirement) = row.requirement.as_deref() else {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the lifetime to be served, got {status} with {answer:?}"
            );
            let created_at = answer["CreatedAt"].as_f64();
            let stored = answer["ExpiresAt"].as_f64();
            match (row.instrumentation_type, expires_at) {
                // The instant the request named, stored as it named it.
                (BREAKPOINT, Some(requested)) => assert_eq!(
                    stored,
                    Some(requested as f64),
                    "case {label:?}: expected the requested expiry {requested} to be stored, got \
                     {:?}",
                    answer["ExpiresAt"]
                ),
                // The documented default, measured from the instant the create was served.
                (BREAKPOINT, None) => {
                    let lifetime = stored.unwrap_or_default() - created_at.unwrap_or_default();
                    assert!(
                        (lifetime - ceiling as f64).abs() < 1.0,
                        "case {label:?}: expected the documented default lifetime of {ceiling} \
                         seconds, got {lifetime} from {:?} to {:?}",
                        answer["CreatedAt"],
                        answer["ExpiresAt"]
                    );
                }
                // A type that persists until it is deleted carries no expiry.
                _ => assert!(
                    stored.is_none(),
                    "case {label:?}: expected no expiry for a {} configuration, got {:?}",
                    row.instrumentation_type,
                    answer["ExpiresAt"]
                ),
            }

            // The lifetime a create reports is the lifetime it STORED, so a read of the same
            // configuration reports the same member.
            let location_hash = answer["LocationHash"].as_str().unwrap_or_default();
            let (read_status, stored_record) =
                get(&router, &get_body(&body, location_hash)).await;
            assert_eq!(
                read_status, 200,
                "case {label:?}: expected the stored configuration to be readable, got \
                 {read_status} with {stored_record:?}"
            );
            // The read answers the whole record under `Configuration`
            // (`smithy_query(query="operation",operation="GetInstrumentationConfiguration")`).
            assert_eq!(
                stored_record["Configuration"]["ExpiresAt"], answer["ExpiresAt"],
                "case {label:?}: expected the read to report the stored expiry {:?}, got {:?}",
                answer["ExpiresAt"], stored_record["Configuration"]["ExpiresAt"]
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {label:?}: expected the lifetime to be refused with HTTP 400, got {status} \
             with {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            Some("ValidationException".to_string()),
            "case {label:?}: expected a ValidationException, got {answer:?}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            one_clause(EXPIRES_AT_PATH, requirement),
            "case {label:?}: expected the one envelope and clause this service words a \
             constraint violation with, got {message:?}"
        );
        if let Some(rejected) = expires_at {
            assert!(
                !message.contains(&rejected.to_string()),
                "case {label:?}: the message must not echo the rejected instant {rejected}, got \
                 {message:?}"
            );
        }
    }
}

/// A refusal of the lifetime rule reads exactly like a refusal of a constraint the MODEL states
/// on the same operation: one envelope, one clause, one lower-camel member path and the
/// requirement it violated — so a client that parses the message to learn which member to fix
/// reads the same two facts whichever side of the service refused its request.
#[tokio::test]
async fn a_handler_enforced_lifetime_reads_like_a_modelled_constraint_of_the_same_operation() {
    let router = router();
    let (floor, _) = window();

    // The modelled constraint: a `Service` one character over its published maximum, which the
    // boundary validator refuses before the handler runs.
    let mut modelled = create_body(BREAKPOINT, 2_000, None);
    modelled["Service"] = json!("s".repeat(SERVICE_NAME_MAX + 1));
    let (modelled_status, modelled_answer) = create(&router, &modelled).await;
    assert_eq!(
        modelled_status, 400,
        "the modelled constraint: expected HTTP 400, got {modelled_status} with {modelled_answer:?}"
    );
    let modelled_message = modelled_answer["message"].as_str().unwrap_or_default();
    assert_eq!(
        modelled_message,
        one_clause(
            SERVICE_PATH,
            &format!("Member must have length less than or equal to {SERVICE_NAME_MAX}")
        ),
        "the modelled constraint: expected the service's one constraint wording, got \
         {modelled_message:?}"
    );

    // The handler-enforced one, on the same operation and in the same request shape.
    let handler = create_body(BREAKPOINT, 2_001, Some(now_seconds() + floor - 1));
    let (handler_status, handler_answer) = create(&router, &handler).await;
    assert_eq!(
        handler_status, modelled_status,
        "the lifetime rule: expected the status a modelled constraint is refused with \
         ({modelled_status}), got {handler_status} with {handler_answer:?}"
    );
    assert_eq!(
        error_code(&handler_answer),
        error_code(&modelled_answer),
        "the lifetime rule: expected the error a modelled constraint is refused with ({:?}), got \
         {:?}",
        error_code(&modelled_answer),
        error_code(&handler_answer)
    );

    let handler_message = handler_answer["message"].as_str().unwrap_or_default();
    for (side, message) in [
        ("the modelled constraint", modelled_message),
        ("the lifetime rule", handler_message),
    ] {
        let (member_path, requirement) = clause_parts(message).unwrap_or_else(|| {
            panic!("{side}: expected one clause under the envelope, got {message:?}")
        });
        assert!(
            member_path
                .split('.')
                .all(|step| step.starts_with(|first: char| first.is_ascii_lowercase())
                    && step.chars().all(|character| character.is_ascii_alphanumeric())),
            "{side}: expected a lower-camel member path with '.' between its steps, got \
             {member_path:?}"
        );
        assert!(
            requirement.starts_with("Member must "),
            "{side}: expected the clause to state the requirement the member violated, got \
             {requirement:?}"
        );
    }
}
