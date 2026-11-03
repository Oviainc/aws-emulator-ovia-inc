// SPDX-License-Identifier: CC0-1.0

//! The shape floors an experiment template has to satisfy, end to end over HTTP.
//!
//! The rule under test: a template a request leaves behind DECLARES at least one action and
//! at least one stop condition, and a log configuration it carries NAMES at least one log
//! destination — so a request whose resulting template declares none of one of those is
//! refused with a 400 `ValidationException` and nothing is stored. `actions` and
//! `stopConditions` are `@required` on the create while their map and list carry no
//! `@length`, and both members of
//! `CreateExperimentTemplateLogConfigurationInput`/`UpdateExperimentTemplateLogConfigurationInput`
//! that name a destination are optional, so each floor is a server-side rule rather than a
//! modelled constraint.
//!
//! One request shape is NOT a template below a floor: a `logConfiguration` carrying none of
//! its members names no configuration, so on the update it removes the member instead of
//! leaving a destination-less one behind ([`Shape::admissible_on_update`]).
//!
//! The rule is about the TEMPLATE, not about one request's shape, so every row below is
//! driven through BOTH `CreateExperimentTemplate` (which builds the template outright) and
//! `UpdateExperimentTemplate` (which builds it by replacing members of a stored one), and
//! the two have to agree. The rows differ along what the rule generalises over: which floor
//! the shape sits at or below, how many actions and stop conditions it declares (one, two,
//! none), which stop-condition source they use, which log destination is named (CloudWatch
//! Logs, S3, both, neither, no log configuration at all) and whether the member is carried
//! at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION,
};

/// A well-formed IAM role ARN for the templates this suite builds.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-template-floors";

/// A CloudWatch alarm ARN, for the rows whose stop condition names the alarm source.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:fis-template-floors";

/// A CloudWatch Logs log group ARN, for the rows that log there.
const LOG_GROUP_ARN: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/fis/floors:*";

/// The `logSchemaVersion` the baseline template this suite updates was created with.
const BASELINE_LOG_SCHEMA_VERSION: i32 = 2;

/// One `aws:fis:wait` action: `duration` is inside the 1..720 minutes FIS documents for it,
/// so no row here is refused for the wait rather than for a floor.
fn wait(duration: &str) -> Value {
    json!({ "actionId": "aws:fis:wait", "parameters": { "duration": duration } })
}

/// One template shape, and whether a template of that shape is admissible.
struct Shape {
    case: &'static str,
    actions: Value,
    stop_conditions: Value,
    /// `None` means the request carries no `logConfiguration` member at all — which on the
    /// create leaves the template logging nowhere, and on the update leaves the stored
    /// configuration exactly as it was.
    log_configuration: Option<Value>,
    admissible: bool,
}

impl Shape {
    /// Whether an UPDATE carrying this shape's members is admissible.
    ///
    /// The floors are the same ones the create is judged by, but one member does not mean
    /// the same thing on both calls: a `logConfiguration` carrying NONE of its members
    /// names no configuration, which the create cannot express at all (it declares
    /// `logSchemaVersion` `@required`) and which on the update REMOVES the template's log
    /// configuration. A template carrying none is not logging, which no floor refuses.
    fn admissible_on_update(&self) -> bool {
        self.admissible || self.log_configuration.as_ref().is_some_and(names_no_configuration)
    }
}

/// Whether a `logConfiguration` a request carries names NO configuration: an object with
/// no member set.
fn names_no_configuration(configuration: &Value) -> bool {
    configuration
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
}

fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "one action, one stop condition, logging to CloudWatch Logs",
            actions: json!({ "wait": wait("PT5M") }),
            stop_conditions: json!([{ "source": "none" }]),
            log_configuration: Some(json!({
                "logSchemaVersion": 1,
                "cloudWatchLogsConfiguration": { "logGroupArn": LOG_GROUP_ARN }
            })),
            admissible: true,
        },
        Shape {
            case: "two actions, two stop conditions, logging to S3",
            actions: json!({
                "first": wait("PT1M"),
                "second": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT2M" },
                            "startAfter": ["first"] }
            }),
            stop_conditions: json!([
                { "source": "aws:cloudwatch:alarm", "value": ALARM_ARN },
                { "source": "none" }
            ]),
            log_configuration: Some(json!({
                "logSchemaVersion": 1,
                "s3Configuration": { "bucketName": "fis-template-floors", "prefix": "two" }
            })),
            admissible: true,
        },
        Shape {
            case: "one action, an alarm stop condition, logging to both destinations",
            actions: json!({ "wait": wait("PT1M") }),
            stop_conditions: json!([{ "source": "aws:cloudwatch:alarm", "value": ALARM_ARN }]),
            log_configuration: Some(json!({
                "logSchemaVersion": 1,
                "cloudWatchLogsConfiguration": { "logGroupArn": LOG_GROUP_ARN },
                "s3Configuration": { "bucketName": "fis-template-floors", "prefix": "both" }
            })),
            admissible: true,
        },
        Shape {
            case: "one action, one stop condition, no log configuration carried",
            actions: json!({ "wait": wait("PT1M") }),
            stop_conditions: json!([{ "source": "none" }]),
            log_configuration: None,
            admissible: true,
        },
        Shape {
            case: "no action at all",
            actions: json!({}),
            stop_conditions: json!([{ "source": "none" }]),
            log_configuration: None,
            admissible: false,
        },
        Shape {
            case: "no stop condition at all",
            actions: json!({ "wait": wait("PT1M") }),
            stop_conditions: json!([]),
            log_configuration: None,
            admissible: false,
        },
        Shape {
            case: "neither an action nor a stop condition",
            actions: json!({}),
            stop_conditions: json!([]),
            log_configuration: None,
            admissible: false,
        },
        Shape {
            case: "a log configuration naming neither destination",
            actions: json!({ "wait": wait("PT1M") }),
            stop_conditions: json!([{ "source": "none" }]),
            log_configuration: Some(json!({ "logSchemaVersion": 1 })),
            admissible: false,
        },
        Shape {
            // An empty configuration names no configuration at all: on the create that is
            // the `@required` `logSchemaVersion` missing, so it is refused; on the update
            // it REMOVES the template's log configuration, which is admissible and is
            // what [`Shape::admissible_on_update`] says.
            case: "an empty log configuration",
            actions: json!({ "wait": wait("PT1M") }),
            stop_conditions: json!([{ "source": "none" }]),
            log_configuration: Some(json!({})),
            admissible: false,
        },
    ]
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

/// How many templates the emulator holds, as `ListExperimentTemplates` reports them.
async fn template_count(router: &MultiServiceRouter, case: &str) -> usize {
    let listed = send(router, "GET", "/experimentTemplates", None, FALLBACK_REGION).await;
    listed
        .member("experimentTemplates")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplates in {:?}", listed.body))
        .len()
}

/// The create body for `shape`.
fn create_body(shape: &Shape) -> Value {
    let mut body = json!({
        "clientToken": token(&shape.case.replace(' ', "-").replace(',', "")),
        "description": format!("template floors: {}", shape.case),
        "roleArn": ROLE_ARN,
        "stopConditions": shape.stop_conditions,
        "targets": {},
        "actions": shape.actions
    });
    if let Some(configuration) = shape.log_configuration.clone() {
        body["logConfiguration"] = configuration;
    }
    body
}

/// The update body for `shape`: the same three members, as a partial update.
fn update_body(shape: &Shape) -> Value {
    let mut body = json!({
        "stopConditions": shape.stop_conditions,
        "actions": shape.actions
    });
    if let Some(configuration) = shape.log_configuration.clone() {
        body["logConfiguration"] = configuration;
    }
    body
}

/// A baseline template every update row starts from: admissible, and carrying a log
/// configuration with a destination so a row that omits `logConfiguration` has a stored one
/// to keep.
fn baseline_body(label: &str) -> Value {
    json!({
        "clientToken": token(&format!("baseline-{}", label.replace(' ', "-").replace(',', ""))),
        "description": format!("template floors baseline: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": wait("PT5M") },
        "logConfiguration": {
            "logSchemaVersion": BASELINE_LOG_SCHEMA_VERSION,
            "cloudWatchLogsConfiguration": { "logGroupArn": LOG_GROUP_ARN }
        }
    })
}

/// `CreateExperimentTemplate` judges the template it builds by the floors: a shape at or
/// above them is stored and readable, and one below them is refused with a 400
/// `ValidationException`, leaving the store empty.
#[tokio::test]
async fn a_create_refuses_a_template_below_the_shape_floors() {
    for shape in shapes() {
        let case = shape.case;
        let router = emulator();

        let answer = create_template(&router, create_body(&shape)).await;
        if shape.admissible {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
                answer.status, answer.body
            );
            let created = answer.member("experimentTemplate").clone();
            let id = created["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {created:?}"))
                .to_string();
            // The answer is the record: reading it back reports the same template, with the
            // actions, stop conditions and log configuration the request named.
            let stored = read_template(&router, case, &id).await;
            assert_eq!(
                stored, created,
                "case {case:?}: GetExperimentTemplate must report what the create stored, got \
                 {stored:?}"
            );
            assert_eq!(
                stored["stopConditions"], shape.stop_conditions,
                "case {case:?}: expected the stop conditions the request named, got {:?}",
                stored["stopConditions"]
            );
            assert_eq!(
                stored["actions"], shape.actions,
                "case {case:?}: expected the actions the request named, got {:?}",
                stored["actions"]
            );
            match shape.log_configuration.as_ref() {
                Some(configuration) => assert_eq!(
                    stored["logConfiguration"], *configuration,
                    "case {case:?}: expected the log configuration the request named, got {:?}",
                    stored["logConfiguration"]
                ),
                None => assert_eq!(
                    stored.get("logConfiguration"),
                    None,
                    "case {case:?}: a template carrying no log configuration must report none, \
                     got {:?}",
                    stored.get("logConfiguration")
                ),
            }
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from CreateExperimentTemplate, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            template_count(&router, case).await,
            0,
            "case {case:?}: a refused create must store no template"
        );
    }
}

/// `UpdateExperimentTemplate` judges the template the update RESULTS in by the same floors:
/// a resulting shape below one of them is refused with a 400 `ValidationException` and the
/// stored template is left exactly as it was.
#[tokio::test]
async fn an_update_refuses_a_resulting_template_below_the_shape_floors() {
    let router = emulator();

    for shape in shapes() {
        let case = shape.case;
        let created = create_template(&router, baseline_body(case)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 for the baseline template, got {} with {:?}",
            created.status, created.body
        );
        let before = created.member("experimentTemplate").clone();
        let id = before["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {before:?}"))
            .to_string();

        let answer = update_template(&router, &id, update_body(&shape)).await;
        if shape.admissible_on_update() {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from UpdateExperimentTemplate, got {} with {:?}",
                answer.status, answer.body
            );
            let after = read_template(&router, case, &id).await;
            assert_eq!(
                after["stopConditions"], shape.stop_conditions,
                "case {case:?}: expected the stop conditions the update named, got {:?}",
                after["stopConditions"]
            );
            assert_eq!(
                after["actions"], shape.actions,
                "case {case:?}: expected the actions the update named, got {:?}",
                after["actions"]
            );
            // A `logConfiguration` the request does not carry leaves the stored one alone,
            // which is what keeps this row above the destination floor; one carrying no
            // member at all removes the member, so the template then reports none.
            let expected_logs = match shape.log_configuration.as_ref() {
                Some(configuration) if names_no_configuration(configuration) => Value::Null,
                Some(configuration) => configuration.clone(),
                None => before["logConfiguration"].clone(),
            };
            assert_eq!(
                after["logConfiguration"], expected_logs,
                "case {case:?}: expected log configuration {expected_logs:?}, got {:?}",
                after["logConfiguration"]
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from UpdateExperimentTemplate, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        let after = read_template(&router, case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the stored template untouched, got \
             {after:?}"
        );
    }
}

/// Each floor is refused on its own, whatever else the update carries: a request that
/// changes only the member sitting below a floor is refused, and the same request with that
/// member at the floor is accepted.
#[tokio::test]
async fn one_member_below_a_floor_is_enough_to_refuse_an_update() {
    let router = emulator();

    // `(case, the member below its floor, the same member at it)`.
    let rows = vec![
        (
            "actions",
            json!({ "actions": {} }),
            json!({ "actions": { "only": wait("PT1M") } }),
        ),
        (
            "stopConditions",
            json!({ "stopConditions": [] }),
            json!({ "stopConditions": [{ "source": "none" }] }),
        ),
        (
            "logConfiguration",
            json!({ "logConfiguration": { "logSchemaVersion": 1 } }),
            json!({
                "logConfiguration": {
                    "logSchemaVersion": 1,
                    "s3Configuration": { "bucketName": "fis-template-floors" }
                }
            }),
        ),
    ];

    for (case, below, at_floor) in rows {
        let created = create_template(&router, baseline_body(&format!("one-{case}"))).await;
        let before = created.member("experimentTemplate").clone();
        let id = before["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {before:?}"))
            .to_string();

        let refused = update_template(&router, &id, below.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for {below:?}, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException for {below:?}, got {:?}",
            refused.body
        );
        let unchanged = read_template(&router, case, &id).await;
        assert_eq!(
            unchanged, before,
            "case {case:?}: the refused update must have changed nothing, got {unchanged:?}"
        );

        let accepted = update_template(&router, &id, at_floor.clone()).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 for {at_floor:?}, got {} with {:?}",
            accepted.status, accepted.body
        );
        let after = read_template(&router, case, &id).await;
        let expected = at_floor
            .as_object()
            .and_then(|body| body.values().next())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            after[case], expected,
            "case {case:?}: expected the member at its floor to be stored as {expected:?}, got \
             {:?}",
            after[case]
        );
    }
}
