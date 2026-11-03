// SPDX-License-Identifier: CC0-1.0

//! The control-plane rules this slice answers a REQUEST with, over the wire.
//!
//! Each test below pins one rule over a SET of inputs rather than one recorded
//! call, and every assertion reads the response body:
//!
//! * a member documented as an Amazon S3 location is refused synchronously, before
//!   any resource is allocated, when its value is not an `s3://` URI;
//! * an `@idempotencyToken` replayed with the same parameters answers with the
//!   resource the first call created, and replayed with DIFFERENT parameters is
//!   refused with `ConflictException` (409);
//! * an `Identifier` that names no resource is answered with the resource's
//!   RESOLVED ARN and a per-request internal id;
//! * `StreamClass` accepts exactly the set the service accepts, which is narrower
//!   than the model's enum;
//! * `AssociateApplications` persists the link both resources then report;
//! * `CreateStreamSessionConnection` resolves its two identifiers from state;
//! * a `ResourceArn` that is not an ARN is a `BadRequestException` (400).

use std::collections::HashMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_gameliftstreams_gameliftstreams::state::Fingerprint;

// The wire harness is this crate's one test harness, declared by the
// `stream_workflow` suite: one emulator on one shared engine, and one restJson1
// call against it.
use crate::stream_workflow::{
    as_label, call, call_in_scope_region, emulator, ready_application, text, ACCOUNT,
    PAST_TRANSITION, REGION,
};

/// A `ClientToken` is modelled `length 32..128, pattern ^[\x21-\x7E]+$`.
const TOKEN_A: &str = "client-token-aaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN_B: &str = "client-token-bbbbbbbbbbbbbbbbbbbbbbbb";
const TOKEN_C: &str = "client-token-cccccccccccccccccccccccc";

/// The wire error code of an error body (`__type`), or the empty string.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The error message of an error body. A modelled error renders the member's
/// `Message`; the request-validation envelope renders `message`.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn application_arn(id: &str) -> String {
    format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/{id}")
}

fn stream_group_arn(id: &str) -> String {
    format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamgroup/{id}")
}

fn stream_session_arn(group: &str, session: &str) -> String {
    format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/{group}/{session}")
}

/// The harness's `CreateApplication` fixture with its `ApplicationSourceUri`
/// replaced by `source` — the one member these tests vary, which is the whole point
/// of the first table below. The rest of the fixture stays the harness's, so there
/// is still one definition of what a valid create looks like.
fn application_body(description: &str, source: &str) -> Value {
    let mut body = crate::stream_workflow::application_body(description);
    body["ApplicationSourceUri"] = json!(source);
    body
}

/// A `CreateStreamGroup` body whose stream class varies and whose
/// `DefaultApplicationIdentifier` is omitted when `default_application` is empty.
///
/// Not the harness's `stream_group` helper: these tests need a group created with
/// NO default application (the association rule of cause 5) and one created with
/// an arbitrary stream class (cause 4), neither of which that helper can express.
fn stream_group_body(description: &str, stream_class: &str, default_application: &str) -> Value {
    let mut body = json!({
        "Description": description,
        "StreamClass": stream_class,
        "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}],
    });
    if !default_application.is_empty() {
        body["DefaultApplicationIdentifier"] = json!(default_application);
    }
    body
}

/// Create an `ACTIVE` stream group whose default application is OPTIONAL: an empty
/// `default_application` leaves `DefaultApplicationIdentifier` out of the request
/// altogether, which is the one shape the harness's own
/// `stream_workflow::active_stream_group` cannot express (it always names an
/// application) and which the association rule below needs. Named apart from that
/// helper deliberately, so the two cannot be confused.
async fn group_with_optional_default_application(
    router: &MultiServiceRouter,
    description: &str,
    default_application: &str,
) -> (String, String) {
    let (status, created) = call(
        router,
        "POST",
        "/streamgroups",
        Some(stream_group_body(description, "gen5n_high", default_application)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {description:?}: expected CreateStreamGroup 201, got {status} {created}"
    );
    std::thread::sleep(PAST_TRANSITION);
    (
        text(&created, "/Id", description),
        text(&created, "/Arn", description),
    )
}

/// How many applications the store holds.
async fn application_count(router: &MultiServiceRouter) -> usize {
    let (_, listed) = call(router, "GET", "/applications", None).await;
    listed["Items"].as_array().map_or(0, Vec::len)
}

/// How many stream groups the store holds.
async fn stream_group_count(router: &MultiServiceRouter) -> usize {
    let (_, listed) = call(router, "GET", "/streamgroups", None).await;
    listed["Items"].as_array().map_or(0, Vec::len)
}

/// Split `"<text> (Internal ID: <uuid>)"` into its text and its internal id, the
/// way a client that compares messages has to.
fn without_internal_id(message: &str, case: &str) -> (String, String) {
    let (text, rest) = message
        .rsplit_once(" (Internal ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: expected a trailing internal id in {message:?}"));
    assert!(
        rest.ends_with(')'),
        "case {case:?}: expected the internal id to be parenthesised, got {message:?}"
    );
    let id = rest.trim_end_matches(')').to_string();
    assert!(
        !id.is_empty(),
        "case {case:?}: expected a non-empty internal id in {message:?}"
    );
    (text.to_string(), id)
}

/// One row of the not-found table: a call whose `Identifier` names no resource, and
/// the message the service must answer it with.
struct NotFound {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The ARN the message must name, whichever form the request used.
    arn: String,
    /// How the service names the kind of resource that was not found.
    kind: &'static str,
}

impl NotFound {
    /// A row that posts `body`.
    fn post(case: &'static str, path: String, body: Value, arn: String) -> Self {
        Self {
            case,
            method: "POST",
            path,
            body: Some(body),
            arn,
            kind: "Resource",
        }
    }

    /// The same row, for a resource the service names by kind.
    fn of_kind(mut self, kind: &'static str) -> Self {
        self.kind = kind;
        self
    }
}

/// `ApplicationSourceUri` must name an Amazon S3 location, and the refusal happens
/// BEFORE anything is allocated.
///
/// The rule covers the shape of the URI, not one recorded value: the scheme and the
/// bucket segment, over the same bucket rule the model already states for this
/// service's other S3 member, `ApplicationLogOutputUri`
/// (`^s3://([a-zA-Z0-9][a-zA-Z0-9._-]{1,61}[a-zA-Z0-9])(/[a-zA-Z0-9._-]+)*/?$`) —
/// so each published bucket-length bound, and one past it, is a row below.
#[tokio::test]
async fn an_application_source_uri_must_name_an_s3_location() {
    let router = emulator();
    let shortest_bucket = "abc";
    let longest_bucket = format!("a{}c", "b".repeat(61));
    let too_long_bucket = format!("a{}c", "b".repeat(62));

    let accepted: Vec<(&str, String)> = vec![
        ("bucket only", "s3://my-bucket".to_string()),
        ("bucket and prefix", "s3://my-bucket/build/".to_string()),
        (
            "multi-level prefix",
            "s3://my.bucket_name-1/a/b/content.zip".to_string(),
        ),
        (
            "shortest published bucket",
            format!("s3://{shortest_bucket}"),
        ),
        ("longest published bucket", format!("s3://{longest_bucket}")),
    ];
    let refused: Vec<(&str, String)> = vec![
        (
            "https scheme",
            "https://example.com/not-an-s3-uri.zip".to_string(),
        ),
        ("file scheme", "file:///tmp/build.zip".to_string()),
        ("no scheme", "my-bucket/build/".to_string()),
        ("one slash", "s3:/my-bucket/build/".to_string()),
        ("upper-case scheme", "S3://my-bucket/build/".to_string()),
        ("bucket below the published bound", "s3://ab".to_string()),
        (
            "bucket past the published bound",
            format!("s3://{too_long_bucket}"),
        ),
        ("bucket starting with a dash", "s3://-my-bucket/x".to_string()),
        ("bucket ending with a dot", "s3://my-bucket./x".to_string()),
        ("space in the bucket", "s3://my bucket/x".to_string()),
    ];

    for (case, source) in accepted {
        let (status, created) = call(
            &router,
            "POST",
            "/applications",
            Some(application_body("accepted-source", &source)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?} ({source}): expected 201, got {status} {created}"
        );
        // Round trip: the stored record is what a later read reports.
        let id = text(&created, "/Id", case);
        let (status, got) = call(&router, "GET", &format!("/applications/{id}"), None).await;
        assert_eq!(
            (status, got["ApplicationSourceUri"].as_str()),
            (200, Some(source.as_str())),
            "case {case:?}: expected GetApplication to report the stored source, got {status} \
             {got}"
        );
    }

    for (case, source) in refused {
        let before = application_count(&router).await;
        let (status, got) = call(
            &router,
            "POST",
            "/applications",
            Some(application_body("refused-source", &source)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({source}): expected 400, got {status} {got}"
        );
        assert_eq!(
            error_code(&got),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {got}"
        );
        let (message, _) = without_internal_id(&error_message(&got), case);
        assert!(
            message.contains("'ApplicationSourceUri'"),
            "case {case:?}: expected the message to name the member, got {message:?}"
        );
        assert_eq!(
            application_count(&router).await,
            before,
            "case {case:?}: a refused create must allocate nothing"
        );
    }
}

/// A `ClientToken` replayed with the SAME parameters answers with the resource the
/// first call created; replayed with DIFFERENT parameters it is refused with
/// `ConflictException` (409) and creates nothing.
///
/// The rule holds for every create in this slice that carries the modelled
/// `@idempotencyToken` and models the conflict, and for any member of the request:
/// the rows differ in a scalar, in a list, and in a map.
#[tokio::test]
async fn a_client_token_replayed_with_other_parameters_conflicts() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "token-fixture").await;
    let (group_id, _) =
        group_with_optional_default_application(&router, "token-fixture-group", &app_id).await;

    // (case, path, first body, a body differing in one member)
    let session_path = format!("/streamgroups/{group_id}/streamsessions");
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "CreateApplication Description",
            "/applications",
            application_body("token-app", "s3://my-bucket/build/"),
            application_body("token-app-DIFFERENT", "s3://my-bucket/build/"),
        ),
        (
            "CreateApplication ApplicationSourceUri",
            "/applications",
            application_body("token-app-2", "s3://my-bucket/build/"),
            application_body("token-app-2", "s3://my-bucket/other-build/"),
        ),
        (
            "CreateApplication Tags",
            "/applications",
            {
                let mut body = application_body("token-app-3", "s3://my-bucket/build/");
                body["Tags"] = json!({"env": "test"});
                body
            },
            {
                let mut body = application_body("token-app-3", "s3://my-bucket/build/");
                body["Tags"] = json!({"env": "prod"});
                body
            },
        ),
        (
            "CreateStreamGroup Description",
            "/streamgroups",
            stream_group_body("token-group", "gen4n_high", ""),
            stream_group_body("token-group-DIFFERENT", "gen4n_high", ""),
        ),
        (
            "CreateStreamGroup StreamClass",
            "/streamgroups",
            stream_group_body("token-group-2", "gen4n_high", ""),
            stream_group_body("token-group-2", "gen5n_ultra", ""),
        ),
        (
            "CreateStreamGroup LocationConfigurations",
            "/streamgroups",
            stream_group_body("token-group-3", "gen4n_high", ""),
            {
                let mut body = stream_group_body("token-group-3", "gen4n_high", "");
                body["LocationConfigurations"] =
                    json!([{"LocationName": REGION, "AlwaysOnCapacity": 2}]);
                body
            },
        ),
        (
            "StartStreamSession Description",
            &session_path,
            json!({
                "Protocol": "WebRTC",
                "SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}",
                "ApplicationIdentifier": app_arn,
                "Description": "token-session",
            }),
            json!({
                "Protocol": "WebRTC",
                "SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}",
                "ApplicationIdentifier": app_arn,
                "Description": "token-session-DIFFERENT",
            }),
        ),
    ];

    for (index, (case, path, first_body, other_body)) in cases.into_iter().enumerate() {
        // A distinct token per row, so one row cannot explain another.
        let token = format!("replay-token-{index}-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let mut first_body = first_body;
        first_body["ClientToken"] = json!(token);
        let mut other_body = other_body;
        other_body["ClientToken"] = json!(token);

        let (status, first) = call(&router, "POST", path, Some(first_body.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first create to succeed, got {status} {first}"
        );
        let first_arn = text(&first, "/Arn", case);

        // Same token, same parameters: the resource the first call created.
        let (status, replayed) = call(&router, "POST", path, Some(first_body)).await;
        assert_eq!(
            (status, replayed["Arn"].as_str()),
            (201, Some(first_arn.as_str())),
            "case {case:?}: expected the replay to answer with {first_arn}, got {status} \
             {replayed}"
        );

        // Same token, one member changed: refused, and nothing created.
        let (status, conflict) = call(&router, "POST", path, Some(other_body)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 for the conflicting replay, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {conflict}"
        );
        let (message, _) = without_internal_id(&error_message(&conflict), case);
        assert!(
            message.contains("'ClientToken'"),
            "case {case:?}: expected the message to name ClientToken, got {message:?}"
        );
    }
}

/// Two requests carrying the same token and the same tags in a different ORDER are
/// the same request: a `HashMap`'s iteration order is not a request parameter.
#[tokio::test]
async fn a_client_token_ignores_the_order_of_a_map_member() {
    let router = emulator();
    let mut first = application_body("tag-order", "s3://my-bucket/build/");
    first["Tags"] = json!({"a": "1", "b": "2", "c": "3"});
    first["ClientToken"] = json!(TOKEN_A);
    let mut reordered = application_body("tag-order", "s3://my-bucket/build/");
    reordered["Tags"] = json!({"c": "3", "b": "2", "a": "1"});
    reordered["ClientToken"] = json!(TOKEN_A);

    let (status, created) = call(&router, "POST", "/applications", Some(first)).await;
    assert_eq!(status, 201, "the first create: got {status} {created}");
    let (status, replayed) = call(&router, "POST", "/applications", Some(reordered)).await;
    assert_eq!(
        (status, replayed["Arn"].as_str()),
        (201, created["Arn"].as_str()),
        "case \"reordered tags\": expected the same application, got {status} {replayed}"
    );
    assert_eq!(
        application_count(&router).await,
        1,
        "case \"reordered tags\": expected exactly one application"
    );

    // Over the wire the JSON object's key order is not observable (`serde_json`'s
    // `Map` is sorted unless `preserve_order` is on), so the ORDER-independence of
    // the fingerprint is asserted on the fingerprint itself, over maps whose
    // iteration order differs because their insertion order did.
    let pairs = [("a", "1"), ("b", "2"), ("c", "3"), ("d", "4"), ("e", "5")];
    let forwards: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let backwards: HashMap<String, String> = pairs
        .iter()
        .rev()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let mut one = Fingerprint::new();
    one.map("Tags", &forwards);
    let one = one.finish();
    let mut other = Fingerprint::new();
    other.map("Tags", &backwards);
    assert_eq!(
        one,
        other.finish(),
        "case \"a map's iteration order\": the same entries must fingerprint alike"
    );

    // And a map that differs in ONE entry fingerprints differently, so the
    // order-independence above is not just "every map looks alike".
    let mut changed = forwards.clone();
    changed.insert("e".to_string(), "6".to_string());
    let mut changed_fingerprint = Fingerprint::new();
    changed_fingerprint.map("Tags", &changed);
    assert_ne!(
        one,
        changed_fingerprint.finish(),
        "case \"a changed map entry\": a different map must fingerprint differently"
    );
}

/// An `Identifier` that names no resource is refused with `ResourceNotFoundException`
/// (404) whose message names the resource's RESOLVED ARN and ends with a per-request
/// internal id.
///
/// The API-wide table for that rule lives in the `not_found_messages` suite beside
/// this one; the rows below are the operations THIS change adds to the API —
/// `AssociateApplications` (resource-scoped, on either of its two identifiers) and
/// `CreateStreamSessionConnection` (session-scoped, so an absent stream group is the
/// same miss as an absent session) — plus the deleted stream group a later read must
/// report by ARN.
#[tokio::test]
async fn a_not_found_names_the_resolved_arn_and_an_internal_id() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "present").await;
    let (group_id, group_arn) =
        group_with_optional_default_application(&router, "present-group", &app_id).await;

    let cases: Vec<NotFound> = vec![
        NotFound::post(
            "AssociateApplications on an unknown stream group",
            "/streamgroups/sg-missing/associations".to_string(),
            json!({"ApplicationIdentifiers": [app_id]}),
            stream_group_arn("sg-missing"),
        ),
        NotFound::post(
            "AssociateApplications with an unknown application",
            format!("/streamgroups/{group_id}/associations"),
            json!({"ApplicationIdentifiers": ["a-missing"]}),
            application_arn("a-missing"),
        ),
        NotFound::post(
            "CreateStreamSessionConnection with an unknown session",
            format!("/streamgroups/{group_id}/streamsessions/ss-missing/connections"),
            json!({"SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}"}),
            stream_session_arn(&group_id, "ss-missing"),
        )
        .of_kind("Stream session"),
        // Session-scoped, so an absent GROUP is the same miss as an absent session:
        // the message names the composite stream session ARN either way.
        NotFound::post(
            "CreateStreamSessionConnection in an unknown stream group",
            "/streamgroups/sg-missing/streamsessions/ss-missing/connections".to_string(),
            json!({"SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}"}),
            stream_session_arn("sg-missing", "ss-missing"),
        )
        .of_kind("Stream session"),
    ];

    for row in cases {
        let NotFound {
            case,
            method,
            path,
            body,
            arn,
            kind,
        } = row;
        let (status, got) = call(&router, method, &path, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {method} {path}, got {status} {got}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {got}"
        );
        let (message, first_id) = without_internal_id(&error_message(&got), case);
        assert_eq!(
            message,
            format!("{kind} '{arn}' does not exist"),
            "case {case:?}: unexpected message, got {message:?}"
        );

        // The internal id is per request: the same call made twice carries the same
        // ARN and a different id.
        let (_, again) = call(&router, method, &path, body).await;
        let (message_again, second_id) = without_internal_id(&error_message(&again), case);
        assert_eq!(
            message_again, message,
            "case {case:?}: the message must not depend on the call, got {message_again:?}"
        );
        assert_ne!(
            first_id, second_id,
            "case {case:?}: the internal id must be per request, got {first_id} twice"
        );
    }

    // And the group that WAS there answers its ARN once it is deleted.
    let (status, got) = call(&router, "DELETE", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(status, 204, "DeleteStreamGroup: got {status} {got}");
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(
        &router,
        "GET",
        &format!("/streamgroups/{}", as_label(&group_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"GetStreamGroup after DeleteStreamGroup\": got {status} {got}"
    );
    let (message, _) = without_internal_id(&error_message(&got), "deleted stream group");
    assert_eq!(
        message,
        format!("Resource '{group_arn}' does not exist"),
        "case \"GetStreamGroup after DeleteStreamGroup\": unexpected message, got {message:?}"
    );
}

/// `StreamClass` accepts exactly the set the service accepts — which is NARROWER
/// than the model's enum — and a value outside it is refused with the enumerated
/// set, whether the model declares that value or not.
#[tokio::test]
async fn a_stream_class_outside_the_service_set_is_refused() {
    let router = emulator();

    // The accepted set, spelled out here from the service's own recorded refusal —
    // carried by the TEST, so every assertion below reads the wire rather than the
    // implementation's own constant.
    let recorded = [
        "gen4n_high",
        "gen4n_ultra",
        "gen4n_win2022",
        "gen5n_high",
        "gen5n_ultra",
        "gen5n_win2022",
        "gen6n_small",
        "gen6n_medium",
        "gen6n_high",
        "gen6n_ultra",
        "gen6n_ultra_win2022",
        "gen6n_pro",
        "gen6n_pro_win2022",
        "gen6e_pro",
        "gen6e_pro_win2022",
    ];
    // Every accepted value creates a group, and the group reports it back.
    for stream_class in recorded {
        let (status, created) = call(
            &router,
            "POST",
            "/streamgroups",
            Some(stream_group_body("accepted-class", stream_class, "")),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {stream_class:?}: expected 201, got {status} {created}"
        );
        let id = text(&created, "/Id", stream_class);
        let (status, got) = call(&router, "GET", &format!("/streamgroups/{id}"), None).await;
        assert_eq!(
            (status, got["StreamClass"].as_str()),
            (200, Some(stream_class)),
            "case {stream_class:?}: expected GetStreamGroup to report the stored class, got \
             {status} {got}"
        );
    }

    // The values the MODEL declares but the service rejects, and a value in neither:
    // all three are refused the same way.
    let enumerated = recorded.join(", ");
    let refused = [
        ("declared by the model only", "gen6n_small_win2022"),
        ("declared by the model only", "gen6n_medium_win2022"),
        ("declared nowhere", "not_a_real_stream_class"),
        ("declared nowhere", "GEN4N_HIGH"),
    ];
    for (case, stream_class) in refused {
        let before = stream_group_count(&router).await;
        let (status, got) = call(
            &router,
            "POST",
            "/streamgroups",
            Some(stream_group_body("refused-class", stream_class, "")),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({stream_class}): expected 400, got {status} {got}"
        );
        assert_eq!(
            error_code(&got),
            "ValidationException",
            "case {case:?} ({stream_class}): expected ValidationException, got {got}"
        );
        assert_eq!(
            error_message(&got),
            format!(
                "1 validation error detected: Value '{stream_class}' at 'streamClass' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{enumerated}]"
            ),
            "case {case:?} ({stream_class}): unexpected refusal"
        );
        assert_eq!(
            stream_group_count(&router).await,
            before,
            "case {case:?} ({stream_class}): a refused create must allocate nothing"
        );
    }
}

/// `AssociateApplications` links applications to an `ACTIVE` stream group and
/// PERSISTS the link: both resources report it afterwards, the first application
/// becomes the group's default when it had none, a repeat changes nothing, and a
/// request naming an application that does not exist leaves the group untouched.
#[tokio::test]
async fn associating_applications_persists_the_link() {
    let router = emulator();
    let (first_app, first_arn) = ready_application(&router, "assoc-one").await;
    let (second_app, second_arn) = ready_application(&router, "assoc-two").await;
    // A group created with no `DefaultApplicationIdentifier`, so the association is
    // what gives it a default.
    let (group_id, group_arn) =
        group_with_optional_default_application(&router, "assoc-group", "").await;
    let associations = format!("/streamgroups/{group_id}/associations");

    // A request naming an application that does not exist changes nothing.
    let (status, got) = call(
        &router,
        "POST",
        &associations,
        Some(json!({"ApplicationIdentifiers": [first_app.clone(), "a-missing"]})),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"one unknown application\": expected 404, got {status} {got}"
    );
    let (_, group) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        group["AssociatedApplications"],
        json!([]),
        "case \"one unknown application\": the group must be unchanged, got {group}"
    );

    // Both forms of identifier in one call: a bare id and an ARN.
    let (status, associated) = call(
        &router,
        "POST",
        &associations,
        Some(json!({"ApplicationIdentifiers": [first_app.clone(), second_arn.clone()]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associate two applications\": expected 200, got {status} {associated}"
    );
    assert_eq!(
        associated["Arn"].as_str(),
        Some(group_arn.as_str()),
        "case \"associate two applications\": expected the group ARN, got {associated}"
    );
    assert_eq!(
        associated["ApplicationArns"],
        json!([first_arn, second_arn]),
        "case \"associate two applications\": unexpected ApplicationArns, got {associated}"
    );

    // Round trip: the stream group reports the link, and so does each application.
    let (_, group) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        group["AssociatedApplications"],
        json!([first_arn, second_arn]),
        "case \"GetStreamGroup after associating\": got {group}"
    );
    assert_eq!(
        (
            group["DefaultApplication"]["Id"].as_str(),
            group["DefaultApplication"]["Arn"].as_str()
        ),
        (Some(first_app.as_str()), Some(first_arn.as_str())),
        "case \"the first application becomes the default\": got {group}"
    );
    for (case, id, arn) in [
        ("first", &first_app, &first_arn),
        ("second", &second_app, &second_arn),
    ] {
        let (_, application) = call(&router, "GET", &format!("/applications/{id}"), None).await;
        assert_eq!(
            application["AssociatedStreamGroups"],
            json!([group_arn]),
            "case {case:?}: expected {arn} to report the group, got {application}"
        );
        assert_eq!(
            application["ReplicationStatuses"],
            json!([{"Location": REGION, "Status": "REPLICATING"}]),
            "case {case:?}: expected a replication row for the group's location, got \
             {application}"
        );
    }
    std::thread::sleep(PAST_TRANSITION);
    let (_, application) = call(&router, "GET", &format!("/applications/{first_app}"), None).await;
    assert_eq!(
        application["ReplicationStatuses"],
        json!([{"Location": REGION, "Status": "COMPLETED"}]),
        "case \"replication completes on the clock\": got {application}"
    );

    // @idempotent: a repeat neither duplicates the link nor moves the default.
    let (status, again) = call(
        &router,
        "POST",
        &associations,
        Some(json!({"ApplicationIdentifiers": [second_arn.clone(), first_app.clone()]})),
    )
    .await;
    assert_eq!(
        (status, &again["ApplicationArns"]),
        (200, &json!([first_arn, second_arn])),
        "case \"repeat association\": got {status} {again}"
    );
    let (_, application) = call(&router, "GET", &format!("/applications/{first_app}"), None).await;
    assert_eq!(
        application["AssociatedStreamGroups"],
        json!([group_arn]),
        "case \"repeat association\": the application must hold one link, got {application}"
    );

    // A group that is not ACTIVE yet refuses the association, with the client-input
    // error this operation models (it models no conflict).
    let (status, created) = call(
        &router,
        "POST",
        "/streamgroups",
        Some(stream_group_body("activating-group", "gen5n_high", "")),
    )
    .await;
    assert_eq!(status, 201, "creating an ACTIVATING group: got {created}");
    let activating = text(&created, "/Id", "activating group");
    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{activating}/associations"),
        Some(json!({"ApplicationIdentifiers": [first_app]})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"ACTIVATING stream group\": expected 400, got {status} {got}"
    );
    assert_eq!(
        error_code(&got),
        "ValidationException",
        "case \"ACTIVATING stream group\": expected ValidationException, got {got}"
    );
}

/// `CreateStreamSessionConnection` resolves its stream group and stream session from
/// state: an identifier that names nothing is a 404, and a session in a status a
/// reconnection cannot start from is the modelled 409 — never a 501 and never a
/// synthesised `SignalResponse`.
#[tokio::test]
async fn a_stream_session_connection_resolves_its_identifiers() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "reconnect-app").await;
    let (group_id, _) =
        group_with_optional_default_application(&router, "reconnect-group", &app_id).await;
    let signal = json!({"SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}"});

    // An unknown stream group.
    let (status, got) = call(
        &router,
        "POST",
        "/streamgroups/sg-bogus/streamsessions/ssdoesnotexist0/connections",
        Some(signal.clone()),
    )
    .await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"unknown stream group\": got {status} {got}"
    );

    // A session this group does not hold.
    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions/ssdoesnotexist0/connections"),
        Some(signal.clone()),
    )
    .await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"unknown stream session\": got {status} {got}"
    );

    // A session that exists but is in a status a reconnection cannot start from, in
    // both of the statuses this emulator can hold it in.
    let (status, session) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}",
            "ApplicationIdentifier": app_arn,
        })),
    )
    .await;
    assert_eq!(status, 201, "StartStreamSession: got {status} {session}");
    // `StartStreamSessionOutput` carries no `Id`: the session id is the last segment
    // of its ARN.
    let session_arn = text(&session, "/Arn", "started session");
    let session_id = session_arn
        .rsplit('/')
        .next()
        .expect("an ARN has a last segment")
        .to_string();
    let connections =
        format!("/streamgroups/{group_id}/streamsessions/{session_id}/connections");
    for case in ["ACTIVATING", "ERROR"] {
        let (status, got) = call(&router, "POST", &connections, Some(signal.clone())).await;
        assert_eq!(
            (status, error_code(&got).as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected the modelled conflict, got {status} {got}"
        );
        assert!(
            got.get("SignalResponse").is_none(),
            "case {case:?}: no stream server ran, so nothing may answer a SignalResponse: {got}"
        );
        // Let the clock take the session from ACTIVATING to its terminal status, and
        // ask again.
        std::thread::sleep(PAST_TRANSITION);
    }
}

/// A `ResourceArn` that is not an ARN at all is refused with `BadRequestException`
/// (400) — the code this API answers a malformed resource ARN with, on every
/// operation that binds one — while a well-formed ARN gets past the check.
#[tokio::test]
async fn a_malformed_resource_arn_is_a_bad_request() {
    let router = emulator();
    let malformed = [
        ("not an ARN at all", "not-an-arn"),
        ("another service", "arn:aws:s3:::my-bucket"),
        (
            "a non-numeric account",
            "arn:aws:gameliftstreams:us-west-2:not-an-account:application%2Fa-1",
        ),
        (
            "a partition that is not aws",
            "arn:aws-cn:gameliftstreams:us-west-2:000000000000:application%2Fa-1",
        ),
    ];
    // Every operation that binds `ResourceArn` as its label.
    let operations = [
        ("TagResource", "POST", "", Some(json!({"Tags": {"k": "v"}}))),
        ("UntagResource", "DELETE", "?tagKeys=k", None),
        ("ListTagsForResource", "GET", "", None),
    ];
    for (case, arn) in malformed {
        for (operation, method, query, body) in &operations {
            let (status, got) =
                call(&router, method, &format!("/tags/{arn}{query}"), body.clone()).await;
            assert_eq!(
                status, 400,
                "case {operation} {case:?} ({arn}): expected 400, got {status} {got}"
            );
            assert_eq!(
                error_code(&got),
                "BadRequestException",
                "case {operation} {case:?} ({arn}): expected BadRequestException, got {got}"
            );
        }
    }

    // And the precedence the override states: a request malformed in the ARN AND
    // in another member is still answered as a bad resource ARN (`Tags` is
    // modelled `length 1..50`, so an empty map is a second violation).
    let (status, got) = call(
        &router,
        "POST",
        "/tags/not-an-arn",
        Some(json!({"Tags": {}})),
    )
    .await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (400, "BadRequestException"),
        "case \"a malformed ARN and a second bad member\": expected the ARN refusal to take \
         precedence, got {status} {got}"
    );

    // A well-formed ARN is not rewritten: it gets past the syntax check and is
    // served by the operation, which stores the tags the request carries.
    let (app_id, _) = ready_application(&router, "taggable").await;
    let arn = as_label(&application_arn(&app_id));
    let (status, got) = call(
        &router,
        "POST",
        &format!("/tags/{arn}"),
        Some(json!({"Tags": {"k": "v"}})),
    )
    .await;
    assert_eq!(
        status, 204,
        "case \"a well-formed ResourceArn\": expected the call to be served, got {status} {got}"
    );
    let (status, listed) = call(&router, "GET", &format!("/tags/{arn}"), None).await;
    assert_eq!(
        (status, &listed["Tags"]),
        (200, &json!({"k": "v"})),
        "case \"a well-formed ResourceArn\": expected the stored tags back, got {status} {listed}"
    );

    // And the rewording is scoped to that one label: a value outside a DIFFERENT
    // member's pattern is still the `ValidationException` the rest of the API
    // refuses a bad value with.
    let (status, got) = call(&router, "GET", "/streamgroups/not%20an%20identifier", None).await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (400, "ValidationException"),
        "case \"an Identifier outside its pattern\": expected ValidationException, got {status} \
         {got}"
    );
}

/// A `ClientToken` is scoped to ONE operation: the same token value used by two
/// different creates is two fresh resources, not a replay, and a token no call has
/// used is always fresh.
#[tokio::test]
async fn a_client_token_is_scoped_to_one_operation() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "scoped-token").await;
    let mut application = application_body("scoped-token-app", "s3://my-bucket/build/");
    application["ClientToken"] = json!(TOKEN_B);
    let mut group = stream_group_body("scoped-token-group", "gen4n_high", &app_id);
    group["ClientToken"] = json!(TOKEN_B);
    let (application_status, _) = call(&router, "POST", "/applications", Some(application)).await;
    let (group_status, group_body) = call(&router, "POST", "/streamgroups", Some(group)).await;
    assert_eq!(
        (application_status, group_status),
        (201, 201),
        "one token value used by two different creates must not collide: {group_body}"
    );
    // And a third create with a token no call has used is fresh.
    let mut other = application_body("other-token-app", "s3://my-bucket/build/");
    other["ClientToken"] = json!(TOKEN_C);
    let (status, created) = call(&router, "POST", "/applications", Some(other)).await;
    assert_eq!(status, 201, "a fresh token: got {status} {created}");
}

/// A record this service STORES is found again by the store's own id, never by
/// re-reading the ARN it was given: a request whose credential scope names a region
/// the emulator would never choose itself still associates, reports and reads back.
///
/// The rows are credential-scope regions of three shapes — an ordinary one, one
/// carrying the ARN field separator, and one that is empty — because the region
/// segment of a minted ARN comes verbatim from the caller's credential scope, and an
/// ARN whose region carries a `:` cannot be split back into its fields.
#[tokio::test]
async fn a_stored_record_is_reached_by_id_whatever_its_arn_looks_like() {
    for scope_region in ["eu-central-1", "a:b:c", ""] {
        let router = emulator();
        let case = if scope_region.is_empty() {
            "an empty credential-scope region"
        } else {
            scope_region
        };

        let (status, created) = call_in_scope_region(
            &router,
            "POST",
            "/applications",
            Some(application_body("odd-scope", "s3://my-bucket/build/")),
            scope_region,
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateApplication 201, got {status} {created}"
        );
        let application_id = text(&created, "/Id", case);
        let application_arn = text(&created, "/Arn", case);
        std::thread::sleep(PAST_TRANSITION);

        let (status, group) = call_in_scope_region(
            &router,
            "POST",
            "/streamgroups",
            Some(stream_group_body("odd-scope-group", "gen5n_high", "")),
            scope_region,
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateStreamGroup 201, got {status} {group}"
        );
        let group_id = text(&group, "/Id", case);
        std::thread::sleep(PAST_TRANSITION);

        // The association is answered from state, by id: no panic, no 5xx.
        let (status, associated) = call_in_scope_region(
            &router,
            "POST",
            &format!("/streamgroups/{group_id}/associations"),
            Some(json!({"ApplicationIdentifiers": [application_id.clone()]})),
            scope_region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AssociateApplications 200, got {status} {associated}"
        );
        assert_eq!(
            associated["ApplicationArns"],
            json!([application_arn.clone()]),
            "case {case:?}: expected the stored application ARN back, got {associated}"
        );

        // And both resources report the link, including the default application the
        // association assigned — whose Id comes from the stored record.
        let (status, read) = call_in_scope_region(
            &router,
            "GET",
            &format!("/streamgroups/{group_id}"),
            None,
            scope_region,
        )
        .await;
        assert_eq!(
            (status, &read["AssociatedApplications"]),
            (200, &json!([application_arn.clone()])),
            "case {case:?}: expected GetStreamGroup to report the link, got {status} {read}"
        );
        assert_eq!(
            read["DefaultApplication"]["Id"],
            json!(application_id),
            "case {case:?}: expected the stored application id as the default, got {read}"
        );
    }
}
