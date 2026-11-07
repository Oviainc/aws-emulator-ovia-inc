// SPDX-License-Identifier: CC0-1.0

//! Naming the two ENDS of a replication: `CreateReplicator` and
//! `UpdateReplicationInfo` each take a replication whose source and target Kafka
//! cluster can be given EITHER by the Amazon MSK cluster's ARN or by the Apache
//! Kafka cluster's id, and the model leaves all four members optional because
//! either one identifies the cluster — not because an end may be left unnamed or
//! named twice.
//!
//! The suites below assert that rule over a TABLE, not one recorded request. The
//! rows differ along which end is not named exactly once, how it fails to be
//! (omitted, given twice, given empty), how the OTHER end is named, which
//! operation carries the replication, which replicator the path names and which
//! version the request quotes — because the rule is about the REQUEST, so none of
//! those may change the answer.
//!
//! The HTTP harness comes from [`crate::kafka_harness`] — the same router, the
//! same SigV4-scoped request builder — rather than a second one built here.

use serde_json::{json, Value};

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// The IAM role every replicator below is created with. `ServiceExecutionRoleArn`
/// is a plain `__string` in the model, so the service stores it verbatim.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// The uuid segment of the resource ARNs these rows point at. No cluster behind
/// them is ever created: `CreateReplicator` is asynchronous and does not resolve
/// the clusters it spans.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// What the service refuses a request that does not name the SOURCE end of its
/// replication exactly once with: one fixed sentence, naming the two members the
/// wire carries that end in, with no part of it taken from the request. Written
/// out here rather than read from the service's own helper, so the wording is
/// pinned by the suite instead of by the code under test.
const SOURCE_UNNAMED_MESSAGE: &str =
    "Must specify either sourceKafkaClusterArn or sourceKafkaClusterId for the source cluster.";

/// The same sentence for the TARGET end.
const TARGET_UNNAMED_MESSAGE: &str =
    "Must specify either targetKafkaClusterArn or targetKafkaClusterId for the target cluster.";

/// The message this service's replicator APIs refuse an ARN they cannot resolve
/// with: the input member at fault, then the fixed not-found sentence (whose
/// apostrophe is U+2019, as the service writes it).
const NOT_FOUND_MESSAGE: &str =
    "Invalid parameter replicatorArn: The requested resource doesn\u{2019}t exist.";

/// A well-formed Amazon MSK cluster ARN in the signing region that no cluster
/// holds — enough for a request that is refused on its BODY, which never resolves
/// the clusters it names.
fn cluster_arn(name: &str) -> String {
    format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{name}/{GHOST_ID}")
}

/// A well-formed replicator ARN that was never issued, so nothing resolves it.
fn absent_replicator_arn(name: &str) -> String {
    format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/{name}/{GHOST_ID}")
}

/// One `KafkaClusters` entry naming an Amazon MSK cluster by ARN.
fn msk_cluster(arn: &str) -> Value {
    json!({
        "amazonMskCluster": { "mskClusterArn": arn },
        "vpcConfig": {
            "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
            "securityGroupIds": ["sg-aaaaaaaaaaaaaaaaa"],
        },
    })
}

/// A `CreateReplicator` body spanning the two MSK clusters `source` and `target`,
/// with one replication whose end members are exactly those in `ends`.
fn create_body(name: &str, source: &str, target: &str, ends: &Value) -> Value {
    let mut info = json!({
        "targetCompressionType": "NONE",
        "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
        "topicReplication": { "topicsToReplicate": [".*"] },
    });
    for (member, value) in ends.as_object().expect("the ends are a JSON object") {
        info[member] = value.clone();
    }
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": [msk_cluster(source), msk_cluster(target)],
        "replicationInfoList": [info],
    })
}

async fn create(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .call("POST", "/replication/v1/replicators", Some(body))
        .await
}

/// `DescribeReplicator` for `arn`.
async fn describe(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

/// `UpdateReplicationInfo` for `arn`, which the model binds to
/// `PUT /replication/v1/replicators/{ReplicatorArn}/replication-info`.
async fn update(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!(
                "/replication/v1/replicators/{}/replication-info",
                pct(arn)
            ),
            Some(body),
        )
        .await
}

/// An `UpdateReplicationInfo` body: the end members in `ends`, the quoted
/// version, and a `TopicReplication` update, so a request the end rule admits is
/// one the operation could otherwise carry out.
fn update_body(ends: &Value, version: &str) -> Value {
    let mut body = json!({
        "currentVersion": version,
        "topicReplication": {
            "topicsToReplicate": ["accepted-.*"],
            "topicsToExclude": ["__.*"],
            "detectAndCopyNewTopics": true,
            "copyTopicConfigurations": true,
            "copyAccessControlListsForTopics": false,
        },
    });
    for (member, value) in ends.as_object().expect("the ends are a JSON object") {
        body[member] = value.clone();
    }
    body
}

/// A replicator spanning two MSK clusters with one replication between them, and
/// the version the reader reports for it — plus the ARNs of the two clusters it
/// spans.
///
/// The clusters are CREATED here: Amazon MSK provisions a replicator only once the
/// Amazon MSK clusters its setup names resolve, and a replicator that never
/// provisions converges to the terminal `FAILED`, which is not a state a
/// configuration change is accepted from. The control below changes this
/// replicator, so it has to be one that reaches `RUNNING`.
async fn replicator(emulator: &Emulator, name: &str) -> (String, String, String, String) {
    let source = emulator
        .create_ok(&ClusterRequest::named(&format!("source-{name}")))
        .await;
    let target = emulator
        .create_ok(&ClusterRequest::named(&format!("target-{name}")))
        .await;
    let ends = json!({
        "sourceKafkaClusterArn": source,
        "targetKafkaClusterArn": target,
    });
    let created = create(emulator, create_body(name, &source, &target, &ends)).await;
    assert_eq!(
        created.status, 200,
        "case {name}: expected CreateReplicator to return 200, got {} {}",
        created.status, created.body
    );
    let arn = created.body["replicatorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: CreateReplicator returned no replicatorArn"))
        .to_string();
    let version = describe(emulator, &arn)
        .await
        .at("/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: DescribeReplicator reported no currentVersion"))
        .to_string();
    (arn, version, source, target)
}

/// Every way a request can fail to name one end of its replication exactly once,
/// with the end the refusal must be about: the pair omitted, given twice, given
/// empty, and both pairs missing at once. `ends` is spliced into whichever
/// operation's body the row drives.
fn unnamed_end_rows(source: &str, target: &str) -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "no-source",
            json!({ "targetKafkaClusterArn": target }),
            SOURCE_UNNAMED_MESSAGE,
        ),
        (
            "no-source-target-by-id",
            json!({ "targetKafkaClusterId": "target-by-id" }),
            SOURCE_UNNAMED_MESSAGE,
        ),
        (
            "no-target",
            json!({ "sourceKafkaClusterArn": source }),
            TARGET_UNNAMED_MESSAGE,
        ),
        (
            "no-target-source-by-id",
            json!({ "sourceKafkaClusterId": "source-by-id" }),
            TARGET_UNNAMED_MESSAGE,
        ),
        // Neither end named: the source end is the one the refusal is about, so a
        // caller fixing one omission at a time is walked through them in order.
        ("no-ends", json!({}), SOURCE_UNNAMED_MESSAGE),
        (
            "source-named-twice",
            json!({
                "sourceKafkaClusterArn": source,
                "sourceKafkaClusterId": "source-by-id",
                "targetKafkaClusterArn": target,
            }),
            SOURCE_UNNAMED_MESSAGE,
        ),
        (
            "target-named-twice",
            json!({
                "sourceKafkaClusterArn": source,
                "targetKafkaClusterArn": target,
                "targetKafkaClusterId": "target-by-id",
            }),
            TARGET_UNNAMED_MESSAGE,
        ),
        // An empty string names no cluster, so a member sent empty leaves its end
        // unnamed rather than naming one nothing resolves.
        (
            "empty-source-arn",
            json!({ "sourceKafkaClusterArn": "", "targetKafkaClusterArn": target }),
            SOURCE_UNNAMED_MESSAGE,
        ),
        (
            "empty-source-id",
            json!({ "sourceKafkaClusterId": "", "targetKafkaClusterArn": target }),
            SOURCE_UNNAMED_MESSAGE,
        ),
        (
            "empty-target-pair",
            json!({
                "sourceKafkaClusterArn": source,
                "targetKafkaClusterArn": "",
                "targetKafkaClusterId": "",
            }),
            TARGET_UNNAMED_MESSAGE,
        ),
    ]
}

/// RULE: `UpdateReplicationInfo` admits the request against the rules of the
/// request itself before it resolves anything the request NAMES, so a request that
/// does not name each end of its replication exactly once is refused HTTP 400
/// `BadRequestException` with that end's fixed sentence — whatever replicator the
/// path addresses, whether or not that replicator exists, and whatever version the
/// request quotes — and changes nothing.
#[tokio::test]
async fn an_unnamed_replication_end_is_refused_before_the_replicator_is_resolved() {
    let emulator = Emulator::new();
    let (present, version, source, target) = replicator(&emulator, "ends").await;
    let intact = describe(&emulator, &present).await.body;

    // The path dimension is what makes the ORDER of the two gates observable: the
    // same body is answered the same way against a replicator that exists and one
    // that was never issued, which only holds if the body is admitted first.
    let paths = [
        ("present", present.clone()),
        ("never-issued", absent_replicator_arn("never-issued")),
    ];
    // Quoting a version the replicator is not at cannot change the answer either:
    // the body is admitted before the version it quotes is compared.
    let versions = [
        ("current", version.clone()),
        ("stale", "K0000000000000".to_string()),
    ];

    for (case, ends, expected) in unnamed_end_rows(&source, &target) {
        for (path_case, arn) in &paths {
            for (version_case, quoted) in &versions {
                let reply = update(&emulator, arn, update_body(&ends, quoted)).await;
                assert_eq!(
                    (reply.status, reply.error_code(), reply.error_message()),
                    (400, "BadRequestException", expected),
                    "case {case}/{path_case}/{version_case}: expected 400 BadRequestException \
                     {expected:?}, got {} {}",
                    reply.status,
                    reply.body
                );
            }
        }
    }

    // None of those refusals wrote anything: the whole reader answer, version and
    // configuration included, is the one from before them.
    assert_eq!(
        describe(&emulator, &present).await.body,
        intact,
        "a refused update must leave the replicator exactly as it was"
    );

    // The control: the same update with each end named exactly once IS carried out,
    // and an independent read reports it. The replicator has to be RUNNING for that:
    // a change is applied only from the state that accepts one, so the clock is
    // allowed to settle this replicator first.
    std::thread::sleep(
        service_com_amazonaws_kafka_kafka::state::REPLICATOR_ACTIVATION_DELAY
            + std::time::Duration::from_millis(250),
    );
    let accepted = update(
        &emulator,
        &present,
        update_body(
            &json!({
                "sourceKafkaClusterArn": source,
                "targetKafkaClusterArn": target,
            }),
            &version,
        ),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a request naming both ends exactly once must be carried out, got {} {}",
        accepted.status, accepted.body
    );
    let described = describe(&emulator, &present).await;
    assert_eq!(
        described.at("/replicationInfoList/0/topicReplication/topicsToReplicate"),
        &json!(["accepted-.*"]),
        "the accepted update must be visible to the reader, got {}",
        described.at("/replicationInfoList/0/topicReplication")
    );
}

/// RULE: only a request that names both its ends reaches the replicator the path
/// addresses, and a well-formed replicator ARN this region holds nothing for is
/// then refused HTTP 404 `NotFoundException` with the refusal every replicator
/// operation answers an ARN it cannot resolve with — the input member at fault,
/// then the service's fixed not-found sentence — and reports that member in
/// `InvalidParameter`.
#[tokio::test]
async fn a_complete_request_against_an_unknown_replicator_is_not_found() {
    let emulator = Emulator::new();
    let source = cluster_arn("source-complete");
    let target = cluster_arn("target-complete");

    // Rows differ in how each end is named — the two ways each, in every
    // combination — because every one of them names the end exactly once and so
    // must reach the replicator lookup.
    let rows: Vec<(&str, Value)> = vec![
        (
            "arn-arn",
            json!({ "sourceKafkaClusterArn": source, "targetKafkaClusterArn": target }),
        ),
        (
            "arn-id",
            json!({ "sourceKafkaClusterArn": source, "targetKafkaClusterId": "target-by-id" }),
        ),
        (
            "id-arn",
            json!({ "sourceKafkaClusterId": "source-by-id", "targetKafkaClusterArn": target }),
        ),
        (
            "id-id",
            json!({
                "sourceKafkaClusterId": "source-by-id",
                "targetKafkaClusterId": "target-by-id",
            }),
        ),
    ];

    for (case, ends) in &rows {
        let arn = absent_replicator_arn(case);
        let reply = update(&emulator, &arn, update_body(ends, "K0000000000000")).await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (404, "NotFoundException", NOT_FOUND_MESSAGE),
            "case {case}: expected 404 NotFoundException {NOT_FOUND_MESSAGE:?}, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("replicatorArn"),
            "case {case}: the refusal must report the member that resolved to nothing, got {}",
            reply.body
        );
    }
}

/// RULE: the same end-naming rule holds wherever a replication is submitted, so a
/// `ReplicationInfoList` entry that does not name an end exactly once is refused
/// HTTP 400 `BadRequestException` with that end's fixed sentence, naming the entry
/// at fault, and registers no replicator.
#[tokio::test]
async fn a_created_replication_that_does_not_name_an_end_is_refused() {
    let emulator = Emulator::new();
    let source = cluster_arn("source-created");
    let target = cluster_arn("target-created");

    for (case, ends, expected) in unnamed_end_rows(&source, &target) {
        let name = format!("created-{case}");
        let reply = create(&emulator, create_body(&name, &source, &target, &ends)).await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (400, "BadRequestException", expected),
            "case {case}: expected 400 BadRequestException {expected:?}, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("ReplicationInfoList[0]"),
            "case {case}: the refusal must name the entry at fault, got {}",
            reply.body
        );
        // A refused create registers nothing, so the name it asked for is still
        // free — and the listing the caller reads next does not hold it.
        let listed = emulator
            .call("GET", "/replication/v1/replicators", None)
            .await;
        assert!(
            !listed.body.to_string().contains(&name),
            "case {case}: a refused create must register nothing, got {}",
            listed.body
        );
    }

    // The control: the same entry with both ends named exactly once is accepted,
    // and the replicator it mints is readable back.
    let (arn, _, _, _) = replicator(&emulator, "created-control").await;
    let described = describe(&emulator, &arn).await;
    assert_eq!(
        described
            .at("/replicationInfoList/0/sourceKafkaClusterAlias")
            .as_str()
            .map(|alias| alias.starts_with("source-created-control")),
        Some(true),
        "the accepted entry must be reported under its source cluster's alias, got {}",
        described.at("/replicationInfoList/0")
    );
}
