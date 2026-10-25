// SPDX-License-Identifier: CC0-1.0

//! The configuration a lifecycle policy must carry, end to end over HTTP.
//!
//! The rule under test: a `CreateLifecyclePolicy` or `UpdateLifecyclePolicy`
//! request must state a COMPLETE policy configuration — every member the
//! documentation says the kind of policy it describes is built from, and, where
//! the documentation states two alternative forms, exactly one of them — and a
//! request that does not is refused with `InvalidRequestException` (HTTP 400,
//! `Code` `InvalidParameter`) before anything is stored or changed, naming the
//! omitted members in `RequiredParameters`. A refusal of the FORM a rule nested
//! inside the configuration was stated in carries neither structured parameter
//! list — those name the request's own parameters — so it is reported by its
//! message and code alone. A complete configuration is accepted and read back.
//!
//! None of these members can be `@required` in the model, because which ones a
//! request must carry depends on the kind of policy it configures. The
//! documentation is what states them:
//!
//! * `CreateLifecyclePolicy` — a policy is configured either by `DefaultPolicy`
//!   ("specify the type of default policy to create") or by `PolicyDetails`
//!   ("the configuration details of the lifecycle policy")
//!   (`smithy_query(query="operation", operation="CreateLifecyclePolicy")`).
//! * `PolicyDetails$ResourceTypes` — "The target resource type for snapshot and
//!   AMI lifecycle policies"; `PolicyDetails$TargetTags` — "The single tag that
//!   identifies targeted resources for this policy";
//!   `PolicyDetails$Schedules` — "A policy can have up to four schedules—one
//!   mandatory schedule and up to three optional schedules".
//! * `PolicyDetails$EventSource` — "[Event-based policies only] The event that
//!   activates the event-based policy"; `PolicyDetails$Actions` — "[Event-based
//!   policies only] The actions to be performed when the event-based policy is
//!   activated" (`smithy_query(query="shape", shape="PolicyDetails")`).
//! * `CreateRule` — "You must specify either CronExpression, or Interval,
//!   IntervalUnit, and Times" (`smithy_query(query="doc", shape="CreateRule")`).
//! * `RetainRule` — "You can retain snapshots based on either a count or a time
//!   interval. Count-based retention: You must specify Count ... Age-based
//!   retention: You must specify Interval and IntervalUnit"
//!   (`smithy_query(query="doc", shape="RetainRule")`).
//!
//! Each table spans the class rather than the requests an issue cited: every
//! policy kind, every member each kind is built from (one at a time and all at
//! once), both either/or forms of each rule and every incomplete form of each,
//! every member of `RetentionIntervalUnitValues`, both the first and a later
//! element of `Schedules`, and both the create and the update.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// A second region, for the cross-Region copy an event-based policy's action
/// targets.
const OTHER_REGION: &str = "eu-west-1";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// `RetentionIntervalUnitValues`, verbatim from the model
/// (`smithy_query(query="shape", shape="RetentionIntervalUnitValues")`). Every
/// entry is exercised as an age-based retention rule's unit; the issue cited
/// only `DAYS`.
const RETENTION_INTERVAL_UNITS: [&str; 4] = ["DAYS", "WEEKS", "MONTHS", "YEARS"];

/// `IntervalUnitValues` offers `HOURS` alone
/// (`smithy_query(query="shape", shape="IntervalUnitValues")`).
const CREATE_INTERVAL_UNIT: &str = "HOURS";

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
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
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

/// What the service owes one request.
#[derive(Debug, Clone)]
enum Owed {
    /// The configuration is complete, so the request is applied.
    Accepted,
    /// Refused, with these wire paths reported in `RequiredParameters`.
    Required(Vec<String>),
    /// Refused because a rule nested in the configuration stated BOTH of its two
    /// alternative forms: reported by the message and the code alone.
    BothForms,
}

/// Assert `answer` is this service's modelled refusal: the status and `Code` of
/// `InvalidRequestException`, and a message saying why.
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
    assert!(
        !answer.message().is_empty(),
        "case {case:?}: a refusal must say why, got {:?}",
        answer.body
    );
}

/// Assert the refusal leaves the `member` parameter list unset.
fn assert_no_parameter_list(case: &str, answer: &Answer, member: &str) {
    let reported = at(&answer.body, &format!("/{member}"));
    assert_eq!(
        reported,
        Value::Null,
        "case {case:?}: {member} must be absent, got {reported:?}"
    );
}

/// Assert `answer` is this service's modelled refusal and reports `parameters`
/// in the `member` list the class of refusal fills in, and nothing in the other
/// list.
fn assert_refused(case: &str, answer: &Answer, member: &str, parameters: &[String]) {
    assert_refusal(case, answer);
    let reported = at(&answer.body, &format!("/{member}"));
    for parameter in parameters {
        assert!(
            reported
                .as_array()
                .is_some_and(|names| names.contains(&json!(parameter))),
            "case {case:?}: {member} must name {parameter:?}, got {reported:?}"
        );
    }
    for other in ["RequiredParameters", "MutuallyExclusiveParameters"]
        .into_iter()
        .filter(|name| *name != member)
    {
        assert_no_parameter_list(case, answer, other);
    }
}

/// Assert `answer` satisfies `owed`, whatever class of refusal it is.
fn assert_owed(case: &str, answer: &Answer, owed: &Owed) {
    match owed {
        Owed::Accepted => assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        ),
        Owed::Required(parameters) => {
            assert_refused(case, answer, "RequiredParameters", parameters);
            // An omitted parameter's refusal is built from the request, so it
            // names what was omitted in its message too.
            for parameter in parameters {
                assert!(
                    answer.message().contains(parameter.as_str()),
                    "case {case:?}: the refusal must name {parameter:?}, got {:?}",
                    answer.message()
                );
            }
        }
        Owed::BothForms => {
            // Neither structured list: each names the request's own parameters,
            // and this refusal is about the form a nested rule was stated in.
            assert_refusal(case, answer);
            assert_no_parameter_list(case, answer, "RequiredParameters");
            assert_no_parameter_list(case, answer, "MutuallyExclusiveParameters");
        }
    }
}

/// The schedule every COMPLETE configuration in this file carries: the interval
/// form of the creation rule and count-based retention.
fn good_schedule(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 1},
    })
}

/// A complete custom EBS snapshot policy configuration.
fn custom_snapshot_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
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

/// A complete event-based policy configuration: an event that activates it and
/// the action it performs.
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

/// A create request carrying `configuration` members on top of the three the
/// model marks `@required`.
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

/// `text` as a `PolicyDescription`: the member is
/// `@pattern(^[0-9A-Za-z _-]+$)`, `@length(max: 500)`, so a case name is spelled
/// in the characters it admits and clipped to its bound. That constraint is the
/// boundary walk's to enforce (`error_paths.rs` pins it) — these rows are about
/// the CONFIGURATION, so their description must never be what is refused.
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

/// `details` with `member` removed, as a client that simply left it out sends it.
fn without(mut details: Value, member: &str) -> Value {
    details
        .as_object_mut()
        .expect("a configuration is a JSON object")
        .remove(member);
    details
}

/// RULE, first half — a create states what the policy would do, in one of the
/// two documented ways, or it is refused.
///
/// The rows span: neither way stated, each `DefaultPolicyTypeValues` member, a
/// default policy stated as a `SIMPLIFIED` configuration, and a complete
/// configuration of each custom policy kind.
#[tokio::test]
async fn a_create_must_state_what_the_policy_would_do() {
    let router = emulator();
    let cases: Vec<(&str, Value, Owed)> = vec![
        (
            // The request the issue reproduced.
            "neither DefaultPolicy nor PolicyDetails",
            json!({}),
            Owed::Required(vec!["PolicyDetails".to_string()]),
        ),
        (
            "a default policy for volumes",
            json!({"DefaultPolicy": "VOLUME"}),
            Owed::Accepted,
        ),
        (
            "a default policy for instances",
            json!({"DefaultPolicy": "INSTANCE"}),
            Owed::Accepted,
        ),
        (
            "a default policy stated as a SIMPLIFIED configuration",
            json!({"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME"}}),
            Owed::Accepted,
        ),
        (
            "a complete custom snapshot configuration",
            json!({"PolicyDetails": custom_snapshot_details()}),
            Owed::Accepted,
        ),
        (
            "a complete custom AMI configuration",
            json!({"PolicyDetails": custom_image_details()}),
            Owed::Accepted,
        ),
        (
            "a complete event-based configuration",
            json!({"PolicyDetails": event_based_details()}),
            Owed::Accepted,
        ),
    ];

    for (case, configuration, owed) in cases {
        let before = policy_count(&router).await;
        let answer = create(&router, create_request(case, configuration)).await;
        assert_owed(case, &answer, &owed);
        match owed {
            Owed::Accepted => {
                // Round trip: the accepted configuration is what a read reports.
                let read = get(&router, &answer.policy_id()).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the accepted policy must be readable, got {} with {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    at(read.policy(), "/Description"),
                    json!(policy_description(case)),
                    "case {case:?}: the read must report the policy that was created, got {:?}",
                    at(read.policy(), "/Description")
                );
                assert_ne!(
                    at(read.policy(), "/PolicyDetails"),
                    Value::Null,
                    "case {case:?}: an accepted policy must report a configuration"
                );
                assert_eq!(
                    policy_count(&router).await,
                    before + 1,
                    "case {case:?}: an accepted create must store its policy"
                );
            }
            _ => assert_eq!(
                policy_count(&router).await,
                before,
                "case {case:?}: a refused create must leave no policy behind"
            ),
        }
    }
}

/// RULE, second half — a custom configuration states every member the kind of
/// policy it describes is built from.
///
/// One row per required member of each policy kind, plus a row that omits every
/// one of them at once: the refusal names each member that was left out, and the
/// complete configuration of the same kind is accepted.
#[tokio::test]
async fn a_configuration_states_every_member_its_policy_kind_is_built_from() {
    let router = emulator();

    // (the kind, the complete configuration, the members that kind is built from)
    let kinds: Vec<(&str, Value, &[&str])> = vec![
        (
            "a custom snapshot policy",
            custom_snapshot_details(),
            &["ResourceTypes", "TargetTags", "Schedules"],
        ),
        (
            "a custom AMI policy",
            custom_image_details(),
            &["ResourceTypes", "TargetTags", "Schedules"],
        ),
        (
            "an event-based policy",
            event_based_details(),
            &["EventSource", "Actions"],
        ),
    ];

    for (kind, complete, members) in kinds {
        // The complete configuration of this kind is accepted, so no row below
        // can be passing for the wrong reason.
        let accepted = create(
            &router,
            create_request(
                &format!("{kind}, complete"),
                json!({"PolicyDetails": complete.clone()}),
            ),
        )
        .await;
        assert_owed(
            &format!("{kind}: the complete configuration"),
            &accepted,
            &Owed::Accepted,
        );

        for member in members {
            let case = format!("{kind} with no {member}");
            let before = policy_count(&router).await;
            let answer = create(
                &router,
                create_request(
                    &case,
                    json!({"PolicyDetails": without(complete.clone(), member)}),
                ),
            )
            .await;
            assert_owed(
                &case,
                &answer,
                &Owed::Required(vec![format!("PolicyDetails.{member}")]),
            );
            assert_eq!(
                policy_count(&router).await,
                before,
                "case {case:?}: a refused create must leave no policy behind"
            );
        }

        // Every one of them omitted at once is still ONE refusal, naming them all.
        let case = format!("{kind} with none of the members it is built from");
        let mut stripped = complete.clone();
        for member in members {
            stripped = without(stripped, member);
        }
        let answer = create(
            &router,
            create_request(&case, json!({"PolicyDetails": stripped})),
        )
        .await;
        assert_owed(
            &case,
            &answer,
            &Owed::Required(
                members
                    .iter()
                    .map(|member| format!("PolicyDetails.{member}"))
                    .collect(),
            ),
        );
    }
}

/// One schedule, and what the service owes the configuration carrying it.
struct RuleCase {
    case: &'static str,
    /// The schedule under test, named by the driver.
    schedule: Value,
    /// Member paths RELATIVE to the schedule a refusal must report in
    /// `RequiredParameters`.
    required: &'static [&'static str],
    /// Whether the schedule states BOTH alternative forms of one of its rules,
    /// which is refused by the message and the code alone.
    both_forms: bool,
}

impl RuleCase {
    /// What the service owes this row when the schedule sits at `position`
    /// (1-based) in `Schedules`.
    fn owed(&self, position: usize) -> Owed {
        if self.both_forms {
            return Owed::BothForms;
        }
        if !self.required.is_empty() {
            return Owed::Required(
                self.required
                    .iter()
                    .map(|member| format!("PolicyDetails.Schedules[{position}].{member}"))
                    .collect(),
            );
        }
        Owed::Accepted
    }
}

/// Run every row against the create (as the only schedule and as the second of
/// two) and against the update, asserting a refused request changes nothing.
async fn assert_rule_cases(cases: Vec<RuleCase>) {
    let router = emulator();

    // The subject of the update rows, holding a configuration the service
    // accepts, so a refused update can be seen to have changed nothing.
    let subject = create(
        &router,
        create_request(
            "schedule rule subject",
            json!({"PolicyDetails": custom_snapshot_details()}),
        ),
    )
    .await;
    assert_eq!(
        subject.status, 200,
        "the subject policy must be created, got {} with {:?}",
        subject.status, subject.body
    );
    let subject_id = subject.policy_id();
    let stored = at(get(&router, &subject_id).await.policy(), "/PolicyDetails");

    for row in cases {
        for position in [1usize, 2] {
            let mut schedules = Vec::new();
            if position == 2 {
                schedules.push(good_schedule("leading"));
            }
            let mut schedule = row.schedule.clone();
            schedule["Name"] = json!("under-test");
            schedules.push(schedule);
            let mut details = custom_snapshot_details();
            details["Schedules"] = Value::Array(schedules);
            let owed = row.owed(position);

            let case = format!("{} (as schedule {position} of a create)", row.case);
            let before = policy_count(&router).await;
            let answer = create(
                &router,
                create_request(&case, json!({"PolicyDetails": details.clone()})),
            )
            .await;
            assert_owed(&case, &answer, &owed);
            let expected = match owed {
                Owed::Accepted => before + 1,
                _ => before,
            };
            assert_eq!(
                policy_count(&router).await,
                expected,
                "case {case:?}: the store must hold {expected} policies afterwards"
            );

            let case = format!("{} (as schedule {position} of an update)", row.case);
            let answer = update(&router, &subject_id, json!({"PolicyDetails": details})).await;
            let owed = row.owed(position);
            assert_owed(&case, &answer, &owed);
            let read = at(get(&router, &subject_id).await.policy(), "/PolicyDetails");
            match owed {
                Owed::Accepted => assert_eq!(
                    at(&read, "/Schedules")
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or_default(),
                    position,
                    "case {case:?}: an accepted update must store its schedules, got {:?}",
                    at(&read, "/Schedules")
                ),
                _ => assert_eq!(
                    read, stored,
                    "case {case:?}: a refused update must leave the stored configuration alone"
                ),
            }
            // Leave the subject holding the accepted configuration again, so the
            // next row's refusal rows compare against a known state.
            let restored = update(
                &router,
                &subject_id,
                json!({"PolicyDetails": custom_snapshot_details()}),
            )
            .await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: the subject policy must be restored, got {} with {:?}",
                restored.status, restored.body
            );
        }
    }
}

/// A schedule states when to create in exactly one of the two documented forms.
///
/// The rows span: neither form, each form alone, each form with and without the
/// members it names, and the cron form beside each member of the interval form.
#[tokio::test]
async fn a_schedule_states_when_to_create_in_exactly_one_form() {
    assert_rule_cases(vec![
        RuleCase {
            case: "a schedule carrying no CreateRule at all",
            schedule: json!({"RetainRule": {"Count": 1}}),
            required: &["CreateRule"],
            both_forms: false,
        },
        RuleCase {
            // The request the issue reproduced.
            case: "an empty CreateRule",
            schedule: json!({"CreateRule": {}, "RetainRule": {"Count": 1}}),
            required: &["CreateRule"],
            both_forms: false,
        },
        RuleCase {
            case: "a CreateRule stating only a Location",
            schedule: json!({"CreateRule": {"Location": "CLOUD"}, "RetainRule": {"Count": 1}}),
            required: &["CreateRule"],
            both_forms: false,
        },
        RuleCase {
            case: "the interval form without its IntervalUnit",
            schedule: json!({"CreateRule": {"Interval": 12}, "RetainRule": {"Count": 1}}),
            required: &["CreateRule.IntervalUnit"],
            both_forms: false,
        },
        RuleCase {
            case: "the interval form without its Interval",
            schedule: json!({
                "CreateRule": {"IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 1},
            }),
            required: &["CreateRule.Interval"],
            both_forms: false,
        },
        RuleCase {
            case: "a CreateRule stating only Times",
            schedule: json!({"CreateRule": {"Times": ["03:00"]}, "RetainRule": {"Count": 1}}),
            required: &["CreateRule.Interval", "CreateRule.IntervalUnit"],
            both_forms: false,
        },
        RuleCase {
            case: "the cron form beside an Interval",
            schedule: json!({
                "CreateRule": {"CronExpression": "cron(0 12 * * ? *)", "Interval": 12},
                "RetainRule": {"Count": 1},
            }),
            required: &[],
            both_forms: true,
        },
        RuleCase {
            // The request the issue reproduced: every member of the interval
            // form beside the cron form.
            case: "the cron form beside a complete interval form",
            schedule: json!({
                "CreateRule": {
                    "CronExpression": "cron(0 12 * * ? *)",
                    "Interval": 12,
                    "IntervalUnit": CREATE_INTERVAL_UNIT,
                },
                "RetainRule": {"Count": 1},
            }),
            required: &[],
            both_forms: true,
        },
        RuleCase {
            case: "the cron form beside the Times the interval form may state",
            schedule: json!({
                "CreateRule": {"CronExpression": "cron(0 12 * * ? *)", "Times": ["03:00"]},
                "RetainRule": {"Count": 1},
            }),
            required: &[],
            both_forms: true,
        },
        RuleCase {
            case: "the cron form beside an IntervalUnit",
            schedule: json!({
                "CreateRule": {
                    "CronExpression": "cron(0 12 * * ? *)",
                    "IntervalUnit": CREATE_INTERVAL_UNIT,
                },
                "RetainRule": {"Count": 1},
            }),
            required: &[],
            both_forms: true,
        },
        RuleCase {
            case: "the interval form alone",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 1},
            }),
            required: &[],
            both_forms: false,
        },
        RuleCase {
            case: "the interval form with the Times it may also state",
            schedule: json!({
                "CreateRule": {
                    "Interval": 6,
                    "IntervalUnit": CREATE_INTERVAL_UNIT,
                    "Times": ["03:00"],
                },
                "RetainRule": {"Count": 1},
            }),
            required: &[],
            both_forms: false,
        },
        RuleCase {
            case: "the cron form alone",
            schedule: json!({
                "CreateRule": {"CronExpression": "cron(0 12 * * ? *)"},
                "RetainRule": {"Count": 1},
            }),
            required: &[],
            both_forms: false,
        },
    ])
    .await;
}

/// A retention rule retains by exactly one of the two documented modes.
///
/// The rows span: neither mode, each mode alone, the age-based mode missing each
/// of the two members it requires, both modes at once in each way they can be
/// stated, and the retention interval of `0` the documentation admits. The
/// absence of a `RetainRule` is not refused: the model declares no retention
/// rule for a schedule, and nothing in the documentation requires one.
#[tokio::test]
async fn a_retain_rule_retains_by_exactly_one_mode() {
    assert_rule_cases(vec![
        RuleCase {
            case: "a schedule carrying no RetainRule at all",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
            }),
            required: &[],
            both_forms: false,
        },
        RuleCase {
            // The request the issue reproduced.
            case: "an empty RetainRule",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {},
            }),
            required: &["RetainRule"],
            both_forms: false,
        },
        RuleCase {
            case: "count-based retention",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 3},
            }),
            required: &[],
            both_forms: false,
        },
        RuleCase {
            // `StandardTierRetainRuleCount` is `@range(min: 0, max: 1000)`, and a
            // count of 0 is documented ("you can specify a retention count of 0
            // to archive snapshots immediately after creation").
            case: "count-based retention at the floor of its range",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 0},
            }),
            required: &[],
            both_forms: false,
        },
        RuleCase {
            case: "count-based retention at the ceiling of its range",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 1000},
            }),
            required: &[],
            both_forms: false,
        },
        RuleCase {
            case: "age-based retention without its IntervalUnit",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Interval": 7},
            }),
            required: &["RetainRule.IntervalUnit"],
            both_forms: false,
        },
        RuleCase {
            case: "age-based retention without its Interval",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"IntervalUnit": "MONTHS"},
            }),
            required: &["RetainRule.Interval"],
            both_forms: false,
        },
        RuleCase {
            // The request the issue reproduced.
            case: "a count beside a complete time interval",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 3, "Interval": 7, "IntervalUnit": "DAYS"},
            }),
            required: &[],
            both_forms: true,
        },
        RuleCase {
            case: "a count beside a retention unit alone",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 3, "IntervalUnit": "WEEKS"},
            }),
            required: &[],
            both_forms: true,
        },
        RuleCase {
            case: "a count beside a retention interval alone",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 3, "Interval": 7},
            }),
            required: &[],
            both_forms: true,
        },
        RuleCase {
            // "If you specify an ArchiveRule for the schedule, then you can
            // specify a retention interval of 0 days": the interval's own
            // floor is a value the age-based mode admits.
            case: "age-based retention at the floor of its range",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Interval": 0, "IntervalUnit": "DAYS"},
            }),
            required: &[],
            both_forms: false,
        },
    ])
    .await;

    // One row per `RetentionIntervalUnitValues` member: the age-based mode is
    // accepted for every unit the model declares, not just the one the issue
    // cited.
    let router = emulator();
    for unit in RETENTION_INTERVAL_UNITS {
        let case = format!("age-based retention in {unit}");
        let mut details = custom_snapshot_details();
        details["Schedules"] = json!([{
            "Name": "aged",
            "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
            "RetainRule": {"Interval": 3, "IntervalUnit": unit},
        }]);
        let answer = create(
            &router,
            create_request(&case, json!({"PolicyDetails": details})),
        )
        .await;
        assert_owed(&case, &answer, &Owed::Accepted);
        let read = get(&router, &answer.policy_id()).await;
        assert_eq!(
            at(
                read.policy(),
                "/PolicyDetails/Schedules/0/RetainRule/IntervalUnit"
            ),
            json!(unit),
            "case {case:?}: the stored retention unit must be the one that was stated, got {:?}",
            at(
                read.policy(),
                "/PolicyDetails/Schedules/0/RetainRule/IntervalUnit"
            )
        );
    }
}

/// The configuration this service REPORTS for a policy is one it accepts back:
/// the members it materializes into a stored schedule never turn a rule into a
/// form the either/or checks refuse.
///
/// The rows span both retention modes, so the materialized retention interval of
/// a count-based rule is exercised beside an age-based rule that stated its own.
#[tokio::test]
async fn a_configuration_read_back_is_accepted_again() {
    let router = emulator();
    let retentions: Vec<(&str, Value)> = vec![
        ("count-based retention", json!({"Count": 7})),
        (
            "age-based retention",
            json!({"Interval": 2, "IntervalUnit": "WEEKS"}),
        ),
    ];

    for (case, retain_rule) in retentions {
        let mut details = custom_snapshot_details();
        details["Schedules"] = json!([{
            "Name": "round-trip",
            "CreateRule": {"Interval": 12, "IntervalUnit": CREATE_INTERVAL_UNIT},
            "RetainRule": retain_rule,
        }]);
        let created = create(
            &router,
            create_request(case, json!({"PolicyDetails": details})),
        )
        .await;
        assert_owed(case, &created, &Owed::Accepted);
        let id = created.policy_id();
        let reported = at(get(&router, &id).await.policy(), "/PolicyDetails");

        let answer = update(&router, &id, json!({"PolicyDetails": reported.clone()})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the configuration the service reports must be one it accepts, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(get(&router, &id).await.policy(), "/PolicyDetails"),
            reported,
            "case {case:?}: re-sending the reported configuration must leave it unchanged"
        );
    }
}
