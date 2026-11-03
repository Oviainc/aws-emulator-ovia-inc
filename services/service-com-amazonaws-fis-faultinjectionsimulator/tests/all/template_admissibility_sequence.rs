// SPDX-License-Identifier: CC0-1.0

//! The one admissibility sequence an experiment template is judged by, end to end over
//! HTTP.
//!
//! The rule under test: which server-side refusals judge an experiment template, in which
//! order, and with which error class, is a property of the TEMPLATE and not of the call
//! that produced it — so `CreateExperimentTemplate`, which builds the template outright,
//! and `UpdateExperimentTemplate`, which builds it by replacing members of a stored one,
//! answer a template of the same shape with the SAME status, the same error code and the
//! same message. FIS states these rules about a template (an execution role that names an
//! IAM role, the stop conditions it declares, the values its experiment options admit, the
//! destination a log configuration names, how many actions it may carry, the order those
//! actions declare through `startAfter` and what FIS's built-in wait action takes), and none
//! of them is expressible as a modelled constraint, so each one is server-side.
//!
//! The rows differ along what the rule generalises over: which refusal of the sequence the
//! template earns (each one in turn, plus rows the sequence admits), its error class (a 400
//! `ValidationException` and the 402 `ServiceQuotaExceededException` the published action
//! quota answers with), how many actions and stop conditions the template declares, which
//! member carries the defect and how many rules the template is outside of at once.
//!
//! `experimentOptions.accountTargeting` is judged by the same sequence but is not a row
//! here: it is settled when a template is created
//! (`UpdateExperimentTemplateExperimentOptionsInput` declares no member for it), so no
//! update can carry it and there is nothing for the two calls to agree about.
//! `tests/all/account_targeting_agreement.rs` covers it.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION,
};

/// A well-formed IAM role ARN, for every row that is not about the role.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-admissibility";

/// A value inside `RoleArn`'s modelled bounds (`@length(20..2048)`, `pattern ^[\S]+$`) that
/// is no IAM role ARN, so only the server-side rule can refuse it.
const NOT_A_ROLE_ARN: &str = "arn:aws:iam::000000000000:user/not-a-role";

/// A CloudWatch alarm ARN, for the rows whose stop condition names the alarm source.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:fis-admissibility";

/// A CloudWatch Logs log group ARN, for the rows that log there.
const LOG_GROUP_ARN: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/fis/seq:*";

/// How many actions one experiment template may carry: the FIS quotas page lists "Actions
/// per experiment template" as 20. Spelled out here rather than imported, so a changed cap
/// in `src/state.rs` fails this suite.
const MAX_ACTIONS_PER_TEMPLATE: usize = 20;

/// One `aws:fis:wait` action with `duration` inside the 1..720 minutes FIS documents for it.
fn wait(duration: &str) -> Value {
    json!({ "actionId": "aws:fis:wait", "parameters": { "duration": duration } })
}

/// `count` distinct `aws:fis:wait` actions, for the rows about the action quota.
fn waits(count: usize) -> Value {
    let mut actions = Map::new();
    for index in 0..count {
        actions.insert(format!("wait-{index}"), wait("PT1M"));
    }
    Value::Object(actions)
}

/// The refusal a template earns: the HTTP status, the wire error code and a fragment the
/// message has to name (the member or the value the rule is about).
struct Refusal {
    status: u16,
    code: &'static str,
    message_names: &'static str,
}

/// A 400 `ValidationException` naming `member`.
fn invalid(member: &'static str) -> Refusal {
    Refusal {
        status: 400,
        code: "ValidationException",
        message_names: member,
    }
}

/// One template shape, and the refusal (if any) the admissibility sequence gives it.
struct Row {
    case: &'static str,
    /// The members that describe this row's template, carried by the create and by the
    /// update alike.
    members: Value,
    /// `None` for a row the sequence admits.
    refusal: Option<Refusal>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a template the whole sequence admits",
            members: json!({
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "actions": { "wait": wait("PT5M") }
            }),
            refusal: None,
        },
        Row {
            case: "an admissible template at the action quota",
            members: json!({
                "stopConditions": [{ "source": "aws:cloudwatch:alarm", "value": ALARM_ARN }],
                "actions": waits(MAX_ACTIONS_PER_TEMPLATE)
            }),
            refusal: None,
        },
        Row {
            case: "an admissible template logging to both destinations",
            members: json!({
                "actions": { "wait": wait("PT720M") },
                "logConfiguration": {
                    "logSchemaVersion": 2,
                    "cloudWatchLogsConfiguration": { "logGroupArn": LOG_GROUP_ARN },
                    "s3Configuration": { "bucketName": "fis-admissibility", "prefix": "both" }
                },
                "experimentOptions": { "emptyTargetResolutionMode": "skip" }
            }),
            refusal: None,
        },
        Row {
            case: "an execution role that is no IAM role arn",
            members: json!({ "roleArn": NOT_A_ROLE_ARN }),
            refusal: Some(invalid("roleArn")),
        },
        Row {
            case: "no stop condition at all",
            members: json!({ "stopConditions": [] }),
            refusal: Some(invalid("stopConditions")),
        },
        Row {
            case: "a stop condition whose source is neither admitted value",
            members: json!({ "stopConditions": [{ "source": "aws:cloudwatch:metric" }] }),
            refusal: Some(invalid("stopConditions[0].source")),
        },
        Row {
            case: "an alarm stop condition naming no alarm",
            members: json!({
                "stopConditions": [
                    { "source": "none" },
                    { "source": "aws:cloudwatch:alarm" }
                ]
            }),
            refusal: Some(invalid("stopConditions[1].value")),
        },
        Row {
            case: "an empty target resolution mode the enum does not admit",
            members: json!({ "experimentOptions": { "emptyTargetResolutionMode": "halt" } }),
            refusal: Some(invalid("experimentOptions.emptyTargetResolutionMode")),
        },
        Row {
            case: "a log configuration naming no destination",
            members: json!({ "logConfiguration": { "logSchemaVersion": 1 } }),
            refusal: Some(invalid("logConfiguration")),
        },
        Row {
            case: "one action past the action quota",
            members: json!({ "actions": waits(MAX_ACTIONS_PER_TEMPLATE + 1) }),
            refusal: Some(Refusal {
                status: 402,
                code: "ServiceQuotaExceededException",
                message_names: "actions in an experiment template",
            }),
        },
        Row {
            case: "no action at all",
            members: json!({ "actions": {} }),
            refusal: Some(invalid("actions")),
        },
        Row {
            case: "a startAfter naming an action the template does not declare",
            members: json!({
                "actions": {
                    "second": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT2M" },
                        "startAfter": ["first"]
                    }
                }
            }),
            refusal: Some(invalid("actions.second.startAfter")),
        },
        Row {
            // `tests/all/action_order_is_acyclic.rs` covers the shapes of the action graph
            // this rule generalises over; the row here is the sequence's own refusal for it.
            case: "a startAfter order no run can carry out",
            members: json!({
                "actions": {
                    "a": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT2M" },
                        "startAfter": ["b"]
                    },
                    "b": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT2M" },
                        "startAfter": ["a"]
                    }
                }
            }),
            refusal: Some(invalid("actions.a.startAfter")),
        },
        Row {
            case: "a wait action carrying no duration",
            members: json!({ "actions": { "bare": { "actionId": "aws:fis:wait" } } }),
            refusal: Some(invalid("actions.bare.parameters")),
        },
        Row {
            case: "a wait one minute past the documented range",
            members: json!({ "actions": { "wait": wait("PT721M") } }),
            refusal: Some(invalid("720 minutes")),
        },
        Row {
            case: "a wait below the documented range",
            members: json!({ "actions": { "wait": wait("PT0M") } }),
            refusal: Some(invalid("720 minutes")),
        },
        Row {
            // `ExperimentTemplateActionParameter` is modelled `pattern ^[\S]+$`, so a
            // value with whitespace in it never reaches the rule: this one is a
            // well-formed parameter that is no duration.
            case: "a wait that is no iso 8601 duration",
            members: json!({ "actions": { "wait": wait("5minutes") } }),
            refusal: Some(invalid("ISO 8601")),
        },
    ]
}

/// The rows a template is outside MORE THAN ONE rule of: the sequence reports the FIRST
/// refusal it reaches, so these pin its ORDER — and both calls have to report the same one.
fn ordering_rows() -> Vec<Row> {
    vec![
        Row {
            // The role is judged before anything about the template's collections.
            case: "no IAM role arn and no action either",
            members: json!({ "roleArn": NOT_A_ROLE_ARN, "actions": {} }),
            refusal: Some(invalid("roleArn")),
        },
        Row {
            // The stop conditions are judged before the action quota.
            case: "no stop condition and one action past the quota",
            members: json!({
                "stopConditions": [],
                "actions": waits(MAX_ACTIONS_PER_TEMPLATE + 1)
            }),
            refusal: Some(invalid("stopConditions")),
        },
        Row {
            // The log configuration is judged before the actions.
            case: "a destination-less log configuration and a wait out of range",
            members: json!({
                "logConfiguration": { "logSchemaVersion": 1 },
                "actions": { "wait": wait("PT900M") }
            }),
            refusal: Some(invalid("logConfiguration")),
        },
        Row {
            // The action quota is judged before the actions themselves, so a template
            // past the quota earns the quota's error class and not a validation failure.
            case: "past the action quota with a wait out of range among them",
            members: {
                let mut actions = waits(MAX_ACTIONS_PER_TEMPLATE + 1);
                actions["wait-0"] = wait("PT721M");
                json!({ "actions": actions })
            },
            refusal: Some(Refusal {
                status: 402,
                code: "ServiceQuotaExceededException",
                message_names: "actions in an experiment template",
            }),
        },
    ]
}

/// The create body for `row`: the members every create requires, with the row's members
/// replacing them.
fn create_body(row: &Row, label: &str) -> Value {
    let mut body = json!({
        "clientToken": token(&format!("{label}-{}", row.case.replace(' ', "-"))),
        "description": format!("admissibility sequence: {}", row.case),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": wait("PT5M") }
    });
    merge(&mut body, &row.members);
    body
}

/// A baseline template every update row starts from: admissible, and carrying a log
/// configuration with a destination so a row that names none has a stored one to keep.
fn baseline_body(label: &str) -> Value {
    json!({
        "clientToken": token(&format!("baseline-{label}")),
        "description": format!("admissibility baseline: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": wait("PT5M") },
        "logConfiguration": {
            "logSchemaVersion": 2,
            "cloudWatchLogsConfiguration": { "logGroupArn": LOG_GROUP_ARN }
        }
    })
}

/// Copy every member of `members` into `body`.
fn merge(body: &mut Value, members: &Value) {
    let (Some(body), Some(members)) = (body.as_object_mut(), members.as_object()) else {
        panic!("both the body and the row's members are JSON objects");
    };
    for (name, value) in members {
        body.insert(name.clone(), value.clone());
    }
}

/// `UpdateExperimentTemplate`: PATCH the template `id` names.
async fn update_template(router: &MultiServiceRouter, id: &str, body: Value) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/experimentTemplates/{id}"),
        Some(body),
        FALLBACK_REGION,
    )
    .await
}

/// The template `id` names, as `GetExperimentTemplate` reports it.
async fn read_template(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = send(
        router,
        "GET",
        &format!("/experimentTemplates/{id}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate").clone()
}

/// The `message` member of an answer, as a refused call carries it.
fn message(answer: &Answer) -> String {
    answer.body["message"].as_str().unwrap_or_default().to_string()
}

/// Assert that `actual` carries every member `expected` names, with that value: a template
/// a call stored reports back the members the request named, beside the ones the service
/// filled in for itself (an experiment option's documented default, for instance).
fn assert_carries(case: &str, member: &str, expected: &Value, actual: &Value) {
    match expected {
        Value::Object(members) => {
            for (name, value) in members {
                assert_carries(case, &format!("{member}.{name}"), value, &actual[name]);
            }
        }
        Value::Array(items) => {
            let actual_items = actual.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: expected {member} to be a list, got {actual:?}")
            });
            assert_eq!(
                items.len(),
                actual_items.len(),
                "case {case:?}: expected {} entries in {member}, got {actual:?}",
                items.len()
            );
            for (index, item) in items.iter().enumerate() {
                assert_carries(case, &format!("{member}[{index}]"), item, &actual_items[index]);
            }
        }
        _ => assert_eq!(
            expected, actual,
            "case {case:?}: expected {member} to be {expected:?}, got {actual:?}"
        ),
    }
}

/// Assert that `answer` is the refusal `expected` describes, or a stored template when the
/// row names none. Returns the refusal's status, code and message, so the two calls can be
/// compared with each other.
fn assert_refusal(case: &str, call: &str, answer: &Answer, expected: &Option<Refusal>) -> String {
    let Some(expected) = expected.as_ref() else {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from {call}, got {} with {:?}",
            answer.status, answer.body
        );
        return String::new();
    };
    assert_eq!(
        answer.status, expected.status,
        "case {case:?}: expected HTTP {} from {call}, got {} with {:?}",
        expected.status, answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        expected.code,
        "case {case:?}: expected error code {:?} from {call}, got {:?}",
        expected.code,
        answer.body
    );
    let message = message(answer);
    assert!(
        message.contains(expected.message_names),
        "case {case:?}: expected the {call} message to name {:?}, got {message:?}",
        expected.message_names
    );
    format!("{} {} {message}", answer.status, answer.error_code())
}

/// Drive every row of `rows` through both calls and assert they agree: each one is judged by
/// the one sequence, so the refusal a create earns is the refusal an update earns, down to
/// the message, and a row the sequence admits is stored and readable from both.
async fn both_calls_agree(label: &str, rows: Vec<Row>) {
    for row in rows {
        let case = row.case;
        let router = emulator();

        let created = create_template(&router, create_body(&row, label)).await;
        let from_create = assert_refusal(case, "CreateExperimentTemplate", &created, &row.refusal);

        let baseline = create_template(
            &router,
            baseline_body(&format!("{label}-{}", case.replace(' ', "-"))),
        )
        .await;
        assert_eq!(
            baseline.status, 200,
            "case {case:?}: expected HTTP 200 for the baseline template, got {} with {:?}",
            baseline.status, baseline.body
        );
        let before = baseline.member("experimentTemplate").clone();
        let id = before["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {before:?}"))
            .to_string();

        let updated = update_template(&router, &id, row.members.clone()).await;
        let from_update = assert_refusal(case, "UpdateExperimentTemplate", &updated, &row.refusal);

        assert_eq!(
            from_create, from_update,
            "case {case:?}: the two calls must judge the same template alike, got \
             {from_create:?} from the create and {from_update:?} from the update"
        );

        if row.refusal.is_some() {
            // A refused update leaves the stored template exactly as it was.
            let after = read_template(&router, case, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must change nothing, got {after:?}"
            );
            continue;
        }

        // An admitted template is PERSISTED by both calls: each one reports the members
        // the request named and a later read agrees.
        let from_create = created.member("experimentTemplate").clone();
        let created_id = from_create["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {from_create:?}"))
            .to_string();
        let stored_create = read_template(&router, case, &created_id).await;
        assert_eq!(
            stored_create, from_create,
            "case {case:?}: GetExperimentTemplate must report what the create stored, got \
             {stored_create:?}"
        );
        assert_carries(case, "experimentTemplate", &row.members, &stored_create);

        let stored_update = read_template(&router, case, &id).await;
        assert_eq!(
            stored_update,
            *updated.member("experimentTemplate"),
            "case {case:?}: GetExperimentTemplate must report what the update stored, got \
             {stored_update:?}"
        );
        assert_carries(case, "experimentTemplate", &row.members, &stored_update);
    }
}

/// Every refusal of the sequence, and every row it admits, judged alike by the create and by
/// the update.
#[tokio::test]
async fn one_sequence_judges_a_created_and_an_updated_template_alike() {
    both_calls_agree("seq", rows()).await;
}

/// A template outside more than one rule at once earns the FIRST refusal of the sequence —
/// the same first one on both calls, in the same error class.
#[tokio::test]
async fn both_calls_report_the_first_refusal_of_the_sequence() {
    both_calls_agree("order", ordering_rows()).await;
}
