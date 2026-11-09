// SPDX-License-Identifier: CC0-1.0

//! A STATE MACHINE as the target of a rule: which execution a delivery starts,
//! end to end over the wire through the real `MultiServiceRouter` with Step
//! Functions mounted beside EventBridge — so the execution is read back where a
//! client reads it, through `ListExecutions` and `DescribeExecution`.
//!
//! The RULE asserted here: EventBridge invokes a state machine target with
//! `states:StartExecution`, whose `input` is the whole event envelope and whose
//! `name` is `<the event's id>_<a UUID>` — the event id is the join key between
//! the published event and the execution it started, and the UUID is minted per
//! INVOCATION, because an execution name "must be unique for your Amazon Web
//! Services account, Region, and state machine for 90 days"
//! (`StartExecution$name`) while one event may be delivered to several
//! state-machine targets and re-delivered to any of them. So two targets of one
//! event, and two deliveries of one event, each get an execution of their own and
//! no two of those names are equal.
//!
//! The rows differ along how many events one publication carries, how many
//! machines one rule names, and how many publications the same rule serves —
//! which is what tells a name minted per invocation from one derived from the
//! event alone.

use serde_json::{json, Value};

use crate::target_dispatch::{
    emulator, events, given_state_machine, invocations, publish_details, sfn, Emulator,
    DETAIL_TYPE, SOURCE,
};

/// Every execution of the state machine at `arn`, as `(name, input)`.
async fn executions(emulator: &Emulator, arn: &str) -> Vec<(String, String)> {
    let (status, listed) = sfn(
        emulator,
        "ListExecutions",
        &json!({ "stateMachineArn": arn }),
    )
    .await;
    assert_eq!(status, 200, "ListExecutions answered {status} {listed}");
    let mut found = Vec::new();
    for execution in listed["executions"].as_array().cloned().unwrap_or_default() {
        let execution_arn = execution["executionArn"].as_str().unwrap_or_default();
        let (status, described) = sfn(
            emulator,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeExecution answered {status} {described}"
        );
        found.push((
            described["name"].as_str().unwrap_or_default().to_string(),
            described["input"].as_str().unwrap_or_default().to_string(),
        ));
    }
    found
}

/// Write a rule matching the published events, carrying one target per state
/// machine ARN in `machines`.
async fn given_rule(emulator: &Emulator, name: &str, machines: &[String]) {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, written) = events(
        emulator,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {written}");
    let targets: Vec<Value> = machines
        .iter()
        .enumerate()
        .map(|(index, arn)| {
            json!({
                "Id": format!("t{index}"),
                "Arn": arn,
                "RoleArn": format!(
                    "arn:aws:iam::{}:role/events-start-execution",
                    engine::emulator_account_id()
                ),
            })
        })
        .collect();
    let (status, targeted) = events(
        emulator,
        "PutTargets",
        &json!({ "Rule": name, "Targets": targets }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: PutTargets answered {status} {targeted}"
    );
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {targeted}"
    );
}

/// The id of the event an execution named `name` was started for, or `None` when
/// the name is not `<an event id>_<a UUID>` at all.
///
/// The form is the one AWS mints: the event id, the one `_` separator, and a
/// UUID in the 8-4-4-4-12 lowercase-hex form every AWS-minted UUID carries, which
/// is what [`is_uuid`] reads.
fn event_id_in(name: &str) -> Option<&str> {
    let (event_id, suffix) = name.rsplit_once('_')?;
    is_uuid(suffix).then_some(event_id)
}

/// Whether `text` has the 8-4-4-4-12 lowercase-hex form of a UUID.
fn is_uuid(text: &str) -> bool {
    let mut groups = text.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_uppercase())
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// One row: how many events one publication carries, how many machines the rule
/// names, and how many times that publication is made.
struct Row {
    case: &'static str,
    details: &'static [&'static str],
    machines: usize,
    publications: usize,
}

/// Each delivery starts ONE execution named `<the event's id>_<a UUID>`, carrying
/// the whole envelope as its input — over a TABLE whose rows differ along how
/// many events, how many machines and how many publications are in play.
#[tokio::test]
async fn a_state_machine_target_starts_an_execution_named_after_the_event() {
    let rows = [
        Row {
            case: "one event, one machine",
            details: &[r#"{"orderId":"A-1"}"#],
            machines: 1,
            publications: 1,
        },
        Row {
            case: "three events in one publication",
            details: &[
                r#"{"orderId":"A-1"}"#,
                r#"{"orderId":"A-2"}"#,
                r#"{"orderId":"A-3"}"#,
            ],
            machines: 1,
            publications: 1,
        },
        Row {
            case: "two events carrying the same detail",
            details: &[r#"{"orderId":"A-1"}"#, r#"{"orderId":"A-1"}"#],
            machines: 1,
            publications: 1,
        },
        Row {
            case: "one event fanned out to two machines",
            details: &[r#"{"orderId":"A-1"}"#],
            machines: 2,
            publications: 1,
        },
        Row {
            case: "the same rule serving two publications",
            details: &[r#"{"orderId":"A-1"}"#],
            machines: 1,
            publications: 2,
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = emulator();
        let mut machines = Vec::new();
        for index in 0..row.machines {
            machines
                .push(given_state_machine(&emulator, &format!("named-after-the-event-{index}")).await);
        }
        given_rule(&emulator, "starts", &machines).await;

        let owned: Vec<String> = row.details.iter().map(|detail| (*detail).to_string()).collect();
        let mut ids = Vec::new();
        for _ in 0..row.publications {
            ids.extend(publish_details(&emulator, &owned).await);
        }

        let recorded = invocations(&emulator);
        assert_eq!(
            recorded.len(),
            ids.len() * row.machines,
            "case {case:?}: expected one handover per published event per machine, got \
             {recorded:?}"
        );

        // Every execution of every machine, and the event each one was named
        // after.
        let mut names = Vec::new();
        for machine in &machines {
            let started = executions(&emulator, machine).await;
            assert_eq!(
                started.len(),
                ids.len(),
                "case {case:?}: expected one execution of {machine} per published event, got \
                 {started:?}"
            );
            let mut named_for: Vec<String> = Vec::new();
            for (name, input) in &started {
                let event_id = event_id_in(name).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected an execution named <event id>_<uuid>, got \
                         {name:?}"
                    )
                });
                named_for.push(event_id.to_string());
                names.push(name.clone());
                let envelope: Value = serde_json::from_str(input).unwrap_or_else(|error| {
                    panic!("case {case:?}: the execution input is not JSON ({error}): {input:?}")
                });
                assert_eq!(
                    envelope["id"].as_str(),
                    Some(event_id),
                    "case {case:?}: expected the input to be the event the execution is named \
                     after, got {envelope}"
                );
                assert_eq!(
                    envelope["source"],
                    json!(SOURCE),
                    "case {case:?}: expected the whole envelope as the input, got {envelope}"
                );
            }
            named_for.sort();
            let mut expected = ids.clone();
            expected.sort();
            assert_eq!(
                named_for, expected,
                "case {case:?}: expected one execution named after each published event, got \
                 {started:?}"
            );
        }
        // No two executions share a name, however many targets and publications
        // the one event reached.
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            names.len(),
            "case {case:?}: expected every execution name to be minted per invocation, got \
             {names:?}"
        );
    }
}

/// An EXPRESS state machine is started the same way: its workflow type bars
/// `ListExecutions` and `DescribeExecution` ("this API action is not supported by
/// EXPRESS state machines"), so what is asserted here is the handover itself —
/// the `StartExecution` EventBridge made under the name it minted was ACCEPTED,
/// rather than refused or never attempted.
#[tokio::test]
async fn an_express_state_machine_target_is_started_too() {
    use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;

    let case = "an EXPRESS machine as the only target";
    let emulator = emulator();
    let role = format!(
        "arn:aws:iam::{}:role/state-machine-execution",
        engine::emulator_account_id()
    );
    let (status, created) = sfn(
        &emulator,
        "CreateStateMachine",
        &json!({
            "name": "express-target",
            "definition": r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
            "roleArn": role,
            "type": "EXPRESS",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStateMachine answered {status} {created}"
    );
    let machine = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no stateMachineArn in {created}"))
        .to_string();
    given_rule(&emulator, "starts-express", &[machine]).await;

    let ids = publish_details(&emulator, &[r#"{"orderId":"A-1"}"#.to_string()]).await;
    let recorded = invocations(&emulator);
    assert_eq!(
        recorded.len(),
        ids.len(),
        "case {case:?}: expected one handover per published event, got {recorded:?}"
    );
    for (_, _, outcome) in &recorded {
        assert_eq!(
            outcome,
            &InvocationOutcome::Invoked,
            "case {case:?}: expected the EXPRESS machine to accept the execution, got \
             {outcome:?}"
        );
    }
}
