// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the two validation refusals a policy write can
//! answer, end to end over HTTP.
//!
//! The rules under test, each one wording of `InvalidRequestException` (HTTP
//! 400, `Code` `InvalidParameter`) — `smithy_query(query="operation",
//! operation="CreateLifecyclePolicy,UpdateLifecyclePolicy")` and
//! `smithy_query(query="shape", shape="InvalidRequestException")`:
//!
//! * A request that left out parameters it is required to state is refused with
//!   the fixed text "The following required parameter(s) are missing: "
//!   followed by the omitted parameters as a brace-wrapped, comma-separated
//!   list, and the same names in `RequiredParameters` ("the request omitted one
//!   or more required parameters"). The variable part comes from the request
//!   alone: each parameter by its own member name, qualified only as far as it
//!   takes to say which element of a collection it belongs to.
//! * An update that asks to change a parameter fixed when the policy was
//!   created is refused with the fixed text "The following parameter(s) cannot
//!   be updated: " followed by those parameters, comma-separated, and nothing
//!   of the values the request stated or the policy holds — so one refusal
//!   reads the same for every policy, account and region. Neither structured
//!   parameter list names an immutable parameter: it was neither omitted nor
//!   stated beside a parameter it cannot be combined with.
//!
//! Which parameters each rule covers comes from the documentation, since
//! `PolicyDetails` declares the members of every policy kind at once and the
//! model can mark none of them `@required`
//! (`smithy_query(query="shape", shape="PolicyDetails")`):
//! `ResourceTypes`/`TargetTags`/`Schedules` for a custom snapshot or AMI
//! policy, `EventSource`/`Actions` for an event-based one, and
//! `PolicyType`/`ResourceTypes`/`PolicyLanguage`/`ResourceType` as the members
//! an update may not change ("You cannot update the policy type or the resource
//! type").
//!
//! The rows span the class rather than the request an issue cited: every policy
//! kind, each required member of each kind alone and all of them at once, the
//! layer ahead of the handler (a `@required` member of the request and one
//! nested inside a list element) beside the handler's own cross-member rules,
//! both operations, and — for the immutability wording — every member of
//! `PolicyTypeValues` and `ResourceTypeValues` as the value asked for, plus a
//! request that asks to change two immutable parameters at once. Every refusal
//! is checked to have written nothing, and the accepted configuration of each
//! kind is read back.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "ap-southeast-2";

/// A destination Region for the event-based policy's copy action, matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "eu-west-1";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// Every `PolicyTypeValues` member, from
/// `smithy_query(query="shape", shape="PolicyTypeValues")`.
const POLICY_TYPE_VALUES: &[&str] = &[
    "EBS_SNAPSHOT_MANAGEMENT",
    "IMAGE_MANAGEMENT",
    "EVENT_BASED_POLICY",
];

/// Every `ResourceTypeValues` member, from
/// `smithy_query(query="shape", shape="ResourceTypeValues")`.
const RESOURCE_TYPE_VALUES: &[&str] = &["VOLUME", "INSTANCE"];

/// Both `PolicyLanguageValues` members, from
/// `smithy_query(query="shape", shape="PolicyLanguageValues")`: `SIMPLIFIED`
/// "To create a default policy", `STANDARD` "To create a custom policy".
const SIMPLIFIED: &str = "SIMPLIFIED";
const STANDARD: &str = "STANDARD";

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
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["Message"].as_str().unwrap_or_default()
    }

    /// The `RequiredParameters` the refusal reported, empty when it reported
    /// none.
    fn required_parameters(&self) -> Vec<String> {
        self.body["RequiredParameters"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
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

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/policies", Some(request)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/policies/{id}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

/// How many policies the store holds, read through `GetLifecyclePolicies`.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    call(router, "GET", "/policies", None).await.body["Policies"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// `text` as a `PolicyDescription`: the member is `@pattern(^[0-9A-Za-z _-]+$)`,
/// `@length(max: 500)`, so a case name is spelled in the characters it admits
/// and clipped to its bound. These rows are about the refusal's WORDING, so
/// their description must never be what is refused.
fn policy_description(text: &str) -> String {
    let spelled: String = text
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ' ' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .take(500)
        .collect();
    if spelled.is_empty() {
        "a policy".to_string()
    } else {
        spelled
    }
}

/// A create request carrying `configuration` on top of the members the model
/// marks `@required`.
fn create_request(description: &str, configuration: Value) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": policy_description(description),
        "State": "ENABLED",
    });
    for (member, value) in configuration
        .as_object()
        .expect("a configuration is a JSON object")
    {
        request[member] = value.clone();
    }
    request
}

/// A schedule stating one form of each of its rules.
fn good_schedule(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
        "RetainRule": {"Count": 7},
    })
}

/// A complete custom snapshot policy configuration targeting `target`.
fn custom_snapshot_details(target: &str) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        // `ResourceTypeValuesList` is `@length(min: 1, max: 1)`, so a custom
        // policy targets exactly one resource type.
        "ResourceTypes": [target],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [good_schedule("daily")],
    })
}

/// A complete custom EBS-backed AMI policy configuration.
fn custom_image_details() -> Value {
    json!({
        "PolicyType": "IMAGE_MANAGEMENT",
        "ResourceTypes": ["INSTANCE"],
        "TargetTags": [{"Key": "backup", "Value": "weekly"}],
        "Schedules": [good_schedule("weekly")],
    })
}

/// A complete event-based policy configuration.
fn event_based_details() -> Value {
    json!({
        "PolicyType": "EVENT_BASED_POLICY",
        "EventSource": {
            "Type": "MANAGED_CWE",
            "Parameters": {
                "EventType": "shareSnapshot",
                "SnapshotOwner": ["000000000000"],
                "DescriptionRegex": "^.*$",
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

/// A default policy's configuration: its own language and target, and a cadence
/// inside the documented ranges.
fn default_policy_details(target: &str) -> Value {
    json!({
        "PolicyLanguage": SIMPLIFIED,
        "ResourceType": target,
        "CreateInterval": 3,
        "RetainInterval": 14,
    })
}

/// `details` with `member` removed, as a client that simply left it out sends it.
fn without(mut details: Value, member: &str) -> Value {
    details
        .as_object_mut()
        .expect("a configuration is a JSON object")
        .remove(member);
    details
}

/// `details` with `member` set to `value`.
fn stating(details: &Value, member: &str, value: Value) -> Value {
    let mut details = details.clone();
    details[member] = value;
    details
}

/// The wording a refusal owes a request that omitted `parameters`.
fn missing_message(parameters: &[&str]) -> String {
    format!(
        "The following required parameter(s) are missing: {{{}}}",
        parameters.join(", ")
    )
}

/// The wording a refusal owes an update that asked to change `parameters`.
fn not_updatable_message(parameters: &[&str]) -> String {
    format!(
        "The following parameter(s) cannot be updated: {}",
        parameters.join(", ")
    )
}

/// Assert `answer` is this service's modelled refusal: the status the model
/// gives the exception and the `Code` of its class.
fn assert_refusal(case: &str, answer: &Answer) {
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
    assert_eq!(
        at(&answer.body, "/Code"),
        json!(INVALID_PARAMETER_CODE),
        "case {case:?}: expected Code {INVALID_PARAMETER_CODE:?}, got {:?}",
        at(&answer.body, "/Code")
    );
}

/// Assert `answer` is the missing-required-parameter refusal for `parameters`:
/// the wording, and the same names in `RequiredParameters`.
fn assert_missing(case: &str, answer: &Answer, parameters: &[&str]) {
    assert_refusal(case, answer);
    let expected = missing_message(parameters);
    assert_eq!(
        answer.message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        answer.message()
    );
    let reported = answer.required_parameters();
    let names: Vec<String> = parameters
        .iter()
        .map(|parameter| (*parameter).to_string())
        .collect();
    assert_eq!(
        reported, names,
        "case {case:?}: expected RequiredParameters {names:?}, got {reported:?}"
    );
}

/// Assert `answer` is the non-updatable-parameter refusal for `parameters`: the
/// wording, neither structured parameter list, and none of `values` — what the
/// request stated and what the policy holds — anywhere in the message.
fn assert_not_updatable(case: &str, answer: &Answer, parameters: &[&str], values: &[&str]) {
    assert_refusal(case, answer);
    let expected = not_updatable_message(parameters);
    assert_eq!(
        answer.message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        answer.message()
    );
    for member in ["RequiredParameters", "MutuallyExclusiveParameters"] {
        let reported = at(&answer.body, &format!("/{member}"));
        assert_eq!(
            reported,
            Value::Null,
            "case {case:?}: an immutable parameter is neither omitted nor combined, so {member} \
             must be absent, got {reported:?}"
        );
    }
    for value in values {
        assert!(
            !answer.message().contains(value),
            "case {case:?}: the refusal must carry no value, got {value:?} in {:?}",
            answer.message()
        );
    }
}

/// One request that omits required parameters, and the parameters a refusal owes
/// it.
struct Missing {
    case: String,
    /// The members the request carries beside the three the model marks
    /// `@required`, or an empty object for a request that carries none.
    configuration: Value,
    /// Whether the request also omits the `@required` `Description`, which the
    /// layer ahead of the handler is the one to refuse.
    omit_description: bool,
    /// The COMPLETE configuration of the same policy kind, for the row to be
    /// driven through `UpdateLifecyclePolicy` against a policy of that kind;
    /// `None` for a row about a member of the request itself, which an update
    /// has nothing to omit.
    complete: Option<Value>,
    /// The parameters the refusal must name, in the order it lists them.
    parameters: Vec<&'static str>,
}

impl Missing {
    fn new(
        case: String,
        configuration: Value,
        complete: Option<Value>,
        parameters: Vec<&'static str>,
    ) -> Self {
        Self {
            case,
            configuration,
            omit_description: false,
            complete,
            parameters,
        }
    }

    /// The request as a client sends it.
    fn request(&self) -> Value {
        let request = create_request(&self.case, self.configuration.clone());
        if self.omit_description {
            without(request, "Description")
        } else {
            request
        }
    }
}

/// Every way a policy write can leave out a parameter it is required to state.
fn missing_rows() -> Vec<Missing> {
    let mut rows = vec![
        // Neither of the two documented ways of saying what the policy would do.
        // An update states no configuration without asking to change one, so
        // this row is the create's own.
        Missing::new(
            "a create stating neither DefaultPolicy nor PolicyDetails".to_string(),
            json!({}),
            None,
            vec!["PolicyDetails"],
        ),
        // The layer ahead of the handler, on a `@required` member of the
        // request itself.
        Missing {
            case: "a create omitting the required Description".to_string(),
            configuration: json!({"PolicyDetails": custom_snapshot_details("VOLUME")}),
            omit_description: true,
            complete: None,
            parameters: vec!["Description"],
        },
        // And the same layer on a `@required` member nested in a list element,
        // which the name has to say WHICH element of: `Tag$Key`.
        Missing::new(
            "a create whose second TargetTags element states no Key".to_string(),
            json!({"PolicyDetails": stating(
                &custom_snapshot_details("VOLUME"),
                "TargetTags",
                json!([{"Key": "backup", "Value": "daily"}, {"Value": "keyless"}]),
            )}),
            Some(custom_snapshot_details("VOLUME")),
            vec!["PolicyDetails.TargetTags[2].Key"],
        ),
    ];

    // The handler's own rule, over every policy kind and every member each kind
    // is built from: one at a time, and all of them at once.
    let kinds: Vec<(&str, Value, Vec<&'static str>)> = vec![
        (
            "a custom snapshot policy",
            custom_snapshot_details("VOLUME"),
            vec!["ResourceTypes", "TargetTags", "Schedules"],
        ),
        (
            "a custom AMI policy",
            custom_image_details(),
            vec!["ResourceTypes", "TargetTags", "Schedules"],
        ),
        (
            "an event-based policy",
            event_based_details(),
            vec!["EventSource", "Actions"],
        ),
    ];
    for (kind, complete, members) in kinds {
        for member in &members {
            rows.push(Missing::new(
                format!("{kind} with no {member}"),
                json!({"PolicyDetails": without(complete.clone(), member)}),
                Some(complete.clone()),
                vec![member],
            ));
        }
        let mut stripped = complete.clone();
        for member in &members {
            stripped = without(stripped, member);
        }
        rows.push(Missing::new(
            format!("{kind} with none of the members it is built from"),
            json!({"PolicyDetails": stripped}),
            Some(complete.clone()),
            members.clone(),
        ));
    }

    rows
}

/// RULE, first wording — a request that omitted required parameters is refused
/// with the missing-required-parameter template, naming what it left out, on
/// both operations, and nothing is written.
#[tokio::test]
async fn an_omitted_parameter_is_refused_with_the_missing_parameter_wording() {
    let router = emulator();

    for row in missing_rows() {
        let before = policy_count(&router).await;

        let case = format!("{} (create)", row.case);
        let answer = create(&router, row.request()).await;
        assert_missing(&case, &answer, &row.parameters);
        assert_eq!(
            policy_count(&router).await,
            before,
            "case {case:?}: a refused create must leave no policy behind"
        );

        // The same row through the update, against a stored policy of its own
        // kind — which also proves the row is refused for the member it is
        // about and not for the configuration around it, since the complete
        // configuration it is stripped from is accepted.
        let Some(complete) = row.complete.as_ref() else {
            continue;
        };
        let subject = create(
            &router,
            create_request(
                &format!("{} subject", row.case),
                json!({"PolicyDetails": complete.clone()}),
            ),
        )
        .await;
        assert_eq!(
            subject.status, 200,
            "case {case:?}: the subject policy must be created, got {} with {:?}",
            subject.status, subject.body
        );
        let id = subject.policy_id();
        let stored = at(get(&router, &id).await.policy(), "/PolicyDetails");

        let case = format!("{} (update)", row.case);
        let answer = update(&router, &id, row.request()).await;
        assert_missing(&case, &answer, &row.parameters);
        assert_eq!(
            at(get(&router, &id).await.policy(), "/PolicyDetails"),
            stored,
            "case {case:?}: a refused update must leave the stored configuration alone"
        );
    }

    // The positive control: the complete configuration of each kind is accepted
    // and read back, so no row above is passing for the wrong reason.
    for (kind, complete) in [
        (
            "a custom snapshot policy",
            custom_snapshot_details("INSTANCE"),
        ),
        ("a custom AMI policy", custom_image_details()),
        ("an event-based policy", event_based_details()),
        ("a default policy", default_policy_details("VOLUME")),
    ] {
        let accepted = create(
            &router,
            create_request(
                &format!("{kind} complete"),
                json!({"PolicyDetails": complete}),
            ),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {kind:?}: the complete configuration must be accepted, got {} with {:?}",
            accepted.status, accepted.body
        );
        let read = get(&router, &accepted.policy_id()).await;
        assert_eq!(
            read.status, 200,
            "case {kind:?}: the accepted policy must be readable, got {} with {:?}",
            read.status, read.body
        );
        assert_ne!(
            at(read.policy(), "/PolicyDetails"),
            Value::Null,
            "case {kind:?}: an accepted policy must report a configuration"
        );
    }
}

/// One stored policy and the replacement configuration an update sends it.
struct Immutable {
    case: String,
    /// The create request that makes the policy.
    create: Value,
    /// The replacement configuration the update states.
    replacement: Value,
    /// The parameters the refusal must name.
    parameters: Vec<&'static str>,
    /// The values that must appear nowhere in the message: what the request
    /// stated and what the policy holds.
    values: Vec<String>,
}

/// Every way an update can ask to change a parameter fixed at creation.
fn immutable_rows() -> Vec<Immutable> {
    let mut rows: Vec<Immutable> = Vec::new();

    // `PolicyType`, over every member of the catalog other than the policy's
    // own, on a custom snapshot policy.
    for stated in POLICY_TYPE_VALUES
        .iter()
        .filter(|stated| **stated != "EBS_SNAPSHOT_MANAGEMENT")
    {
        rows.push(Immutable {
            case: format!("a custom snapshot policy asked to become a {stated} policy"),
            create: create_request(
                "policy type subject",
                json!({"PolicyDetails": custom_snapshot_details("VOLUME")}),
            ),
            replacement: stating(
                &custom_snapshot_details("VOLUME"),
                "PolicyType",
                json!(stated),
            ),
            parameters: vec!["PolicyType"],
            values: vec![(*stated).to_string(), "EBS_SNAPSHOT_MANAGEMENT".to_string()],
        });
    }

    // `ResourceTypes`, over every member of the catalog other than the one a
    // custom policy targets.
    for target in RESOURCE_TYPE_VALUES {
        for stated in RESOURCE_TYPE_VALUES.iter().filter(|stated| stated != &target) {
            rows.push(Immutable {
                case: format!("a custom {target} policy asked to target {stated}"),
                create: create_request(
                    &format!("target subject {target}"),
                    json!({"PolicyDetails": custom_snapshot_details(target)}),
                ),
                replacement: stating(
                    &custom_snapshot_details(target),
                    "ResourceTypes",
                    json!([stated]),
                ),
                parameters: vec!["ResourceTypes"],
                values: vec![(*stated).to_string(), (*target).to_string()],
            });
        }
    }

    // `PolicyLanguage`, in both directions: a custom policy asked for the
    // default-policy language and a default policy asked for the custom one.
    rows.push(Immutable {
        case: format!("a custom policy asked to be a {SIMPLIFIED} configuration"),
        create: create_request(
            "language subject custom",
            json!({"PolicyDetails": custom_snapshot_details("VOLUME")}),
        ),
        replacement: stating(
            &custom_snapshot_details("VOLUME"),
            "PolicyLanguage",
            json!(SIMPLIFIED),
        ),
        parameters: vec!["PolicyLanguage"],
        values: vec![SIMPLIFIED.to_string(), STANDARD.to_string()],
    });
    rows.push(Immutable {
        case: format!("a default policy asked to be a {STANDARD} configuration"),
        create: create_request(
            "language subject default",
            json!({"PolicyDetails": default_policy_details("VOLUME")}),
        ),
        replacement: stating(
            &default_policy_details("VOLUME"),
            "PolicyLanguage",
            json!(STANDARD),
        ),
        parameters: vec!["PolicyLanguage"],
        values: vec![SIMPLIFIED.to_string(), STANDARD.to_string()],
    });

    // `ResourceType`, the member a DEFAULT policy states its target as, over
    // every member of the catalog other than its own.
    for target in RESOURCE_TYPE_VALUES {
        for stated in RESOURCE_TYPE_VALUES.iter().filter(|stated| stated != &target) {
            rows.push(Immutable {
                case: format!("a default {target} policy asked to target {stated}"),
                create: create_request(
                    &format!("default target subject {target}"),
                    json!({"PolicyDetails": default_policy_details(target)}),
                ),
                replacement: stating(
                    &default_policy_details(target),
                    "ResourceType",
                    json!(stated),
                ),
                parameters: vec!["ResourceType"],
                values: vec![(*stated).to_string(), (*target).to_string()],
            });
        }
    }

    // A replacement configuration describing a whole policy of another kind
    // changes several of those members at once — the request the issue
    // reproduced states another `PolicyType` beside the `ResourceTypes` that
    // type targets — and is refused on the first parameter it may not change.
    rows.push(Immutable {
        case: "a custom snapshot policy asked to become an AMI policy targeting INSTANCE"
            .to_string(),
        create: create_request(
            "whole kind subject",
            json!({"PolicyDetails": custom_snapshot_details("VOLUME")}),
        ),
        replacement: custom_image_details(),
        parameters: vec!["PolicyType"],
        values: vec![
            "IMAGE_MANAGEMENT".to_string(),
            "EBS_SNAPSHOT_MANAGEMENT".to_string(),
            "VOLUME".to_string(),
            "INSTANCE".to_string(),
        ],
    });

    rows
}

/// RULE, second wording — an update that asks to change a parameter fixed at
/// creation is refused with the non-updatable-parameter template, naming the
/// parameter and nothing else, and the stored policy is left as it was.
#[tokio::test]
async fn an_immutable_parameter_is_refused_with_the_not_updatable_wording() {
    let router = emulator();

    for row in immutable_rows() {
        let case = row.case.as_str();
        let created = create(&router, row.create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the subject policy must be created, got {} with {:?}",
            created.status, created.body
        );
        let id = created.policy_id();
        let before = get(&router, &id).await;

        let answer = update(&router, &id, json!({"PolicyDetails": row.replacement.clone()})).await;
        let values: Vec<&str> = row.values.iter().map(String::as_str).collect();
        assert_not_updatable(case, &answer, &row.parameters, &values);

        let after = get(&router, &id).await;
        assert_eq!(
            after.policy(),
            before.policy(),
            "case {case:?}: a refused update must leave the stored policy exactly as it was"
        );

        // The positive control for the same row: the policy's OWN configuration
        // restated asks for no change, so it is accepted and the configuration
        // it reports is the one it held ("The local date and time when the
        // lifecycle policy was last modified" is what an accepted update does
        // move).
        let accepted = update(
            &router,
            &id,
            json!({"PolicyDetails": row.create["PolicyDetails"].clone()}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the policy's own configuration restated must be accepted, got {} with \
             {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            at(get(&router, &id).await.policy(), "/PolicyDetails"),
            at(before.policy(), "/PolicyDetails"),
            "case {case:?}: restating the stored configuration must change nothing"
        );
    }
}
