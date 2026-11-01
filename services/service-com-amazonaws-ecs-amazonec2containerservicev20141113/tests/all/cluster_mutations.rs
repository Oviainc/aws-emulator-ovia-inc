// SPDX-License-Identifier: CC0-1.0

//! Cluster mutations, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`]: what a cluster-mutating call writes is what a later read
//! answers with.
//!
//! The rules under test:
//!
//! * A cluster-mutating operation (`UpdateClusterSettings`, `UpdateCluster`,
//!   `PutClusterCapacityProviders`, `DeregisterContainerInstance`) writes into the
//!   record of the cluster the request NAMES, and every later read of that cluster is
//!   computed from what was written — while every other cluster keeps its own values.
//! * `UpdateClusterSettings`: "Modifies the settings to use for a cluster", keyed by
//!   `ClusterSetting$name`.
//! * `ClusterSetting$value`: "The supported values are `enhanced`, `enabled`, and
//!   `disabled`."
//! * `ExecuteCommandConfiguration$logConfiguration`: "When `logging=OVERRIDE` is
//!   specified, a `logConfiguration` must be provided."
//! * `ClusterServiceConnectDefaultsRequest$namespace`: "If you update the cluster with
//!   an empty string `""` for the namespace name, the cluster configuration for Service
//!   Connect is removed."
//! * `Cluster$attachmentsStatus`: `UPDATE_IN_PROGRESS` ("The available capacity
//!   providers for the cluster are updating") -> `UPDATE_COMPLETE` ("The capacity
//!   providers have successfully updated"), a transition the CLOCK owes.
//! * `DeleteCluster`/`UpdateClusterSettings`/`PutClusterCapacityProviders` refuse a
//!   cluster whose attachments are mid-update with `UpdateInProgressException`.
//! * `Cluster$registeredContainerInstancesCount`: "The number of container instances
//!   registered into the cluster. This includes container instances in both `ACTIVE` and
//!   `DRAINING` status."

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{count, list, ok, refused, register_instance, service};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to
/// fall due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// Every value `ClusterSetting$value` accepts, from that member's own doc: "The
/// supported values are `enhanced`, `enabled`, and `disabled`."
const CLUSTER_SETTING_VALUES: [&str; 3] = ["enhanced", "enabled", "disabled"];

/// The fixed sentence Amazon ECS answers for `ClusterNotFoundException` when the name
/// resolves to no cluster record at all, as the wire corpus records it.
const CLUSTER_NOT_FOUND_MESSAGE: &str = "Cluster not found.";

/// The fixed sentence the same error code carries when the cluster IS still there, in the
/// status a delete leaves it in, as the wire corpus records it.
const CLUSTER_INACTIVE_MESSAGE: &str = "The referenced cluster was inactive.";

/// The Amazon ECS service quota "Capacity providers per cluster", which bounds both
/// lists one `PutClusterCapacityProviders` may carry (the handler's
/// `state::MAX_CLUSTER_CAPACITY_PROVIDERS`).
const MAX_CLUSTER_CAPACITY_PROVIDERS: usize = 20;

/// The cluster the describe reads back, which must be the only one the request named.
async fn describe(router: &MultiServiceRouter, cluster: &str, include: Value, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster], "include": include }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: expected no failures describing {cluster:?}, got {body}"
    );
    let clusters = list(&body, "clusters", case);
    assert_eq!(
        clusters.len(),
        1,
        "case {case:?}: expected exactly one cluster back, got {body}"
    );
    clusters[0].clone()
}

/// A cluster's settings as `(name, value)` pairs, sorted so the comparison does not
/// depend on the order the list was written in.
fn settings_of(cluster: &Value, case: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = list(cluster, "settings", case)
        .iter()
        .map(|setting| {
            (
                setting["name"].as_str().unwrap_or_default().to_string(),
                setting["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// The capacity providers and the default strategy a cluster reports, reduced to what
/// the request carried.
fn association_of(cluster: &Value, case: &str) -> (Vec<String>, Vec<(String, i64, i64)>) {
    let providers = list(cluster, "capacityProviders", case)
        .iter()
        .map(|provider| provider.as_str().unwrap_or_default().to_string())
        .collect();
    let strategy = list(cluster, "defaultCapacityProviderStrategy", case)
        .iter()
        .map(|item| {
            (
                item["capacityProvider"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                count(item, "weight", case),
                count(item, "base", case),
            )
        })
        .collect();
    (providers, strategy)
}

/// `registeredContainerInstancesCount` as the cluster reports it now.
async fn registered_count(router: &MultiServiceRouter, cluster: &str, case: &str) -> i64 {
    let described = describe(router, cluster, json!([]), case).await;
    count(&described, "registeredContainerInstancesCount", case)
}

#[tokio::test]
async fn the_optional_cluster_lists_are_reported_as_the_operation_materialises_them() {
    let router = service();

    // Every cluster-returning MUTATION reports the cluster's own stored fields, an
    // EMPTY `statistics` (nothing asked for the launch-type counts) and no `tags` at
    // all; a READ reports `settings`, `statistics` and `tags` empty for the fields its
    // `include` did not ask for.
    let created = ok(
        &router,
        "CreateCluster",
        json!({
            "clusterName": "projection",
            "tags": [{ "key": "purpose", "value": "probe" }],
            "settings": [{ "name": "containerInsights", "value": "disabled" }],
        }),
        "projection",
    )
    .await;
    let updated_settings = ok(
        &router,
        "UpdateClusterSettings",
        json!({
            "cluster": "projection",
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        }),
        "projection",
    )
    .await;
    let updated = ok(
        &router,
        "UpdateCluster",
        json!({
            "cluster": "projection",
            "configuration": { "executeCommandConfiguration": { "logging": "DEFAULT" } },
        }),
        "projection",
    )
    .await;
    let associated = ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": "projection",
            "capacityProviders": ["FARGATE"],
            "defaultCapacityProviderStrategy": [],
        }),
        "projection",
    )
    .await;
    tokio::time::sleep(ONE_SPAN).await;
    let deleted = ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "projection" }),
        "projection",
    )
    .await;

    for (case, body, tags) in [
        ("CreateCluster", &created, 1),
        ("UpdateClusterSettings", &updated_settings, 0),
        ("UpdateCluster", &updated, 0),
        ("PutClusterCapacityProviders", &associated, 0),
        ("DeleteCluster", &deleted, 0),
    ] {
        let cluster = &body["cluster"];
        assert!(
            !list(cluster, "settings", case).is_empty(),
            "case {case:?}: expected a mutation to report the cluster's stored settings, got {cluster}"
        );
        assert_eq!(
            list(cluster, "statistics", case),
            Vec::<Value>::new(),
            "case {case:?}: expected no launch-type statistics from a mutation, got {cluster}"
        );
        // `CreateCluster` answers with the tags the request just applied; a later
        // mutation of the same cluster does not materialise them.
        assert_eq!(
            list(cluster, "tags", case).len(),
            tags,
            "case {case:?}: expected {tags} tag(s) in the response, got {cluster}"
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_lists_its_include_asks_for() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({
            "clusterName": "included",
            "tags": [{ "key": "purpose", "value": "probe" }],
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        }),
        "include",
    )
    .await;

    // One row per include list, asserting what each of the three optional lists carries
    // under it. `statistics` is the launch-type counters, four for each of the four
    // launch types the `LaunchType` enum defines (`Cluster$statistics`: "Additional
    // information about your clusters that are separated by launch type"), which only a
    // STATISTICS read computes.
    let cases: [(&str, Value, usize, usize, usize); 5] = [
        ("no include", json!([]), 0, 0, 0),
        ("SETTINGS", json!(["SETTINGS"]), 1, 0, 0),
        ("TAGS", json!(["TAGS"]), 0, 0, 1),
        ("STATISTICS", json!(["STATISTICS"]), 0, 16, 0),
        (
            "all three",
            json!(["SETTINGS", "STATISTICS", "TAGS"]),
            1,
            16,
            1,
        ),
    ];

    for (case, include, settings, statistics, tags) in cases {
        let described = describe(&router, "included", include, case).await;
        for (member, expected) in [
            ("settings", settings),
            ("statistics", statistics),
            ("tags", tags),
        ] {
            let actual = list(&described, member, case);
            assert_eq!(
                actual.len(),
                expected,
                "case {case:?}: expected {expected} {member} entries, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_that_sets_nothing_reports_the_account_default_container_insights() {
    let router = service();

    // A cluster setting "overrides the containerInsights value set with
    // PutAccountSetting or PutAccountSettingDefault", so a cluster that carries no
    // setting of its own still has an effective value to report: the account one, which
    // is off until an account-setting operation changes it.
    let created = ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "defaulted" }),
        "account default",
    )
    .await;
    assert_eq!(
        settings_of(&created["cluster"], "account default"),
        vec![("containerInsights".to_string(), "disabled".to_string())],
        "expected the create to report the effective containerInsights value, got {created}"
    );
    let described = describe(&router, "defaulted", json!(["SETTINGS"]), "account default").await;
    assert_eq!(
        settings_of(&described, "account default"),
        vec![("containerInsights".to_string(), "disabled".to_string())],
        "expected a SETTINGS read to report the effective value, got {described}"
    );

    // The cluster's own value overrides it, and nothing else reappears beside it.
    ok(
        &router,
        "UpdateClusterSettings",
        json!({
            "cluster": "defaulted",
            "settings": [{ "name": "containerInsights", "value": "enhanced" }],
        }),
        "account default",
    )
    .await;
    let overridden = describe(&router, "defaulted", json!(["SETTINGS"]), "account default").await;
    assert_eq!(
        settings_of(&overridden, "account default"),
        vec![("containerInsights".to_string(), "enhanced".to_string())],
        "expected the cluster's own setting to override the account value, got {overridden}"
    );
}

#[tokio::test]
async fn cluster_settings_are_written_per_cluster_and_read_back() {
    let router = service();

    // The rows differ along what the rule generalises over: which operation carries the
    // settings member, and which of the three documented values it sets — including
    // `enhanced` and `disabled`, which the issue did not cite.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("UpdateClusterSettings turns Container Insights on", "UpdateClusterSettings", "enabled"),
        ("UpdateClusterSettings turns it off again", "UpdateClusterSettings", "disabled"),
        ("UpdateClusterSettings asks for enhanced observability", "UpdateClusterSettings", "enhanced"),
        ("UpdateCluster carries the same settings member", "UpdateCluster", "enabled"),
        ("UpdateCluster asks for enhanced observability", "UpdateCluster", "enhanced"),
    ];

    for (index, (case, operation, value)) in cases.iter().enumerate() {
        assert!(
            CLUSTER_SETTING_VALUES.contains(value),
            "case {case:?}: {value:?} is not one of the documented settings values"
        );
        let target = format!("target-{index}");
        let sibling = format!("sibling-{index}");
        // Both clusters start from the SAME stored value, so a value that crosses
        // between them cannot pass unnoticed.
        for name in [&target, &sibling] {
            ok(
                &router,
                "CreateCluster",
                json!({
                    "clusterName": name,
                    "settings": [{ "name": "containerInsights", "value": "disabled" }],
                }),
                case,
            )
            .await;
        }

        let answered = ok(
            &router,
            operation,
            json!({
                "cluster": target,
                "settings": [{ "name": "containerInsights", "value": value }],
            }),
            case,
        )
        .await;
        assert_eq!(
            settings_of(&answered["cluster"], case),
            vec![("containerInsights".to_string(), (*value).to_string())],
            "case {case:?}: expected {operation} to answer with the settings it wrote, got {answered}"
        );

        let described = describe(&router, &target, json!(["SETTINGS"]), case).await;
        assert_eq!(
            settings_of(&described, case),
            vec![("containerInsights".to_string(), (*value).to_string())],
            "case {case:?}: expected the stored cluster to report {value:?}, got {described}"
        );

        let untouched = describe(&router, &sibling, json!(["SETTINGS"]), case).await;
        assert_eq!(
            settings_of(&untouched, case),
            vec![("containerInsights".to_string(), "disabled".to_string())],
            "case {case:?}: expected the sibling cluster to keep its own settings, got {untouched}"
        );

        // A read that does not ask for SETTINGS reports the member EMPTY rather than
        // the stored settings: "If this field is omitted, this information isn't
        // included."
        let bare = describe(&router, &target, json!([]), case).await;
        assert_eq!(
            list(&bare, "settings", case),
            Vec::<Value>::new(),
            "case {case:?}: expected empty settings without include=[SETTINGS], got {bare}"
        );
    }
}

#[tokio::test]
async fn cluster_configuration_and_service_connect_defaults_round_trip() {
    let router = service();

    // One row per form the `ClusterConfiguration` tree takes: no execute-command
    // logging, the task-definition default, an OVERRIDE carrying its own destination,
    // a customer-managed key, and managed storage.
    type Row = (&'static str, Value, Vec<(&'static str, Option<Value>)>);
    let cases: Vec<Row> = vec![
        (
            "logging NONE",
            json!({ "executeCommandConfiguration": { "logging": "NONE" } }),
            vec![(
                "/configuration/executeCommandConfiguration/logging",
                Some(json!("NONE")),
            )],
        ),
        (
            "logging DEFAULT",
            json!({ "executeCommandConfiguration": { "logging": "DEFAULT" } }),
            vec![
                (
                    "/configuration/executeCommandConfiguration/logging",
                    Some(json!("DEFAULT")),
                ),
                (
                    "/configuration/executeCommandConfiguration/logConfiguration",
                    None,
                ),
            ],
        ),
        (
            "logging OVERRIDE with a CloudWatch and S3 destination",
            json!({
                "executeCommandConfiguration": {
                    "logging": "OVERRIDE",
                    "logConfiguration": {
                        "cloudWatchLogGroupName": "/ecs/execute-command",
                        "cloudWatchEncryptionEnabled": true,
                        "s3BucketName": "execute-command-audit",
                        "s3KeyPrefix": "sessions/",
                        "s3EncryptionEnabled": true,
                    },
                }
            }),
            vec![
                (
                    "/configuration/executeCommandConfiguration/logging",
                    Some(json!("OVERRIDE")),
                ),
                (
                    "/configuration/executeCommandConfiguration/logConfiguration/cloudWatchLogGroupName",
                    Some(json!("/ecs/execute-command")),
                ),
                (
                    "/configuration/executeCommandConfiguration/logConfiguration/cloudWatchEncryptionEnabled",
                    Some(json!(true)),
                ),
                (
                    "/configuration/executeCommandConfiguration/logConfiguration/s3BucketName",
                    Some(json!("execute-command-audit")),
                ),
                (
                    "/configuration/executeCommandConfiguration/logConfiguration/s3KeyPrefix",
                    Some(json!("sessions/")),
                ),
                (
                    "/configuration/executeCommandConfiguration/logConfiguration/s3EncryptionEnabled",
                    Some(json!(true)),
                ),
            ],
        ),
        (
            "a customer-managed key for the execute-command session",
            json!({
                "executeCommandConfiguration": {
                    "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/execute-command",
                    "logging": "DEFAULT",
                }
            }),
            vec![(
                "/configuration/executeCommandConfiguration/kmsKeyId",
                Some(json!("arn:aws:kms:us-west-2:000000000000:key/execute-command")),
            )],
        ),
        (
            "managed storage encryption",
            json!({
                "managedStorageConfiguration": {
                    "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/managed-storage",
                    "fargateEphemeralStorageKmsKeyId":
                        "arn:aws:kms:us-west-2:000000000000:key/ephemeral",
                }
            }),
            vec![
                (
                    "/configuration/managedStorageConfiguration/kmsKeyId",
                    Some(json!("arn:aws:kms:us-west-2:000000000000:key/managed-storage")),
                ),
                (
                    "/configuration/managedStorageConfiguration/fargateEphemeralStorageKmsKeyId",
                    Some(json!("arn:aws:kms:us-west-2:000000000000:key/ephemeral")),
                ),
            ],
        ),
    ];

    for (index, (case, configuration, expectations)) in cases.iter().enumerate() {
        let cluster = format!("configured-{index}");
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": cluster }),
            case,
        )
        .await;
        let created = describe(&router, &cluster, json!(["CONFIGURATIONS"]), case).await;
        assert!(
            created["configuration"].is_null(),
            "case {case:?}: a cluster created without a configuration has none, got {created}"
        );

        let answered = ok(
            &router,
            "UpdateCluster",
            json!({ "cluster": cluster, "configuration": configuration }),
            case,
        )
        .await;
        let described = describe(&router, &cluster, json!(["CONFIGURATIONS"]), case).await;
        for (pointer, expected) in expectations {
            for (source, body) in [("the update response", &answered["cluster"]), ("the stored cluster", &described)] {
                let actual = body.pointer(pointer).cloned().filter(|value| !value.is_null());
                assert_eq!(
                    actual.as_ref(),
                    expected.as_ref(),
                    "case {case:?}: expected {source} to report {pointer} as {expected:?}, got {actual:?} in {body}"
                );
            }
        }
    }

    // The Service Connect default is set by name and REMOVED by the empty namespace.
    let cluster = "service-connect";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "service connect",
    )
    .await;
    for (case, namespace, expected) in [
        ("a namespace name", "shared-namespace", Some("shared-namespace")),
        (
            "a namespace ARN",
            "arn:aws:servicediscovery:us-west-2:000000000000:namespace/ns-abc",
            Some("arn:aws:servicediscovery:us-west-2:000000000000:namespace/ns-abc"),
        ),
        ("an empty namespace removes it", "", None),
    ] {
        ok(
            &router,
            "UpdateCluster",
            json!({
                "cluster": cluster,
                "serviceConnectDefaults": { "namespace": namespace },
            }),
            case,
        )
        .await;
        let described = describe(&router, cluster, json!([]), case).await;
        let actual = described
            .pointer("/serviceConnectDefaults/namespace")
            .and_then(Value::as_str);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored default namespace to be {expected:?}, got {described}"
        );
    }
}

#[tokio::test]
async fn an_override_log_configuration_is_required_and_a_setting_value_is_checked() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "refusals" }),
        "refusals",
    )
    .await;

    // `logging=OVERRIDE` without a `logConfiguration` is refused by every operation
    // that accepts a cluster configuration.
    let bad_configuration = json!({ "executeCommandConfiguration": { "logging": "OVERRIDE" } });
    refused(
        &router,
        "UpdateCluster",
        json!({ "cluster": "refusals", "configuration": bad_configuration.clone() }),
        "InvalidParameterException",
        "UpdateCluster with OVERRIDE and no logConfiguration",
    )
    .await;
    refused(
        &router,
        "CreateCluster",
        json!({ "clusterName": "bad-configuration", "configuration": bad_configuration }),
        "InvalidParameterException",
        "CreateCluster with OVERRIDE and no logConfiguration",
    )
    .await;

    // A settings value outside the documented catalog is refused the same way wherever
    // it is carried. "enable" and "ENABLED" are one character and one case away from a
    // value that is accepted.
    for value in ["enable", "ENABLED", "", "on"] {
        for operation in ["UpdateClusterSettings", "UpdateCluster"] {
            refused(
                &router,
                operation,
                json!({
                    "cluster": "refusals",
                    "settings": [{ "name": "containerInsights", "value": value }],
                }),
                "InvalidParameterException",
                &format!("{operation} with settings value {value:?}"),
            )
            .await;
        }
    }

    // And an unknown cluster is a ClusterNotFoundException from every mutation, with
    // the fixed sentence Amazon ECS answers for that condition — a constant that names
    // no cluster, the same for every request. A DELETED cluster is the same error CODE
    // (the `Cluster$status` doc for INACTIVE is "the cluster has been deleted", so a
    // tombstone is no cluster to act in either) with the OTHER fixed sentence, the one
    // that says the cluster is still there but inactive.
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "deleted" }),
        "tombstone",
    )
    .await;
    ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "deleted" }),
        "tombstone",
    )
    .await;

    for (operation, request) in [
        (
            "UpdateClusterSettings",
            json!({
                "cluster": "no-such-cluster",
                "settings": [{ "name": "containerInsights", "value": "enabled" }],
            }),
        ),
        (
            "UpdateCluster",
            json!({
                "cluster": "no-such-cluster",
                "settings": [{ "name": "containerInsights", "value": "enabled" }],
            }),
        ),
        (
            "PutClusterCapacityProviders",
            json!({
                "cluster": "no-such-cluster",
                "capacityProviders": ["FARGATE"],
                "defaultCapacityProviderStrategy": [],
            }),
        ),
        (
            "DeregisterContainerInstance",
            json!({ "cluster": "no-such-cluster", "containerInstance": "whatever" }),
        ),
        (
            "UpdateCluster",
            json!({
                "cluster": "deleted",
                "configuration": { "executeCommandConfiguration": { "logging": "NONE" } },
            }),
        ),
        (
            "PutClusterCapacityProviders",
            json!({
                "cluster": "deleted",
                "capacityProviders": ["FARGATE"],
                "defaultCapacityProviderStrategy": [],
            }),
        ),
        (
            "DeregisterContainerInstance",
            json!({ "cluster": "deleted", "containerInstance": "whatever" }),
        ),
    ] {
        let body = refused(
            &router,
            operation,
            request.clone(),
            "ClusterNotFoundException",
            &format!("{operation} against {}", request["cluster"]),
        )
        .await;
        // Which of the two fixed sentences is correct is selected by the cluster's own
        // observable state, not by the request: the tombstone still describes, so it is
        // reported as inactive, and the name that never existed as not found.
        let expected = match request["cluster"] == "deleted" {
            true => CLUSTER_INACTIVE_MESSAGE,
            false => CLUSTER_NOT_FOUND_MESSAGE,
        };
        assert_eq!(
            body["message"].as_str(),
            Some(expected),
            "case {operation:?} against {}: expected {expected:?}, got {body}",
            request["cluster"]
        );
    }

    // `UpdateClusterSettings` is the one of these that reports a cluster it DID find, in
    // a status it cannot write settings to, as the state error it is: the tombstone is
    // still there, so the condition is not "no such cluster" — and the sentence names the
    // cluster the request asked for and the state it is not in.
    let body = refused(
        &router,
        "UpdateClusterSettings",
        json!({
            "cluster": "deleted",
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        }),
        "ClientException",
        "UpdateClusterSettings against a deleted cluster",
    )
    .await;
    assert_eq!(
        body["message"].as_str(),
        Some("Cluster deleted is not in ACTIVE state"),
        "case \"UpdateClusterSettings against a deleted cluster\": expected the \
         not-ACTIVE sentence naming the cluster, got {body}"
    );
    // The same call against a name this account never held is still the NOT-FOUND
    // refusal: the two conditions stay apart.
    let body = refused(
        &router,
        "UpdateClusterSettings",
        json!({
            "cluster": "no-such-cluster",
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        }),
        "ClusterNotFoundException",
        "UpdateClusterSettings against a cluster that never existed",
    )
    .await;
    assert_eq!(
        body["message"].as_str(),
        Some(CLUSTER_NOT_FOUND_MESSAGE),
        "case \"UpdateClusterSettings against a cluster that never existed\": expected \
         {CLUSTER_NOT_FOUND_MESSAGE:?}, got {body}"
    );
}

#[tokio::test]
async fn capacity_provider_association_persists_and_its_attachments_complete_on_the_clock() {
    let router = service();

    // The rows differ in how many providers the association names, in which order, and
    // in how long the default strategy is — including the empty strategy the doc calls
    // for ("you must specify an empty array ([]) to bypass defining a default
    // strategy").
    type Row = (&'static str, Vec<&'static str>, Value, Vec<(String, i64, i64)>);
    let cases: Vec<Row> = vec![
        (
            "one Fargate provider with a one-entry strategy",
            vec!["FARGATE"],
            json!([{ "capacityProvider": "FARGATE", "weight": 1 }]),
            vec![("FARGATE".to_string(), 1, 0)],
        ),
        (
            "both Fargate providers with a weighted two-entry strategy",
            vec!["FARGATE", "FARGATE_SPOT"],
            json!([
                { "capacityProvider": "FARGATE", "weight": 1, "base": 2 },
                { "capacityProvider": "FARGATE_SPOT", "weight": 4 },
            ]),
            vec![
                ("FARGATE".to_string(), 1, 2),
                ("FARGATE_SPOT".to_string(), 4, 0),
            ],
        ),
        (
            "the providers in the other order and no default strategy",
            vec!["FARGATE_SPOT", "FARGATE"],
            json!([]),
            Vec::new(),
        ),
    ];

    for (index, (case, providers, strategy, expected_strategy)) in cases.iter().enumerate() {
        let cluster = format!("capacity-{index}");
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": cluster }),
            case,
        )
        .await;

        let answered = ok(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": cluster,
                "capacityProviders": providers,
                "defaultCapacityProviderStrategy": strategy,
            }),
            case,
        )
        .await;
        let (stored_providers, stored_strategy) = association_of(&answered["cluster"], case);
        assert_eq!(
            stored_providers, *providers,
            "case {case:?}: expected the answered capacityProviders to be what was put, got {answered}"
        );
        assert_eq!(
            stored_strategy, *expected_strategy,
            "case {case:?}: expected the answered default strategy to be what was put, got {answered}"
        );
        assert_eq!(
            answered["cluster"]["attachmentsStatus"].as_str(),
            Some("UPDATE_IN_PROGRESS"),
            "case {case:?}: expected the association to start an attachments update, got {answered}"
        );

        // The association is readable before the update lands, and the update itself is
        // still in progress: the span is never zero.
        let during = describe(&router, &cluster, json!(["ATTACHMENTS"]), case).await;
        assert_eq!(
            during["attachmentsStatus"].as_str(),
            Some("UPDATE_IN_PROGRESS"),
            "case {case:?}: expected UPDATE_IN_PROGRESS straight after the put, got {during}"
        );
        assert_eq!(
            list(&during, "attachments", case),
            Vec::<Value>::new(),
            "case {case:?}: a Fargate capacity provider attaches no resource, got {during}"
        );
        let (described_providers, _) = association_of(&during, case);
        assert_eq!(
            described_providers, *providers,
            "case {case:?}: expected the stored association to survive the read, got {during}"
        );

        // While the attachments update, the cluster is busy: every mutation that models
        // the busy-state refusal answers it.
        refused(
            &router,
            "DeleteCluster",
            json!({ "cluster": cluster }),
            "UpdateInProgressException",
            &format!("{case}: delete while the attachments update"),
        )
        .await;
        refused(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": cluster,
                "capacityProviders": ["FARGATE"],
                "defaultCapacityProviderStrategy": [],
            }),
            "UpdateInProgressException",
            &format!("{case}: a second put while the first updates"),
        )
        .await;
        refused(
            &router,
            "UpdateClusterSettings",
            json!({
                "cluster": cluster,
                "settings": [{ "name": "containerInsights", "value": "enabled" }],
            }),
            "UpdateInProgressException",
            &format!("{case}: a settings update while the attachments update"),
        )
        .await;

        // The clock owes the completion, and nothing but the clock delivers it.
        tokio::time::sleep(ONE_SPAN).await;
        let after = describe(&router, &cluster, json!(["ATTACHMENTS"]), case).await;
        assert_eq!(
            after["attachmentsStatus"].as_str(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the attachments update to complete on the clock, got {after}"
        );

        // A read that does not ask for ATTACHMENTS gets neither member.
        let bare = describe(&router, &cluster, json!([]), case).await;
        assert!(
            bare["attachmentsStatus"].is_null() && bare["attachments"].is_null(),
            "case {case:?}: expected no attachments without include=[ATTACHMENTS], got {bare}"
        );

        // Once the attachments are complete the cluster takes mutations again, and the
        // next association REPLACES the previous one.
        let replaced = ok(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": cluster,
                "capacityProviders": ["FARGATE_SPOT"],
                "defaultCapacityProviderStrategy": [
                    { "capacityProvider": "FARGATE_SPOT", "weight": 1 }
                ],
            }),
            case,
        )
        .await;
        let (replaced_providers, replaced_strategy) = association_of(&replaced["cluster"], case);
        assert_eq!(
            replaced_providers,
            vec!["FARGATE_SPOT".to_string()],
            "case {case:?}: expected the second put to replace the association, got {replaced}"
        );
        assert_eq!(
            replaced_strategy,
            vec![("FARGATE_SPOT".to_string(), 1, 0)],
            "case {case:?}: expected the second put to replace the strategy, got {replaced}"
        );

        tokio::time::sleep(ONE_SPAN).await;
        let deleted = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": cluster }),
            case,
        )
        .await;
        assert_eq!(
            deleted["cluster"]["status"].as_str(),
            Some("INACTIVE"),
            "case {case:?}: expected the delete to go through once the attachments completed, got {deleted}"
        );
    }
}

#[tokio::test]
async fn a_capacity_provider_strategy_must_name_associated_providers() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "strategy" }),
        "strategy",
    )
    .await;

    for (case, providers, strategy) in [
        (
            "the strategy names a provider the request does not associate",
            json!(["FARGATE"]),
            json!([{ "capacityProvider": "FARGATE_SPOT", "weight": 1 }]),
        ),
        (
            "the strategy names nothing that is associated",
            json!([]),
            json!([{ "capacityProvider": "FARGATE", "weight": 1 }]),
        ),
        (
            "two entries of the strategy define a base",
            json!(["FARGATE", "FARGATE_SPOT"]),
            json!([
                { "capacityProvider": "FARGATE", "weight": 1, "base": 1 },
                { "capacityProvider": "FARGATE_SPOT", "weight": 1, "base": 1 },
            ]),
        ),
    ] {
        refused(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": "strategy",
                "capacityProviders": providers,
                "defaultCapacityProviderStrategy": strategy,
            }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // A refused put leaves the stored association where it was.
    let described = describe(&router, "strategy", json!(["ATTACHMENTS"]), "strategy").await;
    let (providers, strategy) = association_of(&described, "strategy");
    assert!(
        providers.is_empty() && strategy.is_empty(),
        "expected a refused put to write nothing, got {described}"
    );
    // The Amazon ECS quota "Capacity providers per cluster" is 20, and a strategy
    // entry must name one of the associated providers, so neither list may be longer:
    // the published bound is accepted and one past it is refused. Every name is a
    // provider this account actually holds, since an association only takes providers
    // that exist.
    let at_bound: Vec<String> = (0..MAX_CLUSTER_CAPACITY_PROVIDERS)
        .map(|index| format!("provider-{index}"))
        .collect();
    let mut past_bound = at_bound.clone();
    past_bound.push("provider-too-many".to_string());
    for name in &past_bound {
        ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": name,
                "autoScalingGroupProvider": { "autoScalingGroupArn": format!("asg-{name}") },
            }),
            "the providers the association names",
        )
        .await;
    }
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": "strategy",
            "capacityProviders": at_bound,
            "defaultCapacityProviderStrategy": [],
        }),
        "the quota's worth of capacity providers",
    )
    .await;
    refused(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": "strategy",
            "capacityProviders": past_bound,
            "defaultCapacityProviderStrategy": [],
        }),
        "InvalidParameterException",
        "one capacity provider past the quota",
    )
    .await;
    let long_strategy: Vec<Value> = past_bound
        .iter()
        .map(|provider| json!({ "capacityProvider": provider, "weight": 1 }))
        .collect();
    refused(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": "strategy",
            "capacityProviders": ["FARGATE"],
            "defaultCapacityProviderStrategy": long_strategy,
        }),
        "InvalidParameterException",
        "a strategy longer than the quota",
    )
    .await;
}

#[tokio::test]
async fn deregistration_drops_the_cluster_registered_instance_count() {
    let router = service();

    // The rows differ in how many instances the cluster holds before any of them is
    // deregistered, so the count is asserted as it falls one instance at a time.
    for instances in [1_usize, 2, 3] {
        let case = format!("{instances} registered instance(s)");
        let cluster = format!("drain-{instances}");
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": cluster }),
            &case,
        )
        .await;

        let mut arns = Vec::new();
        for index in 0..instances {
            arns.push(
                register_instance(
                    &router,
                    &cluster,
                    &format!("i-{instances}{index:04}"),
                    &case,
                )
                .await,
            );
        }
        assert_eq!(
            registered_count(&router, &cluster, &case).await,
            instances as i64,
            "case {case:?}: expected the cluster to count every registered instance"
        );

        for (index, arn) in arns.iter().enumerate() {
            let answered = ok(
                &router,
                "DeregisterContainerInstance",
                json!({ "cluster": cluster, "containerInstance": arn }),
                &case,
            )
            .await;
            assert_eq!(
                answered["containerInstance"]["status"].as_str(),
                Some("INACTIVE"),
                "case {case:?}: expected the deregistered instance to report INACTIVE, got {answered}"
            );
            assert_eq!(
                answered["containerInstance"]["containerInstanceArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the answer to be about the instance named, got {answered}"
            );

            let remaining = instances as i64 - index as i64 - 1;
            assert_eq!(
                registered_count(&router, &cluster, &case).await,
                remaining,
                "case {case:?}: expected {remaining} instance(s) left after deregistering {arn}"
            );

            // The instance is still describable, and it is no longer listed: the
            // default `ListContainerInstances` filter is "all states other than
            // INACTIVE".
            let described = ok(
                &router,
                "DescribeContainerInstances",
                json!({ "cluster": cluster, "containerInstances": [arn] }),
                &case,
            )
            .await;
            assert_eq!(
                described["containerInstances"][0]["status"].as_str(),
                Some("INACTIVE"),
                "case {case:?}: expected the stored instance to report INACTIVE, got {described}"
            );
            let listed = ok(
                &router,
                "ListContainerInstances",
                json!({ "cluster": cluster }),
                &case,
            )
            .await;
            let listed: Vec<String> = list(&listed, "containerInstanceArns", &case)
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                !listed.contains(arn),
                "case {case:?}: expected {arn} to drop out of the default listing, got {listed:?}"
            );
        }

        // Deregistering an instance that is already deregistered answers with the same
        // INACTIVE instance at the same version: it is already out of the cluster's
        // capacity, so the repeat call changes nothing.
        if let Some(arn) = arns.first() {
            let first = ok(
                &router,
                "DeregisterContainerInstance",
                json!({ "cluster": cluster, "containerInstance": arn }),
                &case,
            )
            .await;
            assert_eq!(
                (
                    first["containerInstance"]["status"].as_str(),
                    count(&first["containerInstance"], "version", &case)
                ),
                (Some("INACTIVE"), 1),
                "case {case:?}: expected a repeat deregistration to report the instance as it \
                 stands, got {first}"
            );
            assert_eq!(
                registered_count(&router, &cluster, &case).await,
                0,
                "case {case:?}: expected the repeat deregistration to leave the count at zero"
            );
        }

        // The cluster drains to zero, which is what a client waits for before deleting
        // it, and the delete's own cluster view reports the same zero.
        let deleted = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": cluster }),
            &case,
        )
        .await;
        assert_eq!(
            count(
                &deleted["cluster"],
                "registeredContainerInstancesCount",
                &case
            ),
            0,
            "case {case:?}: expected the deleted cluster to report no registered instances, got {deleted}"
        );
    }
}

#[tokio::test]
async fn deregistering_an_instance_that_is_not_registered_is_refused() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "one" }),
        "unknown instance",
    )
    .await;
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "two" }),
        "unknown instance",
    )
    .await;
    let arn = register_instance(&router, "one", "i-90000001", "unknown instance").await;

    for (case, cluster, reference) in [
        ("an id that names nothing", "one", "0123456789abcdef0123456789abcdef"),
        ("an instance of another cluster", "two", arn.as_str()),
    ] {
        refused(
            &router,
            "DeregisterContainerInstance",
            json!({ "cluster": cluster, "containerInstance": reference }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // The refusals left the registered instance alone.
    assert_eq!(
        registered_count(&router, "one", "unknown instance").await,
        1,
        "expected the registered instance to survive a refused deregistration"
    );
}
