// SPDX-License-Identifier: CC0-1.0

//! What a cluster-returning response carries, driven end to end over the wire.
//!
//! The RULE under test: a response from an operation that creates or deletes a cluster
//! reports what the cluster is CONFIGURED to be — the effective value of every setting
//! name the model defines, and the stored `configuration` — plus the account's
//! `clusterCount`, while the collections Amazon ECS only computes or materialises for a
//! read that ASKS for them come back EMPTY: `statistics` ("additional information about
//! your clusters that are separated by launch type", gated behind
//! `DescribeClusters$include` `STATISTICS`) and, where the request mentions tags
//! nowhere, `tags` (gated behind `TAGS`). The same cluster read WITH `include` is the
//! contrast: there the launch-type metrics and the stored tags are reported.

use serde_json::{json, Value};

use crate::ecs_wire::{list, ok, service, string};

/// Every cluster setting name the model defines:
/// `smithy_query(query="shape", shape="ClusterSettingName")` -> values
/// `["containerInsights"]`. A cluster-mutating response carries one entry per name.
const CLUSTER_SETTING_NAMES: [&str; 1] = ["containerInsights"];

/// The value a cluster setting carries when neither the cluster nor the account set
/// one: CloudWatch Container Insights is off until `PutAccountSetting(Default)` or a
/// per-cluster setting turns it on (`ClusterSetting$value`).
const INHERITED_SETTING_VALUE: &str = "disabled";

/// The launch-type metrics `Cluster$statistics` carries for a read that asks for
/// `STATISTICS`, from that member's own doc.
const STATISTIC_NAMES: [&str; 8] = [
    "runningEC2TasksCount",
    "runningFargateTasksCount",
    "pendingEC2TasksCount",
    "pendingFargateTasksCount",
    "activeEC2ServiceCount",
    "activeFargateServiceCount",
    "drainingEC2ServiceCount",
    "drainingFargateServiceCount",
];

/// One row of the projection table: the case name, the `containerInsights` value the
/// request sets (if any), the effective value it should report, the tags the request
/// carries, and the `configuration` it sets (if any).
type ProjectionCase = (
    &'static str,
    Option<&'static str>,
    &'static str,
    Value,
    Option<Value>,
);

/// The `settings` list a cluster reports when `containerInsights` is effectively
/// `value`.
fn effective_settings(value: &str) -> Vec<Value> {
    CLUSTER_SETTING_NAMES
        .iter()
        .map(|name| json!({ "name": name, "value": value }))
        .collect()
}

/// Assert the shape of a `cluster` object answered by a create or a delete.
fn assert_mutation_view(
    cluster: &Value,
    expected_setting: &str,
    configuration: Option<&Value>,
    expected_tags: &[Value],
    case: &str,
    operation: &str,
) {
    assert_eq!(
        list(cluster, "settings", case),
        effective_settings(expected_setting),
        "case {case:?}: {operation} must report the effective cluster settings, got \
         {cluster}"
    );
    assert_eq!(
        cluster["statistics"],
        json!([]),
        "case {case:?}: {operation} asked for no STATISTICS, so the launch-type metrics \
         must be an empty list, got {cluster}"
    );
    assert_eq!(
        list(cluster, "tags", case),
        expected_tags.to_vec(),
        "case {case:?}: unexpected tags from {operation}, got {cluster}"
    );
    match configuration {
        Some(expected) => assert_eq!(
            &cluster["configuration"],
            expected,
            "case {case:?}: {operation} must report the stored configuration, got \
             {cluster}"
        ),
        None => assert!(
            cluster["configuration"].is_null(),
            "case {case:?}: {operation} must report no configuration for a cluster that \
             has none, got {cluster}"
        ),
    }
}

#[tokio::test]
async fn cluster_mutation_responses_report_configured_state_and_no_unasked_collections() {
    let router = service();

    // The rows differ along what the rule generalises over: the setting value (absent,
    // and each value `ClusterSetting$value` documents — "enhanced, enabled, and
    // disabled"), the number of tags (0, 1, 2), and the FORM of the configuration
    // (absent; each `ExecuteCommandLogging` value NONE/DEFAULT/OVERRIDE; managed
    // storage; both halves at once).
    let cases: [ProjectionCase; 6] = [
        (
            "nothing configured",
            None,
            INHERITED_SETTING_VALUE,
            json!([]),
            None,
        ),
        (
            "enabled, one tag, logging DEFAULT",
            Some("enabled"),
            "enabled",
            json!([{ "key": "purpose", "value": "probe" }]),
            Some(json!({ "executeCommandConfiguration": { "logging": "DEFAULT" } })),
        ),
        (
            "explicitly disabled, two tags, logging NONE with a key",
            Some("disabled"),
            "disabled",
            json!([{ "key": "env", "value": "test" }, { "key": "team", "value": "platform" }]),
            Some(json!({
                "executeCommandConfiguration": {
                    "logging": "NONE",
                    "kmsKeyId": "alias/ecs-exec",
                },
            })),
        ),
        (
            "enhanced, no tags, logging OVERRIDE with a log configuration",
            Some("enhanced"),
            "enhanced",
            json!([]),
            Some(json!({
                "executeCommandConfiguration": {
                    "logging": "OVERRIDE",
                    "logConfiguration": {
                        "cloudWatchLogGroupName": "/ecs/projection",
                        "cloudWatchEncryptionEnabled": false,
                        "s3EncryptionEnabled": false,
                    },
                },
            })),
        ),
        (
            "managed storage configuration only",
            None,
            INHERITED_SETTING_VALUE,
            json!([{ "key": "owner", "value": "suite" }]),
            Some(json!({
                "managedStorageConfiguration": { "kmsKeyId": "alias/ecs-managed" },
            })),
        ),
        (
            "both halves of the configuration",
            Some("enabled"),
            "enabled",
            json!([]),
            Some(json!({
                "executeCommandConfiguration": { "logging": "DEFAULT" },
                "managedStorageConfiguration": {
                    "fargateEphemeralStorageKmsKeyId": "alias/fargate",
                },
            })),
        ),
    ];

    for (index, (case, setting, effective, tags, configuration)) in
        cases.into_iter().enumerate()
    {
        let name = format!("projection-{index}");
        let requested_tags: Vec<Value> = tags.as_array().cloned().unwrap_or_default();

        let mut request = json!({ "clusterName": name });
        if let Some(value) = setting {
            request["settings"] = json!([{ "name": "containerInsights", "value": value }]);
        }
        if !requested_tags.is_empty() {
            request["tags"] = tags.clone();
        }
        if let Some(ref value) = configuration {
            request["configuration"] = value.clone();
        }

        let created = ok(&router, "CreateCluster", request, case).await;
        // `CreateCluster` was handed the tags, so its answer materialises them.
        assert_mutation_view(
            &created["cluster"],
            effective,
            configuration.as_ref(),
            &requested_tags,
            case,
            "CreateCluster",
        );
        // The account holds exactly the cluster this row created: every earlier row
        // deleted its own.
        assert_eq!(
            created["clusterCount"],
            json!(1),
            "case {case:?}: CreateCluster must report the account's cluster count, got \
             {created}"
        );

        // The contrast: a read that ASKS for the gated collections gets them, computed
        // from what is stored.
        let described = ok(
            &router,
            "DescribeClusters",
            json!({
                "clusters": [name],
                "include": ["SETTINGS", "TAGS", "CONFIGURATIONS", "STATISTICS"],
            }),
            case,
        )
        .await;
        let read = &list(&described, "clusters", case)[0];
        assert_eq!(
            list(read, "settings", case),
            effective_settings(effective),
            "case {case:?}: include SETTINGS must report the same effective settings, \
             got {read}"
        );
        assert_eq!(
            list(read, "tags", case),
            requested_tags,
            "case {case:?}: include TAGS must report the stored tags, got {read}"
        );
        let statistic_names: Vec<String> = list(read, "statistics", case)
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            statistic_names,
            STATISTIC_NAMES.to_vec(),
            "case {case:?}: include STATISTICS must report every launch-type metric, \
             got {read}"
        );
        match configuration.as_ref() {
            Some(expected) => assert_eq!(
                &read["configuration"],
                expected,
                "case {case:?}: include CONFIGURATIONS must report the stored \
                 configuration, got {read}"
            ),
            None => assert!(
                read["configuration"].is_null(),
                "case {case:?}: a cluster with no configuration reports none, got {read}"
            ),
        }

        // A repeated create is the same cluster, reported through the same projection.
        let again = ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": name }),
            case,
        )
        .await;
        assert_mutation_view(
            &again["cluster"],
            effective,
            configuration.as_ref(),
            &requested_tags,
            case,
            "a repeated CreateCluster",
        );

        // Delete answers from the same stored record — with no tag set, because its
        // request names neither `tags` nor an `include`.
        let deleted = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": name }),
            case,
        )
        .await;
        assert_eq!(
            string(&deleted["cluster"], "status", case),
            "INACTIVE",
            "case {case:?}: a deleted cluster is INACTIVE, got {deleted}"
        );
        assert_mutation_view(
            &deleted["cluster"],
            effective,
            configuration.as_ref(),
            &[],
            case,
            "DeleteCluster",
        );
        assert_eq!(
            deleted["clusterCount"],
            json!(0),
            "case {case:?}: DeleteCluster must report the count left after the delete, \
             got {deleted}"
        );
    }
}

#[tokio::test]
async fn cluster_count_tracks_the_clusters_the_account_holds() {
    let router = service();

    // The rule generalises over how many clusters exist: each create raises the count
    // by one and each delete lowers it by one, whatever the cluster is called.
    let names = ["alpha", "beta-2", "gamma_3", "default"];
    for (index, name) in names.iter().enumerate() {
        let case = format!("create {name}");
        let created = ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": name }),
            &case,
        )
        .await;
        assert_eq!(
            created["clusterCount"],
            json!(index as i64 + 1),
            "case {case:?}: expected clusterCount {}, got {created}",
            index + 1
        );
    }

    for (index, name) in names.iter().enumerate() {
        let case = format!("delete {name}");
        let deleted = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": name }),
            &case,
        )
        .await;
        let expected = names.len() - index - 1;
        assert_eq!(
            deleted["clusterCount"],
            json!(expected as i64),
            "case {case:?}: expected clusterCount {expected}, got {deleted}"
        );
    }
}

#[tokio::test]
async fn statistics_stay_empty_on_a_mutation_even_when_the_cluster_holds_tasks() {
    let case = "a cluster holding tasks";
    let router = service();

    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "counted" }),
        case,
    )
    .await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "web",
            "containerDefinitions": [
                crate::task_lifecycle::container("app", "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        case,
    )
    .await;
    let placed = 3;
    let run = ok(
        &router,
        "RunTask",
        json!({ "cluster": "counted", "taskDefinition": "web", "count": placed }),
        case,
    )
    .await;
    assert_eq!(
        list(&run, "tasks", case).len(),
        placed as usize,
        "case {case:?}: expected {placed} tasks, got {run}"
    );

    // A read that asks for STATISTICS sees the tasks, split by launch type: whether the
    // owed PENDING -> RUNNING transition has fallen due or not, the EC2 metrics account
    // for every task placed.
    let described = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": ["counted"], "include": ["STATISTICS"] }),
        case,
    )
    .await;
    let read = &list(&described, "clusters", case)[0];
    let metric = |name: &str| -> i64 {
        list(read, "statistics", case)
            .iter()
            .find(|entry| entry["name"] == json!(name))
            .and_then(|entry| entry["value"].as_str().map(str::to_string))
            .unwrap_or_else(|| panic!("case {case:?}: expected {name} in {read}"))
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a number for {name} in {read}"))
    };
    assert_eq!(
        metric("runningEC2TasksCount") + metric("pendingEC2TasksCount"),
        placed,
        "case {case:?}: the launch-type metrics must account for every task, got {read}"
    );

    // The mutation projection still reports none of them: nothing asked for STATISTICS.
    // The always-reported counters DO move, so the empty list is a gate, not an empty
    // store.
    let again = ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "counted" }),
        case,
    )
    .await;
    let cluster = &again["cluster"];
    assert_eq!(
        cluster["statistics"],
        json!([]),
        "case {case:?}: a create answers with no launch-type metrics, got {cluster}"
    );
    assert_eq!(
        cluster["runningTasksCount"].as_i64().unwrap_or_default()
            + cluster["pendingTasksCount"].as_i64().unwrap_or_default(),
        placed,
        "case {case:?}: the modelled task counters must still report the tasks, got \
         {cluster}"
    );
}
