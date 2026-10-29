// SPDX-License-Identifier: CC0-1.0

//! A STATE MACHINE as the target of a rule: which execution a delivery starts,
//! end to end over the wire through the real `MultiServiceRouter` with Step
//! Functions mounted beside EventBridge — so the execution is read back where a
//! client reads it, through `ListExecutions` and `DescribeExecution`.
//!
//! The RULE asserted here: EventBridge invokes a state machine target with
//! `states:StartExecution`, whose `input` is the whole event envelope and whose
//! `name` is the id of the event that started it — "the name of the execution …
//! must be unique for your Amazon Web Services account, Region, and state
//! machine for 90 days" (`StartExecution$name`), and the event id is what makes
//! this delivery unique, so one event delivered to one target is ONE execution
//! however many times the delivery is made.
//!
//! The rows differ along how many events are published and how many rules name
//! the same state machine, which is what tells a name derived from the EVENT from
//! one minted per call.

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

/// Write a rule matching the published events, carrying the state machine at
/// `arn` as its only target.
async fn given_rule(emulator: &Emulator, name: &str, arn: &str) {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, written) = events(
        emulator,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {written}");
    let (status, targeted) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": name,
            "Targets": [{
                "Id": "t1",
                "Arn": arn,
                "RoleArn": format!(
                    "arn:aws:iam::{}:role/events-start-execution",
                    engine::emulator_account_id()
                ),
            }],
        }),
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

/// Each delivered event starts ONE execution, named after the event and carrying
/// the whole envelope as its input — over a TABLE whose rows differ along how
/// many events one publication carries.
#[tokio::test]
async fn a_state_machine_target_starts_one_execution_named_after_the_event() {
    // (case, the details of the entries one publication carries)
    let rows: [(&str, &[&str]); 3] = [
        ("one event", &[r#"{"orderId":"A-1"}"#]),
        (
            "three events in one publication",
            &[
                r#"{"orderId":"A-1"}"#,
                r#"{"orderId":"A-2"}"#,
                r#"{"orderId":"A-3"}"#,
            ],
        ),
        (
            "two events carrying the same detail",
            &[r#"{"orderId":"A-1"}"#, r#"{"orderId":"A-1"}"#],
        ),
    ];

    for (case, details) in rows {
        let emulator = emulator();
        let machine = given_state_machine(&emulator, "named-after-the-event").await;
        given_rule(&emulator, "starts", &machine).await;

        let owned: Vec<String> = details.iter().map(|detail| (*detail).to_string()).collect();
        let ids = publish_details(&emulator, &owned).await;

        let recorded = invocations(&emulator);
        assert_eq!(
            recorded.len(),
            ids.len(),
            "case {case:?}: expected one handover per published event, got {recorded:?}"
        );

        let mut started = executions(&emulator, &machine).await;
        started.sort();
        let mut expected: Vec<String> = ids.clone();
        expected.sort();
        let names: Vec<String> = started.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected one execution named after each event id, got {started:?}"
        );
        for (name, input) in &started {
            let envelope: Value = serde_json::from_str(input).unwrap_or_else(|error| {
                panic!("case {case:?}: the execution input is not JSON ({error}): {input:?}")
            });
            assert_eq!(
                envelope["id"].as_str(),
                Some(name.as_str()),
                "case {case:?}: expected the execution's input to be the event it is named \
                 after, got {envelope}"
            );
            assert_eq!(
                envelope["source"], json!(SOURCE),
                "case {case:?}: expected the whole envelope as the input, got {envelope}"
            );
        }
    }
}
