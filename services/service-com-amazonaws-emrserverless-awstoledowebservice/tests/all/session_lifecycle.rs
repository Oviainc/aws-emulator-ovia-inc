// SPDX-License-Identifier: CC0-1.0

//! EMR Serverless interactive sessions, end to end over HTTP.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one
//! request:
//!
//! 1. `StartSession` mints a session on the application it names, inherits from that
//!    application what the session runs on (release label, network), stores what the
//!    request gave, and `GetSession` reports that stored record rather than re-deriving
//!    anything — so every member round-trips.
//! 2. A session is not usable the instant the start returns: it is reported in the
//!    transient state the start left it in, and the clock owes it `STARTED` after the
//!    transition span, with the start instant that state implies.
//! 3. `StartSession`'s documented preconditions are judged against the stored
//!    application: the release it runs (`7.13.0` and later), interactive sessions being
//!    enabled on it, and it being `STARTED` or starting by itself.
//! 4. `clientToken` is an idempotency token: the same token names the same session.
//! 5. A session's `idleTimeoutMinutes` terminates it when it falls due. The span the
//!    emulator serves those configured minutes as is its own, so that one rule is
//!    asserted against a synthetic clock over the stored record rather than waited for.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::ops::types::Session;
use service_com_amazonaws_emrserverless_awstoledowebservice::state::{
    idle_span, PendingState, SessionRecord, LIFECYCLE_TRANSITION,
};

use super::application_lifecycle::{
    call, create_application, delete_application, emulator, get_application,
    matches_request_identity_arn, matches_resource_id, wait_for_transition, Answer,
};

/// The region the test requests are signed for; it is the region every minted ARN
/// names.
const REGION: &str = "eu-west-1";

/// The account id every emulator ARN carries (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// An execution role ARN satisfying `IAMRoleArn`'s pattern.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/EMRServerlessSessionRole";

/// The earliest Amazon EMR release whose applications serve sessions, and the release
/// one step below it.
///
/// Written out here rather than read from `crate::state`, so a wrong bound in `src/`
/// cannot make this test pass. Source: `StartSession`'s documentation — "This operation
/// is supported for EMR release 7.13.0 and later"
/// (`smithy_query(query="doc",shape="StartSession")`).
pub(crate) const EARLIEST_SESSION_RELEASE: &str = "emr-7.13.0";
const BEFORE_EARLIEST_SESSION_RELEASE: &str = "emr-7.12.0";

/// `SessionArn`'s modelled `@pattern`
/// `^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+\/sessions\/[0-9a-zA-Z]+$`
/// with length 60..1024.
fn matches_session_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:emr-serverless:{region}:{ACCOUNT_ID}:/applications/");
    let Some(rest) = arn.strip_prefix(&prefix) else {
        return false;
    };
    let Some((application_id, session_id)) = rest.split_once("/sessions/") else {
        return false;
    };
    !application_id.is_empty()
        && application_id.chars().all(|c| c.is_ascii_alphanumeric())
        && !session_id.is_empty()
        && session_id.chars().all(|c| c.is_ascii_alphanumeric())
        && (60..=1024).contains(&arn.len())
}

/// `String1024`'s constraints on `Session$stateDetails`: length 1..1024 and the pattern
/// `.*\S.*` (at least one non-whitespace character). `Application$stateDetails` is the
/// narrower `String256`, so this is the session's own bound rather than that one.
fn is_state_details(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| (1..=1024).contains(&text.chars().count()) && text.trim() != "")
}

/// The `interactiveConfiguration` that enables sessions on an application.
///
/// Source: `InteractiveConfiguration$sessionEnabled` — "Enables interactive sessions on
/// the application. When set to `true`, you can start interactive sessions using the
/// `StartSession` operation."
pub(crate) fn sessions_enabled() -> Value {
    json!({ "sessionEnabled": true })
}

/// A `CreateApplication` body naming a release label and an interactive configuration.
fn create_application_body(token: &str, release_label: &str, interactive: Option<Value>) -> Value {
    let mut body = json!({
        "clientToken": token,
        "releaseLabel": release_label,
        "type": "Spark",
    });
    if let Some(interactive) = interactive {
        body["interactiveConfiguration"] = interactive;
    }
    body
}

/// Create an application and return `(applicationId, applicationArn)`, waiting out the
/// create transition so the application has settled on `CREATED`.
pub(crate) async fn application(
    router: &MultiServiceRouter,
    token: &str,
    release_label: &str,
    interactive: Option<Value>,
) -> (String, String) {
    let answer = create_application(
        router,
        create_application_body(token, release_label, interactive),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {token:?}: CreateApplication expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let id = answer
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arn = answer.at("/arn").as_str().unwrap_or_default().to_string();
    wait_for_transition();
    (id, arn)
}

/// An application that serves sessions, on the earliest release that does.
pub(crate) async fn session_application(
    router: &MultiServiceRouter,
    token: &str,
) -> (String, String) {
    application(
        router,
        token,
        EARLIEST_SESSION_RELEASE,
        Some(sessions_enabled()),
    )
    .await
}

pub(crate) async fn start_session(
    router: &MultiServiceRouter,
    application_id: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/applications/{application_id}/sessions"),
        Some(body),
    )
    .await
}

pub(crate) async fn get_session(
    router: &MultiServiceRouter,
    application_id: &str,
    session_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/applications/{application_id}/sessions/{session_id}"),
        None,
    )
    .await
}

/// A `StartSession` body carrying only the two required members beyond the token.
pub(crate) fn minimal_session(token: &str) -> Value {
    json!({
        "clientToken": token,
        "executionRoleArn": EXECUTION_ROLE,
    })
}

/// One row of the round-trip table: what the start asked for.
struct SessionCase {
    case: &'static str,
    name: Option<&'static str>,
    tags: Option<Value>,
    idle_timeout_minutes: Option<i64>,
    configuration_overrides: Option<Value>,
}

/// The `StartSession` body this row asks for, with `token`.
fn body_for(case: &SessionCase, token: &str) -> Value {
    let mut body = minimal_session(token);
    if let Some(name) = case.name {
        body["name"] = json!(name);
    }
    if let Some(tags) = case.tags.clone() {
        body["tags"] = tags;
    }
    if let Some(minutes) = case.idle_timeout_minutes {
        body["idleTimeoutMinutes"] = json!(minutes);
    }
    if let Some(overrides) = case.configuration_overrides.clone() {
        body["configurationOverrides"] = overrides;
    }
    body
}

/// The rows of the round-trip table: they differ along which optional members the start
/// names, the number of tags, the idle timeout's value (including both published bounds)
/// and the shape of the configuration overrides.
///
/// Source for the bounds: `StartSessionRequest$idleTimeoutMinutes` is
/// `@range(min: 0, max: 1000000)`.
fn round_trip_cases() -> Vec<SessionCase> {
    vec![
        SessionCase {
            case: "only the required members",
            name: None,
            tags: None,
            idle_timeout_minutes: None,
            configuration_overrides: None,
        },
        SessionCase {
            case: "a named session with one tag",
            name: Some("analytics"),
            tags: Some(json!({ "team": "data" })),
            idle_timeout_minutes: Some(30),
            configuration_overrides: None,
        },
        SessionCase {
            case: "several tags and the idle timeout's lower bound",
            name: Some("two words and a digit 9"),
            tags: Some(json!({ "team": "data", "env": "test", "owner": "nobody" })),
            idle_timeout_minutes: Some(0),
            configuration_overrides: None,
        },
        SessionCase {
            case: "the idle timeout's upper bound and runtime overrides",
            name: Some("a"),
            tags: Some(json!({})),
            idle_timeout_minutes: Some(1_000_000),
            configuration_overrides: Some(json!({
                "runtimeConfiguration": [{
                    "classification": "spark-defaults",
                    "properties": { "spark.executor.cores": "2" },
                }],
            })),
        },
        SessionCase {
            case: "a name at the modelled maximum length",
            name: Some(
                "0123456789012345678901234567890123456789012345678901234567890123\
                 0123456789012345678901234567890123456789012345678901234567890123\
                 0123456789012345678901234567890123456789012345678901234567890123\
                 0123456789012345678901234567890123456789012345678901234567890123",
            ),
            tags: None,
            idle_timeout_minutes: Some(1),
            configuration_overrides: Some(json!({ "runtimeConfiguration": [] })),
        },
    ]
}

/// Rule 1 and rule 2: every member the start gave (or the application it was asked on
/// carries) round-trips through `GetSession`, and the session is reported in the
/// transient state the start left it in until the clock owes it `STARTED`.
#[tokio::test]
async fn start_session_persists_a_session_every_read_reports() {
    let router = emulator();
    let (application_id, application_arn) = session_application(&router, "tok-sessions").await;

    for (index, case) in round_trip_cases().iter().enumerate() {
        let label = case.case;
        let token = format!("tok-session-{index}");
        let started = start_session(&router, &application_id, body_for(case, &token)).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: StartSession expected 200, got {} with {:?}",
            started.status, started.body
        );

        let session_id = started
            .at("/sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            matches_resource_id(&session_id),
            "case {label:?}: sessionId must match SessionId's ^[0-9a-z]+$, got {session_id:?}"
        );
        let arn = started.at("/arn").as_str().unwrap_or_default().to_string();
        assert!(
            matches_session_arn(&arn, REGION),
            "case {label:?}: arn must match SessionArn's pattern, got {arn:?}"
        );
        assert_eq!(
            arn,
            format!("{application_arn}/sessions/{session_id}"),
            "case {label:?}: the session ARN is its application's ARN plus the session id, \
             got {arn:?}"
        );
        assert_eq!(
            started.at("/applicationId").as_str(),
            Some(application_id.as_str()),
            "case {label:?}: StartSession reports the application it started on, got {:?}",
            started.at("/applicationId")
        );

        // The start has returned; the session is reported in the state it left behind.
        let read = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetSession expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.at("/session/state").as_str(),
            Some("SUBMITTED"),
            "case {label:?}: a session the clock has not yet started is SUBMITTED, got {:?}",
            read.at("/session/state")
        );
        assert!(
            is_state_details(&read.at("/session/stateDetails")),
            "case {label:?}: stateDetails must satisfy String1024, got {:?}",
            read.at("/session/stateDetails")
        );
        assert_eq!(
            read.at("/session/startedAt"),
            Value::Null,
            "case {label:?}: a session that has not come up has no start instant, got {:?}",
            read.at("/session/startedAt")
        );
        assert_eq!(
            read.at("/session/endedAt"),
            Value::Null,
            "case {label:?}: a live session has no end instant, got {:?}",
            read.at("/session/endedAt")
        );

        // Rule 1: the stored members, whether they came from the request or from the
        // application the session runs on.
        assert_eq!(
            read.at("/session/sessionId").as_str(),
            Some(session_id.as_str()),
            "case {label:?}: GetSession reports the session that was started, got {:?}",
            read.at("/session/sessionId")
        );
        assert_eq!(
            read.at("/session/arn").as_str(),
            Some(arn.as_str()),
            "case {label:?}: GetSession reports the stored ARN, got {:?}",
            read.at("/session/arn")
        );
        assert_eq!(
            read.at("/session/executionRoleArn").as_str(),
            Some(EXECUTION_ROLE),
            "case {label:?}: GetSession reports the execution role the start named, got {:?}",
            read.at("/session/executionRoleArn")
        );
        assert_eq!(
            read.at("/session/releaseLabel").as_str(),
            Some(EARLIEST_SESSION_RELEASE),
            "case {label:?}: a session runs the release its application runs, got {:?}",
            read.at("/session/releaseLabel")
        );
        assert_eq!(
            read.at("/session/name").as_str(),
            case.name,
            "case {label:?}: GetSession reports the name the start named, got {:?}",
            read.at("/session/name")
        );
        let expected_tags = case.tags.clone().unwrap_or_else(|| json!({}));
        assert_eq!(
            read.at("/session/tags"),
            expected_tags,
            "case {label:?}: expected tags {expected_tags:?}, got {:?}",
            read.at("/session/tags")
        );
        match case.idle_timeout_minutes {
            Some(minutes) => assert_eq!(
                read.at("/session/idleTimeoutMinutes").as_i64(),
                Some(minutes),
                "case {label:?}: expected idleTimeoutMinutes {minutes}, got {:?}",
                read.at("/session/idleTimeoutMinutes")
            ),
            // The member has no documented default, so a session that named no timeout
            // has none in effect rather than one the service invented.
            None => assert_eq!(
                read.at("/session/idleTimeoutMinutes").as_i64().unwrap_or(0),
                0,
                "case {label:?}: a session that named no idle timeout has none, got {:?}",
                read.at("/session/idleTimeoutMinutes")
            ),
        }
        if let Some(overrides) = case.configuration_overrides.clone() {
            assert_eq!(
                read.at("/session/configurationOverrides"),
                overrides,
                "case {label:?}: expected configurationOverrides {overrides:?}, got {:?}",
                read.at("/session/configurationOverrides")
            );
        }
        assert!(
            read.at("/session/createdBy")
                .as_str()
                .is_some_and(matches_request_identity_arn),
            "case {label:?}: createdBy must satisfy RequestIdentityUserArn, got {:?}",
            read.at("/session/createdBy")
        );
        let created_at = read.at("/session/createdAt").as_f64().unwrap_or_default();
        assert!(
            created_at > 0.0,
            "case {label:?}: createdAt must be the instant the session was created, got {:?}",
            read.at("/session/createdAt")
        );
        assert_eq!(
            read.at("/session/updatedAt").as_f64(),
            Some(created_at),
            "case {label:?}: a session nothing has changed yet was updated when created, \
             got {:?}",
            read.at("/session/updatedAt")
        );

        // A read changes nothing a later caller sees.
        let again = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: two reads of the same unchanged session must agree, got {:?} \
             then {:?}",
            read.body, again.body
        );

        // Rule 2: the clock owes the session its started state.
        wait_for_transition();
        let settled = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            settled.at("/session/state").as_str(),
            Some("STARTED"),
            "case {label:?}: the clock owes a submitted session STARTED, got {:?}",
            settled.at("/session/state")
        );
        let started_at = settled
            .at("/session/startedAt")
            .as_f64()
            .unwrap_or_default();
        assert!(
            started_at >= created_at,
            "case {label:?}: startedAt must be at or after createdAt, got {started_at} \
             against {created_at}"
        );
        assert_eq!(
            settled.at("/session/updatedAt").as_f64(),
            Some(started_at),
            "case {label:?}: the transition moved updatedAt to the instant it fell due, \
             got {:?}",
            settled.at("/session/updatedAt")
        );
        // Nothing executes in a session here, so a started session has been idle since
        // it came up.
        assert_eq!(
            settled.at("/session/idleSince").as_f64(),
            Some(started_at),
            "case {label:?}: a started session is idle from its start instant, got {:?}",
            settled.at("/session/idleSince")
        );
        assert_eq!(
            settled.at("/session/endedAt"),
            Value::Null,
            "case {label:?}: a started session has not ended, got {:?}",
            settled.at("/session/endedAt")
        );
    }
}

/// Rule 3, over the release the application runs and over its interactive
/// configuration: a start is refused unless the stored application admits sessions.
#[tokio::test]
async fn start_session_judges_the_stored_application_against_its_preconditions() {
    let router = emulator();
    // (case, releaseLabel, interactiveConfiguration, expected status, expected code).
    // The rows differ along the release (both published bounds and one past the lower
    // one) and along which interactive use cases the application enabled.
    let cases: Vec<(&str, &str, Option<Value>, u16, &str)> = vec![
        (
            "the earliest release that serves sessions",
            EARLIEST_SESSION_RELEASE,
            Some(sessions_enabled()),
            200,
            "",
        ),
        (
            "a later patch release",
            "emr-7.13.1",
            Some(sessions_enabled()),
            200,
            "",
        ),
        (
            "a later major release",
            "emr-8.0.0",
            Some(sessions_enabled()),
            200,
            "",
        ),
        (
            "one release before the earliest that serves sessions",
            BEFORE_EARLIEST_SESSION_RELEASE,
            Some(sessions_enabled()),
            400,
            "ValidationException",
        ),
        (
            "a much earlier release",
            "emr-6.9.0",
            Some(sessions_enabled()),
            400,
            "ValidationException",
        ),
        (
            "no interactive configuration at all",
            EARLIEST_SESSION_RELEASE,
            None,
            400,
            "ValidationException",
        ),
        (
            "sessions explicitly disabled",
            EARLIEST_SESSION_RELEASE,
            Some(json!({ "sessionEnabled": false })),
            400,
            "ValidationException",
        ),
        (
            "another interactive use case enabled, but not sessions",
            EARLIEST_SESSION_RELEASE,
            Some(json!({ "livyEndpointEnabled": true, "studioEnabled": true })),
            400,
            "ValidationException",
        ),
    ];

    for (index, (case, release_label, interactive, status, code)) in cases.iter().enumerate() {
        let (application_id, _) = application(
            &router,
            &format!("tok-app-{index}"),
            release_label,
            interactive.clone(),
        )
        .await;
        let answer = start_session(
            &router,
            &application_id,
            minimal_session(&format!("tok-start-{index}")),
        )
        .await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: StartSession expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {:?} with {:?}",
            answer.error_code(),
            answer.body
        );
    }
}

/// Rule 3, over the application's lifecycle state, and the not-found refusals that
/// precede every other judgement.
#[tokio::test]
async fn start_session_judges_the_application_lifecycle_and_refuses_what_does_not_exist() {
    let router = emulator();

    // An application that starts by itself is started by the call rather than refusing
    // it: `AutoStartConfig$enabled` defaults to true, so a CREATED application admits a
    // session.
    let (auto_start, _) = session_application(&router, "tok-auto-start").await;
    let created_state = get_application(&router, &auto_start)
        .await
        .at("/application/state")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        created_state, "CREATED",
        "case auto-start: the application must be CREATED before the session, got \
         {created_state:?}"
    );
    let started = start_session(&router, &auto_start, minimal_session("tok-s-auto")).await;
    assert_eq!(
        started.status, 200,
        "case auto-start: StartSession expected 200, got {} with {:?}",
        started.status, started.body
    );
    let application_state = get_application(&router, &auto_start)
        .await
        .at("/application/state")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        application_state, "STARTING",
        "case auto-start: the session started the application, got {application_state:?}"
    );

    // An application that does NOT start by itself, and is not STARTED, is refused.
    let manual = create_application(
        &router,
        json!({
            "clientToken": "tok-manual",
            "releaseLabel": EARLIEST_SESSION_RELEASE,
            "type": "Spark",
            "interactiveConfiguration": sessions_enabled(),
            "autoStartConfiguration": { "enabled": false },
        }),
    )
    .await;
    assert_eq!(
        manual.status, 200,
        "case manual-start: CreateApplication expected 200, got {} with {:?}",
        manual.status, manual.body
    );
    let manual_id = manual
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_for_transition();
    let refused = start_session(&router, &manual_id, minimal_session("tok-s-manual")).await;
    assert_eq!(
        refused.status, 400,
        "case manual-start: StartSession expected 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case manual-start: expected ValidationException, got {:?} with {:?}",
        refused.error_code(),
        refused.body
    );

    // A terminated application runs nothing, whatever it would otherwise admit.
    let (terminated, _) = session_application(&router, "tok-terminated").await;
    let deleted = delete_application(&router, &terminated).await;
    assert_eq!(
        deleted.status, 200,
        "case terminated: DeleteApplication expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    let on_terminated =
        start_session(&router, &terminated, minimal_session("tok-s-terminated")).await;
    assert_eq!(
        on_terminated.status, 400,
        "case terminated: StartSession expected 400, got {} with {:?}",
        on_terminated.status, on_terminated.body
    );

    // Nothing is minted for, and nothing is reported about, a resource that does not
    // exist.
    let (known, _) = session_application(&router, "tok-known").await;
    let unknown_cases: Vec<(&str, Answer)> = vec![
        (
            "StartSession on an unknown application",
            start_session(&router, "nosuchapplication", minimal_session("tok-s-unknown")).await,
        ),
        (
            "GetSession on an unknown application",
            get_session(&router, "nosuchapplication", "nosuchsession").await,
        ),
        (
            "GetSession on an unknown session",
            get_session(&router, &known, "nosuchsession").await,
        ),
    ];
    for (case, answer) in unknown_cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} with {:?}",
            answer.error_code(),
            answer.body
        );
    }
}

/// Rule 4: `clientToken` is `@idempotencyToken` on an `@idempotent` operation, so the
/// same token names the same session however often it is retried, and a different token
/// names a new one.
#[tokio::test]
async fn start_session_is_idempotent_in_its_client_token() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-idempotent-app").await;

    // The rows differ along how often one token is retried and along the body it is
    // retried with: a retry answers with the first session either way.
    let first = start_session(&router, &application_id, minimal_session("tok-same")).await;
    assert_eq!(
        first.status, 200,
        "case first start: expected 200, got {} with {:?}",
        first.status, first.body
    );
    let session_id = first
        .at("/sessionId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let retries: Vec<(&str, Value)> = vec![
        ("the identical request", minimal_session("tok-same")),
        (
            "the same token naming other members",
            json!({
                "clientToken": "tok-same",
                "executionRoleArn": EXECUTION_ROLE,
                "name": "a different name",
                "idleTimeoutMinutes": 7,
            }),
        ),
    ];
    for (case, body) in retries {
        let retried = start_session(&router, &application_id, body).await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            retried.status, retried.body
        );
        assert_eq!(
            retried.at("/sessionId").as_str(),
            Some(session_id.as_str()),
            "case {case:?}: a retried start names the first session, got {:?}",
            retried.at("/sessionId")
        );
        assert_eq!(
            retried.at("/arn").as_str(),
            first.at("/arn").as_str(),
            "case {case:?}: a retried start names the first session's ARN, got {:?}",
            retried.at("/arn")
        );
    }

    let other = start_session(&router, &application_id, minimal_session("tok-other")).await;
    assert_eq!(
        other.status, 200,
        "case a new token: expected 200, got {} with {:?}",
        other.status, other.body
    );
    let other_id = other
        .at("/sessionId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        other_id, session_id,
        "case a new token: a new client token starts a new session, got {other_id:?} twice"
    );

    // Both sessions are readable, and each reports its own id.
    for (case, id) in [("the first session", &session_id), ("the second", &other_id)] {
        let read = get_session(&router, &application_id, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetSession expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.at("/session/sessionId").as_str(),
            Some(id.as_str()),
            "case {case:?}: GetSession reports the session asked for, got {:?}",
            read.at("/session/sessionId")
        );
    }
}

/// A session as `StartSession` stores one: `SUBMITTED`, with `STARTED` owed after the
/// transition span and `idleTimeoutMinutes` as the start asked for it.
fn submitted_record(idle_timeout_minutes: Option<i64>, now: SystemTime) -> SessionRecord {
    let created_at = aws_smithy_types::DateTime::from(now);
    SessionRecord {
        session: Session {
            application_id: "0123456789abcdef".to_string(),
            session_id: "00000000000000ff".to_string(),
            arn: format!(
                "arn:aws:emr-serverless:{REGION}:{ACCOUNT_ID}:/applications/\
                 0123456789abcdef/sessions/00000000000000ff"
            ),
            name: None,
            state: "SUBMITTED".to_string(),
            state_details: "Session submitted.".to_string(),
            release_label: EARLIEST_SESSION_RELEASE.to_string(),
            execution_role_arn: EXECUTION_ROLE.to_string(),
            created_by: "arn:aws:iam:::user/tester".to_string(),
            created_at,
            updated_at: created_at,
            started_at: None,
            ended_at: None,
            idle_since: None,
            configuration_overrides: None,
            network_configuration: None,
            idle_timeout_minutes,
            tags: Some(std::collections::HashMap::new()),
            total_resource_utilization: None,
            billed_resource_utilization: None,
            total_execution_duration_seconds: None,
        },
        pending: Some(PendingState {
            due: now + LIFECYCLE_TRANSITION,
            state: "STARTED".to_string(),
            state_details: Some("Session started.".to_string()),
        }),
    }
}

/// The whole seconds of an instant, so two timestamps compare without borrowing.
fn at_secs(value: aws_smithy_types::DateTime) -> i64 {
    value.secs()
}

/// Rule 5: the idle timeout the start asked for terminates the session when it falls
/// due, measured from the instant the session became idle, and a session that asked for
/// none is owed no termination however long nobody reads it.
///
/// The smallest timeout the model admits (`@range(min: 0, max: 1000000)`) is one minute,
/// which the emulator serves as a span of its own choosing, so the owed termination is
/// asserted against a synthetic clock over the stored record instead of being waited for;
/// the HTTP-level tests above cover the same record's `STARTED` transition.
#[test]
fn an_idle_session_is_terminated_when_its_idle_timeout_falls_due() {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // The instant the clock owes the session `STARTED`, which is also the instant it
    // becomes idle: nothing executes in a session here.
    let idle_since = created + LIFECYCLE_TRANSITION;

    // (case, idleTimeoutMinutes, how long after it became idle the reader arrives,
    // expected state). The rows differ along the timeout's value — both published
    // bounds among them — and along whether the reader arrives before or after it falls
    // due.
    let cases: Vec<(&str, Option<i64>, Duration, &str)> = vec![
        (
            "no timeout, a day later",
            None,
            Duration::from_secs(86_400),
            "STARTED",
        ),
        (
            "the range's minimum is no timeout",
            Some(0),
            Duration::from_secs(86_400),
            "STARTED",
        ),
        (
            "the smallest timeout, before it falls due",
            Some(1),
            idle_span(1) - Duration::from_secs(1),
            "STARTED",
        ),
        (
            "the smallest timeout, after it falls due",
            Some(1),
            idle_span(1) + Duration::from_secs(1),
            "TERMINATED",
        ),
        (
            "half an hour, before it falls due",
            Some(30),
            idle_span(30) - Duration::from_secs(1),
            "STARTED",
        ),
        (
            "half an hour, after it falls due",
            Some(30),
            idle_span(30) + Duration::from_secs(1),
            "TERMINATED",
        ),
        (
            "the range's maximum, a day later",
            Some(1_000_000),
            Duration::from_secs(86_400),
            "STARTED",
        ),
    ];

    for (case, minutes, elapsed, expected_state) in cases {
        let mut record = submitted_record(minutes, created);
        record.settle(idle_since + elapsed);
        let session = &record.session;
        assert_eq!(
            session.state, expected_state,
            "case {case:?}: expected state {expected_state:?}, got {:?}",
            session.state
        );
        assert_eq!(
            session.started_at.map(at_secs),
            Some(
                aws_smithy_types::DateTime::from(idle_since).secs()
            ),
            "case {case:?}: the session came up when the transition fell due, got {:?}",
            session.started_at
        );
        assert_eq!(
            session.idle_since.map(at_secs),
            session.started_at.map(at_secs),
            "case {case:?}: a session nothing executes in is idle from its start, got {:?}",
            session.idle_since
        );
        assert!(
            is_state_details(&Value::String(session.state_details.clone())),
            "case {case:?}: stateDetails must satisfy String1024, got {:?}",
            session.state_details
        );
        match (expected_state, minutes) {
            ("TERMINATED", Some(minutes)) => {
                let due = idle_since + idle_span(u64::try_from(minutes).unwrap_or(0));
                assert_eq!(
                    session.ended_at.map(at_secs),
                    Some(aws_smithy_types::DateTime::from(due).secs()),
                    "case {case:?}: the session ended when its idle timeout fell due, got \
                     {:?}",
                    session.ended_at
                );
                assert_eq!(
                    session.updated_at.secs(),
                    aws_smithy_types::DateTime::from(due).secs(),
                    "case {case:?}: updatedAt moved to the instant the termination fell \
                     due, got {:?}",
                    session.updated_at
                );
                assert!(
                    record.pending.is_none(),
                    "case {case:?}: a terminated session is owed nothing further"
                );
            }
            _ => assert!(
                session.ended_at.is_none(),
                "case {case:?}: a live session has no end instant, got {:?}",
                session.ended_at
            ),
        }
    }
}
