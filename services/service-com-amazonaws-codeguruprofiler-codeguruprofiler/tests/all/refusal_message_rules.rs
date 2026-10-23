// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, end to end over HTTP through the router it is mounted
//! in: the message of a constraint violation, of a not-found and of a conflict.
//!
//! Two rules are asserted here, each over a table that varies what the rule generalises
//! over (which member, which constraint class, the value's length, the element's position
//! in its list, the name, the operation) rather than over the one recorded request:
//!
//! 1. A CONSTRAINT refusal is built from the request: `<N> validation error(s) detected: `
//!    over one clause per failing (member, requirement), each naming the value as
//!    SUBMITTED, the member's dotted path and the requirement — with a `@pattern` printed
//!    without the whole-value anchors a model writes, an enum's value set as the service's
//!    own enum declares it, and a failure on a list's ELEMENTS stated at the LIST member
//!    with the element requirement nested as `Member must satisfy constraint: [...]`.
//! 2. A resource refusal names the resource the way the service names it: the profiling
//!    group a non-agent operation cannot find, the account-qualified form the AGENT plane
//!    uses for the same fact, the name AND account a duplicate create collided with, and
//!    `GetProfile`'s FIXED text for a window no profile covers.
//!
//! The status and the error code are asserted in every case too, so a message can never be
//! right on its own.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every account-qualified message carries.
const ACCOUNT: &str = "000000000000";

/// An SNS topic ARN, the one channel URI the model documents
/// (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#ChannelUri")`: "We
/// support a valid SNS topic arn as a channel uri").
const TOPIC_ARN: &str = "arn:aws:sns:us-east-1:000000000000:cgp-anomalies";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The error message, which restJson1 carries in `message` (`Message` on the services
    /// that model it capitalised).
    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind one
/// mounted service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codeguruprofiler_codeguruprofiler::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Vec<u8>) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "codeguru-profiler.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn get(&self, uri: &str) -> Answer {
        self.send("GET", uri, Vec::new()).await
    }

    /// `CreateProfilingGroup` with just a name, which every case that needs an existing
    /// group starts from.
    ///
    /// The `clientToken` is a fixed valid one — `@length 1..64`, `@pattern ^[\w-]+$` — so
    /// that a case about the NAME is never also a case about the token. A token already
    /// used for a different name is a different create, not an idempotent replay, so every
    /// case here still reaches the create path.
    async fn create(&self, name: &str) -> Answer {
        self.send(
            "POST",
            "/profilingGroups?clientToken=cgp-test-token",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await
    }

    async fn created(&self, name: &str) -> &Self {
        let answer = self.create(name).await;
        assert_eq!(
            answer.status,
            201,
            "case {name:?}: the group this case needs was not created: {}",
            String::from_utf8_lossy(&answer.body)
        );
        self
    }
}

/// The envelope AWS wraps one clause in, so each table row states only its clause.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// RULE 1, the `@pattern` rendering: the expression a value had to satisfy is reported
/// WITHOUT the `^`/`$` pair the model writes to demand a whole-string match.
///
/// `profilingGroupName` is `@pattern ^[\w-]+$`
/// (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#ProfilingGroupName")`),
/// and the rows differ in HOW the name breaks it.
#[tokio::test]
async fn a_pattern_violation_reports_the_expression_without_the_models_anchors() {
    let harness = Harness::new();
    // (case, the name sent)
    let rows: [(&str, &str); 6] = [
        ("a space", "has a space"),
        ("a dot", "cgp.group"),
        ("a slash", "cgp/group"),
        ("a colon", "cgp:group"),
        ("a leading plus", "+cgp"),
        ("punctuation only", "!!!"),
    ];

    for (case, name) in rows {
        let answer = harness.create(name).await;
        let expected = one_error(&format!(
            "Value '{name}' at 'profilingGroupName' failed to satisfy constraint: Member \
             must satisfy regular expression pattern: [\\w-]+"
        ));
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}

/// RULE 1, the value echo: a clause states the value the caller submitted, whole — never a
/// shortened rendering of it, and never a byte-count marker.
///
/// `ConfigureAgent.fleetInstanceId` is `@length 1..255`
/// (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#FleetInstanceId")`),
/// so the rows walk both bounds and both sides of the framework's own 256-byte echo cap:
/// the cap is where the value used to be replaced by a prefix plus `... [<n> bytes]`.
#[tokio::test]
async fn a_length_violation_echoes_the_value_the_caller_submitted() {
    let harness = Harness::new();
    harness.created("cgp-echo").await;
    // (case, the fleetInstanceId sent, the requirement it fails)
    let rows: [(&str, String, &str); 6] = [
        (
            "empty, the minimum",
            String::new(),
            "Member must have length greater than or equal to 1",
        ),
        (
            "one past the maximum",
            format!("i-{}", "0".repeat(254)),
            "Member must have length less than or equal to 255",
        ),
        (
            "exactly at the framework's echo cap",
            "x".repeat(256),
            "Member must have length less than or equal to 255",
        ),
        (
            "one past the framework's echo cap",
            "x".repeat(257),
            "Member must have length less than or equal to 255",
        ),
        (
            "the recorded 302-character instance",
            format!("i-{}", "0".repeat(300)),
            "Member must have length less than or equal to 255",
        ),
        (
            "a kilobyte",
            "y".repeat(1024),
            "Member must have length less than or equal to 255",
        ),
    ];

    for (case, value, requirement) in rows {
        let answer = harness
            .send(
                "POST",
                "/profilingGroups/cgp-echo/configureAgent",
                serde_json::to_vec(&serde_json::json!({ "fleetInstanceId": value }))
                    .expect("a serializable body"),
            )
            .await;
        let expected = one_error(&format!(
            "Value '{value}' at 'fleetInstanceId' failed to satisfy constraint: {requirement}"
        ));
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains(" bytes]"),
            "case {case:?}: the message carries a byte-count marker no service emits: {:?}",
            answer.message()
        );
    }
}

/// RULE 1, the list aggregation: a constraint broken by a list's ELEMENTS is reported at
/// the LIST member — the list itself as the value, the element's requirement nested.
///
/// `Channel.eventPublishers` is a list of the `EventPublisher` enum
/// (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#Channel")`), reached
/// through `channels`, itself a list — so the clause's path keeps the enclosing list's own
/// element step. The rows differ in the bad value, in the position of the channel that
/// carries it, and in how many clauses one request produces.
#[tokio::test]
async fn an_element_failure_is_reported_at_its_list_with_the_element_constraint_nested() {
    let harness = Harness::new();
    harness.created("cgp-channels").await;
    let channel = |publisher: &str| {
        serde_json::json!({ "uri": TOPIC_ARN, "eventPublishers": [publisher] })
    };
    let nested = "Member must satisfy constraint: [Member must satisfy enum value set: \
                  [AnomalyDetection]]";
    // (case, the channels sent, the message expected)
    let rows: [(&str, serde_json::Value, String); 4] = [
        (
            "the recorded instance",
            serde_json::json!([channel("NotARealPublisher")]),
            one_error(&format!(
                "Value '[NotARealPublisher]' at 'channels.1.member.eventPublishers' failed \
                 to satisfy constraint: {nested}"
            )),
        ),
        (
            "a value differing only in case",
            serde_json::json!([channel("anomalydetection")]),
            one_error(&format!(
                "Value '[anomalydetection]' at 'channels.1.member.eventPublishers' failed \
                 to satisfy constraint: {nested}"
            )),
        ),
        (
            "the second channel of two",
            serde_json::json!([channel("AnomalyDetection"), channel("Nope")]),
            one_error(&format!(
                "Value '[Nope]' at 'channels.2.member.eventPublishers' failed to satisfy \
                 constraint: {nested}"
            )),
        ),
        (
            "both channels of two",
            serde_json::json!([channel("Nope"), channel("AlsoNope")]),
            format!(
                "2 validation errors detected: Value '[Nope]' at \
                 'channels.1.member.eventPublishers' failed to satisfy constraint: \
                 {nested}; Value '[AlsoNope]' at 'channels.2.member.eventPublishers' \
                 failed to satisfy constraint: {nested}"
            ),
        ),
    ];

    for (case, channels, expected) in rows {
        let answer = harness
            .send(
                "POST",
                "/profilingGroups/cgp-channels/notificationConfiguration",
                serde_json::to_vec(&serde_json::json!({ "channels": channels }))
                    .expect("a serializable body"),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}

/// RULE 1, the value set: an enum violation lists the values the SERVICE accepts, in the
/// order the service's own enum declares them.
///
/// The rows cover every enum an input member of this service targets that a request can
/// reach: the two the recordings pin (`AggregationPeriod` in an order the public model does
/// not list, `ActionGroup` with a value the public model omits) and two the model's own
/// declaration order stands for.
#[tokio::test]
async fn an_enum_violation_lists_the_value_set_the_service_declares() {
    let harness = Harness::new();
    harness.created("cgp-enums").await;
    let window = "startTime=2026-01-01T00:00:00Z&endTime=2026-01-02T00:00:00Z";
    // (case, method, uri, body, the clause expected)
    let rows: [(&str, &str, String, serde_json::Value, &str); 4] = [
        (
            "AggregationPeriod, on ListProfileTimes.period",
            "GET",
            format!("/profilingGroups/cgp-enums/profileTimes?{window}&period=P2D"),
            serde_json::Value::Null,
            "Value 'P2D' at 'period' failed to satisfy constraint: Member must satisfy \
             enum value set: [P1D, PT1H, PT5M]",
        ),
        (
            "ActionGroup, on PutPermission.actionGroup",
            "PUT",
            "/profilingGroups/cgp-enums/policy/bogusPermissions".to_string(),
            serde_json::json!({ "principals": ["arn:aws:iam::000000000000:root"] }),
            "Value 'bogusPermissions' at 'actionGroup' failed to satisfy constraint: \
             Member must satisfy enum value set: [internalAgentPermissions, \
             agentPermissions]",
        ),
        (
            "ComputePlatform, on CreateProfilingGroup.computePlatform",
            "POST",
            "/profilingGroups?clientToken=token-enum".to_string(),
            serde_json::json!({
                "profilingGroupName": "cgp-enum-platform",
                "computePlatform": "NotAPlatform",
            }),
            "Value 'NotAPlatform' at 'computePlatform' failed to satisfy constraint: \
             Member must satisfy enum value set: [Default, AWSLambda]",
        ),
        (
            "OrderBy, on ListProfileTimes.orderBy",
            "GET",
            format!(
                "/profilingGroups/cgp-enums/profileTimes?{window}&period=PT5M&orderBy=Sideways"
            ),
            serde_json::Value::Null,
            "Value 'Sideways' at 'orderBy' failed to satisfy constraint: Member must \
             satisfy enum value set: [TimestampDescending, TimestampAscending]",
        ),
    ];

    for (case, method, uri, body, clause) in rows {
        let body = match body {
            serde_json::Value::Null => Vec::new(),
            body => serde_json::to_vec(&body).expect("a serializable body"),
        };
        let answer = harness.send(method, &uri, body).await;
        let expected = one_error(clause);
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}

/// RULE 2: a profiling group no operation can find is refused 404 with the service's own
/// text, naming the group the request asked for.
///
/// The rows cross the operations that read or delete a group with names that differ in
/// length and in which of the name alphabet's characters they use.
#[tokio::test]
async fn a_group_that_does_not_exist_is_refused_with_the_services_not_found_text() {
    let harness = Harness::new();
    // The operations that look a group up on behalf of a caller that is not the agent
    // plane, each as (method, uri suffix). `DeleteProfilingGroup` is not among them: it
    // acts on the group RECORD and states the record-level form, which
    // `refusal_message_formats.rs` pins.
    let readers: [(&str, &str); 2] = [("GET", ""), ("GET", "/profile?period=PT5M")];
    // (case, the name asked for)
    let names: [(&str, String); 4] = [
        ("one character", "a".to_string()),
        (
            "the recorded instance",
            "sim2real-cgp-nosuch-4e95407142d5".to_string(),
        ),
        (
            "every character of the name alphabet",
            "Group_9-name".to_string(),
        ),
        ("the longest name the model allows", "n".repeat(255)),
    ];

    for (case, name) in names {
        for (method, suffix) in readers {
            let answer = harness
                .send(method, &format!("/profilingGroups/{name}{suffix}"), Vec::new())
                .await;
            let expected = format!(
                "CodeGuru Profiler can\u{2019}t complete the operation because {name} was \
                 not found."
            );
            assert_eq!(
                answer.status, 404,
                "case {case:?} on {method} {suffix:?}: expected 404, got {} ({})",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?} on {method} {suffix:?}: expected ResourceNotFoundException, \
                 got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?} on {method} {suffix:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// RULE 2: the AGENT plane states the same fact in its own account-qualified form.
///
/// The agent APIs are `ConfigureAgent` and `PostAgentProfile` — the two the model's own
/// `ActionGroup` enum names ("Permission group type for Agent APIs - ConfigureAgent,
/// PostAgentProfile") — so both rows of the operation axis must answer the agent text and
/// never the text the readers above get.
#[tokio::test]
async fn the_agent_plane_states_its_own_account_qualified_not_found_text() {
    let harness = Harness::new();
    let agent_calls: [(&str, &str, &[u8]); 2] = [
        ("ConfigureAgent", "/configureAgent", b"{}"),
        ("PostAgentProfile", "/agentProfile", b"{\"ion\": true}"),
    ];
    // (case, the name asked for)
    let names: [(&str, String); 3] = [
        ("one character", "z".to_string()),
        (
            "the recorded instance",
            "sim2real-cgp-noagent-962c43b82c99".to_string(),
        ),
        ("the longest name the model allows", "n".repeat(255)),
    ];

    for (case, name) in names {
        for (operation, suffix, body) in agent_calls {
            let answer = harness
                .send(
                    "POST",
                    &format!("/profilingGroups/{name}{suffix}"),
                    body.to_vec(),
                )
                .await;
            let expected = format!("Profiling group for {ACCOUNT}/{name} does not exist!");
            assert_eq!(
                answer.status, 404,
                "case {case:?} on {operation}: expected 404, got {} ({})",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// RULE 2: a create that collides with a stored group is refused 409 naming the group AND
/// the account that holds it.
///
/// Each row creates the group first and reads it back through `DescribeProfilingGroup`, so
/// the conflict is a real collision with stored state rather than a message produced by
/// nothing.
#[tokio::test]
async fn a_duplicate_create_names_the_group_and_the_account_that_hold_it() {
    let harness = Harness::new();
    // (case, the name created twice)
    let rows: [(&str, String); 3] = [
        ("one character", "q".to_string()),
        (
            "the recorded instance",
            "sim2real-cgp-4e544744e337".to_string(),
        ),
        ("the longest name the model allows", "d".repeat(255)),
    ];

    for (case, name) in rows {
        harness.created(&name).await;
        let described = harness.get(&format!("/profilingGroups/{name}")).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the created group must read back, got {} ({})",
            described.status,
            String::from_utf8_lossy(&described.body)
        );

        // A DIFFERENT client token, so this is a second create rather than an idempotent
        // replay of the first.
        let answer = harness
            .send(
                "POST",
                "/profilingGroups?clientToken=second-token",
                serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                    .expect("a serializable body"),
            )
            .await;
        let expected = format!("ProfilingGroup (name: {name}, accountId: {ACCOUNT}) already exists.");
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}

/// RULE 2: `GetProfile` answers a window no profile covers with its FIXED text — the same
/// string whatever the request asked for, and whatever the group holds.
///
/// The rows differ in why the window is empty (the group has never received a profile; it
/// has one, but the window is before or after it) and in how the window was expressed, and
/// a profile posted into the window is read back to prove the empty answer is computed from
/// what is stored rather than canned.
#[tokio::test]
async fn get_profile_states_its_fixed_text_for_a_window_no_profile_covers() {
    let harness = Harness::new();
    harness.created("cgp-empty").await;
    harness.created("cgp-held").await;
    let posted = harness
        .send(
            "POST",
            "/profilingGroups/cgp-held/agentProfile",
            b"{\"ion\": \"payload\"}".to_vec(),
        )
        .await;
    assert_eq!(
        posted.status,
        204,
        "case the stored profile: expected 204, got {} ({})",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
    let read_back = harness
        .get("/profilingGroups/cgp-held/profile?period=PT5M")
        .await;
    assert_eq!(
        read_back.status,
        200,
        "case the stored profile: it must read back, got {} ({})",
        read_back.status,
        String::from_utf8_lossy(&read_back.body)
    );
    assert_eq!(
        read_back.body,
        b"{\"ion\": \"payload\"}".to_vec(),
        "case the stored profile: GetProfile must return the posted bytes"
    );

    // (case, the group, the window asked for)
    let rows: [(&str, &str, &str); 7] = [
        ("a group holding nothing", "cgp-empty", "?period=PT5M"),
        (
            "a group holding nothing, bounded window",
            "cgp-empty",
            "?startTime=2020-01-01T00:00:00Z&endTime=2020-01-02T00:00:00Z",
        ),
        (
            "a window entirely before the stored profile",
            "cgp-held",
            "?startTime=2020-01-01T00:00:00Z&endTime=2020-01-02T00:00:00Z",
        ),
        (
            "a window entirely after the stored profile",
            "cgp-held",
            "?startTime=2099-01-01T00:00:00Z&endTime=2099-01-02T00:00:00Z",
        ),
        (
            "a window expressed as a start plus a period",
            "cgp-held",
            "?startTime=2020-01-01T00:00:00Z&period=PT5M",
        ),
        (
            "a window at the far end of the representable range",
            "cgp-held",
            "?startTime=9999-12-31T23:59:59Z&period=P1D",
        ),
        (
            "a window at the far start of the representable range",
            "cgp-held",
            "?endTime=0001-01-01T00:00:00Z&period=P1D",
        ),
    ];

    for (case, group, window) in rows {
        let answer = harness
            .get(&format!("/profilingGroups/{group}/profile{window}"))
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            "No profiles found for the requested time range!",
            "case {case:?}: expected the fixed no-data text, got {:?}",
            answer.message()
        );
    }
}
