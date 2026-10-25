// SPDX-License-Identifier: CC0-1.0

//! Which members of `InvalidRequestException` a refused request fills in, end to
//! end over HTTP.
//!
//! The rule under test: the exception's structured parameter lists name the
//! REQUEST's own parameters — `RequiredParameters` is "the request omitted one or
//! more required parameters" and `MutuallyExclusiveParameters` is "the request
//! included parameters that cannot be provided together"
//! (`smithy_query(query="shape", shape="InvalidRequestException")`) — so a
//! refusal of which FORM a rule nested inside the configuration a parameter
//! carries was stated in fills NEITHER list: it is carried by `Message` and
//! `Code` alone, beside the status the model gives the exception. A conflict
//! between the request's own parameters still names them.
//!
//! The nested either/or rules, neither of them expressible in the model:
//!
//! * `CreateRule` — "You must specify either CronExpression, or Interval,
//!   IntervalUnit, and Times" (`smithy_query(query="doc", shape="CreateRule")`).
//! * `RetainRule` — "You can retain snapshots based on either a count or a time
//!   interval. Count-based retention: You must specify Count ... Age-based
//!   retention: You must specify Interval and IntervalUnit"
//!   (`smithy_query(query="doc", shape="RetainRule")`).
//!
//! The request-parameter rule it is contrasted with:
//!
//! * `CreateLifecyclePolicy` — "If you create a default policy, you can specify
//!   the request parameters either in the request body, or in the PolicyDetails
//!   request structure, but not both"
//!   (`smithy_query(query="operation", operation="CreateLifecyclePolicy")`).
//!
//! The table spans the class rather than the one request an issue cited: both
//! nested rules, every way each one's two forms can be stated together (each
//! member of the interval form beside the cron form, and a count beside each
//! member of the time interval), a named and an unnamed schedule, the first and a
//! later element of `Schedules`, and both `CreateLifecyclePolicy` and
//! `UpdateLifecyclePolicy`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The two structured parameter lists `InvalidRequestException` declares.
const PARAMETER_LISTS: [&str; 2] = ["RequiredParameters", "MutuallyExclusiveParameters"];

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

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
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

/// `text` as a `PolicyDescription`: the member is `@pattern(^[0-9A-Za-z _-]+$)`,
/// `@length(max: 500)`, so a case name is spelled in the characters it admits and
/// clipped to its bound. These rows are about the error PAYLOAD, so their
/// description must never be what is refused.
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

/// A complete custom EBS snapshot configuration carrying `schedules`.
fn details_with(schedules: Vec<Value>) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": Value::Array(schedules),
    })
}

/// A schedule stating exactly one form of each of its two rules — the leading
/// element the later-position rows sit behind.
fn good_schedule() -> Value {
    json!({
        "Name": "leading",
        "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 1},
    })
}

/// Assert `answer` is this service's modelled refusal: its status, its `Code` and
/// a message saying why.
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

/// Assert the refusal carries NEITHER structured parameter list.
fn assert_no_parameter_lists(case: &str, answer: &Answer) {
    assert_refusal(case, answer);
    for member in PARAMETER_LISTS {
        let reported = at(&answer.body, &format!("/{member}"));
        assert_eq!(
            reported,
            Value::Null,
            "case {case:?}: {member} must be absent for a nested form conflict, got {reported:?}"
        );
    }
}

/// One schedule whose rules state both of their alternative forms.
struct Conflict {
    case: &'static str,
    /// The schedule as a client sends it, without a `Name`: the driver decides
    /// whether this row's schedule carries one.
    schedule: Value,
}

/// Every way a schedule can state both forms of one of its two rules.
fn conflicts() -> Vec<Conflict> {
    vec![
        Conflict {
            case: "a CreateRule stating the cron form beside an Interval",
            schedule: json!({
                "CreateRule": {"CronExpression": "cron(0 12 * * ? *)", "Interval": 12},
                "RetainRule": {"Count": 1},
            }),
        },
        Conflict {
            case: "a CreateRule stating the cron form beside an IntervalUnit",
            schedule: json!({
                "CreateRule": {
                    "CronExpression": "cron(30 1 * * ? *)",
                    "IntervalUnit": CREATE_INTERVAL_UNIT,
                },
                "RetainRule": {"Count": 2},
            }),
        },
        Conflict {
            case: "a CreateRule stating the cron form beside Times",
            schedule: json!({
                "CreateRule": {"CronExpression": "cron(0 6 1 * ? *)", "Times": ["06:00"]},
                "RetainRule": {"Count": 3},
            }),
        },
        Conflict {
            // The request the issue reproduced: every member of the interval
            // form stated beside the cron form.
            case: "a CreateRule stating the cron form beside a complete interval form",
            schedule: json!({
                "CreateRule": {
                    "CronExpression": "cron(15 23 * * ? *)",
                    "Interval": 24,
                    "IntervalUnit": CREATE_INTERVAL_UNIT,
                    "Times": ["23:15"],
                },
                "RetainRule": {"Count": 4},
            }),
        },
        Conflict {
            case: "a RetainRule stating a Count beside a complete time interval",
            schedule: json!({
                "CreateRule": {"Interval": 12, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 5, "Interval": 7, "IntervalUnit": "DAYS"},
            }),
        },
        Conflict {
            case: "a RetainRule stating a Count beside a retention unit",
            schedule: json!({
                "CreateRule": {"Interval": 8, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 6, "IntervalUnit": "MONTHS"},
            }),
        },
        Conflict {
            case: "a RetainRule stating a Count beside a retention interval",
            schedule: json!({
                "CreateRule": {"Interval": 1, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 7, "Interval": 100},
            }),
        },
    ]
}

/// RULE — a refusal of the FORM a rule nested in the configuration was stated in
/// carries neither structured parameter list.
///
/// Every row is driven as the only schedule and as the second of two, with and
/// without a `Schedule$Name`, through both the create and the update.
#[tokio::test]
async fn a_nested_rule_form_conflict_is_reported_without_a_parameter_list() {
    let router = emulator();

    // The subject of the update rows, holding a configuration the service
    // accepts — which also proves the rows below are refused for their rule and
    // not for the configuration around it.
    let subject = create(
        &router,
        create_request(
            "parameter list subject",
            json!({"PolicyDetails": details_with(vec![good_schedule()])}),
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
    assert_ne!(
        at(&stored, "/Schedules"),
        Value::Null,
        "the subject policy must report the schedule it was created with, got {stored:?}"
    );

    for row in conflicts() {
        for position in [1usize, 2] {
            for named in [false, true] {
                let mut schedule = row.schedule.clone();
                if named {
                    schedule["Name"] = json!("under-test");
                }
                let mut schedules = Vec::new();
                if position == 2 {
                    schedules.push(good_schedule());
                }
                schedules.push(schedule);
                let details = details_with(schedules);
                let naming = if named { "named" } else { "unnamed" };

                let case = format!("{} ({naming}, schedule {position}, create)", row.case);
                let answer = create(
                    &router,
                    create_request(&case, json!({"PolicyDetails": details.clone()})),
                )
                .await;
                assert_no_parameter_lists(&case, &answer);

                let case = format!("{} ({naming}, schedule {position}, update)", row.case);
                let answer = update(&router, &subject_id, json!({"PolicyDetails": details})).await;
                assert_no_parameter_lists(&case, &answer);
                assert_eq!(
                    at(get(&router, &subject_id).await.policy(), "/PolicyDetails"),
                    stored,
                    "case {case:?}: a refused update must leave the stored configuration alone"
                );
            }
        }
    }
}

/// The other half of the rule — a conflict between the REQUEST's own parameters
/// is what `MutuallyExclusiveParameters` names, and it names each of them.
///
/// The rows span the `[Default policies only]` parameters a default-policy
/// request can state in either of the two places, one at a time and all at once.
#[tokio::test]
async fn a_conflict_between_request_parameters_names_them() {
    let router = emulator();
    let body_parameters: Vec<(&str, Value)> = vec![
        ("CreateInterval", json!(3)),
        ("RetainInterval", json!(9)),
        ("CopyTags", json!(true)),
        ("ExtendDeletion", json!(false)),
        ("Exclusions", json!({"ExcludeBootVolumes": true})),
    ];

    let mut every = Vec::new();
    let mut cases: Vec<(String, Vec<(&str, Value)>)> = Vec::new();
    for (member, value) in &body_parameters {
        cases.push((
            format!("{member} stated in the body and in PolicyDetails"),
            vec![(*member, value.clone())],
        ));
        every.push((*member, value.clone()));
    }
    cases.push((
        "every parameter stated in both places".to_string(),
        every.clone(),
    ));

    for (case, stated) in cases {
        let mut details = json!({"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME"});
        let mut request = create_request(
            "default policy stated twice",
            json!({"DefaultPolicy": "VOLUME"}),
        );
        for (member, value) in &stated {
            details[*member] = value.clone();
            request[*member] = value.clone();
        }
        request["PolicyDetails"] = details;

        let answer = create(&router, request).await;
        assert_refusal(&case, &answer);
        let reported = at(&answer.body, "/MutuallyExclusiveParameters");
        for parameter in std::iter::once("PolicyDetails").chain(stated.iter().map(|(name, _)| *name))
        {
            assert!(
                reported
                    .as_array()
                    .is_some_and(|names| names.contains(&json!(parameter))),
                "case {case:?}: MutuallyExclusiveParameters must name {parameter:?}, got \
                 {reported:?}"
            );
        }
    }

    // And the request the rule leaves alone: the same parameters in ONE place are
    // accepted and read back.
    let mut details = json!({"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME"});
    for (member, value) in &every {
        details[*member] = value.clone();
    }
    let accepted = create(
        &router,
        create_request("default policy in one place", json!({"PolicyDetails": details})),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the configuration stated in one place must be accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
    let read = get(&router, &accepted.policy_id()).await;
    assert_eq!(
        read.status, 200,
        "the accepted policy must be readable, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        at(read.policy(), "/PolicyDetails/CreateInterval"),
        json!(3),
        "the read must report the creation frequency that was stated, got {:?}",
        at(read.policy(), "/PolicyDetails/CreateInterval")
    );
}
