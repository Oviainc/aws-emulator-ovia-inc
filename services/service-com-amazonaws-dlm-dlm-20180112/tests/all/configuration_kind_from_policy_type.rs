// SPDX-License-Identifier: CC0-1.0

//! Which KIND of policy a `PolicyDetails` configuration describes, end to end
//! over HTTP.
//!
//! The rule under test: the kind of policy a configuration describes is the one
//! its `PolicyType` names — the type the configuration states, or, on an update,
//! the type the policy already holds, or else that member's documented default
//! `EBS_SNAPSHOT_MANAGEMENT` — and it is never inferred from which other members
//! the configuration happens to carry. So the configuration members a request
//! must state follow that type alone: a snapshot or AMI type requires the members
//! a custom policy is built from (`ResourceTypes`, `TargetTags`, `Schedules`) and
//! `EVENT_BASED_POLICY` requires the ones an event-based policy is built from
//! (`EventSource`, `Actions`), whichever members of the other kind the request
//! also sent. A configuration that leaves out one of them is refused with
//! `InvalidRequestException` (HTTP 400) naming it, and nothing is stored or
//! changed.
//!
//! Sources, from `smithy_query(query="shape", shape="PolicyDetails")`:
//! `PolicyType` — "The type of policy ... Specify `EVENT_BASED_POLICY` to create
//! an event-based policy that performs specific actions when a defined event
//! occurs ... The default is `EBS_SNAPSHOT_MANAGEMENT`"; `EventSource` —
//! "[Event-based policies only] The event that activates the event-based policy";
//! and, on the update path,
//! `smithy_query(query="doc", shape="UpdateLifecyclePolicyRequest",
//! member="PolicyDetails")` — "The configuration of the lifecycle policy. You
//! cannot update the policy type or the resource type."
//!
//! The rows span what the rule generalises over: every `PolicyTypeValues` member
//! and the type left unstated, each kind's own member set against the other
//! kind's, a partial member set, no members at all, the custom policy language
//! stated beside the type and omitted, and the create path against the update
//! path (where the type is carried from the policy rather than defaulted).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The Region an event-based policy's action copies to.
const OTHER_REGION: &str = "eu-west-1";

/// The modelled client error a refused parameter is answered with, and its
/// `@httpError` status.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;

/// `PolicyDetails$PolicyType`: "The default is `EBS_SNAPSHOT_MANAGEMENT`."
const SNAPSHOT_POLICY_TYPE: &str = "EBS_SNAPSHOT_MANAGEMENT";

/// The `PolicyTypeValues` member that "manages the lifecycle of EBS-backed AMIs".
const IMAGE_POLICY_TYPE: &str = "IMAGE_MANAGEMENT";

/// The `PolicyTypeValues` member of a policy that "performs specific actions when
/// a defined event occurs".
const EVENT_BASED_POLICY_TYPE: &str = "EVENT_BASED_POLICY";

/// Every `PolicyTypeValues` member, from
/// `smithy_query(query="shape", shape="PolicyTypeValues")`. A row below states
/// each one, and a row states none.
const POLICY_TYPE_VALUES: &[&str] = &[
    SNAPSHOT_POLICY_TYPE,
    IMAGE_POLICY_TYPE,
    EVENT_BASED_POLICY_TYPE,
];

/// The members a CUSTOM snapshot or AMI policy is built from, in the order a
/// refusal names them.
const CUSTOM_MEMBERS: &[&str] = &["ResourceTypes", "TargetTags", "Schedules"];

/// The members an EVENT-BASED policy is built from.
const EVENT_BASED_MEMBERS: &[&str] = &["EventSource", "Actions"];

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }

    /// The parameters the refusal reports as required, sorted so a row compares
    /// the SET the service asked for.
    fn required_parameters(&self) -> Vec<String> {
        let mut reported: Vec<String> = self.body["RequiredParameters"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        reported
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A SigV4 credential scope naming this service — what routes the request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, description: &str, details: Value) -> Answer {
    call(
        router,
        "POST",
        "/policies",
        Some(json!({
            "ExecutionRoleArn": role_arn(),
            "Description": description,
            "State": "ENABLED",
            "PolicyDetails": details,
        })),
    )
    .await
}

async fn update(router: &MultiServiceRouter, id: &str, details: Value) -> Answer {
    call(
        router,
        "PATCH",
        &format!("/policies/{id}"),
        Some(json!({"PolicyDetails": details})),
    )
    .await
}

/// The stored configuration `GetLifecyclePolicy` reports for `id`.
async fn read_details(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .pointer("/Policy/PolicyDetails")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Whether `GetLifecyclePolicies?resourceTypes=<resource_type>` selects `id`.
async fn selected_by_resource_type(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    id: &str,
) -> bool {
    let answer = call(
        router,
        "GET",
        &format!("/policies?resourceTypes={resource_type}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicies, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Policies"]
        .as_array()
        .is_some_and(|summaries| summaries.iter().any(|summary| summary["PolicyId"] == json!(id)))
}

/// How many policies the store holds, read through `GetLifecyclePolicies`.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    call(router, "GET", "/policies", None).await.body["Policies"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// Assert `answer` is the modelled refusal and reports exactly `members` of the
/// configuration structure as required.
fn assert_requires(case: &str, answer: &Answer, members: &[&str]) {
    assert_eq!(
        answer.status, INVALID_REQUEST_STATUS,
        "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
        "case {case:?}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    // An omitted parameter is named by the member's own name, which is how the
    // refusal lists it.
    let mut expected: Vec<String> = members.iter().map(|member| (*member).to_string()).collect();
    expected.sort();
    let reported = answer.required_parameters();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected RequiredParameters {expected:?}, got {reported:?}"
    );
    for parameter in &expected {
        assert!(
            answer.message().contains(parameter.as_str()),
            "case {case:?}: the refusal must name {parameter:?}, got {:?}",
            answer.message()
        );
    }
}

/// A custom snapshot/AMI policy's own members, targeting `resource_type` and
/// creating a snapshot every `interval` hours.
fn custom_members(resource_type: &str, interval: u32) -> Value {
    json!({
        // `ResourceTypeValuesList` is `@length(min: 1, max: 1)`, so a custom
        // policy targets exactly one resource type.
        "ResourceTypes": [resource_type],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": interval, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// An event-based policy's own members: the event that activates it and the
/// action it performs.
fn event_based_members(description_regex: &str) -> Value {
    json!({
        "EventSource": {
            "Type": "MANAGED_CWE",
            "Parameters": {
                "EventType": "shareSnapshot",
                "SnapshotOwner": ["000000000000"],
                "DescriptionRegex": description_regex,
            },
        },
        "Actions": [{
            "Name": "copy",
            "CrossRegionCopy": [{
                "Target": OTHER_REGION,
                "EncryptionConfiguration": {"Encrypted": false},
                "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"},
            }],
        }],
    })
}

/// `members` as a configuration, with `PolicyType` stated only when one is
/// given and `PolicyLanguage` stated only when one is.
fn details(policy_type: Option<&str>, language: Option<&str>, members: Value) -> Value {
    let mut configuration = json!({});
    if let Some(policy_type) = policy_type {
        configuration["PolicyType"] = json!(policy_type);
    }
    if let Some(language) = language {
        configuration["PolicyLanguage"] = json!(language);
    }
    for (member, value) in members
        .as_object()
        .expect("a configuration is a JSON object")
    {
        configuration[member] = value.clone();
    }
    configuration
}

/// `details` with `member` removed, as a client that simply left it out sends it.
fn without(mut configuration: Value, member: &str) -> Value {
    configuration
        .as_object_mut()
        .expect("a configuration is a JSON object")
        .remove(member);
    configuration
}

/// What the service owes one configuration.
#[derive(Debug)]
enum Owed {
    /// The configuration is complete for the kind its type names, so it is
    /// applied — and the stored configuration reports that type.
    Accepted(&'static str),
    /// Refused, required to state exactly these members of the structure.
    Requires(&'static [&'static str]),
}

/// RULE, create path — the members a configuration must state follow its
/// `PolicyType`, defaulted when it states none, and not the members it carries.
#[tokio::test]
async fn a_configurations_required_members_follow_its_policy_type() {
    let router = emulator();

    let cases: Vec<(String, Value, Owed)> = vec![
        // The event-based member set under every type that is not the
        // event-based one: the configuration describes a snapshot or AMI policy
        // and must state the members one is built from. The first row is the
        // request the issue reproduced — an `EventSource` with no type at all.
        (
            "event-based members, no PolicyType".to_string(),
            details(None, None, event_based_members("^.*$")),
            Owed::Requires(CUSTOM_MEMBERS),
        ),
        (
            "event-based members, no PolicyType, STANDARD language".to_string(),
            details(None, Some("STANDARD"), event_based_members("^.*$")),
            Owed::Requires(CUSTOM_MEMBERS),
        ),
        (
            format!("event-based members, PolicyType {SNAPSHOT_POLICY_TYPE}"),
            details(
                Some(SNAPSHOT_POLICY_TYPE),
                None,
                event_based_members("^.*$"),
            ),
            Owed::Requires(CUSTOM_MEMBERS),
        ),
        (
            format!("event-based members, PolicyType {IMAGE_POLICY_TYPE}"),
            details(Some(IMAGE_POLICY_TYPE), None, event_based_members("^.*$")),
            Owed::Requires(CUSTOM_MEMBERS),
        ),
        (
            "an EventSource alone, no PolicyType".to_string(),
            without(
                details(None, None, event_based_members("^.*$")),
                "Actions",
            ),
            Owed::Requires(CUSTOM_MEMBERS),
        ),
        (
            "no members at all, no PolicyType".to_string(),
            json!({}),
            Owed::Requires(CUSTOM_MEMBERS),
        ),
        // The event-based type requires the event-based members, whichever
        // custom members the request sent.
        (
            format!("custom members, PolicyType {EVENT_BASED_POLICY_TYPE}"),
            details(
                Some(EVENT_BASED_POLICY_TYPE),
                None,
                custom_members("VOLUME", 24),
            ),
            Owed::Requires(EVENT_BASED_MEMBERS),
        ),
        (
            format!("no members at all, PolicyType {EVENT_BASED_POLICY_TYPE}"),
            details(Some(EVENT_BASED_POLICY_TYPE), None, json!({})),
            Owed::Requires(EVENT_BASED_MEMBERS),
        ),
        (
            format!("Actions alone, PolicyType {EVENT_BASED_POLICY_TYPE}"),
            without(
                details(
                    Some(EVENT_BASED_POLICY_TYPE),
                    None,
                    event_based_members("^.*$"),
                ),
                "EventSource",
            ),
            Owed::Requires(&["EventSource"]),
        ),
        // And each complete configuration is accepted and stored under the type
        // its own member names, so no row above passes for the wrong reason.
        (
            "custom members, no PolicyType".to_string(),
            details(None, None, custom_members("VOLUME", 24)),
            Owed::Accepted(SNAPSHOT_POLICY_TYPE),
        ),
        (
            format!("custom members, PolicyType {SNAPSHOT_POLICY_TYPE}"),
            details(
                Some(SNAPSHOT_POLICY_TYPE),
                None,
                custom_members("INSTANCE", 12),
            ),
            Owed::Accepted(SNAPSHOT_POLICY_TYPE),
        ),
        (
            format!("custom members, PolicyType {IMAGE_POLICY_TYPE}"),
            details(
                Some(IMAGE_POLICY_TYPE),
                None,
                custom_members("INSTANCE", 24),
            ),
            Owed::Accepted(IMAGE_POLICY_TYPE),
        ),
        (
            format!("event-based members, PolicyType {EVENT_BASED_POLICY_TYPE}"),
            details(
                Some(EVENT_BASED_POLICY_TYPE),
                None,
                event_based_members("^share-.*$"),
            ),
            Owed::Accepted(EVENT_BASED_POLICY_TYPE),
        ),
    ];

    let mut accepted_types: Vec<&str> = Vec::new();
    for (case, configuration, owed) in &cases {
        let case = case.as_str();
        let before = policy_count(&router).await;
        let answer = create(&router, "policy kind from policy type", configuration.clone()).await;
        match owed {
            Owed::Accepted(policy_type) => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                // Round trip: the stored configuration reports the type that
                // decided which members were required of it.
                let id = answer.policy_id();
                let stored = read_details(&router, case, &id).await;
                assert_eq!(
                    stored.pointer("/PolicyType").cloned().unwrap_or(Value::Null),
                    json!(policy_type),
                    "case {case:?}: the stored configuration must report PolicyType \
                     {policy_type:?}, got {stored:?}"
                );
                accepted_types.push(policy_type);
            }
            Owed::Requires(members) => {
                assert_requires(case, &answer, members);
                assert_eq!(
                    policy_count(&router).await,
                    before,
                    "case {case:?}: a refused create must leave no policy behind"
                );
            }
        }
    }

    // Every member of the catalog is the type of one accepted policy, so no
    // policy kind is judged by a rule no row exercises.
    for policy_type in POLICY_TYPE_VALUES {
        assert!(
            accepted_types.contains(policy_type),
            "the table must accept a policy of each PolicyTypeValues member, \
             {policy_type:?} is missing from {accepted_types:?}"
        );
    }
}

/// One update subject: a stored policy, and the replacement configuration of the
/// OTHER kind that states no type.
struct Subject {
    case: &'static str,
    /// The configuration the policy is created with.
    created: Value,
    /// The resource type the `resourceTypes` filter finds it under, when it is a
    /// custom policy.
    targets: Option<&'static str>,
    /// A replacement of the other kind, stating no `PolicyType`.
    other_kind: Value,
    /// The members the policy's own type requires of a replacement.
    requires: &'static [&'static str],
    /// A replacement of the policy's OWN kind, stating no `PolicyType`, and the
    /// member of it a read must report afterwards.
    same_kind: Value,
    applied: (&'static str, Value),
}

fn subjects() -> Vec<Subject> {
    vec![
        Subject {
            case: "a custom snapshot policy",
            created: details(None, None, custom_members("VOLUME", 24)),
            targets: Some("VOLUME"),
            other_kind: details(None, None, event_based_members("^.*$")),
            requires: CUSTOM_MEMBERS,
            same_kind: details(None, None, custom_members("VOLUME", 12)),
            applied: ("/Schedules/0/CreateRule/Interval", json!(12)),
        },
        Subject {
            case: "a custom AMI policy",
            created: details(
                Some(IMAGE_POLICY_TYPE),
                None,
                custom_members("INSTANCE", 24),
            ),
            targets: Some("INSTANCE"),
            other_kind: details(None, None, event_based_members("^.*$")),
            requires: CUSTOM_MEMBERS,
            same_kind: details(None, None, custom_members("INSTANCE", 6)),
            applied: ("/Schedules/0/CreateRule/Interval", json!(6)),
        },
        Subject {
            case: "an event-based policy",
            created: details(
                Some(EVENT_BASED_POLICY_TYPE),
                None,
                event_based_members("^.*$"),
            ),
            targets: None,
            other_kind: details(None, None, custom_members("VOLUME", 24)),
            requires: EVENT_BASED_MEMBERS,
            same_kind: details(None, None, event_based_members("^share-.*$")),
            applied: (
                "/EventSource/Parameters/DescriptionRegex",
                json!("^share-.*$"),
            ),
        },
    ]
}

/// RULE, update path — a replacement configuration stating no `PolicyType` is
/// judged against the type the POLICY holds, so one carrying the other kind's
/// members is refused for the members its own kind is built from and the stored
/// configuration is left exactly as it was.
#[tokio::test]
async fn an_update_cannot_change_a_policys_kind_by_omitting_its_policy_type() {
    let router = emulator();

    for subject in subjects() {
        let case = subject.case;
        let created = create(&router, "policy kind on update", subject.created).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
            created.status, created.body
        );
        let id = created.policy_id();
        let before = read_details(&router, case, &id).await;

        // The replacement describes the other kind of policy but states no type,
        // so it is judged as a replacement for THIS policy's type: the members
        // that type is built from are missing from it.
        let refused = update(&router, &id, subject.other_kind.clone()).await;
        assert_requires(case, &refused, subject.requires);

        // ...and the refusal changed nothing: the configuration the policy was
        // created with is still the one a read reports.
        let after = read_details(&router, case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the stored configuration unchanged, \
             it became {after:?}"
        );
        if let Some(resource_type) = subject.targets {
            assert!(
                selected_by_resource_type(&router, case, resource_type, &id).await,
                "case {case:?}: the policy must still be selected by resourceTypes \
                 {resource_type:?} after the refused update, its configuration is {after:?}"
            );
        }

        // A replacement of the policy's OWN kind, also stating no type, is
        // accepted and applied — so the refusal above is about the member set,
        // not about an update that omits the type.
        let accepted = update(&router, &id, subject.same_kind).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateLifecyclePolicy, got {} with {:?}",
            accepted.status, accepted.body
        );
        let (pointer, expected) = subject.applied;
        let replaced = read_details(&router, case, &id).await;
        assert_eq!(
            replaced.pointer(pointer).cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: the accepted replacement must report {expected:?} at {pointer}, \
             got {replaced:?}"
        );
    }
}
