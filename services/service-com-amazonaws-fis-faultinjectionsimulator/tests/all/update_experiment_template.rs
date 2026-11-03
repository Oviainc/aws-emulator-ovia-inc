// SPDX-License-Identifier: CC0-1.0

//! `UpdateExperimentTemplate`, end to end over HTTP.
//!
//! The rule under test is the one FIS's update surface states: the request's `id` is the
//! only member it requires, while the create requires `description`, `actions`, `roleArn`
//! and `stopConditions`, so the call is a MEMBER-WISE PARTIAL update — a member the
//! request carries REPLACES the stored value (wholly: a collection by the collection
//! named, a nested structure by the structure named), a member it omits leaves the stored
//! value exactly as it was, and the template that results is admissible only if it
//! satisfies the very rules `CreateExperimentTemplate` judges the template it builds by.
//!
//! The rows of the table below differ along everything that rule generalises over: WHICH
//! member is carried (each of the eight updatable ones on its own, then all of them at
//! once, then none of them), and, for a member that is a collection, how the collection
//! differs from the stored one (a different value, a different size, a different shape of
//! target, a name dropped, a name added, an empty collection). The recorded case — a
//! `description` change — is one row.
//!
//! The refusal table asserts the other half of the rule: a resulting template the create
//! would not accept is refused with the error the create answers (400
//! `ValidationException`, 402 `ServiceQuotaExceededException`) and the stored template is
//! left untouched — including the bounds the `aws:fis:wait` action's `duration` publishes,
//! asserted at each bound and one past it.
//!
//! One nested structure is not a replacement when the request carries it EMPTY: a
//! `logConfiguration` with none of its members set names no configuration, so it removes
//! the one the template carried. [`an_empty_log_configuration_removes_a_templates_logging`]
//! asserts that over the destinations such a configuration can name, over a template that
//! carries none, and one member past the empty form (a schema version with no
//! destination, which names a configuration and is judged as one).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, start_experiment, token, Answer, FALLBACK_REGION,
};

/// A well-formed IAM role ARN for the template this suite creates. The emulator's IAM
/// store holds no role for it, which is what lets `StartExperiment` accept it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-update-suite";

/// A second well-formed IAM role ARN, for the rows that replace `roleArn`.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-update-suite-other";

/// A CloudWatch alarm ARN, for the stop-condition rows: FIS documents that a stop
/// condition whose `source` is `aws:cloudwatch:alarm` carries the alarm's ARN in `value`.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:fis-update-suite";

/// The published bounds of the `aws:fis:wait` action's `duration` parameter: the FIS
/// action reference gives them as 1 to 720 minutes. Spelled out here rather than imported
/// from the crate so a wrong bound in `src/state.rs` fails this suite.
const WAIT_FLOOR_MINUTES: u32 = 1;
/// See [`WAIT_FLOOR_MINUTES`].
const WAIT_CEILING_MINUTES: u32 = 720;

/// The published quota on the actions one experiment template may declare (the FIS user
/// guide's quotas page: 20 actions per experiment template).
const ACTIONS_QUOTA: usize = 20;

/// The `logSchemaVersion` the baseline template is created with, which an update that
/// omits that member has to keep.
const BASELINE_LOG_SCHEMA_VERSION: i32 = 2;

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

/// The create body this suite updates: every member an update may change is present, so
/// each row can be read as "what the update left of the baseline".
fn baseline(label: &str) -> Value {
    json!({
        // `ClientToken` is modelled `^[\S]+$`, so a case label's spaces are not admitted
        // in one.
        "clientToken": token(&label.replace(' ', "-")),
        "description": format!("update suite baseline: {label}"),
        "roleArn": ROLE_ARN,
        "tags": { "Suite": "update" },
        "stopConditions": [{ "source": "none" }],
        "targets": {
            "hosts": {
                "resourceType": "aws:ec2:instance",
                "resourceArns": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
                "selectionMode": "ALL"
            },
            "tagged": {
                "resourceType": "aws:ec2:instance",
                "resourceTags": { "Env": "test" },
                "selectionMode": "COUNT(1)"
            }
        },
        "actions": {
            "wait": {
                "actionId": "aws:fis:wait",
                "description": "the baseline wait",
                "parameters": { "duration": "PT5M" }
            },
            "second": {
                "actionId": "aws:fis:wait",
                "parameters": { "duration": "PT1M" },
                "startAfter": ["wait"]
            }
        },
        "logConfiguration": {
            "logSchemaVersion": BASELINE_LOG_SCHEMA_VERSION,
            "cloudWatchLogsConfiguration": {
                "logGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/fis/update:*"
            }
        },
        "experimentOptions": {
            "accountTargeting": "single-account",
            "emptyTargetResolutionMode": "fail"
        },
        "experimentReportConfiguration": {
            "preExperimentDuration": "PT5M",
            "postExperimentDuration": "PT5M",
            "outputs": {
                "s3Configuration": { "bucketName": "fis-update-suite", "prefix": "baseline" }
            }
        }
    })
}

/// Create the baseline template for `label` and report `(id, the stored template)`.
async fn baseline_template(router: &MultiServiceRouter, label: &str) -> (String, Value) {
    create_from(router, label, baseline(label)).await
}

/// Create the template `body` describes and report `(id, the stored template)`.
async fn create_from(router: &MultiServiceRouter, label: &str, body: Value) -> (String, Value) {
    let created = create_template(router, body).await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        created.status, created.body
    );
    let template = created.member("experimentTemplate").clone();
    let id = template["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experimentTemplate.id in {template:?}"))
        .to_string();
    (id, template)
}

/// Assert that `after` is `before` with exactly the members `expected` names replaced by
/// the values it names — every other member unchanged, no member appearing out of
/// nowhere, and `lastUpdateTime` never earlier than `creationTime`.
fn assert_partial_update(case: &str, before: &Value, after: &Value, expected: &Value) {
    let before = before
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the stored template is not an object: {before:?}"));
    let after = after
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the updated template is not an object: {after:?}"));
    let expected = expected
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the expectation is not an object: {expected:?}"));

    for (member, value) in expected {
        assert_eq!(
            after.get(member).unwrap_or(&Value::Null),
            value,
            "case {case:?}: expected {member} to be {value:?}, got {:?}",
            after.get(member)
        );
    }
    for (member, value) in before {
        if expected.contains_key(member) || member == "lastUpdateTime" {
            continue;
        }
        assert_eq!(
            after.get(member).unwrap_or(&Value::Null),
            value,
            "case {case:?}: a member the request did not carry must keep its stored value; \
             {member} was {value:?} and is now {:?}",
            after.get(member)
        );
    }
    for member in after.keys() {
        assert!(
            before.contains_key(member) || expected.contains_key(member),
            "case {case:?}: {member} appeared without the request naming it: {:?}",
            after.get(member)
        );
    }

    let creation = after["creationTime"].as_f64().unwrap_or_default();
    let last_update = after["lastUpdateTime"].as_f64().unwrap_or_default();
    assert!(
        last_update >= creation,
        "case {case:?}: lastUpdateTime {last_update} must not precede creationTime {creation}"
    );
}

/// The updates this rule is asserted over: `(case, request body, the members the result
/// must carry)`. Everything the body does not name has to come back unchanged, which
/// [`assert_partial_update`] checks against the template as it was before the call.
fn update_cases() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "description only",
            json!({ "description": "a described experiment template" }),
            json!({ "description": "a described experiment template" }),
        ),
        (
            "description at the published 512-character ceiling",
            json!({ "description": "d".repeat(512) }),
            json!({ "description": "d".repeat(512) }),
        ),
        (
            "roleArn only",
            json!({ "roleArn": OTHER_ROLE_ARN }),
            json!({ "roleArn": OTHER_ROLE_ARN }),
        ),
        (
            "stop conditions replaced by an alarm",
            json!({ "stopConditions": [{ "source": "aws:cloudwatch:alarm", "value": ALARM_ARN }] }),
            json!({ "stopConditions": [{ "source": "aws:cloudwatch:alarm", "value": ALARM_ARN }] }),
        ),
        (
            "stop conditions replaced by two conditions",
            json!({
                "stopConditions": [
                    { "source": "aws:cloudwatch:alarm", "value": ALARM_ARN },
                    { "source": "none" }
                ]
            }),
            json!({
                "stopConditions": [
                    { "source": "aws:cloudwatch:alarm", "value": ALARM_ARN },
                    { "source": "none" }
                ]
            }),
        ),
        (
            "targets replaced wholesale by one tag-selected target",
            json!({
                "targets": {
                    "tagged": {
                        "resourceType": "aws:ec2:instance",
                        "resourceTags": { "Env": "staging" },
                        "selectionMode": "PERCENT(50)",
                        "filters": [{ "path": "State.Name", "values": ["running"] }]
                    }
                }
            }),
            json!({
                "targets": {
                    "tagged": {
                        "resourceType": "aws:ec2:instance",
                        "resourceTags": { "Env": "staging" },
                        "selectionMode": "PERCENT(50)",
                        "filters": [{ "path": "State.Name", "values": ["running"] }]
                    }
                }
            }),
        ),
        (
            "targets emptied",
            json!({ "targets": {} }),
            json!({ "targets": {} }),
        ),
        (
            "actions replaced by one entry that keeps the stored actionId",
            json!({
                "actions": {
                    "wait": {
                        "description": "no actionId in this request",
                        "parameters": { "duration": "PT2M" }
                    }
                }
            }),
            json!({
                "actions": {
                    "wait": {
                        "actionId": "aws:fis:wait",
                        "description": "no actionId in this request",
                        "parameters": { "duration": "PT2M" }
                    }
                }
            }),
        ),
        (
            "actions replaced by a name the template did not declare",
            json!({
                "actions": {
                    "fresh": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT3M" }
                    }
                }
            }),
            json!({
                "actions": {
                    "fresh": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT3M" }
                    }
                }
            }),
        ),
        (
            "actions grow to three, chained by startAfter",
            json!({
                "actions": {
                    "wait": { "parameters": { "duration": "PT1M" } },
                    "second": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT1M" },
                        "startAfter": ["wait"]
                    },
                    "third": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT1M" },
                        "startAfter": ["second"]
                    }
                }
            }),
            json!({
                "actions": {
                    "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } },
                    "second": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT1M" },
                        "startAfter": ["wait"]
                    },
                    "third": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT1M" },
                        "startAfter": ["second"]
                    }
                }
            }),
        ),
        (
            "a wait at the published floor",
            json!({
                "actions": {
                    "wait": { "parameters": { "duration": format!("PT{WAIT_FLOOR_MINUTES}M") } }
                }
            }),
            json!({
                "actions": {
                    "wait": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": format!("PT{WAIT_FLOOR_MINUTES}M") }
                    }
                }
            }),
        ),
        (
            "a wait at the published ceiling",
            json!({
                "actions": {
                    "wait": { "parameters": { "duration": format!("PT{WAIT_CEILING_MINUTES}M") } }
                }
            }),
            json!({
                "actions": {
                    "wait": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": format!("PT{WAIT_CEILING_MINUTES}M") }
                    }
                }
            }),
        ),
        (
            "a wait at the published ceiling written in hours",
            json!({ "actions": { "wait": { "parameters": { "duration": "PT12H" } } } }),
            json!({
                "actions": {
                    "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT12H" } }
                }
            }),
        ),
        (
            "the action count at the published quota",
            json!({ "actions": wait_actions(ACTIONS_QUOTA) }),
            json!({ "actions": wait_actions(ACTIONS_QUOTA) }),
        ),
        (
            "empty target resolution mode",
            json!({ "experimentOptions": { "emptyTargetResolutionMode": "skip" } }),
            // `accountTargeting` is settled when the template is created and this request
            // declares no member for it, so it keeps the value the create filled in.
            json!({
                "experimentOptions": {
                    "accountTargeting": "single-account",
                    "emptyTargetResolutionMode": "skip"
                }
            }),
        ),
        (
            "log configuration replaced, keeping the stored schema version",
            json!({
                "logConfiguration": {
                    "s3Configuration": { "bucketName": "fis-update-logs", "prefix": "updated" }
                }
            }),
            json!({
                "logConfiguration": {
                    "logSchemaVersion": BASELINE_LOG_SCHEMA_VERSION,
                    "s3Configuration": { "bucketName": "fis-update-logs", "prefix": "updated" }
                }
            }),
        ),
        (
            "log configuration replaced, with its own schema version",
            json!({
                "logConfiguration": {
                    "logSchemaVersion": 1,
                    "cloudWatchLogsConfiguration": {
                        "logGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/fis/other:*"
                    }
                }
            }),
            json!({
                "logConfiguration": {
                    "logSchemaVersion": 1,
                    "cloudWatchLogsConfiguration": {
                        "logGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/fis/other:*"
                    }
                }
            }),
        ),
        (
            "log configuration removed by an empty configuration",
            json!({ "logConfiguration": {} }),
            // No operation deletes a log configuration, so an empty one is the removal:
            // the member is gone from the template the update reports.
            json!({ "logConfiguration": null }),
        ),
        (
            "report configuration replaced",
            json!({
                "experimentReportConfiguration": {
                    "preExperimentDuration": "PT10M",
                    "dataSources": {
                        "cloudWatchDashboards": [{ "dashboardIdentifier": "fis-dashboard" }]
                    }
                }
            }),
            json!({
                "experimentReportConfiguration": {
                    "preExperimentDuration": "PT10M",
                    "dataSources": {
                        "cloudWatchDashboards": [{ "dashboardIdentifier": "fis-dashboard" }]
                    }
                }
            }),
        ),
        (
            "every updatable member at once",
            json!({
                "description": "everything at once",
                "roleArn": OTHER_ROLE_ARN,
                "stopConditions": [{ "source": "aws:cloudwatch:alarm", "value": ALARM_ARN }],
                "targets": {
                    "hosts": {
                        "resourceType": "aws:ec2:instance",
                        "resourceArns": ["arn:aws:ec2:us-east-1:000000000000:instance/i-9"],
                        "selectionMode": "COUNT(1)"
                    }
                },
                "actions": {
                    "only": {
                        "actionId": "aws:ec2:stop-instances",
                        "parameters": { "startInstancesAfterDuration": "PT9M" },
                        "targets": { "Instances": "hosts" }
                    }
                },
                "logConfiguration": {
                    "logSchemaVersion": 1,
                    "s3Configuration": { "bucketName": "fis-everything", "prefix": "all" }
                },
                "experimentOptions": { "emptyTargetResolutionMode": "skip" },
                "experimentReportConfiguration": { "postExperimentDuration": "PT1M" }
            }),
            json!({
                "description": "everything at once",
                "roleArn": OTHER_ROLE_ARN,
                "stopConditions": [{ "source": "aws:cloudwatch:alarm", "value": ALARM_ARN }],
                "targets": {
                    "hosts": {
                        "resourceType": "aws:ec2:instance",
                        "resourceArns": ["arn:aws:ec2:us-east-1:000000000000:instance/i-9"],
                        "selectionMode": "COUNT(1)"
                    }
                },
                "actions": {
                    "only": {
                        "actionId": "aws:ec2:stop-instances",
                        "parameters": { "startInstancesAfterDuration": "PT9M" },
                        "targets": { "Instances": "hosts" }
                    }
                },
                "logConfiguration": {
                    "logSchemaVersion": 1,
                    "s3Configuration": { "bucketName": "fis-everything", "prefix": "all" }
                },
                "experimentOptions": {
                    "accountTargeting": "single-account",
                    "emptyTargetResolutionMode": "skip"
                },
                "experimentReportConfiguration": { "postExperimentDuration": "PT1M" }
            }),
        ),
        (
            "no updatable member at all",
            json!({}),
            // Nothing to replace, so nothing but the update instant moves.
            json!({}),
        ),
    ]
}

/// `count` distinct wait actions, for the rows that vary how many actions a template
/// declares.
fn wait_actions(count: usize) -> Value {
    let mut actions = serde_json::Map::new();
    for index in 0..count {
        actions.insert(
            format!("wait-{index}"),
            json!({ "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } }),
        );
    }
    Value::Object(actions)
}

/// A member the request carries replaces the stored value; a member it omits leaves the
/// stored value alone — and what the update answers is what the store now holds.
#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_keeps_the_rest() {
    let router = emulator();

    for (case, body, expected) in update_cases() {
        let (id, before) = baseline_template(&router, case).await;

        let answer = update_template(&router, &id, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateExperimentTemplate, got {} with {:?}",
            answer.status, answer.body
        );
        let after = answer.member("experimentTemplate").clone();
        assert_partial_update(case, &before, &after, &expected);

        // The answer is the record, not a re-derived view of the request: reading it back
        // reports the identical template, and reading it twice changes nothing.
        let fetched = read_template(&router, case, &id).await;
        assert_eq!(
            fetched, after,
            "case {case:?}: GetExperimentTemplate must report what the update stored, got \
             {fetched:?}"
        );
        let again = read_template(&router, case, &id).await;
        assert_eq!(
            again, fetched,
            "case {case:?}: two reads of one updated template disagreed: {again:?}"
        );
    }
}

/// The updates a resulting template could not survive: each is refused with the error the
/// create answers for the same template, and the stored template is left untouched.
fn refusal_cases() -> Vec<(&'static str, Value, u16, &'static str)> {
    vec![
        (
            "a roleArn that is not an ARN at all",
            json!({ "roleArn": "fis-update-suite-role-name" }),
            400,
            "ValidationException",
        ),
        (
            "a roleArn naming a role of another service",
            json!({ "roleArn": "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789" }),
            400,
            "ValidationException",
        ),
        (
            "a stop condition source FIS does not define",
            json!({ "stopConditions": [{ "source": "aws:cloudwatch:metric" }] }),
            400,
            "ValidationException",
        ),
        (
            "an alarm stop condition with no alarm",
            json!({ "stopConditions": [{ "source": "aws:cloudwatch:alarm" }] }),
            400,
            "ValidationException",
        ),
        (
            "an empty target resolution mode outside the modelled pair",
            json!({ "experimentOptions": { "emptyTargetResolutionMode": "maybe" } }),
            400,
            "ValidationException",
        ),
        (
            "a wait one minute below the published floor",
            json!({
                "actions": {
                    "wait": { "parameters": { "duration": format!("PT{}M", WAIT_FLOOR_MINUTES - 1) } }
                }
            }),
            400,
            "ValidationException",
        ),
        (
            "a wait one minute past the published ceiling",
            json!({
                "actions": {
                    "wait": {
                        "parameters": { "duration": format!("PT{}M", WAIT_CEILING_MINUTES + 1) }
                    }
                }
            }),
            400,
            "ValidationException",
        ),
        (
            "a wait whose duration is not ISO 8601",
            json!({ "actions": { "wait": { "parameters": { "duration": "5 minutes" } } } }),
            400,
            "ValidationException",
        ),
        (
            "a wait with no duration at all",
            json!({ "actions": { "wait": { "actionId": "aws:fis:wait" } } }),
            400,
            "ValidationException",
        ),
        (
            "a startAfter naming an action the update dropped",
            json!({
                "actions": {
                    "wait": {
                        "parameters": { "duration": "PT1M" },
                        "startAfter": ["second"]
                    }
                }
            }),
            400,
            "ValidationException",
        ),
        (
            "a new action with no actionId to take",
            json!({ "actions": { "fresh": { "parameters": { "duration": "PT1M" } } } }),
            400,
            "ValidationException",
        ),
        (
            // One member past the empty form: this names a configuration, so it REPLACES
            // the stored one — and a log configuration has to name somewhere to log.
            "a log configuration naming a schema version but no destination",
            json!({ "logConfiguration": { "logSchemaVersion": 1 } }),
            400,
            "ValidationException",
        ),
        (
            "one action past the published quota",
            json!({ "actions": wait_actions(ACTIONS_QUOTA + 1) }),
            402,
            "ServiceQuotaExceededException",
        ),
    ]
}

#[tokio::test]
async fn an_update_the_rules_refuse_changes_nothing() {
    let router = emulator();

    for (case, body, status, code) in refusal_cases() {
        let (id, before) = baseline_template(&router, case).await;

        let answer = update_template(&router, &id, body).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {:?}",
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

/// `logSchemaVersion` is required where the create declares it, so an update that omits
/// it takes the stored one — and is refused when the template carries none to take.
#[tokio::test]
async fn a_log_configuration_needs_a_schema_version_from_somewhere() {
    let router = emulator();

    let case = "no stored log configuration";
    let body = json!({
        "clientToken": token("no-logs"),
        "description": "update suite: a template with no log configuration",
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } } }
    });
    let (id, before) = create_from(&router, case, body).await;
    assert_eq!(
        before.get("logConfiguration"),
        None,
        "case {case:?}: the baseline must carry no log configuration, got {before:?}"
    );

    let refused = update_template(
        &router,
        &id,
        json!({
            "logConfiguration": {
                "s3Configuration": { "bucketName": "fis-update-logs", "prefix": "none" }
            }
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 for a log configuration with no schema version, \
         got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {:?}",
        refused.body
    );
    let unchanged = read_template(&router, case, &id).await;
    assert_eq!(
        unchanged, before,
        "case {case:?}: the refused update must have changed nothing, got {unchanged:?}"
    );

    let accepted = update_template(
        &router,
        &id,
        json!({
            "logConfiguration": {
                "logSchemaVersion": 2,
                "s3Configuration": { "bucketName": "fis-update-logs", "prefix": "none" }
            }
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: expected HTTP 200 once the request carries a schema version, got {} \
         with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.member("experimentTemplate")["logConfiguration"],
        json!({
            "logSchemaVersion": 2,
            "s3Configuration": { "bucketName": "fis-update-logs", "prefix": "none" }
        }),
        "case {case:?}: expected the log configuration the request named, got {:?}",
        accepted.body
    );
}

/// What an update does NOT touch: the identity and creation of the template, the tags
/// `TagResource` owns, and the experiments already started from it, which snapshot the
/// template they were started from. `lastUpdateTime` does move, past `creationTime`.
#[tokio::test]
async fn an_update_moves_only_the_template_it_names() {
    let router = emulator();
    let case = "an experiment started before the update";
    let (id, before) = baseline_template(&router, case).await;

    let started = start_experiment(
        &router,
        json!({ "clientToken": token("before-update"), "experimentTemplateId": id }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
        started.status, started.body
    );
    let experiment = started.member("experiment").clone();
    let experiment_id = experiment["id"].as_str().unwrap_or_default().to_string();

    // Long enough that the update instant is distinguishable from the creation instant,
    // so the assertion below is about the stamp and not about the clock's resolution.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let answer = update_template(
        &router,
        &id,
        json!({
            "description": "updated after an experiment started",
            "roleArn": OTHER_ROLE_ARN,
            "actions": { "wait": { "parameters": { "duration": "PT2M" } } }
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from UpdateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    let after = answer.member("experimentTemplate").clone();

    for member in ["id", "arn", "creationTime", "tags", "targetAccountConfigurationsCount"] {
        assert_eq!(
            after[member], before[member],
            "case {case:?}: an update must not touch {member}, got {:?}",
            after[member]
        );
    }
    let creation = after["creationTime"].as_f64().unwrap_or_default();
    let last_update = after["lastUpdateTime"].as_f64().unwrap_or_default();
    assert!(
        last_update > creation,
        "case {case:?}: expected lastUpdateTime to move past creationTime {creation}, got \
         {last_update}"
    );

    // The summary a list reports is the record too, so it carries the update.
    let listed = send(
        &router,
        "GET",
        "/experimentTemplates",
        None,
        FALLBACK_REGION,
    )
    .await;
    let summary = listed
        .member("experimentTemplates")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplates in {:?}", listed.body))
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id.as_str()))
        .unwrap_or_else(|| panic!("case {case:?}: the template is not listed: {:?}", listed.body))
        .clone();
    for member in ["description", "lastUpdateTime", "creationTime", "tags", "arn"] {
        assert_eq!(
            summary[member], after[member],
            "case {case:?}: the listed summary must agree with the updated record on \
             {member}, got {:?}",
            summary[member]
        );
    }

    // The experiment keeps the template it was started from: its own record was
    // snapshotted at start and the update did not reach into it.
    let read_back = send(
        &router,
        "GET",
        &format!("/experiments/{experiment_id}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    let read_back = read_back.member("experiment").clone();
    for member in ["roleArn", "actions", "targets", "stopConditions"] {
        assert_eq!(
            read_back[member], experiment[member],
            "case {case:?}: an update must not reach into an experiment already started; \
             {member} is now {:?}",
            read_back[member]
        );
    }
}

/// The log configurations an empty `logConfiguration` is asserted to remove: `(case, the
/// configuration the template is created with)`. The rows differ along everything that
/// half of the rule generalises over — which destinations the stored configuration names
/// (CloudWatch Logs, Amazon S3, both), which schema version it carries, and whether the
/// template carries a configuration at all, for which the removal has nothing to remove.
fn log_removal_cases() -> Vec<(&'static str, Option<Value>)> {
    vec![
        (
            "a CloudWatch Logs destination",
            Some(json!({
                "logSchemaVersion": BASELINE_LOG_SCHEMA_VERSION,
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/fis/off:*"
                }
            })),
        ),
        (
            "an S3 destination with a prefix",
            Some(json!({
                "logSchemaVersion": BASELINE_LOG_SCHEMA_VERSION,
                "s3Configuration": { "bucketName": "fis-logs-off", "prefix": "stored" }
            })),
        ),
        (
            "both destinations at schema version 1",
            Some(json!({
                "logSchemaVersion": 1,
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/fis/both:*"
                },
                "s3Configuration": { "bucketName": "fis-logs-both" }
            })),
        ),
        ("no stored log configuration at all", None),
    ]
}

/// The create body for a template carrying `logs`, or none when `logs` is `None`.
fn template_logging(label: &str, logs: Option<&Value>) -> Value {
    let mut body = json!({
        "clientToken": token(&format!("logs-{}", label.replace(' ', "-"))),
        "description": format!("update suite: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": {
            "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } }
        }
    });
    if let Some(logs) = logs {
        body["logConfiguration"] = logs.clone();
    }
    body
}

/// A `logConfiguration` carrying NONE of its members names no configuration, so it REMOVES
/// the one the template carried: the update answers a template without that member, every
/// later read reports it gone, an experiment started afterwards carries none, and a second
/// such update has nothing left to remove. A configuration can be given back afterwards,
/// and then has to carry its own schema version — there is no stored one left to take.
#[tokio::test]
async fn an_empty_log_configuration_removes_a_templates_logging() {
    let router = emulator();

    for (case, stored) in log_removal_cases() {
        let (id, before) = create_from(&router, case, template_logging(case, stored.as_ref())).await;
        assert_eq!(
            before.get("logConfiguration").cloned(),
            stored,
            "case {case:?}: the created template must carry the configuration the create \
             named, got {:?}",
            before.get("logConfiguration")
        );

        let answer = update_template(&router, &id, json!({ "logConfiguration": {} })).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 for an empty logConfiguration, got {} with {:?}",
            answer.status, answer.body
        );
        let after = answer.member("experimentTemplate").clone();
        assert_eq!(
            after.get("logConfiguration"),
            None,
            "case {case:?}: expected no logConfiguration member at all, got {:?}",
            after.get("logConfiguration")
        );
        // Nothing but the logging (and the update instant) moved.
        assert_partial_update(case, &before, &after, &json!({ "logConfiguration": null }));

        let fetched = read_template(&router, case, &id).await;
        assert_eq!(
            fetched, after,
            "case {case:?}: GetExperimentTemplate must report the template the removal \
             stored, got {fetched:?}"
        );

        // Removing again has nothing to remove, and is not a refusal either.
        let again = update_template(&router, &id, json!({ "logConfiguration": {} })).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected HTTP 200 for a second empty logConfiguration, got {} \
             with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.member("experimentTemplate").get("logConfiguration"),
            None,
            "case {case:?}: the template must still carry no logConfiguration, got {:?}",
            again.body
        );

        // An experiment started from the template now carries no log configuration: it
        // takes the one the template holds, and the template holds none.
        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("logs-off-{}", case.replace(' ', "-"))),
                "experimentTemplateId": id
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.member("experiment").get("logConfiguration"),
            None,
            "case {case:?}: an experiment started after the removal must carry no \
             logConfiguration, got {:?}",
            started.member("experiment").get("logConfiguration")
        );

        // The removal left no schema version behind, so giving logging back needs one.
        let refused = update_template(
            &router,
            &id,
            json!({ "logConfiguration": { "s3Configuration": { "bucketName": "fis-relog" } } }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a new log configuration with no schema \
             version, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            refused.body
        );

        let relogged = json!({
            "logSchemaVersion": BASELINE_LOG_SCHEMA_VERSION,
            "s3Configuration": { "bucketName": "fis-relog", "prefix": "again" }
        });
        let readded =
            update_template(&router, &id, json!({ "logConfiguration": relogged.clone() })).await;
        assert_eq!(
            readded.status, 200,
            "case {case:?}: expected HTTP 200 once the new configuration carries a schema \
             version, got {} with {:?}",
            readded.status, readded.body
        );
        assert_eq!(
            readded.member("experimentTemplate")["logConfiguration"],
            relogged,
            "case {case:?}: expected the log configuration the request named, got {:?}",
            readded.body
        );
        let refetched = read_template(&router, case, &id).await;
        assert_eq!(
            refetched["logConfiguration"], relogged,
            "case {case:?}: GetExperimentTemplate must report the configuration given back, \
             got {:?}",
            refetched["logConfiguration"]
        );
    }
}
