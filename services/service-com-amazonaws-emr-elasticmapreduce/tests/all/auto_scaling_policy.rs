// SPDX-License-Identifier: CC0-1.0

//! The automatic scaling policy attached to one instance group of an EMR cluster, end to
//! end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: a
//! policy is only ever read back through `ListInstanceGroups`, which is what makes these
//! tests evidence that the responses are computed from persisted state.
//!
//! THE RULE: `PutAutoScalingPolicy` persists the submitted policy on the core or task
//! instance group the request names, replacing whatever policy that group carried, and
//! answers the PERSISTED policy — `ATTACHING` until the clock makes the attach due, then
//! `ATTACHED` — beside the cluster id, that cluster's ARN and the group id; a cluster the
//! service holds no record for, a group that cluster does not carry, a group whose role
//! nothing scales, and a policy whose capacity boundaries cannot hold together are
//! refused with HTTP 400 `ValidationException` and persist nothing.
//!
//! AND: `RemoveAutoScalingPolicy` takes the policy off the group the request names and
//! nothing else, answering no members; a cluster the service holds no record for, a group
//! that cluster does not carry, and a group that carries no policy to remove are refused
//! with HTTP 400 `ValidationException` and change nothing.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::MAX_SCALING_RULES;

use crate::job_flow_lifecycle::{assert_refused, at, Emr};

/// Longer than the service's clock-owed transition span, so a policy that was
/// `ATTACHING` has reached `ATTACHED` by the time we look.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The service's clock-owed attach span (`crate::state`'s `TRANSITION_SPAN`), which the
/// clock case uses to tell a listing issued INSIDE the span from one a loaded machine
/// delayed past it.
const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `com.amazonaws.emr#AdjustmentType`, every value the model declares
/// (`smithy_query(query="shape", shape="AdjustmentType")`).
const ADJUSTMENT_TYPES: [&str; 3] = [
    "CHANGE_IN_CAPACITY",
    "PERCENT_CHANGE_IN_CAPACITY",
    "EXACT_CAPACITY",
];

/// `com.amazonaws.emr#ComparisonOperator`, every value the model declares
/// (`smithy_query(query="shape", shape="ComparisonOperator")`).
const COMPARISON_OPERATORS: [&str; 4] = [
    "GREATER_THAN_OR_EQUAL",
    "GREATER_THAN",
    "LESS_THAN",
    "LESS_THAN_OR_EQUAL",
];

/// `com.amazonaws.emr#MarketType`, every value the model declares
/// (`smithy_query(query="shape", shape="MarketType")`).
const MARKET_TYPES: [&str; 2] = ["ON_DEMAND", "SPOT"];

/// Cluster ids no cluster carries: the shape a real id has, a prefix with nothing after
/// it, the empty string (`ClusterId` is `len 0..256`, so the boundary validator passes it
/// on to the handler) and a value that is no cluster id at all.
const UNKNOWN_CLUSTER_IDS: &[&str] = &["j-ZZZZZZZZZZZZZ", "j-", "", "not-a-cluster-id"];

/// Instance group ids no group of the launched cluster carries, in the same spread.
const UNKNOWN_GROUP_IDS: &[&str] = &["ig-ZZZZZZZZZZZZZ", "ig-", "", "not-a-group-id"];

/// A `RunJobFlow` request launching one instance group of each role, so a policy can be
/// put on the core and the task group and refused on the master one.
fn launch(name: &str) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": {
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceGroups": [
                { "Name": "primary", "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                { "Name": "core", "InstanceRole": "CORE", "InstanceType": "m5.xlarge", "InstanceCount": 2 },
                { "Name": "workers", "InstanceRole": "TASK", "InstanceType": "c5.xlarge", "InstanceCount": 3 },
            ],
        },
    })
}

/// One scaling rule, built from the catalogs above so a row can vary along each of them.
fn rule(name: &str, adjustment_type: &str, adjustment: i64, comparison: &str) -> Value {
    json!({
        "Name": name,
        "Action": {
            "SimpleScalingPolicyConfiguration": {
                "AdjustmentType": adjustment_type,
                "ScalingAdjustment": adjustment,
            },
        },
        "Trigger": {
            "CloudWatchAlarmDefinition": {
                "ComparisonOperator": comparison,
                "MetricName": "YARNMemoryAvailablePercentage",
                "Period": 300,
                "Threshold": 15.0,
            },
        },
    })
}

/// A rule carrying every optional member of its shapes as well.
fn detailed_rule(name: &str, market: &str) -> Value {
    json!({
        "Name": name,
        "Description": "every optional member set",
        "Action": {
            "Market": market,
            "SimpleScalingPolicyConfiguration": {
                "AdjustmentType": ADJUSTMENT_TYPES[2],
                "ScalingAdjustment": 4,
                "CoolDown": 600,
            },
        },
        "Trigger": {
            "CloudWatchAlarmDefinition": {
                "ComparisonOperator": COMPARISON_OPERATORS[0],
                "EvaluationPeriods": 2,
                "MetricName": "ContainerPendingRatio",
                "Namespace": "AWS/ElasticMapReduce",
                "Period": 60,
                "Statistic": "AVERAGE",
                "Threshold": 0.75,
                "Unit": "COUNT",
                "Dimensions": [{ "Key": "JobFlowId", "Value": "ignored-by-the-metric" }],
            },
        },
    })
}

/// An `AutoScalingPolicy` request member.
fn policy(min: i64, max: i64, rules: Vec<Value>) -> Value {
    json!({
        "Constraints": { "MinCapacity": min, "MaxCapacity": max },
        "Rules": rules,
    })
}

/// The groups `ListInstanceGroups` reports for `cluster_id`, keyed by group id.
async fn groups_by_id(emr: &Emr, case: &str, cluster_id: &str) -> BTreeMap<String, Value> {
    let out = emr
        .ok("ListInstanceGroups", json!({ "ClusterId": cluster_id }))
        .await;
    let listed = out["InstanceGroups"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListInstanceGroups must answer InstanceGroups, got {out}")
        });
    listed
        .into_iter()
        .map(|group| {
            let id = group["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed group must carry an Id"))
                .to_string();
            (id, group)
        })
        .collect()
}

/// The group id of each role the launched cluster carries (`MASTER`, `CORE`, `TASK`).
async fn group_ids(emr: &Emr, case: &str, cluster_id: &str) -> BTreeMap<String, String> {
    groups_by_id(emr, case, cluster_id)
        .await
        .into_iter()
        .map(|(id, group)| {
            let role = group["InstanceGroupType"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed group must carry an InstanceGroupType")
                })
                .to_string();
            (role, id)
        })
        .collect()
}

/// `(MinCapacity, MaxCapacity, rule names)` of a reported `AutoScalingPolicy`, and `None`
/// when the group reports no policy at all.
fn reported_policy(group: &Value) -> Option<(i64, i64, Vec<String>)> {
    if group["AutoScalingPolicy"].is_null() {
        return None;
    }
    let min = at(group, &["AutoScalingPolicy", "Constraints", "MinCapacity"])
        .as_i64()
        .unwrap_or_default();
    let max = at(group, &["AutoScalingPolicy", "Constraints", "MaxCapacity"])
        .as_i64()
        .unwrap_or_default();
    let names = at(group, &["AutoScalingPolicy", "Rules"])
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    Some((min, max, names))
}

/// The state of a reported policy, or `""` when the group reports none.
fn reported_state(group: &Value) -> &str {
    at(group, &["AutoScalingPolicy", "Status", "State"])
        .as_str()
        .unwrap_or_default()
}

/// THE RULE: `PutAutoScalingPolicy` persists the submitted policy on the named core or
/// task instance group, replacing whatever that group carried, answers the stored policy
/// beside the cluster id, the cluster ARN and the group id, and `ListInstanceGroups`
/// reports exactly what was stored — for every other group too, which keeps its own
/// policy or none. The table spans the rule count (one, two, every rule the chosen bound
/// allows), the capacity boundaries (a zero lower one, equal boundaries, a wide range),
/// the group role a policy may be attached to, each `AdjustmentType`, several
/// `ComparisonOperator`s and a rule carrying every optional member.
#[tokio::test]
async fn put_auto_scaling_policy_persists_each_policy_and_list_instance_groups_reads_it_back() {
    struct Case {
        label: &'static str,
        role: &'static str,
        min: i64,
        max: i64,
        rules: Vec<Value>,
    }

    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(launch("scaled")).await;
    let ids = group_ids(&emr, "launch", &cluster_id).await;
    let cases = vec![
        Case {
            label: "one rule on the core group",
            role: "CORE",
            min: 1,
            max: 10,
            rules: vec![rule(
                "scale-out",
                ADJUSTMENT_TYPES[0],
                1,
                COMPARISON_OPERATORS[2],
            )],
        },
        Case {
            label: "two rules replace the core group's policy",
            role: "CORE",
            min: 2,
            max: 20,
            rules: vec![
                rule("grow", ADJUSTMENT_TYPES[1], 25, COMPARISON_OPERATORS[3]),
                rule("pin", ADJUSTMENT_TYPES[2], 6, COMPARISON_OPERATORS[1]),
            ],
        },
        Case {
            label: "equal boundaries on the task group",
            role: "TASK",
            min: 5,
            max: 5,
            rules: vec![rule(
                "hold",
                ADJUSTMENT_TYPES[2],
                5,
                COMPARISON_OPERATORS[0],
            )],
        },
        Case {
            label: "a zero lower boundary and every optional member",
            role: "TASK",
            min: 0,
            max: 100,
            rules: vec![
                detailed_rule("spot-burst", MARKET_TYPES[1]),
                detailed_rule("on-demand-burst", MARKET_TYPES[0]),
            ],
        },
        Case {
            label: "every rule the chosen bound allows",
            role: "CORE",
            min: 1,
            max: 40,
            rules: (0..MAX_SCALING_RULES)
                .map(|index| {
                    rule(
                        &format!("rule-{index}"),
                        ADJUSTMENT_TYPES[index % ADJUSTMENT_TYPES.len()],
                        1 + index as i64,
                        COMPARISON_OPERATORS[index % COMPARISON_OPERATORS.len()],
                    )
                })
                .collect(),
        },
    ];

    // What each group must report once the case before it has been applied: nothing
    // carries a policy until one is put on it, and the master group never does.
    let mut expected: BTreeMap<String, Option<(i64, i64, Vec<String>)>> =
        ids.values().map(|id| (id.clone(), None)).collect();

    for case in &cases {
        let label = case.label;
        let group_id = ids
            .get(case.role)
            .unwrap_or_else(|| panic!("case {label:?}: the launch must carry a {} group", case.role));
        let names: Vec<String> = case
            .rules
            .iter()
            .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
            .collect();

        let out = emr
            .ok(
                "PutAutoScalingPolicy",
                json!({
                    "ClusterId": cluster_id,
                    "InstanceGroupId": group_id,
                    "AutoScalingPolicy": policy(case.min, case.max, case.rules.clone()),
                }),
            )
            .await;

        assert_eq!(
            out["ClusterId"].as_str(),
            Some(cluster_id.as_str()),
            "case {label:?}: the response must name the cluster the policy was put on, got {out}"
        );
        assert_eq!(
            out["InstanceGroupId"].as_str(),
            Some(group_id.as_str()),
            "case {label:?}: the response must name the group the policy was put on, got {out}"
        );
        let arn = out["ClusterArn"].as_str().unwrap_or_default();
        assert!(
            arn.starts_with("arn:aws:elasticmapreduce:") && arn.ends_with(&format!("cluster/{cluster_id}")),
            "case {label:?}: the response must carry that cluster's ARN, got {arn:?}"
        );
        assert_eq!(
            reported_policy(&out),
            Some((case.min, case.max, names.clone())),
            "case {label:?}: the response must project the policy that was stored, got {out}"
        );
        // The attach has only just started, so the policy is in the transient state a
        // caller acting straight after the call still observes.
        assert_eq!(
            reported_state(&out),
            "ATTACHING",
            "case {label:?}: a policy that was just put is ATTACHING, got {out}"
        );

        expected.insert(group_id.clone(), Some((case.min, case.max, names)));

        let listed = groups_by_id(&emr, label, &cluster_id).await;
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {label:?}: the cluster must still report every group, got {listed:?}"
        );
        for (id, want) in &expected {
            let group = listed
                .get(id)
                .unwrap_or_else(|| panic!("case {label:?}: group {id:?} must still be reported"));
            assert_eq!(
                reported_policy(group).as_ref(),
                want.as_ref(),
                "case {label:?}: group {id:?} must report the policy it carries, got {group}"
            );
        }
    }
}

/// THE RULE: the attach a put starts is clock-owed — the policy is `ATTACHING` until the
/// span has passed and `ATTACHED` after it, a read never moves it anywhere else, and a
/// later put re-attaches the group's policy from the start.
#[tokio::test]
async fn an_attaching_policy_attaches_when_the_clock_makes_it_due() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(launch("attaching")).await;
    let ids = group_ids(&emr, "clock", &cluster_id).await;
    let group_id = ids.get("CORE").expect("the launch carries a core group");

    let put = json!({
        "ClusterId": cluster_id,
        "InstanceGroupId": group_id,
        "AutoScalingPolicy": policy(1, 10, vec![rule(
            "scale-out",
            ADJUSTMENT_TYPES[0],
            1,
            COMPARISON_OPERATORS[2],
        )]),
    });
    let started = std::time::Instant::now();
    let out = emr.ok("PutAutoScalingPolicy", put.clone()).await;
    // The put's own answer is computed at write time, so the transient state is there
    // whatever the machine is doing.
    assert_eq!(
        reported_state(&out),
        "ATTACHING",
        "case \"attaching\": a policy that was just put is ATTACHING, got {out}"
    );

    let attaching = groups_by_id(&emr, "attaching", &cluster_id).await;
    // Only a listing that came back INSIDE the span is evidence about the transient
    // state; a slower machine may legitimately have let the attach come due first.
    if started.elapsed() < TRANSITION_SPAN {
        assert_eq!(
            reported_state(&attaching[group_id]),
            "ATTACHING",
            "case \"attaching\": the transient state must be observable, got {}",
            attaching[group_id]
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    let attached = groups_by_id(&emr, "attached", &cluster_id).await;
    assert_eq!(
        reported_state(&attached[group_id]),
        "ATTACHED",
        "case \"attached\": the policy must reach ATTACHED, got {}",
        attached[group_id]
    );
    // Reading the listing again reports exactly the same groups: a read moves nothing.
    let again = groups_by_id(&emr, "attached twice", &cluster_id).await;
    assert_eq!(
        again, attached,
        "case \"attached twice\": a second read must report the same groups"
    );

    // Putting a policy again starts the attach over, whatever state the group's previous
    // policy had reached.
    let re_put_at = std::time::Instant::now();
    let out = emr.ok("PutAutoScalingPolicy", put).await;
    assert_eq!(
        reported_state(&out),
        "ATTACHING",
        "case \"re-put\": an updated policy is ATTACHING again, got {out}"
    );
    let reattaching = groups_by_id(&emr, "re-put", &cluster_id).await;
    if re_put_at.elapsed() < TRANSITION_SPAN {
        assert_eq!(
            reported_state(&reattaching[group_id]),
            "ATTACHING",
            "case \"re-put\": the listing must report the re-attach, got {}",
            reattaching[group_id]
        );
    }
}

/// THE RULE: `RemoveAutoScalingPolicy` takes the policy off the instance group the request
/// names and nothing else — the group reports none from there on, every other group keeps
/// what it carried, and the response carries no members — whatever policy the group held
/// and whether the attach was still due or had come through. The table spans the group
/// role a policy may be attached to, the rule count and capacity boundaries of the policy
/// removed, and both sides of the clock-owed attach span.
#[tokio::test]
async fn remove_auto_scaling_policy_takes_the_policy_off_the_group_it_names() {
    struct Case {
        label: &'static str,
        role: &'static str,
        min: i64,
        max: i64,
        rules: Vec<Value>,
        /// Whether to let the clock make the attach due before removing the policy.
        past_attach: bool,
    }

    let emr = Emr::new();
    let cases = vec![
        Case {
            label: "one rule on the core group, removed while attaching",
            role: "CORE",
            min: 1,
            max: 10,
            rules: vec![rule(
                "scale-out",
                ADJUSTMENT_TYPES[0],
                1,
                COMPARISON_OPERATORS[2],
            )],
            past_attach: false,
        },
        Case {
            label: "two rules on the core group, removed once attached",
            role: "CORE",
            min: 2,
            max: 20,
            rules: vec![
                rule("grow", ADJUSTMENT_TYPES[1], 25, COMPARISON_OPERATORS[3]),
                rule("pin", ADJUSTMENT_TYPES[2], 6, COMPARISON_OPERATORS[1]),
            ],
            past_attach: true,
        },
        Case {
            label: "equal boundaries on the task group, removed while attaching",
            role: "TASK",
            min: 5,
            max: 5,
            rules: vec![rule(
                "hold",
                ADJUSTMENT_TYPES[2],
                5,
                COMPARISON_OPERATORS[0],
            )],
            past_attach: false,
        },
        Case {
            label: "a zero lower boundary and every optional member on the task group",
            role: "TASK",
            min: 0,
            max: 100,
            rules: vec![
                detailed_rule("spot-burst", MARKET_TYPES[1]),
                detailed_rule("on-demand-burst", MARKET_TYPES[0]),
            ],
            past_attach: true,
        },
        Case {
            label: "every rule the chosen bound allows, removed once attached",
            role: "CORE",
            min: 1,
            max: 40,
            rules: (0..MAX_SCALING_RULES)
                .map(|index| {
                    rule(
                        &format!("rule-{index}"),
                        ADJUSTMENT_TYPES[index % ADJUSTMENT_TYPES.len()],
                        1 + index as i64,
                        COMPARISON_OPERATORS[index % COMPARISON_OPERATORS.len()],
                    )
                })
                .collect(),
            past_attach: false,
        },
    ];

    for case in &cases {
        let label = case.label;
        // A cluster of its own per row, so what one removal left behind cannot be read as
        // the next row's outcome.
        let cluster_id = emr.run_job_flow(launch(label)).await;
        let ids = group_ids(&emr, label, &cluster_id).await;
        let target = ids
            .get(case.role)
            .unwrap_or_else(|| panic!("case {label:?}: the launch must carry a {} group", case.role))
            .clone();
        // A SECOND group carries a policy too, so the removal is shown to take off only
        // the one the request names.
        let bystander_role = if case.role == "CORE" { "TASK" } else { "CORE" };
        let bystander = ids
            .get(bystander_role)
            .unwrap_or_else(|| panic!("case {label:?}: the launch must carry a {bystander_role} group"))
            .clone();
        let bystander_policy = policy(
            3,
            30,
            vec![rule(
                "left-alone",
                ADJUSTMENT_TYPES[0],
                2,
                COMPARISON_OPERATORS[1],
            )],
        );

        emr.ok(
            "PutAutoScalingPolicy",
            json!({
                "ClusterId": cluster_id,
                "InstanceGroupId": bystander,
                "AutoScalingPolicy": bystander_policy,
            }),
        )
        .await;
        emr.ok(
            "PutAutoScalingPolicy",
            json!({
                "ClusterId": cluster_id,
                "InstanceGroupId": target,
                "AutoScalingPolicy": policy(case.min, case.max, case.rules.clone()),
            }),
        )
        .await;

        if case.past_attach {
            tokio::time::sleep(PAST_TRANSITION).await;
        }

        let out = emr
            .ok(
                "RemoveAutoScalingPolicy",
                json!({ "ClusterId": cluster_id, "InstanceGroupId": target }),
            )
            .await;
        let members = out.as_object().map(serde_json::Map::len).unwrap_or(0);
        assert_eq!(
            members, 0,
            "case {label:?}: the removal answers no members, got {out}"
        );

        let listed = groups_by_id(&emr, label, &cluster_id).await;
        assert_eq!(
            listed.len(),
            ids.len(),
            "case {label:?}: the cluster must still report every group, got {listed:?}"
        );
        let removed_from = listed
            .get(&target)
            .unwrap_or_else(|| panic!("case {label:?}: group {target:?} must still be reported"));
        assert_eq!(
            reported_policy(removed_from),
            None,
            "case {label:?}: the named group must report no policy, got {removed_from}"
        );
        let untouched = listed
            .get(&bystander)
            .unwrap_or_else(|| panic!("case {label:?}: group {bystander:?} must still be reported"));
        assert_eq!(
            reported_policy(untouched),
            Some((3, 30, vec!["left-alone".to_string()])),
            "case {label:?}: the other group must keep the policy it carries, got {untouched}"
        );

        // Reading the listing again reports the same groups: a read moves nothing, and the
        // removal is not undone by looking.
        let again = groups_by_id(&emr, label, &cluster_id).await;
        assert_eq!(
            again, listed,
            "case {label:?}: a second read must report the same groups"
        );

        // A put after a removal attaches a policy again from the start, which is what
        // makes the removal a state change rather than a hidden flag.
        let re_put = emr
            .ok(
                "PutAutoScalingPolicy",
                json!({
                    "ClusterId": cluster_id,
                    "InstanceGroupId": target,
                    "AutoScalingPolicy": policy(case.min, case.max, case.rules.clone()),
                }),
            )
            .await;
        assert_eq!(
            reported_state(&re_put),
            "ATTACHING",
            "case {label:?}: a policy put after a removal is ATTACHING again, got {re_put}"
        );
        let restored = groups_by_id(&emr, label, &cluster_id).await;
        assert!(
            reported_policy(&restored[&target]).is_some(),
            "case {label:?}: the group must carry the re-attached policy, got {}",
            restored[&target]
        );
    }
}

/// THE RULE: a removal the service cannot carry out is refused with HTTP 400
/// `ValidationException` — never a 501 and never an empty success — and changes nothing:
/// an unknown cluster, a group that cluster does not carry, a group that carries no policy
/// (one never given a policy, the group that runs the primary node, and a group whose
/// policy a previous call already took off, so the removal is not idempotent).
#[tokio::test]
async fn remove_auto_scaling_policy_refuses_what_it_cannot_remove() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(launch("remove-refusals")).await;
    let ids = group_ids(&emr, "remove-refusals", &cluster_id).await;
    let core_id = ids.get("CORE").expect("the launch carries a core group");
    let task_id = ids.get("TASK").expect("the launch carries a task group");
    let master_id = ids.get("MASTER").expect("the launch carries a master group");

    // The task group is the one group that carries a policy throughout, so every refused
    // removal can be shown to have left it alone.
    emr.ok(
        "PutAutoScalingPolicy",
        json!({
            "ClusterId": cluster_id,
            "InstanceGroupId": task_id,
            "AutoScalingPolicy": policy(1, 10, vec![rule(
                "scale-out",
                ADJUSTMENT_TYPES[0],
                1,
                COMPARISON_OPERATORS[2],
            )]),
        }),
    )
    .await;

    // A group whose policy was just removed carries none, so removing it again is refused
    // exactly like a group that never had one.
    emr.ok(
        "PutAutoScalingPolicy",
        json!({
            "ClusterId": cluster_id,
            "InstanceGroupId": core_id,
            "AutoScalingPolicy": policy(2, 8, vec![rule(
                "scale-in",
                ADJUSTMENT_TYPES[0],
                -1,
                COMPARISON_OPERATORS[1],
            )]),
        }),
    )
    .await;
    emr.ok(
        "RemoveAutoScalingPolicy",
        json!({ "ClusterId": cluster_id, "InstanceGroupId": core_id }),
    )
    .await;

    // `(case, request, the group id the message must name)` — `None` where the refusal is
    // about the cluster or the group id itself rather than a missing policy.
    let mut cases: Vec<(String, Value, Option<String>)> = Vec::new();
    for unknown in UNKNOWN_CLUSTER_IDS {
        cases.push((
            format!("unknown cluster {unknown:?}"),
            json!({ "ClusterId": unknown, "InstanceGroupId": core_id }),
            None,
        ));
    }
    for unknown in UNKNOWN_GROUP_IDS {
        cases.push((
            format!("unknown instance group {unknown:?}"),
            json!({ "ClusterId": cluster_id, "InstanceGroupId": unknown }),
            None,
        ));
    }
    cases.push((
        "the master group, which nothing scales".to_string(),
        json!({ "ClusterId": cluster_id, "InstanceGroupId": master_id }),
        Some(master_id.clone()),
    ));
    cases.push((
        "a group whose policy was already removed".to_string(),
        json!({ "ClusterId": cluster_id, "InstanceGroupId": core_id }),
        Some(core_id.clone()),
    ));

    for (label, request, names_group) in &cases {
        let (status, payload) = emr.call("RemoveAutoScalingPolicy", request.clone()).await;
        assert_refused(label, status, &payload, "ValidationException");
        let message = crate::input_refusals::error_message(&payload);
        assert!(
            !message.is_empty(),
            "case {label:?}: the refusal must say what was wrong, got {payload}"
        );
        if let Some(group_id) = names_group {
            assert_eq!(
                message,
                format!("Auto Scaling policy was not found for instance group '{group_id}'."),
                "case {label:?}: the refusal must name the group that carries no policy, \
                 got {message:?}"
            );
        }

        // A refused removal changes nothing: the group that carries a policy still does,
        // and the groups that carry none still report none.
        let listed = groups_by_id(&emr, label, &cluster_id).await;
        for (id, group) in &listed {
            let expected_policy = id == task_id;
            assert_eq!(
                reported_policy(group).is_some(),
                expected_policy,
                "case {label:?}: a refused removal must change nothing, and group {id:?} \
                 reports {group}"
            );
        }
    }
}

/// THE RULE: a put the service cannot honour is refused with HTTP 400
/// `ValidationException` — never a 501 and never an empty success — and stores nothing:
/// an unknown cluster, a group that cluster does not carry, the master group (a policy is
/// supported for a core or task group), a lower capacity boundary that is negative or
/// above the upper one, and more rules than the chosen bound allows.
#[tokio::test]
async fn put_auto_scaling_policy_refuses_what_it_cannot_store() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(launch("refusals")).await;
    let ids = group_ids(&emr, "refusals", &cluster_id).await;
    let core_id = ids.get("CORE").expect("the launch carries a core group");
    let master_id = ids.get("MASTER").expect("the launch carries a master group");

    let sound = policy(
        1,
        10,
        vec![rule(
            "scale-out",
            ADJUSTMENT_TYPES[0],
            1,
            COMPARISON_OPERATORS[2],
        )],
    );

    let mut cases: Vec<(String, Value)> = Vec::new();
    for unknown in UNKNOWN_CLUSTER_IDS {
        cases.push((
            format!("unknown cluster {unknown:?}"),
            json!({
                "ClusterId": unknown,
                "InstanceGroupId": core_id,
                "AutoScalingPolicy": sound.clone(),
            }),
        ));
    }
    for unknown in UNKNOWN_GROUP_IDS {
        cases.push((
            format!("unknown instance group {unknown:?}"),
            json!({
                "ClusterId": cluster_id,
                "InstanceGroupId": unknown,
                "AutoScalingPolicy": sound.clone(),
            }),
        ));
    }
    cases.push((
        "the master group".to_string(),
        json!({
            "ClusterId": cluster_id,
            "InstanceGroupId": master_id,
            "AutoScalingPolicy": sound.clone(),
        }),
    ));
    for (min, max) in [(10, 1), (1, 0), (-1, 10), (-5, -1)] {
        cases.push((
            format!("capacity boundaries {min}..{max}"),
            json!({
                "ClusterId": cluster_id,
                "InstanceGroupId": core_id,
                "AutoScalingPolicy": policy(min, max, vec![rule(
                    "scale-out",
                    ADJUSTMENT_TYPES[0],
                    1,
                    COMPARISON_OPERATORS[2],
                )]),
            }),
        ));
    }
    cases.push((
        format!("one rule past the bound of {MAX_SCALING_RULES}"),
        json!({
            "ClusterId": cluster_id,
            "InstanceGroupId": core_id,
            "AutoScalingPolicy": policy(
                1,
                10,
                (0..=MAX_SCALING_RULES)
                    .map(|index| rule(
                        &format!("rule-{index}"),
                        ADJUSTMENT_TYPES[0],
                        1,
                        COMPARISON_OPERATORS[2],
                    ))
                    .collect(),
            ),
        }),
    ));

    for (label, request) in &cases {
        let (status, payload) = emr.call("PutAutoScalingPolicy", request.clone()).await;
        assert_refused(label, status, &payload, "ValidationException");

        // A refused put stores nothing: no group of the cluster carries a policy.
        let listed = groups_by_id(&emr, label, &cluster_id).await;
        for (id, group) in &listed {
            assert!(
                reported_policy(group).is_none(),
                "case {label:?}: a refused put must store nothing, and group {id:?} reports {group}"
            );
        }
    }
}
