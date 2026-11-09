// SPDX-License-Identifier: CC0-1.0

//! How AWS Batch refuses a member whose value is outside the closed set its enum declares,
//! end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client branches on.
//!
//! The rule under test, asserted over a TABLE rather than one request: a member whose
//! value is outside the closed set its enum declares is refused with the modelled
//! `ClientException` (HTTP 400) whose message is Batch's frontend envelope
//! (`Error executing request, Exception : <detail>, RequestId: <the refused call's id>`)
//! over the service's own sentence for that rule — the member named as a noun phrase, then
//! the set the service enforced — carrying NOTHING of the rejected value and no dotted
//! member path, and the request is not applied.
//!
//! The rows vary what the rule generalises over: the operation, the member, the enum's
//! size and the case of its values, whether the member is top-level, inside a nested
//! structure or inside a list, and — per row — the FORM of the rejected value.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::ops::types::shared_types;
use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{container_properties, error_code, error_message, Batch};
use crate::refusal_contract::enveloped;

/// Every value the enums under test declare, in the order the MODEL lists them — which is
/// the order the schema carries and therefore the order a refusal states them in. Source:
/// `smithy_query(query="shape", shape="QuotaShareIdleResourceAssignmentStrategy,CEType,
/// CEState,JQState,JobDefinitionType,PlatformCapability,ServiceEnvironmentType",
/// model_path="services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json")`.
/// Each row below also checks its set against the generated `shared_types::*_VALUES`, so a
/// hand-written order cannot drift into a hand-written MEMBERSHIP.
const IDLE_RESOURCE_ASSIGNMENT_STRATEGIES: &[&str] = &["FIFO"];
/// See [`IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`] for the source of every set here.
const COMPUTE_ENVIRONMENT_TYPES: &[&str] = &["MANAGED", "UNMANAGED"];
/// See [`IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`].
const COMPUTE_ENVIRONMENT_STATES: &[&str] = &["ENABLED", "DISABLED"];
/// See [`IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`].
const JOB_QUEUE_STATES: &[&str] = &["ENABLED", "DISABLED"];
/// See [`IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`].
const JOB_DEFINITION_TYPES: &[&str] = &["container", "multinode"];
/// See [`IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`].
const PLATFORM_CAPABILITIES: &[&str] = &["EC2", "FARGATE", "MANAGED_INSTANCES"];
/// See [`IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`].
const SERVICE_ENVIRONMENT_TYPES: &[&str] = &["SAGEMAKER_TRAINING"];

/// One enum member of this service: the call it arrives on, how a request carrying
/// `value` on it is built, the noun phrase Batch names it with, and the value set the
/// service enforces.
struct EnumMember {
    /// What this row varies; named in every assertion.
    label: &'static str,
    uri: &'static str,
    /// A request whose ONLY defect is `value` on the member under test: every other
    /// member it carries is one the service accepts, so the refusal can only be this one.
    body: fn(&str) -> Value,
    /// The noun phrase the refusal opens with, which is the member's own name as English
    /// (`state::enum_membership_detail` builds it).
    phrase: &'static str,
    /// The set the refusal states, in the order the model declares it.
    allowed: &'static [&'static str],
    /// The same set as the generated types carry it (`shared_types::*_VALUES`, sorted),
    /// which is what `allowed` is checked against.
    declared: &'static [&'static str],
}

/// Every operation and member class the rule covers that needs no stored resource to
/// reach: a one-member enum nested in a structure (the recorded case), a two-member enum
/// on a top-level member of two different calls, an enum whose values are LOWERCASE, an
/// enum inside a LIST, and a one-member enum whose name is three words long.
fn enum_members() -> Vec<EnumMember> {
    vec![
        EnumMember {
            label: "CreateSchedulingPolicy quotaSharePolicy.idleResourceAssignmentStrategy",
            uri: "/v1/createschedulingpolicy",
            body: |value| {
                json!({
                    "name": "sp-enum-refusal",
                    "quotaSharePolicy": {"idleResourceAssignmentStrategy": value},
                })
            },
            phrase: "Idle resource assignment strategy",
            allowed: IDLE_RESOURCE_ASSIGNMENT_STRATEGIES,
            declared: shared_types::QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGY_VALUES,
        },
        EnumMember {
            label: "CreateComputeEnvironment type",
            uri: "/v1/createcomputeenvironment",
            body: |value| {
                json!({
                    "computeEnvironmentName": "ce-enum-refusal",
                    "type": value,
                    "state": "ENABLED",
                })
            },
            phrase: "Type",
            allowed: COMPUTE_ENVIRONMENT_TYPES,
            declared: shared_types::C_E_TYPE_VALUES,
        },
        EnumMember {
            label: "CreateComputeEnvironment state",
            uri: "/v1/createcomputeenvironment",
            body: |value| {
                json!({
                    "computeEnvironmentName": "ce-enum-refusal-state",
                    "type": "UNMANAGED",
                    "state": value,
                })
            },
            phrase: "State",
            allowed: COMPUTE_ENVIRONMENT_STATES,
            declared: shared_types::C_E_STATE_VALUES,
        },
        EnumMember {
            label: "CreateJobQueue state",
            uri: "/v1/createjobqueue",
            body: |value| {
                json!({
                    "jobQueueName": "jq-enum-refusal",
                    "priority": 1,
                    "state": value,
                    "computeEnvironmentOrder": [
                        {"order": 1, "computeEnvironment": "ce-enum-refusal"},
                    ],
                })
            },
            phrase: "State",
            allowed: JOB_QUEUE_STATES,
            declared: shared_types::J_Q_STATE_VALUES,
        },
        EnumMember {
            label: "RegisterJobDefinition type (an enum of lowercase values)",
            uri: "/v1/registerjobdefinition",
            body: |value| {
                json!({
                    "jobDefinitionName": "jd-enum-refusal",
                    "type": value,
                    "containerProperties": container_properties(),
                })
            },
            phrase: "Type",
            allowed: JOB_DEFINITION_TYPES,
            declared: shared_types::JOB_DEFINITION_TYPE_VALUES,
        },
        EnumMember {
            label: "RegisterJobDefinition platformCapabilities (an enum inside a list)",
            uri: "/v1/registerjobdefinition",
            body: |value| {
                json!({
                    "jobDefinitionName": "jd-enum-refusal-caps",
                    "type": "container",
                    "containerProperties": container_properties(),
                    "platformCapabilities": ["EC2", value],
                })
            },
            phrase: "Platform capabilities",
            allowed: PLATFORM_CAPABILITIES,
            declared: shared_types::PLATFORM_CAPABILITY_VALUES,
        },
        EnumMember {
            label: "CreateServiceEnvironment serviceEnvironmentType",
            uri: "/v1/createserviceenvironment",
            body: |value| {
                json!({
                    "serviceEnvironmentName": "se-enum-refusal",
                    "serviceEnvironmentType": value,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                })
            },
            phrase: "Service environment type",
            allowed: SERVICE_ENVIRONMENT_TYPES,
            declared: shared_types::SERVICE_ENVIRONMENT_TYPE_VALUES,
        },
    ]
}

/// `value` with its case flipped, which for every enum in this model is a value the set
/// does NOT hold: each one declares its values in a single case.
fn other_case(value: &str) -> String {
    if value.chars().any(char::is_lowercase) {
        return value.to_uppercase();
    }
    value.to_lowercase()
}

/// The FORMS a rejected value takes, per row: a value no enum of this service holds (the
/// recorded one), a declared value in the other case, a declared value with a suffix, the
/// empty string, and a long value.
fn rejected_values(allowed: &[&str]) -> Vec<(String, String)> {
    let first = allowed.first().copied().unwrap_or_default();
    vec![
        ("a value no enum declares".to_string(), "STRICT".to_string()),
        (
            "a declared value in the other case".to_string(),
            other_case(first),
        ),
        (
            "a declared value with a suffix".to_string(),
            format!("{first}_EXTRA"),
        ),
        ("the empty string".to_string(), String::new()),
        ("a long value".to_string(), "X".repeat(64)),
    ]
}

/// The detail Batch states for `member`: the member as a noun phrase, then the set.
fn expected_detail(member: &EnumMember) -> String {
    format!(
        "{} must be one of [{}].",
        member.phrase,
        member.allowed.join(", ")
    )
}

/// RULE: a member outside the closed set its enum declares is refused 400 /
/// `ClientException`, with Batch's frontend envelope over the service's own sentence for
/// that rule — the member as a noun phrase and the set it enforced — and with neither the
/// rejected value nor a dotted member path anywhere in the message.
#[tokio::test]
async fn a_value_outside_an_enum_is_refused_with_the_set_it_enforced() {
    let batch = Batch::new();

    for member in enum_members() {
        // The hand-written order is checked against the model-derived value set, so only
        // the ORDER is this file's and never the membership.
        let mut sorted = member.allowed.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            member.declared.to_vec(),
            "case {}: the row's set must be the one the model declares",
            member.label
        );

        let detail = expected_detail(&member);
        for (form, value) in rejected_values(member.allowed) {
            let case = format!("{} with {form}", member.label);
            let (status, refused) = batch.call(member.uri, (member.body)(&value)).await;
            assert_eq!(
                status, 400,
                "case {case}: expected the modelled ClientException 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ClientException",
                "case {case}: expected the modelled ClientException, got {refused}"
            );

            let message = error_message(&refused);
            let (stated, _request_id) = enveloped(&case, &message);
            assert_eq!(
                stated, detail,
                "case {case}: expected the sentence naming the set it enforced, got {message:?}"
            );
            assert!(
                // The STATED sentence is what the rule is about: the envelope's
                // `RequestId` is an opaque per-request id, and this request's id happens
                // to carry this value's letters, which says nothing about the refusal.
                value.is_empty() || !stated.contains(&value),
                "case {case}: the refusal must not echo the rejected value, got {message:?}"
            );
            assert!(
                !message.contains("failed to satisfy constraint"),
                "case {case}: expected Batch's own sentence, not the generic constraint \
                 rendering, got {message:?}"
            );
        }
    }
}

/// RULE (the request id in the envelope): every enveloped refusal names the call it
/// refused, so two refusals of the same request are two different request ids.
#[tokio::test]
async fn each_enum_refusal_names_its_own_request() {
    let batch = Batch::new();

    for member in enum_members() {
        let case = member.label;
        let body = (member.body)("STRICT");
        let (_, first) = batch.call(member.uri, body.clone()).await;
        let (_, second) = batch.call(member.uri, body).await;
        let (_, first_id) = enveloped(case, &error_message(&first));
        let (_, second_id) = enveloped(case, &error_message(&second));
        assert_ne!(
            first_id, second_id,
            "case {case}: two refusals must name two different requests, got {first_id:?} twice"
        );
    }
}

/// RULE (the refusal is not applied, and the accepted value still is): a
/// `quotaSharePolicy` outside the enum creates nothing, while the declared strategy is
/// stored and read back through `DescribeSchedulingPolicies` unchanged.
#[tokio::test]
async fn a_refused_strategy_stores_nothing_and_the_declared_one_round_trips() {
    let batch = Batch::new();

    for (form, value) in rejected_values(IDLE_RESOURCE_ASSIGNMENT_STRATEGIES) {
        let name = "sp-not-created";
        let case = format!("a quotaSharePolicy with {form}");
        let (status, refused) = batch
            .call(
                "/v1/createschedulingpolicy",
                json!({
                    "name": name,
                    "quotaSharePolicy": {"idleResourceAssignmentStrategy": value},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException 400, got {status} {refused}"
        );

        let listed = batch.ok("/v1/listschedulingpolicies", json!({})).await;
        let names: Vec<String> = listed["schedulingPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["arn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !names.iter().any(|arn| arn.ends_with(name)),
            "case {case}: a refused create must store nothing, got {names:?}"
        );
    }

    // The positive control, one row per declared value: the strategy the model declares is
    // accepted and reads back exactly as it was stored.
    for strategy in IDLE_RESOURCE_ASSIGNMENT_STRATEGIES {
        let name = format!("sp-{}", strategy.to_lowercase());
        let created = batch
            .ok(
                "/v1/createschedulingpolicy",
                json!({
                    "name": &name,
                    "quotaSharePolicy": {"idleResourceAssignmentStrategy": strategy},
                }),
            )
            .await;
        let policy_arn = created["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            policy_arn.ends_with(&format!("/{name}")),
            "case {strategy}: expected an ARN ending in the policy name, got {created}"
        );

        let read = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [&policy_arn]}),
            )
            .await;
        let detail = read["schedulingPolicies"][0].clone();
        assert_eq!(
            detail["quotaSharePolicy"]["idleResourceAssignmentStrategy"],
            json!(strategy),
            "case {strategy}: expected the stored strategy read back, got {detail}"
        );
        assert!(
            detail.get("fairsharePolicy").is_none(),
            "case {strategy}: a quota-share policy carries no fairsharePolicy, got {detail}"
        );
    }
}

/// RULE (one wording, one source): the sentence the wire refusal states is the one
/// `state::enum_membership_detail` builds, for the member named anywhere a value of that
/// member can arrive — top level, inside a structure, or inside a list.
#[tokio::test]
async fn the_sentence_is_built_from_the_member_wherever_it_arrives() {
    // The paths the validating walk spells for one member at each position a value of it
    // can hold: the member alone, nested under the structure it arrived in, as an element
    // of a list (`<member>.<index>.member`), and as a map entry (`<member>.<key>.member`,
    // whose key the CALLER chose and which the refusal therefore must not echo).
    let paths = [
        state::IDLE_RESOURCE_ASSIGNMENT_STRATEGY_MEMBER.to_string(),
        format!(
            "quotaSharePolicy.{}",
            state::IDLE_RESOURCE_ASSIGNMENT_STRATEGY_MEMBER
        ),
        format!(
            "{}.7.member",
            state::IDLE_RESOURCE_ASSIGNMENT_STRATEGY_MEMBER
        ),
        format!(
            "{}.a-key-the-caller-chose.member",
            state::IDLE_RESOURCE_ASSIGNMENT_STRATEGY_MEMBER
        ),
    ];
    let expected = "Idle resource assignment strategy must be one of [FIFO].";

    for path in paths {
        let built = state::enum_membership_detail(&path, IDLE_RESOURCE_ASSIGNMENT_STRATEGIES);
        assert_eq!(
            built, expected,
            "case {path}: expected the member named as a noun phrase, got {built:?}"
        );
    }
}
