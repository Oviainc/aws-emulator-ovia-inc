// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, end to end over HTTP through the router it is
//! mounted in.
//!
//! An error's status and code classify it; the `message` is the only place a refusal says
//! WHICH resource or WHICH input it is about, so the rules below are about that text. Each
//! is asserted over a table whose rows differ along what the rule generalises over — the
//! submitted value, the member it sat at, the operation, the group name, the window —
//! never on one privileged recorded request:
//!
//! 1. a constraint violation is AWS's standard sentence (`N validation error(s) detected:
//!    Value '<submitted>' at '<member path>' failed to satisfy constraint: <requirement>`),
//!    and a `@pattern` requirement states the expression a value has to satisfy WITHOUT
//!    the `^` / `$` anchors the model spells the whole-string match with;
//! 2. a refusal about the profiling-group RECORD itself — the collision a create hits and
//!    the absence a delete hits — names the group together with the account it lives in:
//!    `ProfilingGroup (name: <name>, accountId: <account>) already exists.` /
//!    `… not found.`;
//! 3. every OTHER group-scoped operation states its own inability to act on a group that
//!    is not there: `CodeGuru Profiler can’t complete the operation because <name> was not
//!    found.` (with the typographic apostrophe the service's text uses);
//! 4. a 404 about missing profile DATA — the group IS there, but it holds no profile
//!    covering the requested range — carries the service's FIXED text, `No profiles found
//!    for the requested time range!`, with nothing of the request in it: that constant is
//!    the only thing telling it apart from rule 3's refusal, since both are 404
//!    `ResourceNotFoundException`.
//!
//! The harness is this suite's own: nothing in `tests/` is shared between suites here.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The account every resource this emulator mints lives in, which is what a
/// record-level refusal names beside the group. Read from the runtime rather than
/// written out, so this suite cannot drift from the ARNs the service emits.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// `ProfilingGroupName`'s `@pattern` as the model declares it, and the expression a
/// refusal states — the same expression without the whole-string anchors. Source:
/// `smithy_query(query="shape", shape="ProfilingGroupName")` — `^[\w-]+$`, length 1..255.
/// `ClientToken` and `PaginationToken` declare the same expression (length 1..64).
const WORD_OR_DASH_PATTERN: &str = r"[\w-]+";

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
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The modelled error's `message` — the `@required` member every error shape of this
    /// service declares.
    fn error_message(&self) -> String {
        self.json()
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service. The engine is kept so a test can place a retained profile at a
/// chosen instant, which the clock cannot be asked for.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codeguruprofiler_codeguruprofiler::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    async fn send(&self, method: &str, uri: &str, content_type: &str, body: Vec<u8>) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "codeguru-profiler.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", content_type)
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn get(&self, uri: &str) -> Answer {
        self.send("GET", uri, "application/json", Vec::new()).await
    }

    async fn create(&self, name: &str) -> Answer {
        self.create_with_token(name, &format!("tok-{name}")).await
    }

    async fn create_with_token(&self, name: &str, token: &str) -> Answer {
        self.send(
            "POST",
            &format!("/profilingGroups?clientToken={token}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({"profilingGroupName": name}))
                .expect("a serializable body"),
        )
        .await
    }

    async fn delete(&self, name: &str) -> Answer {
        self.send(
            "DELETE",
            &format!("/profilingGroups/{name}"),
            "application/json",
            Vec::new(),
        )
        .await
    }

    /// Post one agent profile to `name`, then re-stamp its receipt time to `at` (epoch
    /// seconds): the clock cannot be asked for a past instant, so this is how a test
    /// places a retained profile outside the window a later read asks about. It writes
    /// the same stored field `PostAgentProfile` writes and nothing else.
    async fn post_profile_at(&self, name: &str, at: i64) {
        let posted = self
            .send(
                "POST",
                &format!("/profilingGroups/{name}/agentProfile?profileToken=tok-{at}"),
                "application/json",
                b"{\"profile\":\"payload\"}".to_vec(),
            )
            .await;
        assert_eq!(
            posted.status,
            204,
            "posting a profile to {name:?} must succeed, got {} {}",
            posted.status,
            String::from_utf8_lossy(&posted.body)
        );
        let state = self
            .engine
            .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
        let mut guard = state.lock();
        let group = guard
            .groups
            .get_mut(name)
            .unwrap_or_else(|| panic!("no stored group named {name}"));
        let stamped = DateTime::from_secs(at);
        for profile in group.profiles.iter_mut() {
            profile.received_at = stamped;
        }
        group.latest_agent_profile_reported_at = Some(stamped);
    }
}

/// An epoch-second instant in the ISO 8601 form the modelled `date-time` query members
/// carry.
fn iso8601(secs: f64) -> String {
    DateTime::from_secs_f64(secs)
        .fmt(Format::DateTime)
        .expect("a renderable instant")
}

/// AWS's constraint-violation envelope over ONE clause: what rule 1 requires for a
/// single-violation request.
fn one_violation(value_clause: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: {value_clause} at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The record-level refusal of rule 2.
fn record_message(name: &str, outcome: &str) -> String {
    format!(
        "ProfilingGroup (name: {name}, accountId: {}) {outcome}",
        account_id()
    )
}

/// The group-scoped not-found refusal of rule 3. The apostrophe is U+2019, as the
/// service's own text spells it.
fn not_found_message(name: &str) -> String {
    format!("CodeGuru Profiler can\u{2019}t complete the operation because {name} was not found.")
}

/// The fixed text of rule 4.
const NO_PROFILES_IN_RANGE: &str = "No profiles found for the requested time range!";

/// One request that must be refused with a constraint-violation message, and the single
/// clause that message must carry. Named fields rather than a tuple, because a row
/// carries seven independent things.
struct ViolationCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Vec<u8>,
    /// The subject half of the clause: `Value '<submitted>'`, or `Value null` for a
    /// member that was absent.
    value_clause: String,
    /// The dotted member path the clause names.
    path: &'static str,
    /// The requirement half of the clause.
    requirement: String,
}

#[tokio::test]
async fn a_pattern_requirement_states_the_expression_without_the_models_anchors() {
    let harness = Harness::new();
    let base: f64 = 1_590_969_600.0;
    let window = format!(
        "startTime={}&endTime={}",
        iso8601(base),
        iso8601(base + 3_600.0)
    );

    // The requirement every row below must state: the modelled expression, unanchored.
    let requirement =
        format!("Member must satisfy regular expression pattern: {WORD_OR_DASH_PATTERN}");
    // One row per (operation, member, way of breaking the expression): three member
    // names, four operations, a BODY member and two query members, and values broken by a
    // space, a bang, a dot, a slash, a colon and a tilde. The recorded
    // `profilingGroupName` case is one row.
    let pattern_violation = |case: &'static str,
                             method: &'static str,
                             uri: String,
                             body: Vec<u8>,
                             submitted: &str,
                             path: &'static str| ViolationCase {
        case,
        method,
        uri,
        body,
        value_clause: format!("Value '{submitted}'"),
        path,
        requirement: requirement.clone(),
    };
    let cases: Vec<ViolationCase> = vec![
        pattern_violation(
            "a create whose body name carries spaces and a bang",
            "POST",
            "/profilingGroups?clientToken=tok-1".to_string(),
            serde_json::to_vec(&serde_json::json!({
                "profilingGroupName": "bad name with spaces!"
            }))
            .expect("a serializable body"),
            "bad name with spaces!",
            "profilingGroupName",
        ),
        pattern_violation(
            "a create whose body name carries a dot",
            "POST",
            "/profilingGroups?clientToken=tok-2".to_string(),
            serde_json::to_vec(&serde_json::json!({"profilingGroupName": "group.one"}))
                .expect("a serializable body"),
            "group.one",
            "profilingGroupName",
        ),
        pattern_violation(
            "a create whose body name carries a slash",
            "POST",
            "/profilingGroups?clientToken=tok-3".to_string(),
            serde_json::to_vec(&serde_json::json!({"profilingGroupName": "group/one"}))
                .expect("a serializable body"),
            "group/one",
            "profilingGroupName",
        ),
        pattern_violation(
            "a create whose clientToken query value carries a colon",
            "POST",
            "/profilingGroups?clientToken=tok:1".to_string(),
            serde_json::to_vec(&serde_json::json!({"profilingGroupName": "well-formed"}))
                .expect("a serializable body"),
            "tok:1",
            "clientToken",
        ),
        pattern_violation(
            "a findings-report listing whose nextToken carries a plus",
            "GET",
            format!("/internal/profilingGroups/well-formed/findingsReports?{window}&nextToken=a+b"),
            Vec::new(),
            "a b",
            "nextToken",
        ),
        pattern_violation(
            "a profile post whose profileToken carries a tilde",
            "POST",
            "/profilingGroups/well-formed/agentProfile?profileToken=tok~1".to_string(),
            b"{}".to_vec(),
            "tok~1",
            "profileToken",
        ),
    ];

    for expected_case in cases {
        let answer = assert_refused_with_clause(&harness, &expected_case).await;
        // The anchoring is the ONE thing that must not be in the stated expression,
        // however the sentence is otherwise worded.
        assert!(
            !answer.error_message().contains("^[\\w-]+$"),
            "case {:?}: the stated pattern must carry no anchors, got {:?}",
            expected_case.case,
            answer.error_message()
        );
    }
}

/// Send one [`ViolationCase`] and assert the refusal it must get: HTTP 400,
/// `ValidationException`, and AWS's envelope over exactly that one clause. Returns the
/// answer so a caller can assert more about the same message.
async fn assert_refused_with_clause(harness: &Harness, expected: &ViolationCase) -> Answer {
    let case = expected.case;
    let answer = harness
        .send(
            expected.method,
            &expected.uri,
            "application/json",
            expected.body.clone(),
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {}",
        String::from_utf8_lossy(&answer.body)
    );
    let message = one_violation(&expected.value_clause, expected.path, &expected.requirement);
    assert_eq!(
        answer.error_message(),
        message,
        "case {case:?}: expected {message:?}, got {:?}",
        answer.error_message()
    );
    answer
}

#[tokio::test]
async fn a_non_pattern_constraint_clause_is_worded_the_way_aws_words_it() {
    let harness = Harness::new();

    // One row per constraint class this service's members declare — `@required` absence,
    // both `@length` bounds and both `@range` bounds — so the pattern rendering above is
    // shown to be the only clause that differs from the framework's AWS-standard wording.
    // Each bound is taken from the model and exercised one step PAST it:
    // `ClientToken` @length 1..64, `ProfilingGroupName` @length 1..255,
    // `MaxDepth` @range 1..10000 (`smithy_query(query="operation",
    // operation="CreateProfilingGroup,GetProfile")`).
    let long_token = "t".repeat(65);
    let long_name = "n".repeat(256);
    let cases: Vec<ViolationCase> = vec![
        ViolationCase {
            case: "a create with no profilingGroupName at all",
            method: "POST",
            uri: "/profilingGroups?clientToken=tok-a".to_string(),
            body: serde_json::to_vec(&serde_json::json!({})).expect("a serializable body"),
            value_clause: "Value null".to_string(),
            path: "profilingGroupName",
            requirement: "Member must not be null".to_string(),
        },
        ViolationCase {
            case: "a create whose clientToken is one character past its maximum",
            method: "POST",
            uri: format!("/profilingGroups?clientToken={long_token}"),
            body: serde_json::to_vec(&serde_json::json!({"profilingGroupName": "well-formed"}))
                .expect("a serializable body"),
            value_clause: format!("Value '{long_token}'"),
            path: "clientToken",
            requirement: "Member must have length less than or equal to 64".to_string(),
        },
        ViolationCase {
            case: "a create whose profilingGroupName is one character past its maximum",
            method: "POST",
            uri: "/profilingGroups?clientToken=tok-b".to_string(),
            body: serde_json::to_vec(
                &serde_json::json!({"profilingGroupName": long_name.clone()}),
            )
            .expect("a serializable body"),
            value_clause: format!("Value '{long_name}'"),
            path: "profilingGroupName",
            requirement: "Member must have length less than or equal to 255".to_string(),
        },
        ViolationCase {
            case: "a profile read whose maxDepth is one below its minimum",
            method: "GET",
            uri: "/profilingGroups/well-formed/profile?period=PT5M&maxDepth=0".to_string(),
            body: Vec::new(),
            value_clause: "Value '0'".to_string(),
            path: "maxDepth",
            requirement: "Member must have value greater than or equal to 1".to_string(),
        },
        ViolationCase {
            case: "a profile read whose maxDepth is one past its maximum",
            method: "GET",
            uri: "/profilingGroups/well-formed/profile?period=PT5M&maxDepth=10001".to_string(),
            body: Vec::new(),
            value_clause: "Value '10001'".to_string(),
            path: "maxDepth",
            requirement: "Member must have value less than or equal to 10000".to_string(),
        },
    ];

    for expected_case in cases {
        assert_refused_with_clause(&harness, &expected_case).await;
    }
}

#[tokio::test]
async fn a_refusal_about_the_group_record_names_the_group_and_the_account() {
    let harness = Harness::new();

    // Names differing along every axis the modelled `ProfilingGroupName` allows: word
    // characters, digits, underscores, hyphens, a single character, and the length
    // maximum itself. The format is the rule, so no name is privileged.
    let names: Vec<String> = vec![
        "one".to_string(),
        "Group_2".to_string(),
        "a-b-c-1234567890".to_string(),
        "x".to_string(),
        "n".repeat(255),
    ];

    for (index, name) in names.iter().enumerate() {
        // The `clientToken` is bounded at 64 characters, so it is the row's index and not
        // the group name: the name is what varies here, the token only has to be new.
        let first_token = format!("first-{index}");
        let second_token = format!("second-{index}");
        assert_eq!(
            harness.create_with_token(name, &first_token).await.status,
            201,
            "name {name:?}: the first create must succeed"
        );

        // A second create of the same name under a DIFFERENT token is the collision.
        let conflict = harness.create_with_token(name, &second_token).await;
        assert_eq!(
            conflict.status, 409,
            "name {name:?}: a duplicate create must answer 409, got {} {}",
            conflict.status,
            String::from_utf8_lossy(&conflict.body)
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "name {name:?}: expected ConflictException, got {}",
            String::from_utf8_lossy(&conflict.body)
        );
        let expected = record_message(name, "already exists.");
        assert_eq!(
            conflict.error_message(),
            expected,
            "name {name:?}: expected {expected:?}, got {:?}",
            conflict.error_message()
        );

        // Deleting it twice: the first delete succeeds, the second is the record-level
        // absence — the SAME form with the other outcome.
        assert_eq!(
            harness.delete(name).await.status,
            204,
            "name {name:?}: the first delete must succeed"
        );
        let missing = harness.delete(name).await;
        assert_eq!(
            missing.status, 404,
            "name {name:?}: deleting a group that is gone must answer 404, got {} {}",
            missing.status,
            String::from_utf8_lossy(&missing.body)
        );
        assert_eq!(
            missing.error_code(),
            "ResourceNotFoundException",
            "name {name:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&missing.body)
        );
        let expected = record_message(name, "not found.");
        assert_eq!(
            missing.error_message(),
            expected,
            "name {name:?}: expected {expected:?}, got {:?}",
            missing.error_message()
        );
    }
}

#[tokio::test]
async fn every_other_group_scoped_operation_states_its_own_not_found_text() {
    let harness = Harness::new();
    let base: f64 = 1_590_969_600.0;
    let window = format!(
        "startTime={}&endTime={}",
        iso8601(base),
        iso8601(base + 3_600.0)
    );

    // One row per IMPLEMENTED operation that resolves a profiling group by name, read
    // and write alike, over names that were never created. The method, the URI shape and
    // the body differ per row; the refusal does not.
    let names = ["never-created", "Absent_1", "a-b-c"];
    for name in names {
        let cases: Vec<(&str, &str, String, &str, Vec<u8>)> = vec![            (
                "DescribeProfilingGroup",
                "GET",
                format!("/profilingGroups/{name}"),
                "application/json",
                Vec::new(),
            ),
            (
                "UpdateProfilingGroup",
                "PUT",
                format!("/profilingGroups/{name}"),
                "application/json",
                serde_json::to_vec(&serde_json::json!({
                    "agentOrchestrationConfig": {"profilingEnabled": true}
                }))
                .expect("a serializable body"),
            ),
            (
                "ConfigureAgent",
                "POST",
                format!("/profilingGroups/{name}/configureAgent"),
                "application/json",
                serde_json::to_vec(&serde_json::json!({})).expect("a serializable body"),
            ),
            (
                "PostAgentProfile",
                "POST",
                format!("/profilingGroups/{name}/agentProfile?profileToken=tok-1"),
                "application/json",
                b"{\"profile\":\"payload\"}".to_vec(),
            ),
            (
                "GetProfile",
                "GET",
                format!("/profilingGroups/{name}/profile?period=PT5M"),
                "application/json",
                Vec::new(),
            ),
            (
                "GetRecommendations",
                "GET",
                format!("/internal/profilingGroups/{name}/recommendations?{window}"),
                "application/json",
                Vec::new(),
            ),
            (
                "ListFindingsReports",
                "GET",
                format!("/internal/profilingGroups/{name}/findingsReports?{window}"),
                "application/json",
                Vec::new(),
            ),
            (
                "GetPolicy",
                "GET",
                format!("/profilingGroups/{name}/policy"),
                "application/json",
                Vec::new(),
            ),
            (
                "ListProfileTimes",
                "GET",
                format!("/profilingGroups/{name}/profileTimes?{window}&period=PT5M"),
                "application/json",
                Vec::new(),
            ),
            (
                "UntagResource",
                "DELETE",
                format!(
                    "/tags/arn%3Aaws%3Acodeguru-profiler%3Aus-east-1%3A{}%3AprofilingGroup%2F{name}?tagKeys=env",
                    account_id()
                ),
                "application/json",
                Vec::new(),
            ),
        ];

        for (operation, method, uri, content_type, body) in cases {
            let answer = harness.send(method, &uri, content_type, body).await;
            assert_eq!(
                answer.status, 404,
                "case {operation} {name:?}: expected HTTP 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {operation} {name:?}: expected ResourceNotFoundException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let expected = not_found_message(name);
            assert_eq!(
                answer.error_message(),
                expected,
                "case {operation} {name:?}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
            // The record-level form belongs to create and delete alone, so a refusal
            // about operating on a group never names an account.
            assert!(
                !answer.error_message().contains(account_id()),
                "case {operation} {name:?}: this refusal must not name an account, got {:?}",
                answer.error_message()
            );
        }
    }
}

#[tokio::test]
async fn a_group_holding_no_profile_in_the_requested_range_states_the_fixed_text() {
    let harness = Harness::new();
    let base: f64 = 1_590_969_600.0;

    // Rows: (case, group name, whether the group holds a profile, the window query).
    // They vary the group, whether any profile is retained at all, where the retained
    // profile sits relative to the window, and how the window is expressed (both bounds,
    // one bound plus `period`, `period` alone) — the message is a CONSTANT, so no row's
    // value may appear in it.
    let cases: Vec<(&str, &str, Option<i64>, String)> = vec![
        (
            "a group that has never been posted to, read over a window",
            "empty-group",
            None,
            format!(
                "startTime={}&endTime={}",
                iso8601(base),
                iso8601(base + 3_600.0)
            ),
        ),
        (
            "a group that has never been posted to, read by period alone",
            "Empty_2",
            None,
            "period=PT5M".to_string(),
        ),
        (
            "a retained profile an hour before the window",
            "before-window",
            Some(base as i64 - 3_600),
            format!(
                "startTime={}&endTime={}",
                iso8601(base),
                iso8601(base + 3_600.0)
            ),
        ),
        (
            "a retained profile a day after the window",
            "after-window",
            Some(base as i64 + 86_400),
            format!(
                "startTime={}&endTime={}",
                iso8601(base),
                iso8601(base + 3_600.0)
            ),
        ),
        (
            "a retained profile just outside a window given as endTime plus period",
            "edge-window",
            Some(base as i64 - 301),
            format!("endTime={}&period=PT5M", iso8601(base)),
        ),
    ];

    for (case, name, profile_at, query) in cases {
        assert_eq!(
            harness.create(name).await.status,
            201,
            "case {case:?}: the group must be created"
        );
        if let Some(at) = profile_at {
            harness.post_profile_at(name, at).await;
        }

        let answer = harness
            .get(&format!("/profilingGroups/{name}/profile?{query}"))
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_message(),
            NO_PROFILES_IN_RANGE,
            "case {case:?}: expected {NO_PROFILES_IN_RANGE:?}, got {:?}",
            answer.error_message()
        );
        // The text is fixed, so nothing of the request — not the group, not the bounds —
        // may be interpolated into it, and it is NOT the group-missing refusal.
        assert!(
            !answer.error_message().contains(name),
            "case {case:?}: the fixed text must not name the group, got {:?}",
            answer.error_message()
        );
        assert_ne!(
            answer.error_message(),
            not_found_message(name),
            "case {case:?}: a group that exists must not be reported as not found"
        );
    }
}

#[tokio::test]
async fn a_group_that_does_hold_a_profile_in_the_range_is_still_served() {
    // The refusals above must not be reachable for a request that should succeed: the
    // group is read back through `DescribeProfilingGroup` and its profile through
    // `GetProfile`, so rule 4's 404 is shown to be about absent DATA and not about every
    // read.
    let harness = Harness::new();
    let name = "served-group";
    let at: i64 = 1_590_969_600;
    assert_eq!(harness.create(name).await.status, 201, "create must succeed");
    harness.post_profile_at(name, at).await;

    let described = harness.get(&format!("/profilingGroups/{name}")).await;
    assert_eq!(
        described.status,
        200,
        "the created group must read back, got {} {}",
        described.status,
        String::from_utf8_lossy(&described.body)
    );
    assert_eq!(
        described
            .json()
            .pointer("/name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        name,
        "the described group must be the one created, got {}",
        String::from_utf8_lossy(&described.body)
    );

    let profile = harness
        .get(&format!(
            "/profilingGroups/{name}/profile?startTime={}&endTime={}",
            iso8601(at as f64 - 60.0),
            iso8601(at as f64 + 60.0)
        ))
        .await;
    assert_eq!(
        profile.status,
        200,
        "a profile inside the window must be served, got {} {}",
        profile.status,
        String::from_utf8_lossy(&profile.body)
    );
    assert_eq!(
        profile.body,
        b"{\"profile\":\"payload\"}".to_vec(),
        "the served profile must be the bytes posted, got {}",
        String::from_utf8_lossy(&profile.body)
    );
}
