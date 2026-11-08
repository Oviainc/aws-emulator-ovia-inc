// SPDX-License-Identifier: CC0-1.0

//! One wording for every input constraint this service enforces in a handler, over HTTP.
//!
//! A few of this service's documented input rules cannot be stated in the model — "both of
//! these members or neither", "this member is only supported for one value of that one", a
//! window measured from the time of the request, a member required for one value of a sibling,
//! a page bound whose model declares only its maximum — so the handler enforces them itself.
//! Whichever side refuses a request, the client reads ONE form of `ValidationException`
//! message: the `N validation error(s) detected:` envelope over a `Value at '<memberPath>'
//! failed to satisfy constraint: <requirement>` clause that names the member the request must
//! fix, in the lower-camel spelling the boundary validator uses, and nothing of the rejected
//! value.
//!
//! The rows span what the rule generalises over: which OPERATION refuses (the create, the
//! status read, a paginated list), which KIND of rule fires (a conditionally required member,
//! a member only one sibling value supports, a lifetime window, an ordered pair of instants, a
//! page bound), whether the member is a top-level one or nested inside a union, and the VALUE
//! and magnitude that violated it — plus a control row refused by the boundary validator
//! itself, which is the wording all of them have to match.

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

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The `ProgrammingLanguage` whose code locations must name a class, as
/// `smithy_query(query="doc",shape="CodeLocation",member="ClassName")` states it.
const JAVA: &str = "Java";

/// The scope every configuration below is stored in.
const SERVICE: &str = "checkout";
const ENVIRONMENT: &str = "eks:cluster-prod/namespace";

/// The published maximum of the create's `Service` member
/// (`smithy_query(query="operation",operation="CreateInstrumentationConfiguration")`:
/// `Service` has `length 1..255`).
const SERVICE_NAME_MAX: usize = 255;

/// The routes the rows are sent to.
const CREATE: &str = "/create-instrumentation-configuration";
const GET_STATUS: &str = "/get-instrumentation-configuration-status";
const LIST_SERVICE_STATES: &str = "/service/states";

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

/// The epoch second the test reads the clock at, which every window and lifetime below is
/// stated relative to.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The epoch second a LIFETIME is stated relative to: the clock read ROUNDED UP to the next
/// whole second.
///
/// A lifetime the service measures from the instant it serves the request is compared at
/// nanosecond precision, while the member carries whole seconds: read DOWN, the fraction of the
/// current second is already spent before the request goes out, so a row one second inside a
/// bound has only that fraction of slack and lands outside the bound on a loaded machine.
/// Rounded up, such a row keeps a whole second of slack, and a row one second OUTSIDE a bound
/// stays outside it — the rounding is strictly less than one second.
fn request_instant_seconds() -> i64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch");
    elapsed.as_secs() as i64 + i64::from(elapsed.subsec_nanos() > 0)
}

/// A `CodeLocation` of the language that requires a class name, naming one.
fn java_location(line_number: i64) -> Value {
    json!({
        "Language": JAVA,
        "ClassName": "com.example.CartService",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": line_number,
    })
}

/// The create body storing `code_location` under `instrumentation_type`.
fn create_body(instrumentation_type: &str, code_location: Value) -> Value {
    json!({
        "InstrumentationType": instrumentation_type,
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SNAPSHOT,
        "Location": { "CodeLocation": code_location },
        "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } },
    })
}

/// The status-read body for the configuration `identifier` names.
fn status_body(identifier: Value) -> Value {
    json!({
        "InstrumentationType": BREAKPOINT,
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SNAPSHOT,
        "LocationIdentifier": identifier,
    })
}

/// `body` with each member of `members` set on it.
fn with_members(body: &Value, members: Value) -> Value {
    let mut extended = body.clone();
    for (member, value) in members.as_object().expect("an object of members") {
        extended[member] = value.clone();
    }
    extended
}

/// The message a refusal of `member_path` for `requirement` must carry, built the way the
/// boundary validator's own refusals are rendered: the AWS envelope over one clause.
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

/// The member path a clause names, as spelled between the quotes.
fn member_path_of(message: &str) -> Option<&str> {
    let (_, after) = message.split_once("Value at '")?;
    after.split_once('\'').map(|(path, _)| path)
}

/// One refused row: the request, the member the refusal must name and the requirement it must
/// state, plus the caller input the message must not echo.
struct Refused {
    label: String,
    uri: &'static str,
    body: Value,
    /// `ExpiresAt`, as a number of seconds after the TIME OF THE REQUEST, for a row about the
    /// lifetime rule. Rendered by [`body_at_call_time`] rather than stored in `body`.
    expires_in: Option<i64>,
    member_path: &'static str,
    requirement: String,
    rejected: Vec<String>,
}

/// One served row: a request on the admitted side of the same rule, so the table pins which
/// requests are refused as well as how a refusal reads.
struct Served {
    label: String,
    uri: &'static str,
    body: Value,
    /// See [`Refused::expires_in`].
    expires_in: Option<i64>,
}

/// The body a row is actually sent with.
///
/// A lifetime the service measures from the TIME OF THE REQUEST is rendered here, against a
/// clock read immediately before that request goes out, because that is the instant the rule is
/// measured from: an absolute instant computed when the table was built drifts by however long
/// the rows before it took, so a row one second inside a bound would land outside it. The read
/// is rounded up ([`request_instant_seconds`]) so the whole second it falls in is slack rather
/// than already spent.
fn body_at_call_time(body: &Value, expires_in: Option<i64>) -> Value {
    match expires_in {
        Some(offset) => with_members(
            body,
            json!({ "ExpiresAt": request_instant_seconds() + offset }),
        ),
        None => body.clone(),
    }
}

/// The values a row's message must not echo: the ones the table named, plus the `ExpiresAt` the
/// row was rendered with, which is only known once [`body_at_call_time`] has read the clock.
fn rejected_values(case: &Refused, sent: &Value) -> Vec<String> {
    let mut rejected = case.rejected.clone();
    if let Some(expires_at) = sent["ExpiresAt"].as_i64() {
        rejected.push(expires_at.to_string());
    }
    rejected
}

/// Create one BREAKPOINT configuration and answer the `LocationHash` it was stored under.
async fn stored_configuration(router: &MultiServiceRouter) -> String {
    let (status, created) = call(router, CREATE, &create_body(BREAKPOINT, java_location(1))).await;
    assert_eq!(
        status, 200,
        "expected the configuration the status rows read to be created, got {status} with \
         {created}"
    );
    created["LocationHash"]
        .as_str()
        .expect("the create answers a LocationHash")
        .to_string()
}

/// The refused rows, over every input constraint this crate enforces in a handler.
fn refused_rows(now: i64, hash: &str) -> Vec<Refused> {
    let status = status_body(json!({ "LocationHash": hash }));
    // `ExpiresAt`'s published window for BREAKPOINT ("must be between 5 min and 24 hours"),
    // taken from the crate's own constants so the bound the message states and the bound it is
    // measured against cannot differ. Each bound is probed one second OUTSIDE it, as a number of
    // seconds after the time of the request: the lifetime is measured from the instant the create
    // is served, so the instant is read where the request goes out ([`body_at_call_time`]) and a
    // row exactly on the bound would still race with the fraction of a second that takes.
    let floor = BREAKPOINT_MIN_EXPIRY.as_secs() as i64;
    let ceiling = BREAKPOINT_MAX_EXPIRY.as_secs() as i64;
    vec![
        Refused {
            label: "status read, only StartTime".to_string(),
            uri: GET_STATUS,
            body: with_members(&status, json!({ "StartTime": now - 60 })),
            expires_in: None,
            member_path: "endTime",
            requirement: "Member must not be null when startTime is provided".to_string(),
            rejected: vec![(now - 60).to_string()],
        },
        Refused {
            label: "status read, only StartTime, a window a week wide".to_string(),
            uri: GET_STATUS,
            body: with_members(&status, json!({ "StartTime": now - 7 * 24 * 3_600 })),
            expires_in: None,
            member_path: "endTime",
            requirement: "Member must not be null when startTime is provided".to_string(),
            rejected: vec![(now - 7 * 24 * 3_600).to_string()],
        },
        Refused {
            label: "status read, only EndTime".to_string(),
            uri: GET_STATUS,
            body: with_members(&status, json!({ "EndTime": now })),
            expires_in: None,
            member_path: "startTime",
            requirement: "Member must not be null when endTime is provided".to_string(),
            rejected: vec![now.to_string()],
        },
        Refused {
            label: "status read, only EndTime, at the epoch".to_string(),
            uri: GET_STATUS,
            body: with_members(&status, json!({ "EndTime": 0 })),
            expires_in: None,
            member_path: "startTime",
            requirement: "Member must not be null when endTime is provided".to_string(),
            rejected: Vec::new(),
        },
        Refused {
            label: "status read, a window that ends a minute before it begins".to_string(),
            uri: GET_STATUS,
            body: with_members(&status, json!({ "StartTime": now, "EndTime": now - 60 })),
            expires_in: None,
            member_path: "endTime",
            requirement: "Member must have value greater than or equal to the value of startTime"
                .to_string(),
            rejected: vec![now.to_string(), (now - 60).to_string()],
        },
        Refused {
            label: "status read, a window that ends a day before it begins".to_string(),
            uri: GET_STATUS,
            body: with_members(
                &status,
                json!({ "StartTime": now, "EndTime": now - 24 * 3_600 }),
            ),
            expires_in: None,
            member_path: "endTime",
            requirement: "Member must have value greater than or equal to the value of startTime"
                .to_string(),
            rejected: vec![(now - 24 * 3_600).to_string()],
        },
        Refused {
            label: "status read, a window that ends half a second before it begins".to_string(),
            uri: GET_STATUS,
            body: with_members(
                &status,
                json!({ "StartTime": now as f64 + 0.5, "EndTime": now }),
            ),
            expires_in: None,
            member_path: "endTime",
            requirement: "Member must have value greater than or equal to the value of startTime"
                .to_string(),
            rejected: vec![now.to_string()],
        },
        Refused {
            label: "status read, a Java location identifier naming no class".to_string(),
            uri: GET_STATUS,
            body: status_body(json!({
                "CodeLocation": { "Language": JAVA, "FilePath": "src/Cart.java" },
            })),
            expires_in: None,
            member_path: "locationIdentifier.codeLocation.className",
            requirement: format!("Member must not be null for a {JAVA} code location"),
            rejected: vec!["src/Cart.java".to_string()],
        },
        Refused {
            label: "create, a Java location naming no class".to_string(),
            uri: CREATE,
            body: create_body(
                BREAKPOINT,
                json!({ "Language": JAVA, "FilePath": "src/Cart.java" }),
            ),
            expires_in: None,
            member_path: "location.codeLocation.className",
            requirement: format!("Member must not be null for a {JAVA} code location"),
            rejected: vec!["src/Cart.java".to_string()],
        },
        Refused {
            label: "create, an ExpiresAt on a PROBE configuration".to_string(),
            uri: CREATE,
            body: create_body(PROBE, java_location(10)),
            expires_in: Some(3_600),
            member_path: "expiresAt",
            requirement: format!(
                "Member must be null for a {PROBE} instrumentation configuration"
            ),
            rejected: Vec::new(),
        },
        Refused {
            label: "create, an ExpiresAt one second under the floor".to_string(),
            uri: CREATE,
            body: create_body(BREAKPOINT, java_location(11)),
            expires_in: Some(floor - 1),
            member_path: "expiresAt",
            requirement: format!(
                "Member must have value greater than or equal to {floor} seconds after the time \
                 of the request"
            ),
            rejected: Vec::new(),
        },
        Refused {
            label: "create, an ExpiresAt already in the past".to_string(),
            uri: CREATE,
            body: create_body(BREAKPOINT, java_location(12)),
            expires_in: Some(-1),
            member_path: "expiresAt",
            requirement: format!(
                "Member must have value greater than or equal to {floor} seconds after the time \
                 of the request"
            ),
            rejected: Vec::new(),
        },
        Refused {
            label: "create, an ExpiresAt one second over the ceiling".to_string(),
            uri: CREATE,
            body: create_body(BREAKPOINT, java_location(13)),
            expires_in: Some(ceiling + 1),
            member_path: "expiresAt",
            requirement: format!(
                "Member must have value less than or equal to {ceiling} seconds after the time \
                 of the request"
            ),
            rejected: Vec::new(),
        },
        Refused {
            label: "create, an ExpiresAt a year out".to_string(),
            uri: CREATE,
            body: create_body(BREAKPOINT, java_location(14)),
            expires_in: Some(365 * 24 * 3_600),
            member_path: "expiresAt",
            requirement: format!(
                "Member must have value less than or equal to {ceiling} seconds after the time \
                 of the request"
            ),
            rejected: Vec::new(),
        },
        // The page bound of a list whose model declares only its maximum: the same class of
        // handler-enforced rule, already worded by the builder, and the wording the rows above
        // have to read like.
        Refused {
            label: "list service states, a page bound under the minimum".to_string(),
            uri: LIST_SERVICE_STATES,
            body: json!({
                "StartTime": now - 3_600,
                "EndTime": now,
                "MaxResults": 0,
            }),
            expires_in: None,
            member_path: "maxResults",
            requirement: "Member must have value greater than or equal to 1".to_string(),
            rejected: Vec::new(),
        },
        // The control row: a constraint the model DOES state, refused by the boundary
        // validator before any handler runs. `Service` is modelled `length 1..255`
        // (`smithy_query(query="shape",shape="InstrumentationServiceName")`), so a name one
        // character over the maximum violates exactly that one constraint.
        Refused {
            label: "create, a Service longer than the model admits".to_string(),
            uri: CREATE,
            body: with_members(
                &create_body(BREAKPOINT, java_location(20)),
                json!({ "Service": "s".repeat(SERVICE_NAME_MAX + 1) }),
            ),
            expires_in: None,
            member_path: "service",
            requirement: format!(
                "Member must have length less than or equal to {SERVICE_NAME_MAX}"
            ),
            rejected: Vec::new(),
        },
    ]
}

/// The served rows: requests on the admitted side of each rule above.
fn served_rows(now: i64, hash: &str) -> Vec<Served> {
    let status = status_body(json!({ "LocationHash": hash }));
    let floor = BREAKPOINT_MIN_EXPIRY.as_secs() as i64;
    let ceiling = BREAKPOINT_MAX_EXPIRY.as_secs() as i64;
    vec![
        Served {
            label: "status read, neither end of the window".to_string(),
            uri: GET_STATUS,
            body: status.clone(),
            expires_in: None,
        },
        Served {
            label: "status read, a window that runs forwards".to_string(),
            uri: GET_STATUS,
            body: with_members(&status, json!({ "StartTime": now - 3_600, "EndTime": now })),
            expires_in: None,
        },
        Served {
            label: "status read, a window whose ends are one instant".to_string(),
            uri: GET_STATUS,
            body: with_members(&status, json!({ "StartTime": now, "EndTime": now })),
            expires_in: None,
        },
        Served {
            label: "create, no ExpiresAt on a PROBE configuration".to_string(),
            uri: CREATE,
            body: create_body(PROBE, java_location(30)),
            expires_in: None,
        },
        Served {
            label: "create, an ExpiresAt one second inside the floor".to_string(),
            uri: CREATE,
            body: create_body(BREAKPOINT, java_location(31)),
            expires_in: Some(floor + 1),
        },
        Served {
            label: "create, an ExpiresAt one second inside the ceiling".to_string(),
            uri: CREATE,
            body: create_body(BREAKPOINT, java_location(32)),
            expires_in: Some(ceiling - 1),
        },
        Served {
            label: "list service states, the smallest page bound the service serves".to_string(),
            uri: LIST_SERVICE_STATES,
            body: json!({ "StartTime": now - 3_600, "EndTime": now, "MaxResults": 1 }),
            expires_in: None,
        },
    ]
}

/// Every input constraint this service enforces in a handler is refused with the one
/// `ValidationException` wording the boundary validator uses: the envelope, the clause, the
/// member path and the requirement — and nothing of the rejected value.
#[tokio::test]
async fn every_handler_enforced_constraint_states_its_member_and_requirement() {
    let router = router();
    let hash = stored_configuration(&router).await;
    let now = now_seconds();

    for case in refused_rows(now, &hash) {
        let label = &case.label;
        let sent = body_at_call_time(&case.body, case.expires_in);
        let (status, answer) = call(&router, case.uri, &sent).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            Some("ValidationException".to_string()),
            "case {label:?}: expected a ValidationException, got {answer:?}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            one_clause(case.member_path, &case.requirement),
            "case {label:?}: expected the one envelope and clause this service words a \
             constraint violation with, got {message:?}"
        );
        for rejected in &rejected_values(&case, &sent) {
            assert!(
                !message.contains(rejected),
                "case {label:?}: the message must not echo the rejected value {rejected:?}, got \
                 {message:?}"
            );
        }
    }

    for case in served_rows(now, &hash) {
        let label = &case.label;
        let sent = body_at_call_time(&case.body, case.expires_in);
        let (status, answer) = call(&router, case.uri, &sent).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the request to be served, got {status} with {answer:?}"
        );
    }
}

/// The member path every refusal names is spelled the way the boundary validator spells one
/// for this service: lower camel case, a `.` between the steps of a nested member, and no
/// other separator — so a client reads one spelling of one member whichever side refused it.
#[tokio::test]
async fn every_refusal_spells_its_member_path_the_one_way() {
    let router = router();
    let hash = stored_configuration(&router).await;
    let now = now_seconds();

    for case in refused_rows(now, &hash) {
        let label = &case.label;
        let sent = body_at_call_time(&case.body, case.expires_in);
        let (_, answer) = call(&router, case.uri, &sent).await;
        let message = answer["message"].as_str().unwrap_or_default().to_string();
        let path = member_path_of(&message)
            .unwrap_or_else(|| panic!("case {label:?}: no member path in {message:?}"));
        for step in path.split('.') {
            let first = step.chars().next().unwrap_or(' ');
            assert!(
                first.is_ascii_lowercase(),
                "case {label:?}: expected every step of the member path to start lower case, \
                 got {path:?}"
            );
            assert!(
                step.chars().all(|character| character.is_ascii_alphanumeric()),
                "case {label:?}: expected the member path to carry no separator but '.', got \
                 {path:?}"
            );
        }
    }
}
