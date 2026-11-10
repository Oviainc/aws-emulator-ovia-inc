// SPDX-License-Identifier: CC0-1.0

//! The form every either/or rule nested in a schedule must be stated in, end to
//! end over HTTP.
//!
//! The rule under test: a rule nested in a `PolicyDetails$Schedules` element
//! whose documentation offers two alternative forms — a count of resources, or a
//! time interval stated as an `Interval` and an `IntervalUnit` — must state
//! exactly one of those forms and state it completely, and where the
//! documentation names one form per retention mode, the form it states must be
//! the mode the schedule retains by; a `CreateLifecyclePolicy` or
//! `UpdateLifecyclePolicy` request whose schedule states neither form, both
//! forms, an incomplete form or the other mode's form is refused with
//! `InvalidRequestException` (HTTP 400, `Code` `InvalidParameter`) before
//! anything is stored or changed. The refusal of an omitted member names it in
//! `RequiredParameters`; the refusal of a doubly stated rule carries neither
//! structured parameter list, since each of those names the request's own
//! parameters and this is a rule about a structure nested inside one of them.
//!
//! None of it is expressible in the model: every one of these members is
//! optional, and which of them a rule must carry depends on the form it is
//! stated in and on the schedule around it. The documentation is what states it:
//!
//! * `FastRestoreRule` — "You can enable fast snapshot restore based on either a
//!   count or a time interval"
//!   (`smithy_query(query="doc", shape="FastRestoreRule")`).
//! * `DeprecateRule` — "For age-based schedules, you must specify Interval and
//!   IntervalUnit. For count-based schedules, you must specify Count"
//!   (`smithy_query(query="doc", shape="DeprecateRule")`).
//! * `RetentionArchiveTier`, nested in a schedule's `ArchiveRule` — "For
//!   count-based schedules, you must specify Count. For age-based schedules, you
//!   must specify Interval and IntervalUnit"
//!   (`smithy_query(query="doc", shape="RetentionArchiveTier")`).
//! * `RetainRule` — "You can retain snapshots based on either a count or a time
//!   interval. Count-based retention: You must specify Count ... Age-based
//!   retention: You must specify Interval and IntervalUnit"
//!   (`smithy_query(query="doc", shape="RetainRule")`), the parallel sentence
//!   whose enforcement these rows are driven beside, so one walk answers them
//!   all the same way.
//!
//! The tables span the class rather than the one request the issue cited: every
//! rule carrying such a sentence, each of its two forms, each form with every
//! member it names omitted in turn, the rule stating neither form and the rule
//! stating both in each way both can be stated, every member of
//! `RetentionIntervalUnitValues`, each bound of the count's range, each retention
//! mode a schedule can decide a form by (and the schedule that states no
//! retention at all), both the first and a later element of `Schedules`, and both
//! the create and the update.

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

/// `RetentionIntervalUnitValues`, verbatim from the model
/// (`smithy_query(query="shape", shape="RetentionIntervalUnitValues")`). Every
/// entry is exercised as an interval form's unit; the issue cited only `DAYS`.
const RETENTION_INTERVAL_UNITS: [&str; 4] = ["DAYS", "WEEKS", "MONTHS", "YEARS"];

/// `IntervalUnitValues` offers `HOURS` alone
/// (`smithy_query(query="shape", shape="IntervalUnitValues")`).
const CREATE_INTERVAL_UNIT: &str = "HOURS";

/// A creation rule that creates one snapshot a month, the frequency a schedule
/// carrying an `ArchiveRule` must run at least as rarely as (see
/// [`create_rule_for`]).
const MONTHLY_CRON_EXPRESSION: &str = "cron(0 0 1 * ? *)";

/// The bounds of `Count` (`@range(min: 1, max: 1000)`), which is what
/// `FastRestoreRule$Count`, `DeprecateRule$Count` and
/// `RetentionArchiveTier$Count` target
/// (`smithy_query(query="shape", shape="FastRestoreRule")`).
const COUNT_FLOOR: i64 = 1;
const COUNT_CEILING: i64 = 1000;

/// The bound of `Interval` (`@range(min: 1)`), the shape those rules' intervals
/// target.
const INTERVAL_FLOOR: i64 = 1;

/// How the schedule under test is named, so a refusal can be seen to blame it.
const SUBJECT_SCHEDULE: &str = "under-test";

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

/// True when `stored` carries everything `stated` states: every member of an
/// object and every element of a list, compared the same way. Members the
/// service MATERIALIZES on top of what a request stated (a schedule's
/// `CopyTags`, a rule interval's zero) are left out of the comparison, since
/// this file is about the form a rule was stated in.
fn carries(stored: &Value, stated: &Value) -> bool {
    match (stored, stated) {
        (Value::Object(stored), Value::Object(stated)) => stated
            .iter()
            .all(|(member, value)| stored.get(member).is_some_and(|held| carries(held, value))),
        (Value::Array(stored), Value::Array(stated)) => {
            stored.len() == stated.len()
                && stored
                    .iter()
                    .zip(stated)
                    .all(|(held, value)| carries(held, value))
        }
        (held, value) => held == value,
    }
}

/// A create request carrying `configuration` on top of the three members the
/// model marks `@required`. The description is spelled in the characters
/// `PolicyDescription`'s `@pattern(^[0-9A-Za-z _-]+$)` admits and clipped to its
/// `@length(max: 500)`, so a case name is never what is refused.
fn create_request(description: &str, configuration: Value) -> Value {
    let spelled: String = description
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
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": spelled,
        "State": "ENABLED",
        "PolicyDetails": configuration,
    })
}

/// The kind of policy whose documentation declares the rule under test:
/// `FastRestoreRule` and `ArchiveRule` are `[Custom snapshot policies only]`,
/// `DeprecateRule` is `[Custom AMI policies only]`
/// (`smithy_query(query="shape", shape="Schedule")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyKind {
    Snapshot,
    Image,
}

impl PolicyKind {
    /// A complete configuration of this kind, carrying `schedules`.
    fn details(self, schedules: Vec<Value>) -> Value {
        match self {
            PolicyKind::Snapshot => json!({
                "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
                "ResourceTypes": ["VOLUME"],
                "TargetTags": [{"Key": "backup", "Value": "daily"}],
                "Schedules": schedules,
            }),
            PolicyKind::Image => json!({
                "PolicyType": "IMAGE_MANAGEMENT",
                "ResourceTypes": ["INSTANCE"],
                "TargetTags": [{"Key": "backup", "Value": "daily"}],
                "Schedules": schedules,
            }),
        }
    }
}

/// A schedule this service accepts: the interval form of the creation rule and
/// count-based retention.
fn good_schedule(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 2},
    })
}

/// The retention mode a schedule states, which is what decides the form of the
/// rules documented one form per mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// "Count-based retention: You must specify Count."
    Count,
    /// "Age-based retention: You must specify Interval and IntervalUnit."
    Age,
    /// A schedule stating no retention rule at all, which the service accepts
    /// while no other rule of the schedule acts on the snapshots it retains.
    None,
}

impl Mode {
    /// The `RetainRule` a schedule in this mode states, if any.
    fn retain_rule(self) -> Option<Value> {
        match self {
            Mode::Count => Some(json!({"Count": 7})),
            Mode::Age => Some(json!({"Interval": 30, "IntervalUnit": "DAYS"})),
            Mode::None => None,
        }
    }

    /// How a case name reads the mode.
    fn described(self) -> &'static str {
        match self {
            Mode::Count => "count based retention",
            Mode::Age => "age based retention",
            Mode::None => "a schedule that states no retention",
        }
    }
}

/// What the service owes one request.
#[derive(Debug, Clone)]
enum Owed {
    /// The rule states exactly one complete form, so the request is applied.
    Accepted,
    /// Refused, naming exactly these paths — relative to the schedule — in
    /// `RequiredParameters`.
    Required(Vec<String>),
    /// Refused because the rule stated BOTH of its two alternative forms:
    /// reported by the message and the code alone.
    BothForms,
}

/// One schedule under test: the rule it carries, the retention it states, and
/// what the service owes the configuration carrying it.
struct FormCase {
    case: String,
    /// The kind of policy whose documentation declares the rule.
    policy: PolicyKind,
    /// The rule SHAPE's name, which is how its documentation names it and how a
    /// refusal of a doubly stated rule names it.
    shape: &'static str,
    /// The `Schedule` members carrying the rule under test.
    members: Value,
    /// The retention the schedule states.
    mode: Mode,
    owed: Owed,
}

/// Assert `answer` is this service's modelled refusal: the status, wire code and
/// `Code` of `InvalidRequestException`, and a message naming the schedule.
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
        answer.message().contains(&format!("{{{SUBJECT_SCHEDULE}}}")),
        "case {case:?}: the refusal must name the schedule that carries the fault, got {:?}",
        answer.message()
    );
}

/// Assert the `member` parameter list is absent from the refusal.
fn assert_no_parameter_list(case: &str, answer: &Answer, member: &str) {
    let reported = at(&answer.body, &format!("/{member}"));
    assert_eq!(
        reported,
        Value::Null,
        "case {case:?}: {member} must be absent, got {reported:?}"
    );
}

/// Assert `answer` satisfies `owed` for a schedule at `position` (1-based) in
/// `Schedules`.
fn assert_owed(case: &str, answer: &Answer, owed: &Owed, shape: &str, position: usize) {
    match owed {
        Owed::Accepted => assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        ),
        Owed::Required(members) => {
            assert_refusal(case, answer);
            let expected: Vec<String> = members
                .iter()
                .map(|member| format!("PolicyDetails.Schedules[{position}].{member}"))
                .collect();
            assert_eq!(
                at(&answer.body, "/RequiredParameters"),
                json!(expected),
                "case {case:?}: RequiredParameters must name {expected:?}, got {:?}",
                at(&answer.body, "/RequiredParameters")
            );
            for parameter in &expected {
                assert!(
                    answer.message().contains(parameter.as_str()),
                    "case {case:?}: the refusal must name {parameter:?}, got {:?}",
                    answer.message()
                );
            }
            assert_no_parameter_list(case, answer, "MutuallyExclusiveParameters");
        }
        Owed::BothForms => {
            assert_refusal(case, answer);
            // Neither structured list: each names the request's own parameters,
            // and this refusal is about the form a nested rule was stated in.
            assert_no_parameter_list(case, answer, "RequiredParameters");
            assert_no_parameter_list(case, answer, "MutuallyExclusiveParameters");
            assert!(
                answer.message().contains(shape),
                "case {case:?}: the refusal must name the {shape} stated twice over, got {:?}",
                answer.message()
            );
        }
    }
}

/// The creation rule a schedule carrying `members` states.
///
/// The second bullet of `CreateRule`'s note — "If you need to specify an
/// ArchiveRule for the schedule, then you must specify a creation frequency of
/// at least 28 days" (`smithy_query(query="doc", shape="CreateRule")`) — cannot
/// be met by the interval form, whose only unit is `HOURS` and whose largest
/// supported interval is 24, so a schedule carrying an `ArchiveRule` states its
/// creation rule as a monthly cron expression instead. This file is about the
/// FORM each nested rule is stated in; that creation floor is
/// `tests/all/archive_creation_frequency.rs`'s.
fn create_rule_for(members: &Value) -> Value {
    if members.get("ArchiveRule").is_some() {
        json!({"CronExpression": MONTHLY_CRON_EXPRESSION})
    } else {
        json!({"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT})
    }
}

/// The schedule a row drives: the creation rule every accepted schedule states,
/// the retention its mode states, and the members carrying the rule under test.
fn schedule_for(row: &FormCase) -> Value {
    let mut schedule = json!({
        "Name": SUBJECT_SCHEDULE,
        "CreateRule": create_rule_for(&row.members),
    });
    if let Some(retain_rule) = row.mode.retain_rule() {
        schedule["RetainRule"] = retain_rule;
    }
    for (member, value) in row.members.as_object().expect("a rule is a JSON object") {
        schedule[member] = value.clone();
    }
    schedule
}

/// A policy of `kind` holding a configuration the service accepts, for the
/// update rows to patch: `PolicyDetails$PolicyType` cannot be changed by an
/// update, so a row's update is driven against a subject of its own kind.
async fn subject(router: &MultiServiceRouter, kind: PolicyKind) -> String {
    let created = create(
        router,
        create_request(
            "schedule rule form subject",
            kind.details(vec![good_schedule("daily")]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the {kind:?} subject policy must be created, got {} with {:?}",
        created.status, created.body
    );
    created.policy_id()
}

/// Run every row against the create (as the only schedule and as the second of
/// two) and against the update, asserting a refused request stores and changes
/// nothing and an accepted one is read back carrying the rule it stated.
async fn assert_form_cases(rows: Vec<FormCase>) {
    let router = emulator();
    let subjects = [
        (PolicyKind::Snapshot, subject(&router, PolicyKind::Snapshot).await),
        (PolicyKind::Image, subject(&router, PolicyKind::Image).await),
    ];

    for row in rows {
        for position in [1usize, 2] {
            let mut schedules = Vec::new();
            if position == 2 {
                schedules.push(good_schedule("leading"));
            }
            schedules.push(schedule_for(&row));
            let details = row.policy.details(schedules);

            let case = format!("{} (as schedule {position} of a create)", row.case);
            let before = policy_count(&router).await;
            let answer = create(
                &router,
                create_request(&case, details.clone()),
            )
            .await;
            assert_owed(&case, &answer, &row.owed, row.shape, position);
            match row.owed {
                Owed::Accepted => {
                    assert_eq!(
                        policy_count(&router).await,
                        before + 1,
                        "case {case:?}: an accepted create must store its policy"
                    );
                    let stored = at(
                        get(&router, &answer.policy_id()).await.policy(),
                        &format!("/PolicyDetails/Schedules/{}", position - 1),
                    );
                    assert!(
                        carries(&stored, &row.members),
                        "case {case:?}: the stored schedule must carry the rule it stated \
                         ({:?}), got {stored:?}",
                        row.members
                    );
                }
                _ => assert_eq!(
                    policy_count(&router).await,
                    before,
                    "case {case:?}: a refused create must store no policy"
                ),
            }

            // The update runs the same walk, so it owes the same answer, and a
            // refused one leaves the stored configuration exactly as it was.
            let subject_id = subjects
                .iter()
                .find(|(kind, _)| *kind == row.policy)
                .map(|(_, id)| id.as_str())
                .expect("every policy kind has a subject");
            let stored_before = at(get(&router, subject_id).await.policy(), "/PolicyDetails");
            let case = format!("{} (as schedule {position} of an update)", row.case);
            let answer = update(&router, subject_id, json!({"PolicyDetails": details})).await;
            assert_owed(&case, &answer, &row.owed, row.shape, position);
            let read = at(get(&router, subject_id).await.policy(), "/PolicyDetails");
            match row.owed {
                Owed::Accepted => {
                    let stored = at(&read, &format!("/Schedules/{}", position - 1));
                    assert!(
                        carries(&stored, &row.members),
                        "case {case:?}: an accepted update must store the rule it stated \
                         ({:?}), got {stored:?}",
                        row.members
                    );
                    // Leave the subject holding the accepted configuration
                    // again, so the next row compares against a known state.
                    let restored = update(
                        &router,
                        subject_id,
                        json!({"PolicyDetails": row.policy.details(vec![good_schedule("daily")])}),
                    )
                    .await;
                    assert_eq!(
                        restored.status, 200,
                        "case {case:?}: the subject policy must be restored, got {} with {:?}",
                        restored.status, restored.body
                    );
                }
                _ => assert_eq!(
                    read, stored_before,
                    "case {case:?}: a refused update must leave the stored configuration alone"
                ),
            }
        }
    }
}

/// The rows of a rule whose two forms the CALLER chooses between, built around
/// the `extra` members the rule carries beside the form (a `FastRestoreRule`'s
/// Availability Zones) and wrapped by `carry` into the schedule members.
fn either_or_rows(
    shape: &'static str,
    policy: PolicyKind,
    extra: Value,
    carry: fn(Value) -> Value,
) -> Vec<FormCase> {
    let rule = |form: Value| -> Value {
        let mut rule = extra.clone();
        for (member, value) in form.as_object().expect("a form is a JSON object") {
            rule[member] = value.clone();
        }
        carry(rule)
    };
    // Every row of a free-choice rule is judged the same way whatever the
    // schedule retains by, so these are driven beside count-based retention and
    // the mode cross is left to the rules whose form follows it.
    let mut rows = vec![
        FormCase {
            case: format!("a {shape} stating a count"),
            policy,
            shape,
            members: rule(json!({"Count": COUNT_FLOOR})),
            mode: Mode::Count,
            owed: Owed::Accepted,
        },
        FormCase {
            case: format!("a {shape} stating the count at the ceiling of its range"),
            policy,
            shape,
            members: rule(json!({"Count": COUNT_CEILING})),
            mode: Mode::Count,
            owed: Owed::Accepted,
        },
        FormCase {
            case: format!("a {shape} stating an interval at the floor of its range"),
            policy,
            shape,
            members: rule(json!({"Interval": INTERVAL_FLOOR, "IntervalUnit": "DAYS"})),
            mode: Mode::Count,
            owed: Owed::Accepted,
        },
        FormCase {
            // The request the issue reproduced for the interval form.
            case: format!("a {shape} stating an interval and no unit"),
            policy,
            shape,
            members: rule(json!({"Interval": 3})),
            mode: Mode::Count,
            owed: Owed::Required(vec![format!("{shape}.IntervalUnit")]),
        },
        FormCase {
            case: format!("a {shape} stating a unit and no interval"),
            policy,
            shape,
            members: rule(json!({"IntervalUnit": "WEEKS"})),
            mode: Mode::Count,
            owed: Owed::Required(vec![format!("{shape}.Interval")]),
        },
        FormCase {
            // The request the issue reproduced for a rule stating neither form.
            case: format!("a {shape} stating neither form"),
            policy,
            shape,
            members: rule(json!({})),
            mode: Mode::Count,
            owed: Owed::Required(vec![shape.to_string()]),
        },
        FormCase {
            // The request the issue reproduced for both forms at once.
            case: format!("a {shape} stating a count beside a complete interval"),
            policy,
            shape,
            members: rule(json!({"Count": 1, "Interval": 3, "IntervalUnit": "DAYS"})),
            mode: Mode::Count,
            owed: Owed::BothForms,
        },
        FormCase {
            case: format!("a {shape} stating a count beside an interval alone"),
            policy,
            shape,
            members: rule(json!({"Count": 1, "Interval": 3})),
            mode: Mode::Count,
            owed: Owed::BothForms,
        },
        FormCase {
            case: format!("a {shape} stating a count beside a unit alone"),
            policy,
            shape,
            members: rule(json!({"Count": 1, "IntervalUnit": "MONTHS"})),
            mode: Mode::Count,
            owed: Owed::BothForms,
        },
    ];
    // One row per `RetentionIntervalUnitValues` member: the interval form is
    // accepted in every unit the model declares, not just the one the issue
    // cited.
    for unit in RETENTION_INTERVAL_UNITS {
        rows.push(FormCase {
            case: format!("a {shape} stating an interval in {unit}"),
            policy,
            shape,
            members: rule(json!({"Interval": 3, "IntervalUnit": unit})),
            mode: Mode::Count,
            owed: Owed::Accepted,
        });
    }
    rows
}

/// The rows of a rule whose form follows the retention mode of the schedule
/// carrying it: every form crossed with every mode a schedule can state.
fn mode_bound_rows(
    shape: &'static str,
    path: &'static str,
    policy: PolicyKind,
    carry: fn(Value) -> Value,
) -> Vec<FormCase> {
    // The form the schedule's mode requires, as a refusal names the members of
    // it that were omitted.
    let count_form = vec![format!("{path}.Count")];
    let interval_form = vec![format!("{path}.Interval"), format!("{path}.IntervalUnit")];
    let mut rows = Vec::new();
    for mode in [Mode::Count, Mode::Age, Mode::None] {
        let forms: Vec<(String, Value, Owed)> = vec![
            (
                "a count".to_string(),
                json!({"Count": 3}),
                match mode {
                    Mode::Age => Owed::Required(interval_form.clone()),
                    _ => Owed::Accepted,
                },
            ),
            (
                "a complete interval".to_string(),
                json!({"Interval": 3, "IntervalUnit": "DAYS"}),
                match mode {
                    Mode::Count => Owed::Required(count_form.clone()),
                    _ => Owed::Accepted,
                },
            ),
            (
                // The request the issue reproduced.
                "an interval and no unit".to_string(),
                json!({"Interval": 3}),
                match mode {
                    Mode::Count => Owed::Required(count_form.clone()),
                    _ => Owed::Required(vec![format!("{path}.IntervalUnit")]),
                },
            ),
            (
                "a unit and no interval".to_string(),
                json!({"IntervalUnit": "MONTHS"}),
                match mode {
                    Mode::Count => Owed::Required(count_form.clone()),
                    _ => Owed::Required(vec![format!("{path}.Interval")]),
                },
            ),
            (
                // The request the issue reproduced.
                "neither form".to_string(),
                json!({}),
                match mode {
                    Mode::Count => Owed::Required(count_form.clone()),
                    Mode::Age => Owed::Required(interval_form.clone()),
                    Mode::None => Owed::Required(vec![path.to_string()]),
                },
            ),
            (
                "a count beside a complete interval".to_string(),
                json!({"Count": 3, "Interval": 3, "IntervalUnit": "DAYS"}),
                Owed::BothForms,
            ),
            (
                "a count beside an interval alone".to_string(),
                json!({"Count": 3, "Interval": 3}),
                Owed::BothForms,
            ),
        ];
        for (form, rule, owed) in forms {
            rows.push(FormCase {
                case: format!("a {shape} stating {form} beside {}", mode.described()),
                policy,
                shape,
                members: carry(rule),
                mode,
                owed,
            });
        }
    }
    rows
}

/// RULE, for a rule the caller chooses the form of — a `FastRestoreRule` enables
/// fast snapshot restore based on either a count or a time interval, and a
/// schedule stating neither, both, or an incomplete interval is refused.
#[tokio::test]
async fn a_fast_restore_rule_enables_restore_in_exactly_one_form() {
    assert_form_cases(either_or_rows(
        "FastRestoreRule",
        PolicyKind::Snapshot,
        // The Availability Zones say WHERE fast restore is enabled, not how much
        // of it, so they state neither of the two forms.
        json!({"AvailabilityZones": ["us-west-2a"]}),
        |rule| json!({"FastRestoreRule": rule}),
    ))
    .await;
}

/// The same rule for a schedule's retention rule, the parallel sentence this
/// service already enforced: one walk answers both the same way.
#[tokio::test]
async fn a_retain_rule_retains_in_exactly_one_form() {
    assert_form_cases(either_or_rows(
        "RetainRule",
        PolicyKind::Snapshot,
        json!({}),
        |rule| json!({"RetainRule": rule}),
    ))
    .await;
}

/// RULE, for a rule whose form the schedule's retention mode decides — for an
/// age-based schedule a `DeprecateRule` states an Interval and an IntervalUnit,
/// for a count-based schedule a Count, and a rule stating the other mode's form,
/// neither form or both is refused.
#[tokio::test]
async fn a_deprecate_rule_deprecates_in_the_schedules_retention_mode() {
    assert_form_cases(mode_bound_rows(
        "DeprecateRule",
        "DeprecateRule",
        PolicyKind::Image,
        |rule| json!({"DeprecateRule": rule}),
    ))
    .await;
}

/// The same sentence, on the archive-tier retention nested in a schedule's
/// `ArchiveRule`.
#[tokio::test]
async fn an_archive_tier_retains_in_the_schedules_retention_mode() {
    assert_form_cases(mode_bound_rows(
        "RetentionArchiveTier",
        "ArchiveRule.RetainRule.RetentionArchiveTier",
        PolicyKind::Snapshot,
        |rule| json!({"ArchiveRule": {"RetainRule": {"RetentionArchiveTier": rule}}}),
    ))
    .await;
}
