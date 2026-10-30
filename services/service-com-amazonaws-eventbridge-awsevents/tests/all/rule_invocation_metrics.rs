// SPDX-License-Identifier: CC0-1.0

//! What came of every target handover is counted on the MATCHED RULE's own
//! AWS/Events invocation metrics, dimensioned by the bus the rule is on and the
//! rule's name.
//!
//! The RULE asserted here: for each target of a rule whose `EventPattern`
//! matched a published event, EventBridge publishes exactly one AWS/Events
//! datapoint of value 1 for that delivery, dimensioned by BOTH the event bus and
//! the rule name — `Invocations` when the target's API call was made, and
//! `FailedInvocations` when the invocation was permanently refused (the same
//! refusal that sends the event to the target's dead-letter queue) — and
//! attributes it to that one rule, so a sibling rule on the same bus, matched by
//! the same event, is counted only for its own deliveries. The dimension set is
//! the rule's whole identity because a rule's name alone does not name a rule
//! ("CloudWatch treats each unique combination of dimensions as a separate
//! metric", `GetMetricStatistics$Dimensions`).
//!
//! The table spans the class rather than the one recorded case: its rows differ
//! along the BUS the rules live on (a custom bus and the default one), along HOW
//! MANY matching events the round carries (so a count is a count rather than a
//! flag), along the FORM of the grant the delivering rule's role carries (the
//! target's own ARN as the statement's resource, or the action named through a
//! wildcard) and along the FORM in which the other rule's role withholds the
//! call (a policy allowing another action, or one explicitly denying this one).
//!
//! The harness (the router with EventBridge, SQS, Step Functions, IAM and
//! CloudWatch on one engine) is the one `target_dispatch` owns.

use serde_json::json;
use service_com_amazonaws_eventbridge_awsevents::state::{
    DEFAULT_EVENT_BUS_NAME, FAILED_INVOCATIONS_METRIC, INVOCATIONS_METRIC, MATCHED_EVENTS_METRIC,
    NO_PERMISSIONS, SERVICE_PRINCIPAL, TRIGGERED_RULES_METRIC,
};

use crate::target_dispatch::{
    drain, emulator, events, given_queue, given_state_machine, iam, metric_sum, ordered,
    published_series, rule_series, sfn, trust_policy, xml_text, Emulator, DETAIL_TYPE, REGION,
    SOURCE,
};

/// The rule whose target role lets the invocation through, and the one whose
/// role withholds it. Both match every event published below.
const DELIVERING_RULE: &str = "delivering";
const REFUSED_RULE: &str = "refused";

/// The id of the one target each rule carries.
const TARGET_ID: &str = "t1";

/// The action a state-machine target's invocation performs, and so the one the
/// target's role has to be allowed to make.
const TARGET_ACTION: &str = "states:StartExecution";

/// An identity policy with one statement, stating `effect` for `action` on
/// `resource`.
fn identity_statement(effect: &str, action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": action, "Resource": resource }],
    })
    .to_string()
}

/// A role called `name` trusting the EventBridge service principal and carrying
/// `document` as its one inline policy; answers the ARN IAM persisted.
async fn given_role(emulator: &Emulator, case: &str, name: &str, document: &str) -> String {
    let trust = trust_policy(SERVICE_PRINCIPAL);
    let (status, created) = iam(
        emulator,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    let arn = xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"));
    let (status, written) = iam(
        emulator,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "delivery"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy answered {status}: {written}"
    );
    arn
}

/// The bus a row's rules live on, created when it is not the default one.
async fn given_bus(emulator: &Emulator, case: &str, bus: &str) {
    if bus == DEFAULT_EVENT_BUS_NAME {
        return;
    }
    let (status, created) = events(emulator, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus answered {status} {created}"
    );
}

/// A rule called `name` on `bus`, matching every event published below, carrying
/// one state-machine target invoked under `role_arn` and dead-lettering to
/// `dead_letter_arn`.
async fn given_rule(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    name: &str,
    target_arn: &str,
    role_arn: &str,
    dead_letter_arn: &str,
) {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule answered {status} {body}"
    );
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": bus,
            "Targets": [{
                "Id": TARGET_ID,
                "Arn": target_arn,
                "RoleArn": role_arn,
                // The recorded wiring: no retry at all, and a dead-letter queue
                // for what the invocation refuses.
                "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: a target is authorized at delivery time, so PutTargets accepts it: {body}"
    );
}

/// Publish one event onto `bus`, carrying `nonce` in its detail; answers its
/// `EventId`.
async fn publish_onto(emulator: &Emulator, case: &str, bus: &str, nonce: &str) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "nonce": nonce }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no EventId in {body}"))
        .to_string()
}

/// The inputs of every execution of the state machine `arn`.
async fn execution_inputs(emulator: &Emulator, case: &str, arn: &str) -> Vec<String> {
    let (status, listed) = sfn(
        emulator,
        "ListExecutions",
        &json!({ "stateMachineArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListExecutions answered {status} {listed}"
    );
    let mut inputs = Vec::new();
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
            "case {case:?}: DescribeExecution answered {status} {described}"
        );
        inputs.push(described["input"].as_str().unwrap_or_default().to_string());
    }
    inputs
}

/// The form in which the delivering rule's role carries the grant.
#[derive(Debug, Clone, Copy)]
enum Grant {
    /// The statement names the target state machine's own ARN as its resource.
    OnTheTarget,
    /// The statement names the action through a wildcard, on every resource.
    ThroughAWildcard,
}

/// The form in which the other rule's role withholds the call.
#[derive(Debug, Clone, Copy)]
enum Withheld {
    /// Its policy allows another action of the same service.
    AnotherAction,
    /// Its policy explicitly denies this one.
    ExplicitDeny,
}

/// One row: which bus, how many matching events, and the two policy forms.
struct Row {
    case: &'static str,
    bus: &'static str,
    events: usize,
    grant: Grant,
    withheld: Withheld,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a custom bus, one event, the grant on the target itself",
            bus: "orders-bus",
            events: 1,
            grant: Grant::OnTheTarget,
            withheld: Withheld::AnotherAction,
        },
        Row {
            case: "the default bus, three events, the grant through a wildcard",
            bus: DEFAULT_EVENT_BUS_NAME,
            events: 3,
            grant: Grant::ThroughAWildcard,
            withheld: Withheld::ExplicitDeny,
        },
        Row {
            case: "another custom bus, two events, an explicit deny on the other rule",
            bus: "payments.bus-2",
            events: 2,
            grant: Grant::OnTheTarget,
            withheld: Withheld::ExplicitDeny,
        },
    ]
}

#[tokio::test]
async fn each_delivery_is_counted_on_its_own_rules_invocation_metric() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, row.bus).await;
        let delivering_target = given_state_machine(&emulator, "delivering-flow").await;
        let refused_target = given_state_machine(&emulator, "refused-flow").await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;

        let granting = match row.grant {
            Grant::OnTheTarget => identity_statement("Allow", TARGET_ACTION, &delivering_target),
            Grant::ThroughAWildcard => identity_statement("Allow", "states:*", "*"),
        };
        let withholding = match row.withheld {
            Withheld::AnotherAction => {
                identity_statement("Allow", "states:DescribeStateMachine", "*")
            }
            Withheld::ExplicitDeny => identity_statement("Deny", TARGET_ACTION, "*"),
        };
        let delivering_role = given_role(&emulator, case, "delivering-role", &granting).await;
        let refused_role = given_role(&emulator, case, "refused-role", &withholding).await;

        given_rule(
            &emulator,
            case,
            row.bus,
            DELIVERING_RULE,
            &delivering_target,
            &delivering_role,
            &dead_letter_arn,
        )
        .await;
        given_rule(
            &emulator,
            case,
            row.bus,
            REFUSED_RULE,
            &refused_target,
            &refused_role,
            &dead_letter_arn,
        )
        .await;

        let mut event_ids = Vec::new();
        for round in 0..row.events {
            event_ids.push(publish_onto(&emulator, case, row.bus, &format!("n-{round}")).await);
        }

        // The delivering rule's target holds one execution per published event,
        // each carrying that event's envelope as its input; the refused rule's
        // target holds nothing.
        let delivered = execution_inputs(&emulator, case, &delivering_target).await;
        assert_eq!(
            delivered.len(),
            row.events,
            "case {case:?}: expected {} execution(s) of the allowed target, got {delivered:?}",
            row.events
        );
        for event_id in &event_ids {
            assert!(
                delivered.iter().any(|input| input.contains(event_id)),
                "case {case:?}: expected an execution whose input carries the envelope \
                 {event_id}, got {delivered:?}"
            );
        }
        let refused = execution_inputs(&emulator, case, &refused_target).await;
        assert!(
            refused.is_empty(),
            "case {case:?}: a refused delivery starts nothing, got {refused:?}"
        );

        // The refusal the metric counts is the same one the dead-letter message
        // carries: one copy per refused delivery, naming the rule, the target
        // and why.
        let dead_letters = drain(&emulator, &dead_letter_url).await;
        assert_eq!(
            dead_letters.len(),
            row.events,
            "case {case:?}: expected {} dead-letter message(s), got {dead_letters:?}",
            row.events
        );
        // A rule on the `default` bus carries no bus segment in its ARN; a rule
        // on any other bus names that bus.
        let refused_rule_resource = if row.bus == DEFAULT_EVENT_BUS_NAME {
            format!("rule/{REFUSED_RULE}")
        } else {
            format!("rule/{}/{REFUSED_RULE}", row.bus)
        };
        let refused_rule_arn = format!(
            "arn:aws:events:{REGION}:{}:{refused_rule_resource}",
            engine::emulator_account_id()
        );
        for message in &dead_letters {
            let attributes = &message["MessageAttributes"];
            assert_eq!(
                attributes["ERROR_CODE"]["StringValue"],
                json!(NO_PERMISSIONS),
                "case {case:?}: expected the dead-letter ERROR_CODE {NO_PERMISSIONS}, \
                 got {message}"
            );
            assert_eq!(
                attributes["TARGET_ARN"]["StringValue"],
                json!(refused_target),
                "case {case:?}: the dead-letter message names the refused target, got {message}"
            );
            assert_eq!(
                attributes["RULE_ARN"]["StringValue"],
                json!(refused_rule_arn),
                "case {case:?}: the dead-letter message names the refused rule, got {message}"
            );
            let reported = attributes["ERROR_MESSAGE"]["StringValue"]
                .as_str()
                .unwrap_or_default();
            assert!(
                reported.contains(TARGET_ACTION)
                    && reported.contains("assumed-role/refused-role/"),
                "case {case:?}: the refusal names the action and the role session it was \
                 attempted as, got {reported:?}"
            );
        }

        // The metrics: one datapoint per delivery, on the metric that delivery's
        // outcome decides, under the bus and the rule it belongs to — and
        // nothing attributed to the sibling rule matched by the same events.
        let expected = f64::from(u32::try_from(row.events).unwrap_or(u32::MAX));
        let rows = [
            (INVOCATIONS_METRIC, DELIVERING_RULE, expected),
            (FAILED_INVOCATIONS_METRIC, DELIVERING_RULE, 0.0),
            (FAILED_INVOCATIONS_METRIC, REFUSED_RULE, expected),
            (INVOCATIONS_METRIC, REFUSED_RULE, 0.0),
        ];
        for (metric, rule, want) in rows {
            let got = metric_sum(&emulator, metric, row.bus, rule).await;
            assert_eq!(
                got, want,
                "case {case:?}: expected {metric} for rule {rule} on bus {} to sum to {want}, \
                 got {got}",
                row.bus
            );
        }

        // And the series themselves: each metric is published under the bus AND
        // the rule and under the rule name alone, so the two rules of one bus —
        // and the same rule name on another bus — are separate metrics. Both
        // rules matched every event, so both carry the match series; only the
        // delivery series differ.
        let series = published_series(&emulator).await;
        let mut expected_series = rule_series(
            row.bus,
            DELIVERING_RULE,
            &[
                INVOCATIONS_METRIC,
                MATCHED_EVENTS_METRIC,
                TRIGGERED_RULES_METRIC,
            ],
        );
        expected_series.extend(rule_series(
            row.bus,
            REFUSED_RULE,
            &[
                FAILED_INVOCATIONS_METRIC,
                MATCHED_EVENTS_METRIC,
                TRIGGERED_RULES_METRIC,
            ],
        ));
        assert_eq!(
            ordered(&series),
            ordered(&expected_series),
            "case {case:?}: expected one series per metric per dimension set of each rule, \
             got {series:?}"
        );
    }
}
