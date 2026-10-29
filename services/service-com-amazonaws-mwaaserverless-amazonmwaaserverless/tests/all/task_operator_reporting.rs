// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `OperatorName` a task instance reports out of the operator
//! its workflow definition declared.
//!
//! The rule under test: a value this service reports out of the caller's definition file
//! is reported only when it is one the modelled member that carries it can hold. Both
//! `GetTaskInstanceResponse$OperatorName` and `TaskInstanceSummary$OperatorName` target
//! `GenericString`, modelled `@length(min: 1, max: 255)` with `@pattern(^.*$)`
//! (`smithy_query(query="shape",
//! shape="GetTaskInstanceResponse,TaskInstanceSummary,GenericString")`), so a declared
//! operator name inside those constraints is reported verbatim by both readers and one
//! outside them is reported as no member at all — never clipped, because a cut operator
//! name names a different operator than the definition declared. The task still runs
//! either way: an operator name is not what an instance is reported BY, so the run
//! executes the task and reports its outcome whatever the declaration carried.
//!
//! Every test drives the real wire path through the same router the emulator boots, with
//! the Amazon S3 service mounted beside this one on ONE shared engine, because a
//! definition is a YAML object a run's version snapshotted out of Amazon S3. The table
//! spans what the rule generalises over: whether an operator is declared at all, both
//! published length bounds and one character past each of them, a declaration of several
//! hundred characters, the same lengths written in multi-byte characters (the modelled
//! length counts characters, not bytes), a value whose punctuation and spaces no
//! identifier would carry but the member's pattern does, and a value carrying a line
//! break, which that pattern does not.

use crate::run_execution_outcome::{read_task_instance, run_to_completion};
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router, stage,
};
use serde_json::{json, Value};

/// The fewest characters the `GenericString` both `OperatorName` members target can
/// carry (`smithy_query(query="shape", shape="GenericString")` declares it 1..255).
const MIN_OPERATOR_NAME_CHARS: usize = 1;

/// The most characters that same `GenericString` can carry.
const MAX_OPERATOR_NAME_CHARS: usize = 255;

/// The identifier every row's single task is declared, stored and reported under.
const TASK_ID: &str = "t1";

/// The `TaskInstanceStatus` a task that ran to completion reports
/// (`smithy_query(query="shape", shape="TaskInstanceStatus")`).
const TASK_SUCCESS: &str = "SUCCESS";

/// One row: the operator a definition's single task declares, and the `OperatorName` the
/// two readers must therefore report for the instance of that task.
struct Row {
    case: String,
    /// The scalar the task entry declares `operator:` with, written exactly as the YAML
    /// document carries it, and `None` for an entry that declares no operator at all.
    declared: Option<String>,
    /// The `OperatorName` both readers must report, and `None` for a declaration the
    /// modelled member cannot carry, which is reported as no member at all.
    expected: Option<String>,
}

/// `character`, `count` times.
fn repeated(character: char, count: usize) -> String {
    std::iter::repeat_n(character, count).collect()
}

/// `value` as a double-quoted YAML scalar, so the document carries it verbatim however
/// it is punctuated. None of the values below holds a quote or a backslash of its own.
fn quoted(value: &str) -> String {
    format!("\"{value}\"")
}

/// The table. Rows differ along the declared value's presence, length, character width
/// and form — never merely along the workflow's name.
fn rows() -> Vec<Row> {
    let shortest = repeated('a', MIN_OPERATOR_NAME_CHARS);
    let longest = repeated('A', MAX_OPERATOR_NAME_CHARS);
    let one_past_longest = repeated('A', MAX_OPERATOR_NAME_CHARS + 1);
    // Two bytes per character, so these two span the modelled bound in CHARACTERS while
    // holding twice as many bytes.
    let longest_wide = repeated('é', MAX_OPERATOR_NAME_CHARS);
    let one_past_longest_wide = repeated('é', MAX_OPERATOR_NAME_CHARS + 1);
    vec![
        Row {
            case: "an ordinary operator name".to_string(),
            declared: Some(quoted("EmptyOperator")),
            expected: Some("EmptyOperator".to_string()),
        },
        Row {
            case: "a task entry declaring no operator".to_string(),
            declared: None,
            expected: None,
        },
        Row {
            case: format!("the shortest name the member carries ({MIN_OPERATOR_NAME_CHARS})"),
            declared: Some(quoted(&shortest)),
            expected: Some(shortest.clone()),
        },
        Row {
            case: "an empty declared operator name".to_string(),
            declared: Some(quoted("")),
            expected: None,
        },
        Row {
            case: format!("the longest name the member carries ({MAX_OPERATOR_NAME_CHARS})"),
            declared: Some(quoted(&longest)),
            expected: Some(longest.clone()),
        },
        Row {
            case: format!(
                "one character past the modelled maximum ({})",
                MAX_OPERATOR_NAME_CHARS + 1
            ),
            declared: Some(quoted(&one_past_longest)),
            expected: None,
        },
        Row {
            case: "a declaration of three hundred characters".to_string(),
            declared: Some(quoted(&repeated('b', 300))),
            expected: None,
        },
        Row {
            case: format!("{MAX_OPERATOR_NAME_CHARS} multi-byte characters"),
            declared: Some(quoted(&longest_wide)),
            expected: Some(longest_wide.clone()),
        },
        Row {
            case: format!("{} multi-byte characters", MAX_OPERATOR_NAME_CHARS + 1),
            declared: Some(quoted(&one_past_longest_wide)),
            expected: None,
        },
        Row {
            // The member's pattern is `^.*$`, so a name no identifier shape would carry
            // is still one this member can: the check is the modelled one, not a
            // spelling rule of this emulator's own.
            case: "a name carrying spaces and punctuation".to_string(),
            declared: Some(quoted("my custom.operator-v2 (beta)")),
            expected: Some("my custom.operator-v2 (beta)".to_string()),
        },
        Row {
            // A double-quoted YAML scalar turns the escape into a real line break, which
            // that same pattern does not carry.
            case: "a name carrying a line break".to_string(),
            declared: Some(r#""Empty\nOperator""#.to_string()),
            expected: None,
        },
    ]
}

/// The definition one row stages: a single task under `TASK_ID`, declaring the row's
/// operator scalar when it has one.
fn definition(row: &Row) -> String {
    match &row.declared {
        Some(scalar) => format!("tasks:\n  - id: {TASK_ID}\n    operator: {scalar}\n"),
        None => format!("tasks:\n  - id: {TASK_ID}\n"),
    }
}

/// The `OperatorName` a reported member holds, and `None` when the member is absent (or
/// explicitly null, which is the same answer on the wire).
fn reported(member: &Value) -> Option<&str> {
    member.as_str()
}

#[tokio::test]
async fn an_instance_reports_the_declared_operator_only_when_the_modelled_member_carries_it() {
    let router = definition_router();
    create_bucket(&router).await;

    // Every run is started FIRST and polled afterwards, so the rows' executions overlap
    // on the one clock they all run on rather than being waited for one after another.
    let rows = rows();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let key = format!("dags/operator-{index}.yaml");
        stage(&router, &key, definition(row).as_bytes()).await;
        let arn = create_workflow(&router, &format!("operatorName{index}"), &key).await;
        let (status, run) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected StartWorkflowRun to succeed, got {status} {run:?}",
            row.case
        );
        let run_id = run["RunId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a started run has a RunId", row.case))
            .to_string();
        started.push((row, arn, run_id));
    }

    for (row, arn, run_id) in started {
        let case = row.case.as_str();
        let expected = row.expected.as_deref();
        run_to_completion(&router, &arn, &run_id, case).await;

        // The task ran whatever its declaration carried: an unreportable operator name
        // refuses nothing.
        let read = read_task_instance(&router, &arn, &run_id, TASK_ID, case).await;
        assert_eq!(
            read["Status"].as_str(),
            Some(TASK_SUCCESS),
            "case {case:?}: the declared task must still run, got {:?}",
            read["Status"]
        );
        assert_eq!(
            read["TaskId"].as_str(),
            Some(TASK_ID),
            "case {case:?}: the instance must name the task it executes, got {:?}",
            read["TaskId"]
        );
        assert_eq!(
            reported(&read["OperatorName"]),
            expected,
            "case {case:?}: GetTaskInstance must report OperatorName {expected:?}, got {:?}",
            read["OperatorName"]
        );

        // The listing reads the same record through the other modelled member, so it
        // answers with the same value.
        let (status, page) = call(
            &router,
            "ListTaskInstances",
            json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTaskInstances to succeed, got {status} {page:?}"
        );
        let summaries = page["TaskInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TaskInstances list, got {page:?}"))
            .clone();
        let summary = summaries
            .iter()
            .find(|summary| summary["TaskInstanceId"].as_str() == Some(TASK_ID))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a summary for {TASK_ID:?}, got {summaries:?}")
            });
        assert_eq!(
            reported(&summary["OperatorName"]),
            expected,
            "case {case:?}: ListTaskInstances must report OperatorName {expected:?}, got {:?}",
            summary["OperatorName"]
        );
    }
}
