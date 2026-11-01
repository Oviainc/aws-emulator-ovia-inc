// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH declared schedule values are cron expressions — the
//! `ScheduleConfiguration` a read reports, and the automated trigger that configuration
//! arms.
//!
//! The rule under test: in a cron expression every field carries a value — a number, a
//! wildcard, a day-of-month qualifier or a month or day-of-week name — and the list,
//! range, step and nth-weekday punctuation only separates those values, so a field that
//! leaves a position it marks out empty is not a cron field and the value it is written in
//! is not "a cron expression ... standard cron syntax"
//! (`smithy_query(query="doc", shape="ScheduleConfiguration", member="CronExpression")`).
//! Such a declaration therefore reports no `ScheduleConfiguration` at all, exactly as any
//! other value that is no cron expression does, and configures no trigger — so the service
//! starts no run of its own for it.
//!
//! The tables span what the rule generalises over: WHICH punctuation a field is written
//! with (every separator, each on its own, and a wildcard doubled), WHERE in the
//! expression the valueless field sits (first, middle, last), HOW MANY fields the
//! expression has (standard five and EventBridge Scheduler's six), and — as the positive
//! half of the same rule — fields whose separators do stand between values (lists, ranges,
//! steps, the nth-weekday qualifier, the month and day-of-week names, the day-of-month
//! qualifiers).
//!
//! Every request goes over the real wire path through the same router the emulator boots,
//! with the Amazon S3 service mounted beside this one on ONE shared engine, because a
//! definition is snapshotted by calling S3 in process exactly as the assembled emulator
//! does. A firing instant is a stored clock field, so the trigger is seeded from the store
//! rather than by racing the wall clock.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::RUN_TYPE_SCHEDULED;

use crate::definition_declared_schedule_trigger::{listed_runs, owe_one_firing, trigger};
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router_on, get_workflow, reported_cron, stage,
    BUCKET,
};
use crate::workflow_lifecycle::role_arn;

/// One row of the table: the value a definition declares its schedule as, and the cron
/// expression a read must report for it — `None` when that value is no cron expression, so
/// nothing is reported and no trigger is configured.
struct Field {
    case: &'static str,
    schedule: &'static str,
    cron: Option<&'static str>,
}

/// The table.
///
/// The accepted spellings are the ones the reader's own vocabulary names — the separators
/// `,`, `-`, `/` and `#`, the wildcards `*` and `?`, the day-of-month qualifiers `L` and
/// `LW`, and the three-letter month and day-of-week names — in the field counts standard
/// cron (five) and EventBridge Scheduler (six, with a year) are written with; none of them
/// is the five-field expression the sibling suites already read. The rejected spellings are
/// the same punctuation with no value beside it.
const FIELDS: &[Field] = &[
    // The positive half of the rule: separators standing BETWEEN values, one row per kind.
    Field {
        case: "a list, a range and a step, each between values",
        schedule: "0,30 8-17 1,15 */2 *",
        cron: Some("0,30 8-17 1,15 */2 *"),
    },
    Field {
        case: "named months and weekdays in a range and a list",
        schedule: "15 6 ? JAN-MAR MON,WED,FRI",
        cron: Some("15 6 ? JAN-MAR MON,WED,FRI"),
    },
    Field {
        case: "the nth-weekday qualifier between a name and a number",
        schedule: "0 9 ? * TUE#2",
        cron: Some("0 9 ? * TUE#2"),
    },
    Field {
        case: "a six-field expression carrying a year",
        schedule: "30 1 1 DEC ? 2031",
        cron: Some("30 1 1 DEC ? 2031"),
    },
    Field {
        case: "the weekday-of-the-last-day qualifier",
        schedule: "45 23 LW * ?",
        cron: Some("45 23 LW * ?"),
    },
    Field {
        case: "a step over a range",
        schedule: "0 0-23/3 L * ?",
        cron: Some("0 0-23/3 L * ?"),
    },
    // The rejected half: a field that marks out a position and leaves it empty.
    Field {
        case: "every field written as one range separator",
        schedule: "- - - - -",
        cron: None,
    },
    Field {
        case: "every field written as one list separator",
        schedule: ", , , , ,",
        cron: None,
    },
    Field {
        case: "every field written as one step separator",
        schedule: "/ / / / /",
        cron: None,
    },
    Field {
        case: "every field written as one nth-weekday qualifier",
        schedule: "# # # # #",
        cron: None,
    },
    Field {
        case: "a step with nothing to step by, in the first field",
        schedule: "*/ * * * *",
        cron: None,
    },
    Field {
        case: "a range missing its end, in the day-of-month field",
        schedule: "0 12 1- * *",
        cron: None,
    },
    Field {
        case: "a range missing its start, in the month field",
        schedule: "0 12 * -5 *",
        cron: None,
    },
    Field {
        case: "a list with an empty item, in the last field",
        schedule: "0 12 * * 1,,3",
        cron: None,
    },
    Field {
        case: "a wildcard doubled rather than separated",
        schedule: "** * * * *",
        cron: None,
    },
    Field {
        case: "a six-field expression whose year field carries no value",
        schedule: "0 12 * * ? -",
        cron: None,
    },
    Field {
        case: "a valueless field among five otherwise valid ones",
        schedule: "0 12 # * *",
        cron: None,
    },
];

/// The definition declaring `schedule`, written the way a DAG declares one.
fn definition(schedule: &str) -> String {
    format!("name: pipeline\nschedule: \"{schedule}\"\ntasks:\n  - id: extract\n")
}

#[tokio::test]
async fn a_schedule_field_carrying_no_value_is_no_cron_expression_to_report() {
    let router = definition_router_on(Arc::new(engine::InMemoryEngine::new()));
    create_bucket(&router).await;

    for (index, row) in FIELDS.iter().enumerate() {
        let case = row.case;
        let key = format!("dags/field-{index}.yaml");
        stage(&router, &key, definition(row.schedule).as_bytes()).await;
        let arn = create_workflow(&router, &format!("cronField{index}"), &key).await;

        let fetched = get_workflow(&router, &arn, None).await;
        assert_eq!(
            reported_cron(&fetched),
            row.cron,
            "case {case:?}: expected the cron expression {:?} for the declared {:?}, got {:?}",
            row.cron,
            row.schedule,
            fetched["ScheduleConfiguration"]
        );
        if row.cron.is_none() {
            assert!(
                fetched["ScheduleConfiguration"].is_null(),
                "case {case:?}: a value that is no cron expression reports no \
                 ScheduleConfiguration, got {:?}",
                fetched["ScheduleConfiguration"]
            );
        }
        // The definition content itself is reported whatever it declares: only the
        // schedule READ out of it is in question here.
        assert_eq!(
            fetched["WorkflowDefinition"].as_str(),
            Some(definition(row.schedule).as_str()),
            "case {case:?}: expected the snapshotted definition content, got {:?}",
            fetched["WorkflowDefinition"]
        );

        // The version listing answers from the same frozen snapshot, so it reports the
        // same thing.
        let (status, listed) =
            call(&router, "ListWorkflowVersions", json!({"WorkflowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflowVersions to succeed, got {status} {listed:?}"
        );
        let summaries = listed["WorkflowVersions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected the one created version to be listed, got {summaries:?}"
        );
        assert_eq!(
            reported_cron(&summaries[0]),
            row.cron,
            "case {case:?}: expected the version summary to report {:?}, got {:?}",
            row.cron,
            summaries[0]["ScheduleConfiguration"]
        );
    }
}

#[tokio::test]
async fn only_a_declared_cron_expression_arms_the_automated_trigger() {
    for (index, row) in FIELDS.iter().enumerate() {
        let case = row.case;
        // A fresh emulator per row, so one row's workflow cannot answer another's read.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = definition_router_on(Arc::clone(&engine));
        create_bucket(&router).await;
        let key = format!("dags/trigger-{index}.yaml");
        stage(&router, &key, definition(row.schedule).as_bytes()).await;
        let arn = create_workflow(&router, &format!("cronTrigger{index}"), &key).await;

        let automated = row.cron.is_some();
        assert_eq!(
            trigger(&engine, &arn).is_some(),
            automated,
            "case {case:?}: expected the service to {} a workflow declaring {:?} on its \
             own, got a next firing of {:?}",
            if automated { "execute" } else { "not execute" },
            row.schedule,
            trigger(&engine, &arn)
        );

        // One owed firing produces exactly one run nobody started — and a workflow whose
        // declaration is no cron expression has no firing to owe, so its history stays
        // empty however far the clock runs.
        let owed = owe_one_firing(&engine, &arn);
        assert_eq!(
            owed.is_some(),
            automated,
            "case {case:?}: only a workflow the service executes has a firing to owe, got \
             {owed:?}"
        );
        let runs = listed_runs(&router, &arn, case).await;
        assert_eq!(
            runs.len(),
            usize::from(automated),
            "case {case:?}: expected {} automated run(s) for the declared {:?}, got {runs:?}",
            usize::from(automated),
            row.schedule
        );
        if automated {
            assert_eq!(
                runs[0]["RunType"].as_str(),
                Some(RUN_TYPE_SCHEDULED),
                "case {case:?}: a run the workflow's own trigger started reports \
                 {RUN_TYPE_SCHEDULED}, got {:?}",
                runs[0]["RunType"]
            );
        }
    }
}

#[tokio::test]
async fn an_update_follows_the_new_definition_s_declaration_either_way() {
    // The rows differ along what the UPDATE does to the declaration: replaces a cron
    // expression with a value that is none, replaces such a value with a real expression,
    // and moves between two values that are both no expression at all.
    let rows = [
        (
            "a cron expression replaced by a field carrying no value",
            "0 5 * * *",
            Some("0 5 * * *"),
            "- - - - -",
            None,
        ),
        (
            "a field carrying no value replaced by a cron expression",
            "*/ * * * *",
            None,
            "0,30 8-17 1,15 */2 *",
            Some("0,30 8-17 1,15 */2 *"),
        ),
        (
            "one valueless field replaced by another",
            "# # # # #",
            None,
            "0 12 * * 1,,3",
            None,
        ),
    ];

    for (index, (case, first, first_cron, second, second_cron)) in rows.into_iter().enumerate() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = definition_router_on(Arc::clone(&engine));
        create_bucket(&router).await;
        let key = format!("dags/updated-{index}.yaml");
        stage(&router, &key, definition(first).as_bytes()).await;
        let arn = create_workflow(&router, &format!("cronUpdate{index}"), &key).await;
        assert_eq!(
            reported_cron(&get_workflow(&router, &arn, None).await),
            first_cron,
            "case {case:?}: the created version reports what its definition declares"
        );
        assert_eq!(
            trigger(&engine, &arn).is_some(),
            first_cron.is_some(),
            "case {case:?}: the created version's declaration decides its trigger, got {:?}",
            trigger(&engine, &arn)
        );

        stage(&router, &key, definition(second).as_bytes()).await;
        let (status, updated) = call(
            &router,
            "UpdateWorkflow",
            json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": role_arn(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );

        // ROUND TRIP: the promoted version is read back through the reader every workflow
        // is read through, and it is that version's declaration the trigger follows.
        assert_eq!(
            reported_cron(&get_workflow(&router, &arn, None).await),
            second_cron,
            "case {case:?}: expected the promoted version to report {second_cron:?}"
        );
        assert_eq!(
            trigger(&engine, &arn).is_some(),
            second_cron.is_some(),
            "case {case:?}: the promoted version's declaration decides the trigger, got {:?}",
            trigger(&engine, &arn)
        );

        let runs_before = listed_runs(&router, &arn, case).await.len();
        owe_one_firing(&engine, &arn);
        let runs = listed_runs(&router, &arn, case).await;
        assert_eq!(
            runs.len(),
            runs_before + usize::from(second_cron.is_some()),
            "case {case:?}: expected {} further automated run(s) after the update, got \
             {runs:?}",
            usize::from(second_cron.is_some())
        );
    }
}
