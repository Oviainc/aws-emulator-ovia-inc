// SPDX-License-Identifier: CC0-1.0

//! A replicator the service admitted against references that resolve to NOTHING is
//! a real resource of this service, driven end to end over the same router the
//! emulator serves from (the shared harness in `tests/all/kafka_harness.rs`).
//!
//! Three rules, each asserted over a TABLE rather than over one recorded request:
//!
//! 1. `CreateReplicator` is admitted on what the REQUEST itself says: the Amazon
//!    MSK clusters a setup names are another resource's to resolve, so a create
//!    naming never-issued cluster ARNs answers HTTP 200 with a minted
//!    `ReplicatorArn` and `ReplicatorState` `CREATING`, the replicator it minted is
//!    registered (every reader serves it, and `ListReplicators` finds it by name),
//!    and the unresolved reference surfaces on the replicator's OWN state once the
//!    clock settles it — `FAILED` carrying the modelled `StateInfo` — never as a
//!    synchronous refusal of the create.
//! 2. Such a replicator is a taggable resource like any other: `ListTagsForResource`
//!    reports what the create stored, `TagResource` merges the pairs it carries, a
//!    key beginning with the prefix AWS reserves is refused with the service's fixed
//!    sentence and writes nothing, and `UntagResource` removes exactly the keys it
//!    names.
//! 3. A replicator name identifies a replicator within an account and region, so the
//!    refusal of a name already in use is a function of the NAME alone: every repeat
//!    answers the identical conflict, and the registry still holds exactly one
//!    replicator under that name. A create the service refuses from the request
//!    alone registers nothing, so the name it asked for is reported by no listing and
//!    is free for the next create.
//!
//! The rows differ along what each rule generalises over: how far from this
//! registry the unresolvable cluster reference is (this region, another region,
//! another account), which end of the flow carries it, whether the setup names an
//! Amazon MSK cluster at all (a self-managed Apache Kafka cluster is nothing this
//! service resolves, so such a setup is the control that settles `RUNNING`), which
//! members the create spells out, the state the replicator is in when a tagging
//! call arrives, the spelling of a reserved tag key, and which synchronous refusal
//! class the create earns.
//!
//! Every expected sentence is WRITTEN OUT here rather than imported from the
//! implementation, so an assertion cannot agree with a constant that is wrong.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::REPLICATOR_ACTIVATION_DELAY;

use crate::kafka_harness::{
    assert_name_already_exists, pct, Emulator, Reply, ACCOUNT, REGION,
};

/// The IAM role every row hands the replicator: a role of the caller's own account,
/// so the pass-role decision admits it and the rule under test is what answers.
/// `ServiceExecutionRoleArn` is a plain `__string` in the model, so the service
/// stores and reports it verbatim.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// The `ReplicationStateInfo$Code` Amazon MSK reports for a replicator whose setup
/// names an Amazon MSK cluster that does not exist, and the FIXED text of the
/// message that goes with it — whose one variable part is the cluster ARN the
/// request named.
const INVALID_CLUSTER_CODE: &str = "InvalidInput.InvalidKafkaCluster";
const INVALID_CLUSTER_MESSAGE: &str =
    "The specified Amazon MSK Cluster does not exist. Cluster ARN: ";

/// What Amazon MSK answers a tag key beginning with the prefix it reserves for
/// itself. The parenthetical is part of the sentence, and the offending key is not
/// in it.
const RESERVED_TAG_KEY: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// What Amazon MSK answers a replicator whose cluster list names one Kafka cluster
/// twice.
const DUPLICATE_CLUSTERS: &str =
    "Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs \
     are unique.";

/// A well-formed uuid, as the id segment of a cluster ARN.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// Sleep past the emulator's own activation span, so a later read sees a settled
/// replicator. `std::thread::sleep` because the workspace's tokio carries no timer
/// feature; nothing else has to make progress here.
fn sleep_past_settling() {
    std::thread::sleep(REPLICATOR_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// A well-formed Amazon MSK cluster ARN that was never issued: `region` and
/// `account` vary, because a cluster of another region or another account is a
/// cluster this registry holds no record of just the same.
fn ghost_cluster_arn(region: &str, account: &str, name: &str) -> String {
    format!("arn:aws:kafka:{region}:{account}:cluster/{name}/{GHOST_ID}")
}

/// How one end of a setup names its Kafka cluster: an Amazon MSK cluster by ARN, or
/// a self-managed Apache Kafka cluster by id. Both kinds, because the rule is about
/// what the service RESOLVES and only the Amazon MSK cluster is a resource it holds
/// records of.
#[derive(Clone, Debug)]
enum ClusterRef {
    Msk(String),
    Apache(String),
}

impl ClusterRef {
    /// This end as a `KafkaClusters` entry, placed in `subnets` — the client VPC
    /// configuration MSK creates the replicator's own interfaces in.
    fn entry(&self, subnets: &[&str]) -> Value {
        let mut entry = match self {
            ClusterRef::Msk(arn) => json!({ "amazonMskCluster": { "mskClusterArn": arn } }),
            ClusterRef::Apache(id) => json!({
                "apacheKafkaCluster": {
                    "apacheKafkaClusterId": id,
                    "bootstrapBrokerString": format!("b-1.{id}.example.internal:9092"),
                },
            }),
        };
        entry["vpcConfig"] = json!({ "subnetIds": subnets });
        entry
    }

    /// The `ReplicationInfo` member naming this end of the flow, and its value.
    fn flow_member(&self, end: &str) -> (String, Value) {
        match self {
            ClusterRef::Msk(arn) => (format!("{end}KafkaClusterArn"), json!(arn)),
            ClusterRef::Apache(id) => (format!("{end}KafkaClusterId"), json!(id)),
        }
    }

    /// The Amazon MSK cluster ARN this end names, or `None` for a self-managed
    /// Apache Kafka cluster — which is no resource of this service, so nothing of it
    /// is resolved.
    fn msk_arn(&self) -> Option<&str> {
        match self {
            ClusterRef::Msk(arn) => Some(arn.as_str()),
            ClusterRef::Apache(_) => None,
        }
    }
}

/// The subnets a row places the replicator's client interfaces in. Amazon EC2 is
/// not mounted beside this service here, so nothing is known about the account's
/// subnets and these ids name a placement of the request alone — in EC2's 17-hex id
/// form, as the recorded request spells it.
const SUBNETS: [&str; 2] = ["subnet-0a1b2c3d4e5f60718", "subnet-0a1b2c3d4e5f60719"];

/// A `CreateReplicator` body: one flow from `source` to `target`, each end also
/// named as a `KafkaClusters` entry, with `tags` when the row carries any.
fn create_body(name: &str, source: &ClusterRef, target: &ClusterRef, tags: &[(&str, &str)]) -> Value {
    let mut flow = json!({
        "targetCompressionType": "NONE",
        "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
        "topicReplication": { "topicsToReplicate": [".*"] },
    });
    for (end, reference) in [("source", source), ("target", target)] {
        let (member, value) = reference.flow_member(end);
        flow[member] = value;
    }
    let mut body = json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": [source.entry(&SUBNETS), target.entry(&SUBNETS)],
        "replicationInfoList": [flow],
    });
    if !tags.is_empty() {
        body["tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
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

async fn delete(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

/// The names `ListReplicators` reports under the `ReplicatorNameFilter` `prefix` —
/// the listing is the registry's own answer to "is there a replicator called this".
async fn listed_names(emulator: &Emulator, prefix: &str) -> Vec<String> {
    let reply = emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators?replicatorNameFilter={}", pct(prefix)),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "ListReplicators({prefix:?}) must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/replicators")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("replicatorName").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn list_tags(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/tags/{}", pct(arn)), None)
        .await
}

async fn tag(emulator: &Emulator, arn: &str, pairs: &[(&str, &str)]) -> Reply {
    let body = json!({
        "tags": Value::Object(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        ),
    });
    emulator
        .call("POST", &format!("/v1/tags/{}", pct(arn)), Some(body))
        .await
}

/// `UntagResource` binds `TagKeys` to the `tagKeys` query parameter, which a list
/// repeats once per key.
async fn untag(emulator: &Emulator, arn: &str, keys: &[&str]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .call("DELETE", &format!("/v1/tags/{}?{query}", pct(arn)), None)
        .await
}

/// The tag map a row expects, as `ListTagsForResource` reports it.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// One `CreateReplicator` setup a row of the admission table drives: the case it
/// is named by, the two ends of its one replication flow, and the tags the create
/// carries.
struct Setup {
    case: &'static str,
    source: ClusterRef,
    target: ClusterRef,
    tags: Vec<(&'static str, &'static str)>,
}

/// One row of the tagging table: the case, whether the replicator is left to settle
/// before the tagging calls arrive, and the tag set its create stored.
struct Tagged {
    case: &'static str,
    settle: bool,
    stored: Vec<(&'static str, &'static str)>,
}

/// Create `body`, assert the create was ADMITTED and reports the request's own
/// name, and hand back the minted ARN.
async fn create_ok(emulator: &Emulator, case: &str, name: &str, body: Value) -> String {
    let reply = create(emulator, body).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (200, ""),
        "case {case}: expected CreateReplicator to admit the request, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/replicatorState").as_str(),
        Some("CREATING"),
        "case {case}: an admitted create reports CREATING, got {}",
        reply.at("/replicatorState")
    );
    assert_eq!(
        reply.at("/replicatorName").as_str(),
        Some(name),
        "case {case}: the create reports the name it was given, got {}",
        reply.at("/replicatorName")
    );
    let arn = reply
        .at("/replicatorArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateReplicator returned no replicatorArn"))
        .to_string();
    let expected_prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/{name}/");
    assert!(
        arn.starts_with(&expected_prefix),
        "case {case}: expected an ARN under {expected_prefix:?}, got {arn:?}"
    );
    arn
}

/// RULE: `CreateReplicator` is admitted on what the request itself says — the
/// Amazon MSK clusters a setup names are resolved while the replicator is
/// PROVISIONED, not while the request is admitted — so a create naming never-issued
/// cluster ARNs is accepted with `ReplicatorState` `CREATING`, the replicator it
/// mints is registered and found by name, and the unresolved reference surfaces on
/// that replicator's own state: the clock owes it the terminal `FAILED` with the
/// modelled `StateInfo`, which is also the state it is deleted from. A setup naming
/// no Amazon MSK cluster at all has nothing to resolve and is owed `RUNNING` with no
/// `StateInfo`.
#[tokio::test]
async fn a_create_naming_references_that_resolve_to_nothing_is_admitted_and_settles() {
    let emulator = Emulator::new();

    // The rows differ along how far from this registry the unresolvable reference
    // is, which END carries it, whether the setup names an Amazon MSK cluster at
    // all, and which members the create spells out.
    let here_source = ClusterRef::Msk(ghost_cluster_arn(REGION, ACCOUNT, "refonly-source"));
    let here_target = ClusterRef::Msk(ghost_cluster_arn(REGION, ACCOUNT, "refonly-target"));
    let far = ClusterRef::Msk(ghost_cluster_arn("eu-west-1", ACCOUNT, "refonly-far"));
    let other_account =
        ClusterRef::Msk(ghost_cluster_arn(REGION, "111122223333", "refonly-elsewhere"));
    let apache_source = ClusterRef::Apache("refonly-self-managed-source".to_string());
    let apache_target = ClusterRef::Apache("refonly-self-managed-target".to_string());

    let rows = vec![
        Setup {
            case: "both-never-issued",
            source: here_source.clone(),
            target: here_target.clone(),
            tags: vec![],
        },
        Setup {
            case: "source-in-another-region",
            source: far,
            target: here_target.clone(),
            tags: vec![("owner", "platform")],
        },
        Setup {
            case: "source-in-another-account",
            source: other_account,
            target: here_target.clone(),
            tags: vec![("owner", "platform"), ("tier", "test")],
        },
        // The source is a self-managed Apache Kafka cluster, which this service
        // resolves nothing about — so the ONE Amazon MSK reference is the target,
        // and that is the one the replicator's state reports.
        Setup {
            case: "apache-source-msk-target",
            source: apache_source.clone(),
            target: here_target,
            tags: vec![],
        },
        // The control: a setup with no Amazon MSK cluster in it has nothing to
        // resolve, so the clock owes it RUNNING.
        Setup {
            case: "both-self-managed",
            source: apache_source,
            target: apache_target,
            tags: vec![("owner", "platform")],
        },
    ];

    let mut minted: Vec<(&str, String, Option<String>)> = Vec::new();
    for Setup {
        case,
        source,
        target,
        tags,
    } in &rows
    {
        let name = format!("refonly-{case}");
        let arn = create_ok(
            &emulator,
            case,
            &name,
            create_body(&name, source, target, tags),
        )
        .await;

        // The replicator is REGISTERED: the listing finds it by name, which is the
        // registry's own answer rather than the create's.
        assert_eq!(
            listed_names(&emulator, &name).await,
            vec![name.clone()],
            "case {case}: the admitted replicator must be listed under its name"
        );

        // The span is never zero: a caller reading straight after the create sees
        // the transient state, and a state that explains itself carries no
        // `StateInfo`.
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
            "case {case}: a CREATING replicator reports no stateInfo, got {}",
            immediate.at("/stateInfo")
        );
        // The subnets the create named are stored as submitted: they are Amazon
        // EC2's resources, and this emulator has no EC2 mounted to resolve them
        // against, so nothing is known about them and nothing is refused on it.
        assert_eq!(
            immediate.at("/kafkaClusters/0/vpcConfig/subnetIds"),
            &json!(SUBNETS),
            "case {case}: the client subnets must be reported as submitted, got {}",
            immediate.at("/kafkaClusters/0/vpcConfig/subnetIds")
        );

        // The first entry of the cluster list that names an Amazon MSK cluster this
        // registry holds no record of is the reference the replicator reports.
        let failing = [source, target]
            .iter()
            .filter_map(|reference| reference.msk_arn())
            .map(str::to_string)
            .next();
        minted.push((case, arn, failing));
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

        // A replicator the caller was handed is one the caller can delete, and
        // afterwards the ARN resolves to nothing.
        let deleted = delete(&emulator, arn).await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (200, ""),
            "case {case}: a settled replicator must be deletable, got {} {}",
            deleted.status,
            deleted.body
        );
        assert_eq!(
            deleted.at("/replicatorState").as_str(),
            Some("DELETING"),
            "case {case}: a deleted replicator is reported DELETING, got {}",
            deleted.body
        );
        assert_eq!(
            describe(&emulator, arn).await.status,
            404,
            "case {case}: a deleted replicator must read as missing"
        );
    }
}

/// RULE: a replicator the service admitted is a taggable resource like any other,
/// whatever its state: `ListTagsForResource` reports the tag set the create stored,
/// `TagResource` merges the pairs it carries into it, a key beginning with the
/// prefix AWS reserves is refused with HTTP 400 `BadRequestException` carrying the
/// service's fixed sentence and writes nothing, and `UntagResource` removes exactly
/// the keys it names.
#[tokio::test]
async fn a_replicator_is_taggable_whatever_state_it_settled_in() {
    let emulator = Emulator::new();
    let source = ClusterRef::Msk(ghost_cluster_arn(REGION, ACCOUNT, "tagging-source"));
    let target = ClusterRef::Msk(ghost_cluster_arn(REGION, ACCOUNT, "tagging-target"));

    // The rows differ along the state the replicator is in when the tagging calls
    // arrive — the transient `CREATING` and the terminal `FAILED` its unresolved
    // clusters settle it into — and along the tag set the create stored.
    let created_tags: Vec<(&str, &str)> = vec![("owner", "platform")];
    let rows = vec![
        Tagged {
            case: "creating",
            settle: false,
            stored: vec![],
        },
        Tagged {
            case: "settled-failed",
            settle: true,
            stored: created_tags.clone(),
        },
    ];

    for Tagged {
        case,
        settle,
        stored,
    } in &rows
    {
        let name = format!("tagging-{case}");
        let arn = create_ok(
            &emulator,
            case,
            &name,
            create_body(&name, &source, &target, stored),
        )
        .await;
        if *settle {
            sleep_past_settling();
            // The terminal state its unresolved clusters owe it, so the rows below
            // really do span the two states a tagging call can arrive in.
            assert_eq!(
                describe(&emulator, &arn).await.at("/replicatorState").as_str(),
                Some("FAILED"),
                "case {case}: the replicator must have settled FAILED before it is tagged"
            );
        }

        // What the create stored is what the reader reports — an empty map where
        // the create carried no tags, never an absent member.
        assert_eq!(
            list_tags(&emulator, &arn).await.at("/tags"),
            &tag_map(stored),
            "case {case}: expected the create's tag set {:?}",
            tag_map(stored)
        );

        // The positive control: a tag written after the create round-trips through
        // the reader, beside the ones the create stored.
        let added = tag(&emulator, &arn, &[("positive-control", "ok")]).await;
        assert_eq!(
            added.status, 204,
            "case {case}: expected TagResource to answer 204, got {} {}",
            added.status, added.body
        );
        let mut union = stored.clone();
        union.push(("positive-control", "ok"));
        assert_eq!(
            list_tags(&emulator, &arn).await.at("/tags"),
            &tag_map(&union),
            "case {case}: expected the merged tag set {:?}",
            tag_map(&union)
        );

        // A key carrying the prefix AWS reserves is refused, in every spelling of
        // it, and the refusal writes nothing: the tag set is the one above.
        for reserved in ["aws:reserved", "AWS:Reserved", "aWs:managed", "aws:"] {
            let refused = tag(&emulator, &arn, &[(reserved, "nope")]).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, "BadRequestException"),
                "case {case} ({reserved:?}): expected 400 BadRequestException, got {} {}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                RESERVED_TAG_KEY,
                "case {case} ({reserved:?}): expected the whole fixed sentence, got {:?}",
                refused.error_message()
            );
            assert_eq!(
                list_tags(&emulator, &arn).await.at("/tags"),
                &tag_map(&union),
                "case {case} ({reserved:?}): a refused tag write must disturb nothing"
            );
        }

        // `UntagResource` removes exactly the keys it names, and a key the resource
        // does not carry is not an error.
        let removed = untag(&emulator, &arn, &["positive-control", "never-written"]).await;
        assert_eq!(
            removed.status, 204,
            "case {case}: expected UntagResource to answer 204, got {} {}",
            removed.status, removed.body
        );
        assert_eq!(
            list_tags(&emulator, &arn).await.at("/tags"),
            &tag_map(stored),
            "case {case}: expected the create's tag set back, got {}",
            list_tags(&emulator, &arn).await.at("/tags")
        );
    }
}

/// RULE: a replicator name identifies a replicator within an account and region, so
/// the refusal of a name already in use is a function of the NAME alone — every
/// repeat answers the identical HTTP 409 `ConflictException` with the service's one
/// fixed sentence, and the registry still holds exactly one replicator under that
/// name.
#[tokio::test]
async fn a_repeated_name_is_refused_identically_however_often_it_is_tried() {
    let emulator = Emulator::new();
    let source = ClusterRef::Msk(ghost_cluster_arn(REGION, ACCOUNT, "conflict-source"));
    let target = ClusterRef::Msk(ghost_cluster_arn(REGION, ACCOUNT, "conflict-target"));
    let other_source = ClusterRef::Msk(ghost_cluster_arn("eu-west-1", ACCOUNT, "conflict-other"));
    let apache = ClusterRef::Apache("conflict-self-managed".to_string());

    // Rows span the name forms the model's `^[0-9A-Za-z][0-9A-Za-z-]{0,}$` pattern
    // and its 1..128 length admit: one character, hyphenated, mixed case with
    // digits, and the 128-character ceiling.
    let names = [
        "Z".to_string(),
        "conflict-with-hyphens".to_string(),
        "Conflict9Name".to_string(),
        format!("c{}", "9".repeat(127)),
    ];

    for name in &names {
        let arn = create_ok(
            &emulator,
            name,
            name,
            create_body(name, &source, &target, &[]),
        )
        .await;

        // The retries differ in EVERYTHING but the name — the clusters they name,
        // the kind of cluster, the tags they carry — so what is refused is the name
        // alone.
        let retries: Vec<(&str, Value)> = vec![
            ("the same request again", create_body(name, &source, &target, &[])),
            (
                "another setup under the same name",
                create_body(name, &other_source, &target, &[("attempt", "two")]),
            ),
            (
                "self-managed clusters under the same name",
                create_body(name, &apache, &target, &[]),
            ),
        ];
        let mut answers: Vec<Value> = Vec::new();
        for (retry_case, body) in retries {
            let case = format!("{name}: {retry_case}");
            let conflict = create(&emulator, body).await;
            assert_name_already_exists(&conflict, &case, "replicatorName", name, &arn);
            answers.push(conflict.body);
        }
        // IDEMPOTENT: the refusal does not change with the attempt.
        for (index, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer, &answers[0],
                "case {name} (retry {index}): every repeat must answer identically, got {answer} \
                 after {}",
                answers[0]
            );
        }

        // And the name still resolves to exactly ONE replicator: the first.
        assert_eq!(
            listed_names(&emulator, name).await,
            vec![name.clone()],
            "case {name}: the name must hold exactly one replicator"
        );
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {name}: the first replicator must survive every refused create, got {} {}",
            described.status, described.body
        );
    }
}

/// RULE: a create this service refuses from the request alone registers NOTHING, so
/// the name it asked for is reported by no listing and is free for the next create —
/// whichever of the synchronous admission rules refused it.
#[tokio::test]
async fn a_synchronously_refused_create_leaves_its_name_free() {
    let emulator = Emulator::new();
    let one = ghost_cluster_arn(REGION, ACCOUNT, "refused-one");
    let two = ghost_cluster_arn(REGION, ACCOUNT, "refused-two");
    let msk_one = ClusterRef::Msk(one.clone());
    let msk_two = ClusterRef::Msk(two.clone());

    // Rows: one per refusal class this handler answers from the request alone, each
    // under its own name. The expected sentence is given where the service states
    // one for that class; the rule itself is about what the refusal LEAVES BEHIND.
    let duplicate_in_the_list = {
        let mut body = create_body("refused-duplicate-list", &msk_one, &msk_two, &[]);
        // The same cluster ARN in both `KafkaClusters` entries, with the flow still
        // naming the two ends it was given.
        body["kafkaClusters"] = json!([msk_one.entry(&SUBNETS), msk_one.entry(&SUBNETS)]);
        body["replicationInfoList"][0]["targetKafkaClusterArn"] = json!(one);
        body
    };
    let one_cluster_only = {
        let mut body = create_body("refused-one-cluster", &msk_one, &msk_two, &[]);
        body["kafkaClusters"] = json!([msk_one.entry(&SUBNETS)]);
        body
    };
    let reserved_tag = create_body(
        "refused-reserved-tag",
        &msk_one,
        &msk_two,
        &[("aws:managed", "true")],
    );
    let rows: Vec<(&str, &str, Option<&str>, Value)> = vec![
        (
            "one cluster ARN twice in the list",
            "refused-duplicate-list",
            Some(DUPLICATE_CLUSTERS),
            duplicate_in_the_list,
        ),
        (
            "a cluster list that names one end only",
            "refused-one-cluster",
            None,
            one_cluster_only,
        ),
        (
            "a reserved tag key in the create's own tag map",
            "refused-reserved-tag",
            Some(RESERVED_TAG_KEY),
            reserved_tag,
        ),
    ];

    for (case, name, message, body) in rows {
        let refused = create(&emulator, body).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException, got {} {}",
            refused.status,
            refused.body
        );
        if let Some(expected) = message {
            assert_eq!(
                refused.error_message(),
                expected,
                "case {case}: expected the whole fixed sentence, got {:?}",
                refused.error_message()
            );
        }
        // Nothing was registered under the name the refused create asked for.
        assert_eq!(
            listed_names(&emulator, name).await,
            Vec::<String>::new(),
            "case {case}: a refused create must register nothing under {name:?}"
        );
        // So the name is free: a well-formed create under it is admitted rather
        // than conflicting with a half-written record.
        let arn = create_ok(
            &emulator,
            case,
            name,
            create_body(name, &msk_one, &msk_two, &[]),
        )
        .await;
        assert_eq!(
            listed_names(&emulator, name).await,
            vec![name.to_string()],
            "case {case}: the replicator created under the freed name must be listed ({arn})"
        );
    }
}
