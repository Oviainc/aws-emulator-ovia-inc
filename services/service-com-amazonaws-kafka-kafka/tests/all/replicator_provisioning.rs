// SPDX-License-Identifier: CC0-1.0

//! How an MSK Replicator is PROVISIONED, and what `DescribeReplicator` projects
//! from the record once it has settled — driven end to end over the same
//! `MultiServiceRouter` the emulator serves from (the shared harness in
//! `tests/all/kafka_harness.rs`).
//!
//! Three rules, each asserted over a TABLE rather than one recorded request:
//!
//! 1. `CreateReplicator` is admitted on what the request itself says and the
//!    Amazon MSK clusters it names are resolved while the replicator is
//!    PROVISIONED, so the clock owes a replicator whose setup names a cluster that
//!    does not exist the terminal `FAILED` with the modelled `StateInfo`, and one
//!    whose clusters all resolve `RUNNING` with none.
//! 2. A Kafka cluster's alias is DERIVED from its ARN — the cluster name truncated
//!    to 28 characters, a hyphen and the cluster id's 12 hexadecimal digits — in
//!    one place, so `KafkaClusters[].KafkaClusterAlias` and the
//!    `ReplicationInfoList[]` aliases of the same cluster always agree.
//! 3. Every modelled member of the stored configuration is reported, with the
//!    value the service APPLIED where the request carried none: an empty list
//!    rather than an absent member, a tag set that is an empty map, and the
//!    service's default for each feature switch and configuration type.
//!
//! The rows differ along what each rule generalises over: which end of the setup
//! resolves and which does not, how far from this emulator's registry the named
//! cluster is (another region, another account, absent), how long the cluster name
//! is and how its id is spelled, and which members a create spells out.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::REPLICATOR_ACTIVATION_DELAY;

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// The IAM role every row hands the replicator. `ServiceExecutionRoleArn` is a
/// plain `__string` in the model, so the service stores and reports it verbatim.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// The `ReplicationStateInfo$Code` MSK reports for a replicator whose setup names
/// an Amazon MSK cluster that does not exist, and the FIXED text of the message
/// that goes with it — whose one variable part is the cluster ARN the request
/// named. Written out here rather than built from the service's own helper, so the
/// wording is pinned by this suite instead of by the code under test.
const INVALID_CLUSTER_CODE: &str = "InvalidInput.InvalidKafkaCluster";
const INVALID_CLUSTER_MESSAGE: &str =
    "The specified Amazon MSK Cluster does not exist. Cluster ARN: ";

/// How many characters of a cluster's name its alias carries, and how many
/// hexadecimal digits of its id follow them. From
/// `KafkaClusterDescription$KafkaClusterAlias` ("The alias of the Kafka cluster.
/// Used to prefix names of replicated topics."): the alias identifies the cluster,
/// so it carries the start of the name and the cluster's own id.
const ALIAS_NAME_CHARS: usize = 28;
const ALIAS_ID_CHARS: usize = 12;

/// Sleep past the emulator's own activation span, so a later read sees a settled
/// replicator. `std::thread::sleep` because the workspace's tokio carries no timer
/// feature; nothing else has to make progress here.
fn sleep_past_settling() {
    std::thread::sleep(REPLICATOR_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// The alias MSK derives for the cluster `arn` names, spelled out from the rule
/// rather than from the service's helper: the cluster NAME truncated to
/// [`ALIAS_NAME_CHARS`] characters, a hyphen, and the final group of the uuid in
/// the ARN's id segment ([`ALIAS_ID_CHARS`] hexadecimal digits), zero-filled when
/// the ARN names no uuid.
fn derived_alias(arn: &str) -> String {
    let mut segments = arn
        .splitn(6, ':')
        .nth(5)
        .unwrap_or_default()
        .split('/')
        .skip(1);
    let name: String = segments
        .next()
        .unwrap_or_default()
        .chars()
        .take(ALIAS_NAME_CHARS)
        .collect();
    let id = segments.next().unwrap_or_default();
    let groups: Vec<&str> = id.splitn(6, '-').collect();
    let uuid = groups.len() >= 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, ALIAS_ID_CHARS])
            .all(|(group, width)| {
                group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
    let tail = if uuid {
        groups[4].to_string()
    } else {
        "0".repeat(ALIAS_ID_CHARS)
    };
    format!("{name}-{tail}")
}

/// An MSK cluster ARN in `REGION`/`ACCOUNT` for a cluster named `name` with the
/// id `id` — a cluster this emulator does not hold.
fn absent_cluster_arn(name: &str, id: &str) -> String {
    format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{name}/{id}")
}

/// A well-formed uuid, as the id segment of a cluster ARN.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555";

/// `CreateReplicator` naming `clusters` (ARNs) as its two ends, with `extra`
/// merged into the request body.
fn create_body(name: &str, clusters: &[&str], extra: Value) -> Value {
    let mut body = json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": clusters
            .iter()
            .map(|arn| json!({
                "amazonMskCluster": { "mskClusterArn": arn },
                "vpcConfig": {
                    "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
                },
            }))
            .collect::<Vec<_>>(),
        "replicationInfoList": [{
            "sourceKafkaClusterArn": clusters.first().copied().unwrap_or_default(),
            "targetKafkaClusterArn": clusters.get(1).copied().unwrap_or_default(),
            "targetCompressionType": "NONE",
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
            "topicReplication": { "topicsToReplicate": [".*"] },
        }],
    });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    body
}

async fn create(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .call("POST", "/replication/v1/replicators", Some(body))
        .await
}

async fn describe(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

/// Create `body`, assert the create was ADMITTED, and hand back the minted ARN.
async fn create_ok(emulator: &Emulator, case: &str, body: Value) -> String {
    let reply = create(emulator, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected CreateReplicator to admit the request, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.at("/replicatorState").as_str(),
        Some("CREATING"),
        "case {case}: an admitted create reports CREATING, got {}",
        reply.at("/replicatorState")
    );
    reply
        .at("/replicatorArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateReplicator returned no replicatorArn"))
        .to_string()
}

/// RULE: the Amazon MSK clusters a replicator names are resolved while it is
/// PROVISIONED, not while the request is admitted — so a create naming a cluster
/// that does not exist is accepted and the clock then owes that replicator the
/// terminal `FAILED` with `StateInfo` naming the offending ARN, while a replicator
/// whose every named cluster resolves is owed `RUNNING` and reports no `StateInfo`
/// at all.
#[tokio::test]
async fn a_replicator_converges_to_failed_when_a_named_cluster_does_not_exist() {
    let emulator = Emulator::new();

    // Clusters this emulator really holds: one here, one in another region (a
    // replicator replicates BETWEEN regions, so a cluster of another region is a
    // cluster that exists).
    let here = emulator.create_ok(&ClusterRequest::named("provisioning-here")).await;
    let far = emulator
        .create_ok_in("eu-west-1", &ClusterRequest::named("provisioning-far"))
        .await;
    let absent_source = absent_cluster_arn("provisioning-absent-source", GHOST_ID);
    let absent_target = absent_cluster_arn("provisioning-absent-target", GHOST_ID);
    let other_account = format!(
        "arn:aws:kafka:{REGION}:111122223333:cluster/provisioning-other-account/{GHOST_ID}"
    );

    // `(case, the two ends, the ARN whose resolution fails — None when the
    // replicator must run)`. The rows differ along WHICH end fails and how far
    // from this registry the named cluster is.
    let rows: Vec<(&str, [&str; 2], Option<&str>)> = vec![
        ("both-present", [here.as_str(), far.as_str()], None),
        (
            "target-absent",
            [here.as_str(), absent_target.as_str()],
            Some(absent_target.as_str()),
        ),
        (
            "source-absent",
            [absent_source.as_str(), here.as_str()],
            Some(absent_source.as_str()),
        ),
        (
            "both-absent",
            [absent_source.as_str(), absent_target.as_str()],
            // The FIRST entry that does not resolve is the one reported, so one
            // setup yields one reason.
            Some(absent_source.as_str()),
        ),
        (
            "another-account",
            [other_account.as_str(), here.as_str()],
            Some(other_account.as_str()),
        ),
    ];

    let mut minted: Vec<(&str, String, Option<&str>)> = Vec::new();
    for (case, clusters, failing) in &rows {
        let arn = create_ok(
            &emulator,
            case,
            create_body(&format!("converge-{case}"), clusters, Value::Null),
        )
        .await;
        // The span is never zero: a caller reading straight after the create sees
        // the transient state, whichever way the replicator is heading.
        let immediate = describe(&emulator, &arn).await;
        assert_eq!(
            immediate.at("/replicatorState").as_str(),
            Some("CREATING"),
            "case {case}: a replicator read straight after the create is still CREATING, got {}",
            immediate.at("/replicatorState")
        );
        assert_eq!(
            immediate.at("/stateInfo"),
            &Value::Null,
            "case {case}: a CREATING replicator explains itself, got {}",
            immediate.at("/stateInfo")
        );
        minted.push((case, arn, *failing));
    }

    sleep_past_settling();
    for (case, arn, failing) in &minted {
        // Read twice: the transition the clock owed is materialised once, and a
        // read never moves the state on again.
        for attempt in 1..=2 {
            let settled = describe(&emulator, arn).await;
            let (expected_state, expected_info) = match failing {
                Some(cluster) => (
                    "FAILED",
                    json!({
                        "code": INVALID_CLUSTER_CODE,
                        "message": format!("{INVALID_CLUSTER_MESSAGE}{cluster}"),
                    }),
                ),
                None => ("RUNNING", Value::Null),
            };
            assert_eq!(
                settled.at("/replicatorState").as_str(),
                Some(expected_state),
                "case {case} (read {attempt}): expected replicatorState {expected_state}, got {}",
                settled.at("/replicatorState")
            );
            assert_eq!(
                settled.at("/stateInfo"),
                &expected_info,
                "case {case} (read {attempt}): expected stateInfo {expected_info}, got {}",
                settled.at("/stateInfo")
            );
        }
    }
}

/// RULE: a Kafka cluster's alias is DERIVED from its ARN — the cluster name
/// truncated to 28 characters, a hyphen, and the cluster id's 12 hexadecimal
/// digits (zeroes when the ARN names no uuid) — and it is derived in ONE place, so
/// the alias `KafkaClusters[]` reports and the aliases
/// `ReplicationInfoList[].SourceKafkaClusterAlias` /
/// `TargetKafkaClusterAlias` carry are the same string.
#[tokio::test]
async fn a_cluster_alias_is_derived_from_the_cluster_arn() {
    let emulator = Emulator::new();

    // A cluster this emulator really minted, so one row runs against an ARN the
    // service wrote rather than one the suite wrote.
    let minted = emulator.create_ok(&ClusterRequest::named("alias-minted")).await;

    // Rows differ along what the derivation has to cope with: a short name, a name
    // of exactly the alias's name length, a longer one, a longer one whose 28th
    // character is the separator (so the alias carries two hyphens in a row), and
    // an id that is not a uuid at all.
    let twenty_eight = "a".repeat(ALIAS_NAME_CHARS);
    let longer = format!("{}-overflowing-tail", "b".repeat(ALIAS_NAME_CHARS));
    let hyphen_at_the_cut = format!("{}-cluster", "c".repeat(ALIAS_NAME_CHARS - 1));
    let rows: Vec<(&str, String)> = vec![
        ("minted-by-the-service", minted.clone()),
        ("short-name", absent_cluster_arn("alias-short", GHOST_ID)),
        ("name-at-the-cut", absent_cluster_arn(&twenty_eight, GHOST_ID)),
        ("name-past-the-cut", absent_cluster_arn(&longer, GHOST_ID)),
        (
            "separator-at-the-cut",
            absent_cluster_arn(&hyphen_at_the_cut, GHOST_ID),
        ),
        ("id-is-not-a-uuid", absent_cluster_arn("alias-plain-id", "7")),
        (
            "uuid-with-an-ordinal",
            absent_cluster_arn("alias-ordinal", &format!("{GHOST_ID}-9")),
        ),
    ];

    for (index, (case, source)) in rows.iter().enumerate() {
        let target = absent_cluster_arn(&format!("alias-target-{index}"), GHOST_ID);
        let arn = create_ok(
            &emulator,
            case,
            create_body(
                &format!("alias-{index}"),
                &[source.as_str(), target.as_str()],
                Value::Null,
            ),
        )
        .await;
        let described = describe(&emulator, &arn).await;
        let expected = derived_alias(source);
        assert_eq!(
            described.at("/kafkaClusters/0/kafkaClusterAlias").as_str(),
            Some(expected.as_str()),
            "case {case}: expected kafkaClusterAlias {expected:?}, got {}",
            described.at("/kafkaClusters/0/kafkaClusterAlias")
        );
        // The alias is at most one name-length plus the hyphen and the id digits,
        // however long the cluster's name is.
        assert!(
            expected.chars().count() <= ALIAS_NAME_CHARS + 1 + ALIAS_ID_CHARS,
            "case {case}: the alias must be bounded, got {expected:?}"
        );
        // The flow reports the SAME alias for the same cluster — the one thing
        // other replication APIs address that end by.
        assert_eq!(
            described
                .at("/replicationInfoList/0/sourceKafkaClusterAlias")
                .as_str(),
            Some(expected.as_str()),
            "case {case}: the flow's sourceKafkaClusterAlias must be the derived alias, got {}",
            described.at("/replicationInfoList/0/sourceKafkaClusterAlias")
        );
        assert_eq!(
            described
                .at("/replicationInfoList/0/targetKafkaClusterAlias")
                .as_str(),
            Some(derived_alias(&target).as_str()),
            "case {case}: the flow's targetKafkaClusterAlias must be the derived alias, got {}",
            described.at("/replicationInfoList/0/targetKafkaClusterAlias")
        );
    }
}

/// RULE: the reader reports the configuration the replication RUNS under, so every
/// modelled member of a stored replication is reported — with the value the
/// service APPLIED where the request carried none: a selection the request did not
/// narrow excludes nothing (an empty list, not an absent member), a feature switch
/// it did not set is on, each configuration shape carries the service's default
/// type, a client VPC configuration reports an empty security-group list, and a
/// replicator that carries no tags reports an empty tag set.
#[tokio::test]
async fn an_omitted_member_is_reported_with_the_value_the_service_applied() {
    let emulator = Emulator::new();
    let source = absent_cluster_arn("defaults-source", GHOST_ID);
    let target = absent_cluster_arn("defaults-target", GHOST_ID);

    // Row one: the request spells out ONLY the required members, so every
    // reported value below is one the service applied.
    let minimal = create_ok(
        &emulator,
        "minimal",
        create_body(
            "defaults-minimal",
            &[source.as_str(), target.as_str()],
            Value::Null,
        ),
    )
    .await;
    let described = describe(&emulator, &minimal).await;
    let expected_flow = json!({
        "consumerGroupReplication": {
            "consumerGroupsToReplicate": [".*"],
            "consumerGroupsToExclude": [],
            "detectAndCopyNewConsumerGroups": true,
            "synchroniseConsumerGroupOffsets": true,
            "consumerGroupOffsetSyncMode": "LEGACY",
        },
        "sourceKafkaClusterAlias": derived_alias(&source),
        "targetCompressionType": "NONE",
        "targetKafkaClusterAlias": derived_alias(&target),
        "topicReplication": {
            "topicsToReplicate": [".*"],
            "topicsToExclude": [],
            "copyAccessControlListsForTopics": true,
            "copyTopicConfigurations": true,
            "detectAndCopyNewTopics": true,
            "startingPosition": { "type": "LATEST" },
            "topicNameConfiguration": { "type": "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS" },
        },
    });
    assert_eq!(
        described.at("/replicationInfoList/0"),
        &expected_flow,
        "case minimal: expected the applied configuration {expected_flow}, got {}",
        described.at("/replicationInfoList/0")
    );
    assert_eq!(
        described.at("/kafkaClusters/0/vpcConfig/securityGroupIds"),
        &json!([]),
        "case minimal: a client VPC configuration reports an empty security-group list, got {}",
        described.at("/kafkaClusters/0/vpcConfig")
    );
    assert_eq!(
        described.at("/tags"),
        &json!({}),
        "case minimal: a replicator with no tags reports an empty tag set, got {}",
        described.at("/tags")
    );

    // Row two: the same members spelled out, each with the value the service would
    // NOT have applied, so the reader is shown to report the record rather than a
    // default. `Tags` and the security groups are given too.
    let spelled_out = create_ok(
        &emulator,
        "spelled-out",
        create_body(
            "defaults-spelled-out",
            &[source.as_str(), target.as_str()],
            json!({
                "tags": { "env": "prod" },
                "kafkaClusters": [
                    {
                        "amazonMskCluster": { "mskClusterArn": source.as_str() },
                        "vpcConfig": {
                            "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
                            "securityGroupIds": ["sg-aaaaaaaaaaaaaaaaa"],
                        },
                    },
                    {
                        "amazonMskCluster": { "mskClusterArn": target.as_str() },
                        "vpcConfig": {
                            "subnetIds": ["subnet-bbbbbbbbbbbbbbbbb", "subnet-ccccccccccccccccc"],
                        },
                    },
                ],
                "replicationInfoList": [{
                    "sourceKafkaClusterArn": source.as_str(),
                    "targetKafkaClusterArn": target.as_str(),
                    "targetCompressionType": "GZIP",
                    "consumerGroupReplication": {
                        "consumerGroupsToReplicate": ["orders-.*"],
                        "consumerGroupsToExclude": ["console-consumer-.*"],
                        "detectAndCopyNewConsumerGroups": false,
                        "synchroniseConsumerGroupOffsets": false,
                        "consumerGroupOffsetSyncMode": "ENHANCED",
                    },
                    "topicReplication": {
                        "topicsToReplicate": ["orders-.*"],
                        "topicsToExclude": ["__.*"],
                        "copyAccessControlListsForTopics": false,
                        "copyTopicConfigurations": false,
                        "detectAndCopyNewTopics": false,
                        "startingPosition": { "type": "EARLIEST" },
                        "topicNameConfiguration": { "type": "IDENTICAL" },
                    },
                }],
            }),
        ),
    )
    .await;
    let described = describe(&emulator, &spelled_out).await;
    let expected_flow = json!({
        "consumerGroupReplication": {
            "consumerGroupsToReplicate": ["orders-.*"],
            "consumerGroupsToExclude": ["console-consumer-.*"],
            "detectAndCopyNewConsumerGroups": false,
            "synchroniseConsumerGroupOffsets": false,
            "consumerGroupOffsetSyncMode": "ENHANCED",
        },
        "sourceKafkaClusterAlias": derived_alias(&source),
        "targetCompressionType": "GZIP",
        "targetKafkaClusterAlias": derived_alias(&target),
        "topicReplication": {
            "topicsToReplicate": ["orders-.*"],
            "topicsToExclude": ["__.*"],
            "copyAccessControlListsForTopics": false,
            "copyTopicConfigurations": false,
            "detectAndCopyNewTopics": false,
            "startingPosition": { "type": "EARLIEST" },
            "topicNameConfiguration": { "type": "IDENTICAL" },
        },
    });
    assert_eq!(
        described.at("/replicationInfoList/0"),
        &expected_flow,
        "case spelled-out: expected the submitted configuration {expected_flow}, got {}",
        described.at("/replicationInfoList/0")
    );
    assert_eq!(
        described.at("/kafkaClusters/0/vpcConfig/securityGroupIds"),
        &json!(["sg-aaaaaaaaaaaaaaaaa"]),
        "case spelled-out: the submitted security groups are reported, got {}",
        described.at("/kafkaClusters/0/vpcConfig")
    );
    assert_eq!(
        described.at("/kafkaClusters/1/vpcConfig/securityGroupIds"),
        &json!([]),
        "case spelled-out: the entry that attached none reports an empty list, got {}",
        described.at("/kafkaClusters/1/vpcConfig")
    );
    assert_eq!(
        described.at("/tags"),
        &json!({ "env": "prod" }),
        "case spelled-out: the submitted tags are reported, got {}",
        described.at("/tags")
    );

    // And the projection is a READ: two reads of either replicator answer the
    // same, so nothing a reader does changes what a later caller sees.
    for (case, arn) in [("minimal", &minimal), ("spelled-out", &spelled_out)] {
        let first = describe(&emulator, arn).await;
        let second = describe(&emulator, arn).await;
        assert_eq!(
            first.body, second.body,
            "case {case}: two reads must answer the same, got {} then {}",
            first.body, second.body
        );
    }
}
