// SPDX-License-Identifier: CC0-1.0

//! A rule target that names another EVENT BUS is authorized against THAT bus's
//! resource policy, end to end over the wire through the real
//! `MultiServiceRouter` with SQS, IAM and CloudWatch mounted beside EventBridge.
//!
//! The RULE asserted here: reaching an event-bus target is PUBLISHING onto that
//! bus, so the destination bus's own resource policy (the document
//! `PutPermission` installs and `DescribeEventBus` reports) authorizes the
//! forward under the identity it is made as — and a statement of that policy
//! explicitly denying that identity the publication refuses it: nothing is
//! published onto the destination bus, so none of its rules matches the event and
//! none of their targets is handed anything. The refused event goes to the
//! forwarding target's `DeadLetterConfig` queue carrying the condition
//! `NO_PERMISSIONS`, and no target invocation having been made, nothing is
//! counted on the rule's `FailedInvocations`.
//!
//! The table spans the class rather than the one recorded case: its rows differ
//! along the EFFECT the destination policy states (none at all, an allow, a
//! deny), along the SPELLING of the principal the statement names (the
//! forwarding role, every principal, the role's account, another role), along
//! the ACTION element (the publication itself, a wildcard over the service,
//! another action of it), along the RESOURCE element (the destination bus,
//! another bus) and along the FORM the policy was installed in (a whole policy
//! document, or `PutPermission`'s own grant members).
//!
//! A twin control rule on the SOURCE bus, whose target is a queue no policy
//! denies, receives every event these rows publish — so an absence at the
//! destination is attributable to the destination policy rather than to the
//! publication not having happened.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one
//! engine) is the one `target_dispatch` owns. The rule and bus helpers below are
//! this suite's rather than that one's because every call here NAMES a bus — the
//! source bus, the destination bus — where that suite's helpers address the
//! account's default one and are pinned by the suites that use them; they are
//! `pub(crate)` because `bus_forward_failure_accounting`, which pins what a
//! refused forward is ACCOUNTED as, builds the same three rules over the same two
//! buses.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    InvocationOutcome, FAILED_INVOCATIONS_METRIC, NO_PERMISSIONS, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, events, given_queue, given_role, invocations, metric_sum, published_series,
    Emulator, DETAIL_TYPE, REGION, SOURCE,
};

/// The bus the event is published onto, and the bus the forwarding rule's target
/// names.
pub(crate) const SOURCE_BUS: &str = "forwarding-bus";
pub(crate) const DESTINATION_BUS: &str = "destination-bus";

/// The rules: the one whose target is the destination bus, the twin control on
/// the source bus, and the one on the destination bus that would deliver the
/// forwarded copy to a queue.
pub(crate) const FORWARDING_RULE: &str = "forwards";
pub(crate) const CONTROL_RULE: &str = "control";
pub(crate) const RECEIVING_RULE: &str = "receives";

/// The id of the one target each rule carries.
pub(crate) const TARGET_ID: &str = "t1";

/// The role the forward is made under. Its own inline policy ALLOWS the
/// publication, so every row below turns on the destination bus's policy alone.
pub(crate) const FORWARDING_ROLE: &str = "bus-forward-role";
const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// The ARN of the bus `name` of the Region every call here is signed for, as a
/// target and as a policy statement's resource name it.
pub(crate) fn bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The ARN of a role the account does NOT hold, for a statement that names a
/// principal other than the forwarding one.
fn other_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/some-other-role",
        engine::emulator_account_id()
    )
}

/// The ARN of the rule `name` on the bus `bus`, as a dead-letter message names
/// the rule whose delivery failed.
pub(crate) fn rule_arn(bus: &str, name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:rule/{bus}/{name}",
        engine::emulator_account_id()
    )
}

/// Create the bus `name`.
pub(crate) async fn given_bus(emulator: &Emulator, case: &str, name: &str) {
    let (status, created) = events(emulator, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus answered {status} {created}"
    );
}

/// A rule called `name` on `bus` matching every event published below, carrying
/// `targets`.
pub(crate) async fn given_rule(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    name: &str,
    targets: Value,
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
        "case {case:?}: PutRule {name} answered {status} {body}"
    );
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({ "Rule": name, "EventBusName": bus, "Targets": targets }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets on {name} answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets on {name} reported a failed entry: {body}"
    );
}

/// Publish one event onto the source bus, carrying `nonce` in its detail;
/// answers its `EventId`.
async fn publish(emulator: &Emulator, case: &str, nonce: &str) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": SOURCE_BUS,
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

/// What the row installs on the DESTINATION bus's resource policy.
#[derive(Debug, Clone, Copy)]
enum Policy {
    /// Nothing at all: a bus no `PutPermission` has reached.
    None,
    /// A whole policy document stating `effect` for `action` on `resource`,
    /// naming `principal`.
    Document {
        effect: &'static str,
        principal: Principal,
        action: ActionElement,
        resource: Resource,
    },
    /// `PutPermission`'s own grant members (`StatementId`, `Action`,
    /// `Principal`), which install an allow for an account.
    Grant,
}

/// Whom the row's statement names.
#[derive(Debug, Clone, Copy)]
enum Principal {
    /// The role the forward is made under.
    ForwardingRole,
    /// Every principal there is (`"*"`).
    Everyone,
    /// The account the forwarding role belongs to, by its id.
    TheAccount,
    /// A role other than the forwarding one.
    AnotherRole,
}

/// Which action element the row's statement carries.
#[derive(Debug, Clone, Copy)]
enum ActionElement {
    /// The publication itself.
    PutEvents,
    /// Every action of this service, through a wildcard.
    EveryEventsAction,
    /// Another action of this service.
    AnotherAction,
}

/// Which resource element the row's statement carries.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// The destination bus the forward publishes onto.
    DestinationBus,
    /// The bus the event was published on, which the forward does not publish
    /// onto.
    SourceBus,
}

impl Principal {
    /// The `Principal` element as the policy language writes it.
    fn element(self, role_arn: &str) -> Value {
        match self {
            Self::ForwardingRole => json!({ "AWS": role_arn }),
            Self::Everyone => json!("*"),
            Self::TheAccount => json!({ "AWS": engine::emulator_account_id() }),
            Self::AnotherRole => json!({ "AWS": other_role_arn() }),
        }
    }
}

impl ActionElement {
    fn element(self) -> &'static str {
        match self {
            Self::PutEvents => PUT_EVENTS_ACTION,
            Self::EveryEventsAction => "events:*",
            Self::AnotherAction => "events:PutRule",
        }
    }
}

impl Resource {
    fn element(self) -> String {
        match self {
            Self::DestinationBus => bus_arn(DESTINATION_BUS),
            Self::SourceBus => bus_arn(SOURCE_BUS),
        }
    }
}

/// Install the row's policy on the destination bus, and answer the document
/// `DescribeEventBus` must report back for it.
async fn given_destination_policy(
    emulator: &Emulator,
    case: &str,
    policy: Policy,
    role_arn: &str,
) -> Option<String> {
    let request = match policy {
        Policy::None => return None,
        Policy::Document {
            effect,
            principal,
            action,
            resource,
        } => {
            let document = json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "forwards",
                    "Effect": effect,
                    "Principal": principal.element(role_arn),
                    "Action": action.element(),
                    "Resource": resource.element(),
                }],
            })
            .to_string();
            json!({ "EventBusName": DESTINATION_BUS, "Policy": document })
        }
        Policy::Grant => json!({
            "EventBusName": DESTINATION_BUS,
            "StatementId": "forwards",
            "Action": PUT_EVENTS_ACTION,
            "Principal": engine::emulator_account_id(),
        }),
    };
    let (status, body) = events(emulator, "PutPermission", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutPermission answered {status} {body}"
    );
    // Read the installed policy back through the implemented reader, so what the
    // forward is decided against is what a client can see on the bus.
    let (status, described) = events(
        emulator,
        "DescribeEventBus",
        &json!({ "Name": DESTINATION_BUS }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeEventBus answered {status} {described}"
    );
    let reported = described["Policy"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the destination bus reports no Policy: {described}")
        })
        .to_string();
    assert!(
        reported.contains(PUT_EVENTS_ACTION) || reported.contains("events:"),
        "case {case:?}: expected the installed statement in the reported policy, got {reported}"
    );
    Some(reported)
}

/// One row: the policy the destination bus carries, and whether the forward
/// reaches that bus under it.
struct Row {
    case: &'static str,
    policy: Policy,
    forwarded: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a destination bus carrying no resource policy at all",
            policy: Policy::None,
            forwarded: true,
        },
        Row {
            case: "a policy allowing the forwarding role the publication",
            policy: Policy::Document {
                effect: "Allow",
                principal: Principal::ForwardingRole,
                action: ActionElement::PutEvents,
                resource: Resource::DestinationBus,
            },
            forwarded: true,
        },
        Row {
            case: "a policy allowing the account through PutPermission's grant members",
            policy: Policy::Grant,
            forwarded: true,
        },
        Row {
            case: "a policy denying the forwarding role the publication",
            policy: Policy::Document {
                effect: "Deny",
                principal: Principal::ForwardingRole,
                action: ActionElement::PutEvents,
                resource: Resource::DestinationBus,
            },
            forwarded: false,
        },
        Row {
            case: "a policy denying every principal the publication",
            policy: Policy::Document {
                effect: "Deny",
                principal: Principal::Everyone,
                action: ActionElement::PutEvents,
                resource: Resource::DestinationBus,
            },
            forwarded: false,
        },
        Row {
            case: "a policy denying the forwarding role's account the publication",
            policy: Policy::Document {
                effect: "Deny",
                principal: Principal::TheAccount,
                action: ActionElement::PutEvents,
                resource: Resource::DestinationBus,
            },
            forwarded: false,
        },
        Row {
            case: "a policy denying the forwarding role every action of the service",
            policy: Policy::Document {
                effect: "Deny",
                principal: Principal::ForwardingRole,
                action: ActionElement::EveryEventsAction,
                resource: Resource::DestinationBus,
            },
            forwarded: false,
        },
        Row {
            case: "a policy denying another role the publication",
            policy: Policy::Document {
                effect: "Deny",
                principal: Principal::AnotherRole,
                action: ActionElement::PutEvents,
                resource: Resource::DestinationBus,
            },
            forwarded: true,
        },
        Row {
            case: "a policy denying the forwarding role another action of the service",
            policy: Policy::Document {
                effect: "Deny",
                principal: Principal::ForwardingRole,
                action: ActionElement::AnotherAction,
                resource: Resource::DestinationBus,
            },
            forwarded: true,
        },
        Row {
            case: "a policy denying the forwarding role the publication onto another bus",
            policy: Policy::Document {
                effect: "Deny",
                principal: Principal::ForwardingRole,
                action: ActionElement::PutEvents,
                resource: Resource::SourceBus,
            },
            forwarded: true,
        },
    ]
}

#[tokio::test]
async fn the_destination_buss_resource_policy_authorizes_a_bus_to_bus_forward() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, SOURCE_BUS).await;
        given_bus(&emulator, case, DESTINATION_BUS).await;
        let (consumer_url, consumer_arn) = given_queue(&emulator, "destination-consumer").await;
        let (control_url, control_arn) = given_queue(&emulator, "control").await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        // The identity half of the decision allows the publication, so each row
        // turns on the destination bus's policy alone.
        let role_arn = given_role(
            &emulator,
            FORWARDING_ROLE,
            SERVICE_PRINCIPAL,
            Some(PUT_EVENTS_ACTION),
        )
        .await;

        given_rule(
            &emulator,
            case,
            DESTINATION_BUS,
            RECEIVING_RULE,
            json!([{ "Id": TARGET_ID, "Arn": consumer_arn }]),
        )
        .await;
        given_rule(
            &emulator,
            case,
            SOURCE_BUS,
            CONTROL_RULE,
            json!([{ "Id": TARGET_ID, "Arn": control_arn }]),
        )
        .await;
        let destination_arn = bus_arn(DESTINATION_BUS);
        given_rule(
            &emulator,
            case,
            SOURCE_BUS,
            FORWARDING_RULE,
            json!([{
                "Id": TARGET_ID,
                "Arn": destination_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }]),
        )
        .await;
        given_destination_policy(&emulator, case, row.policy, &role_arn).await;

        let event_id = publish(&emulator, case, "n-1").await;

        // The twin control: the publication reached the source bus's rules in
        // every row, so an absence at the destination is the destination
        // policy's doing.
        let control = drain(&emulator, &control_url).await;
        assert_eq!(
            control.len(),
            1,
            "case {case:?}: expected the control rule's queue to hold the event, got {control:?}"
        );

        // What the forward did or did not put on the destination bus: its rule's
        // target holds a copy only when the forward was authorized.
        let forwarded = drain(&emulator, &consumer_url).await;
        let expected_copies = usize::from(row.forwarded);
        assert_eq!(
            forwarded.len(),
            expected_copies,
            "case {case:?}: expected {expected_copies} copy/copies on the destination bus's \
             consumer queue, got {forwarded:?}"
        );

        // What was recorded about the handover to the bus target.
        let recorded = invocations(&emulator);
        let (_, _, outcome) = recorded
            .iter()
            .find(|(rule, target, _)| rule == FORWARDING_RULE && *target == destination_arn)
            .unwrap_or_else(|| {
                panic!("case {case:?}: nothing was recorded for the bus target: {recorded:?}")
            });
        let dead_letters = drain(&emulator, &dead_letter_url).await;
        if row.forwarded {
            assert_eq!(
                *outcome,
                InvocationOutcome::Invoked,
                "case {case:?}: expected the forward to go through, got {outcome:?}"
            );
            assert!(
                dead_letters.is_empty(),
                "case {case:?}: a forward that was published is not dead-lettered, \
                 got {dead_letters:?}"
            );
        } else {
            let InvocationOutcome::Failed { code, message } = outcome else {
                panic!("case {case:?}: expected a refused forward, got {outcome:?}");
            };
            assert_eq!(
                code, NO_PERMISSIONS,
                "case {case:?}: expected the condition {NO_PERMISSIONS}, got {outcome:?}"
            );
            assert!(
                message.contains(PUT_EVENTS_ACTION) && message.contains(&destination_arn),
                "case {case:?}: expected the refusal to name the action and the bus, \
                 got {message:?}"
            );
            // The refusal AWS records for it: the event on the forwarding
            // target's dead-letter queue, carrying the condition.
            assert_eq!(
                dead_letters.len(),
                1,
                "case {case:?}: expected the refused event on the dead-letter queue, \
                 got {dead_letters:?}"
            );
            let message = &dead_letters[0];
            let attributes = &message["MessageAttributes"];
            assert_eq!(
                attributes["ERROR_CODE"]["StringValue"],
                json!(NO_PERMISSIONS),
                "case {case:?}: expected the dead-letter ERROR_CODE {NO_PERMISSIONS}, \
                 got {message}"
            );
            assert_eq!(
                attributes["TARGET_ARN"]["StringValue"],
                json!(destination_arn),
                "case {case:?}: the dead-letter message names the bus target, got {message}"
            );
            assert_eq!(
                attributes["RULE_ARN"]["StringValue"],
                json!(rule_arn(SOURCE_BUS, FORWARDING_RULE)),
                "case {case:?}: the dead-letter message names the forwarding rule, got {message}"
            );
            let body = message["Body"].as_str().unwrap_or_default();
            assert!(
                body.contains(&event_id),
                "case {case:?}: expected the refused event itself on the dead-letter queue, \
                 got {body:?}"
            );
            // Nothing of the destination bus saw the event: no rule of it
            // matched, so no target of it was handed anything.
            assert!(
                !recorded
                    .iter()
                    .any(|(rule, _, _)| rule == RECEIVING_RULE),
                "case {case:?}: a refused forward reaches no rule of the destination bus, \
                 got {recorded:?}"
            );
        }

        // A refusal for want of a permission invokes no target, so nothing is
        // counted on the forwarding rule's FailedInvocations — and, the metric
        // being emitted only when there is something to count, the namespace
        // holds no such series at all.
        let failed = metric_sum(
            &emulator,
            FAILED_INVOCATIONS_METRIC,
            SOURCE_BUS,
            FORWARDING_RULE,
        )
        .await;
        assert_eq!(
            failed, 0.0,
            "case {case:?}: expected no failed invocation counted for the forwarding rule, \
             got {failed}"
        );
        let series = published_series(&emulator).await;
        assert!(
            !series
                .iter()
                .any(|entry| entry["MetricName"] == json!(FAILED_INVOCATIONS_METRIC)),
            "case {case:?}: expected no FailedInvocations series in EventBridge's namespace, \
             got {series:?}"
        );
    }
}
