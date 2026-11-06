// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `MonitorInstances`: the monitoring move it starts, and the layer it
//! validates the instance ids it is given in.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness, and every monitoring
//! state is read back through `DescribeInstances` rather than out of the store, so what is
//! asserted is what a caller sees. Each rule holds over a CLASS of requests — any batch
//! size, an instance still coming up, an id outside the instance-id form in any of the ways
//! a value can be outside it — so every assertion is driven from a TABLE whose rows differ
//! along what the rule generalises over; one request is a single row of it.
//!
//! The rules under test are:
//!
//! 1. the call turns detailed monitoring ON for every distinct instance it names: each one
//!    reports the transient `pending` at once and reaches `enabled` at the instant the clock
//!    owes it, whether or not anybody calls again, and the answer carries one entry per
//!    distinct instance in the order the request first names it;
//! 2. the `InstanceIds` list is validated as a request PARAMETER: a value outside the form
//!    EC2 issues an instance id in is refused `InvalidParameterValue`, naming the parameter
//!    and the form it expected, while a value INSIDE that form is looked up and an id this
//!    account holds no instance for is `InvalidInstanceID.NotFound`;
//! 3. `UnmonitorInstances` reads the same member of the same shape through the same layer,
//!    so both halves of the monitoring move refuse an id the same way;
//! 4. a refused call acts on no instance it named: the monitoring of every instance in the
//!    store is exactly what it was before.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    INSTANCE_IDS_PARAMETER, INSTANCE_ID_PREFIX, MONITORING_STATE_DISABLED,
    MONITORING_STATE_ENABLED, MONITORING_STATE_PENDING, TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child, child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names.
const AMI: &str = "ami-0123456789abcdef0";

/// Both halves of the monitoring move, which rule 3 holds over.
const MONITORING_ACTIONS: [&str; 2] = ["MonitorInstances", "UnmonitorInstances"];

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The `InstanceId.N` fields one request carries for `instance_ids`, as ec2Query flattens
/// the list (`MonitorInstancesRequest$InstanceIds` is spelled `InstanceId` on the wire).
fn instance_fields(instance_ids: &[String]) -> Vec<(String, String)> {
    instance_ids
        .iter()
        .enumerate()
        .map(|(index, instance_id)| p(&format!("InstanceId.{}", index + 1), instance_id.clone()))
        .collect()
}

/// One call that must succeed, as its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: Vec<(String, String)>,
) -> String {
    let (status, body) = ec2_query::call(router, REGION, action, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Launch one instance and answer its id.
async fn launch(router: &MultiServiceRouter, case: &str) -> String {
    let body = ok(
        router,
        case,
        "RunInstances",
        vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
    )
    .await;
    ec2_query::element(&body, "instanceId").unwrap_or_else(|| {
        panic!("case {case:?}: RunInstances must report the instance it minted: {body}")
    })
}

/// The detailed-monitoring state `DescribeInstances` reports for one instance, read through
/// the document so the instance's own `<monitoring><state>` is never taken for an
/// identically named member of something nested inside it.
async fn monitoring_state(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let body = ok(
        router,
        case,
        "DescribeInstances",
        vec![p("InstanceId.1", instance_id)],
    )
    .await;
    let parsed = document(case, &body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child_text(*node, "instanceId").as_deref() == Some(instance_id)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: DescribeInstances must report {instance_id}: {body}")
        });
    child(instance, "monitoring")
        .and_then(|monitoring| child_text(monitoring, "state"))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the report of {instance_id} must carry its monitoring state: \
                 {body}"
            )
        })
}

/// One row of the monitoring table: how many distinct instances the call names, how many
/// ids the request carries for them, and whether it is made while they are still coming up.
struct MonitoringRow {
    case: &'static str,
    distinct: usize,
    named: usize,
    while_pending: bool,
}

/// Rule 1: the call turns monitoring on for every distinct instance it names — `pending` at
/// once and `enabled` at the instant the clock owes it, read back through
/// `DescribeInstances`.
#[tokio::test]
async fn monitor_instances_turns_monitoring_on_for_every_distinct_instance_it_names() {
    let rows = [
        MonitoringRow {
            case: "one instance",
            distinct: 1,
            named: 1,
            while_pending: false,
        },
        MonitoringRow {
            case: "three instances",
            distinct: 3,
            named: 3,
            while_pending: false,
        },
        MonitoringRow {
            case: "two instances, the first named twice",
            distinct: 2,
            named: 3,
            while_pending: false,
        },
        MonitoringRow {
            case: "one instance named four times",
            distinct: 1,
            named: 4,
            while_pending: false,
        },
        MonitoringRow {
            case: "an instance still coming up",
            distinct: 1,
            named: 1,
            while_pending: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let (router, _engine) = ec2_query::emulator();

        let mut instance_ids = Vec::new();
        for _ in 0..row.distinct {
            instance_ids.push(launch(&router, case).await);
        }
        if !row.while_pending {
            past_the_transition().await;
        }
        for instance_id in &instance_ids {
            let state = monitoring_state(&router, case, instance_id).await;
            assert_eq!(
                state, MONITORING_STATE_DISABLED,
                "case {case:?}: a launch that asked for no detailed monitoring leaves \
                 {instance_id} at {MONITORING_STATE_DISABLED}, got {state:?}"
            );
        }

        // A row that names more ids than it has instances repeats the first one, which is
        // what the one-entry-per-instance rule is about.
        let mut named = instance_ids.clone();
        while named.len() < row.named {
            named.push(instance_ids[0].clone());
        }

        let body = ok(
            &router,
            case,
            "MonitorInstances",
            instance_fields(&named),
        )
        .await;
        let answered_ids = ec2_query::elements(&body, "instanceId");
        assert_eq!(
            answered_ids, instance_ids,
            "case {case:?}: one entry per distinct instance in the order the request first \
             names it, expected {instance_ids:?}, got {answered_ids:?} in {body}"
        );
        let answered_states = ec2_query::elements(&body, "state");
        assert_eq!(
            answered_states,
            vec![MONITORING_STATE_PENDING.to_string(); instance_ids.len()],
            "case {case:?}: every entry must report {MONITORING_STATE_PENDING}, got \
             {answered_states:?} in {body}"
        );

        // The transient state is what a caller reading straight afterwards sees.
        for instance_id in &instance_ids {
            let state = monitoring_state(&router, case, instance_id).await;
            assert_eq!(
                state, MONITORING_STATE_PENDING,
                "case {case:?}: {instance_id} must report {MONITORING_STATE_PENDING} \
                 straight after the call, got {state:?}"
            );
        }

        // And the move the clock owes falls due without anybody calling again.
        past_the_transition().await;
        for instance_id in &instance_ids {
            let state = monitoring_state(&router, case, instance_id).await;
            assert_eq!(
                state, MONITORING_STATE_ENABLED,
                "case {case:?}: {instance_id} must reach {MONITORING_STATE_ENABLED}, got \
                 {state:?}"
            );
        }
    }
}

/// One row of the id table: a value the request names an instance with, and whether it is
/// inside the form EC2 issues an instance id in.
struct IdRow {
    case: &'static str,
    /// The value the request carries. A row inside the form names no instance of this
    /// account, so the lookup is what refuses it.
    instance_id: &'static str,
    /// Whether the value is inside the instance-id form, so it is LOOKED UP rather than
    /// refused for the parameter.
    inside_the_form: bool,
}

/// The ways a value can be outside the form EC2 issues an instance id in, and the two forms
/// that are inside it.
///
/// The forms themselves are `crate::state::instance_id_is_well_formed`'s: `i-` and either 8
/// or 17 lowercase hexadecimal digits, the long form under a leading `0`, and never the
/// all-zero placeholder.
fn id_rows() -> Vec<IdRow> {
    vec![
        IdRow {
            case: "a value that reads like no resource id at all",
            instance_id: "not-an-instance",
            inside_the_form: false,
        },
        IdRow {
            case: "the prefix with no digits",
            instance_id: "i-",
            inside_the_form: false,
        },
        IdRow {
            case: "one digit short of the long form",
            instance_id: "i-0123456789abcde",
            inside_the_form: false,
        },
        IdRow {
            case: "the long form in upper case",
            instance_id: "i-0123456789ABCDEF0",
            inside_the_form: false,
        },
        IdRow {
            case: "a digit that is not hexadecimal",
            instance_id: "i-0123456789abcdefg",
            inside_the_form: false,
        },
        IdRow {
            case: "the long form with a leading digit above zero",
            instance_id: "i-1234567890abcdef0",
            inside_the_form: false,
        },
        IdRow {
            case: "the all-zero placeholder",
            instance_id: "i-00000000000000000",
            inside_the_form: false,
        },
        IdRow {
            case: "an id of another resource kind",
            instance_id: "vol-0123456789abcdef0",
            inside_the_form: false,
        },
        IdRow {
            case: "the long form, naming no instance of this account",
            instance_id: "i-0fedcba987654321f",
            inside_the_form: true,
        },
        IdRow {
            case: "the short form, naming no instance of this account",
            instance_id: "i-0abc1234",
            inside_the_form: true,
        },
    ]
}

/// What a refusal of `instance_id` at the PARAMETER layer says: the parameter the service
/// names and the id form it expected, built from the request's own value.
fn parameter_value_message(instance_id: &str) -> String {
    format!(
        "Value ({instance_id}) for parameter {INSTANCE_IDS_PARAMETER} is invalid. \
         Expected: '{INSTANCE_ID_PREFIX}-...'."
    )
}

/// Rules 2, 3 and 4: both halves of the monitoring move validate their `InstanceIds` list
/// as a request parameter — a value outside the instance-id form is refused for the
/// parameter, a value inside it is looked up — and neither acts on the instance it could
/// resolve beside the id it could not.
#[tokio::test]
async fn a_monitoring_call_refuses_a_value_outside_the_instance_id_form_for_the_parameter() {
    for action in MONITORING_ACTIONS {
        let (router, _engine) = ec2_query::emulator();
        let held = launch(&router, action).await;
        past_the_transition().await;

        for row in id_rows() {
            let IdRow {
                case,
                instance_id,
                inside_the_form,
            } = row;
            // The id under test is named BESIDE one this account really holds, so each row
            // also pins that a refused call acts on neither of them.
            let before = monitoring_state(&router, case, &held).await;
            let (status, body) = ec2_query::call(
                &router,
                REGION,
                action,
                &instance_fields(&[held.clone(), instance_id.to_string()]),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from {action} for {instance_id}, got \
                 {status} with body {body}"
            );

            let (expected_code, expected_message) = if inside_the_form {
                (
                    "InvalidInstanceID.NotFound".to_string(),
                    format!("The instance ID '{instance_id}' does not exist"),
                )
            } else {
                (
                    "InvalidParameterValue".to_string(),
                    parameter_value_message(instance_id),
                )
            };
            let code = ec2_query::error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected the error code {expected_code} from {action}, got \
                 {code:?} with body {body}"
            );
            let message = ec2_query::error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected the message {expected_message:?} from {action}, got \
                 {message:?} with body {body}"
            );

            let after = monitoring_state(&router, case, &held).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused {action} must leave the monitoring of {held} as it \
                 was, expected {before:?}, got {after:?}"
            );
        }
    }
}
