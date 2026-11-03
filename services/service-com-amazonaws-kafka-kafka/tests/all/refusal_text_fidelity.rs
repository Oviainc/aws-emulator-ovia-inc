// SPDX-License-Identifier: CC0-1.0

//! The TEXT Amazon MSK refuses a request with, driven end to end over the same
//! router the emulator serves from.
//!
//! The RULE under test: a refusal of this service carries MSK's own wording for
//! the error class it belongs to — the part MSK keeps constant is the same
//! sentence for every request that meets that condition, and the only variable
//! parts are the ones MSK's own template has (the state a cluster is in, the
//! version a request named, the versions the service publishes). Nothing else of
//! the request reaches the text: not the ARN, not the region the endpoint
//! serves, not the offending key, value or entry.
//!
//! Each suite asserts that over a TABLE rather than one request, with rows
//! differing along what the rule generalises over — which reference, which
//! operation, which spelling, which position in a list — and with a control that
//! pins the other side of the rule wherever one exists. Every expected sentence
//! is WRITTEN OUT here rather than imported from the implementation, so an
//! assertion cannot agree with a constant that is wrong.
//!
//! The classes covered here are the ones no other suite pins. The taken-name
//! conflict is `kafka_harness::assert_name_already_exists` (every create suite),
//! the unsupported-`KafkaVersions` template is `configuration_lifecycle`, the
//! page-size and absent-member refusals are `refusal_wording`, and the
//! broker-level state refusals are `cluster_delete_state_guard`.

use serde_json::{json, Value};

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// What MSK answers an operation that models `NotFoundException` for a reference
/// it holds no resource for: fixed text end to end. The apostrophe is U+2019, as
/// the service writes it.
const RESOURCE_NOT_FOUND: &str = "The requested resource doesn\u{2019}t exist.";

/// What MSK answers a parameter it will not accept — the generic sentence its
/// `BadRequestException` carries, which names neither the member nor the value.
const PARAMETERS_NOT_VALID: &str = "One or more of the parameters are not valid.";

/// What MSK answers a tag key beginning with the prefix it reserves for itself.
/// The parenthetical is part of the sentence, and the offending key is not in it.
const RESERVED_TAG_KEY: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// What MSK answers a `SecretArnList` entry that is not a Secrets Manager secret
/// ARN: fixed text, with the offending entry not echoed back.
const SECRET_ARN_INVALID: &str = "The provided secret ARN is invalid.";

/// What MSK answers a replicator that names one Kafka cluster twice.
const DUPLICATE_CLUSTERS: &str =
    "Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs \
     are unique.";

/// The IAM role a `CreateReplicator` row hands the service: a role of the
/// caller's own account, so the pass-role decision admits it and the rule under
/// test is what answers.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// A well-formed MSK cluster ARN of the caller's account that was never issued.
/// `region` varies, because a regional endpoint holds only its own region's
/// clusters, and an ARN from another region resolves to nothing here too.
fn cluster_arn(region: &str, name: &str) -> String {
    format!("arn:aws:kafka:{region}:{ACCOUNT}:cluster/{name}/{GHOST_ID}")
}

/// One reader of a cluster, as a row addresses it: the URI under the cluster's
/// ARN, the status and code it refuses an unresolvable reference with, and that
/// refusal's whole sentence.
struct Reader {
    case: &'static str,
    method: &'static str,
    /// The request URI, built from the percent-encoded ARN.
    uri: fn(&str) -> String,
    body: Option<fn() -> Value>,
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// RULE: a cluster reference this endpoint resolves to nothing is refused with
/// the sentence of the error class the operation models — [`RESOURCE_NOT_FOUND`]
/// where it models `NotFoundException`, and the service's generic parameter
/// sentence [`PARAMETERS_NOT_VALID`] where it models none — and that sentence is
/// the WHOLE message: no part of it comes from the request, so the ARN, the
/// cluster name and the region the endpoint serves are all absent from it.
#[tokio::test]
async fn an_unresolvable_cluster_reference_is_refused_in_the_services_fixed_words() {
    let emulator = Emulator::new();

    // Rows: every operation of this crate that resolves a cluster by ARN and
    // refuses one it cannot resolve. The two classes are both represented, so
    // the rule is not a statement about one error shape.
    let readers = [
        Reader {
            case: "DescribeCluster",
            method: "GET",
            uri: |arn| format!("/v1/clusters/{}", pct(arn)),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: RESOURCE_NOT_FOUND,
        },
        Reader {
            case: "DescribeClusterV2",
            method: "GET",
            uri: |arn| format!("/api/v2/clusters/{}", pct(arn)),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: RESOURCE_NOT_FOUND,
        },
        Reader {
            case: "ListNodes",
            method: "GET",
            uri: |arn| format!("/v1/clusters/{}/nodes", pct(arn)),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: RESOURCE_NOT_FOUND,
        },
        Reader {
            // Models no `NotFoundException`, yet the service answers one for a
            // reference it cannot resolve — the same code, status and sentence
            // the readers that DO model it answer.
            case: "GetBootstrapBrokers",
            method: "GET",
            uri: |arn| format!("/v1/clusters/{}/bootstrap-brokers", pct(arn)),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: RESOURCE_NOT_FOUND,
        },
        Reader {
            case: "ListClientVpcConnections",
            method: "GET",
            uri: |arn| format!("/v1/clusters/{}/client-vpc-connections", pct(arn)),
            body: None,
            status: 400,
            code: "BadRequestException",
            message: PARAMETERS_NOT_VALID,
        },
        Reader {
            case: "PutClusterPolicy",
            method: "PUT",
            uri: |arn| format!("/v1/clusters/{}/policy", pct(arn)),
            body: Some(|| json!({ "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}" })),
            status: 400,
            code: "BadRequestException",
            message: PARAMETERS_NOT_VALID,
        },
    ];

    // Reference rows: a name no create ever asked for, a name that LOOKS like
    // one this suite stores, one whose region segment is another region's, and
    // one whose name carries the kind of text a composed message would echo.
    let stored = emulator
        .create_ok(&ClusterRequest::named("fidelity-stored"))
        .await;
    let references = [
        ("never-issued", cluster_arn(REGION, "fidelity-absent")),
        ("another-region", cluster_arn("eu-central-1", "fidelity-stored")),
        ("same-name-other-id", {
            // The stored cluster's own name with an id it does not have, so the
            // reference is close to a live one without being it.
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/fidelity-stored/{GHOST_ID}")
        }),
        (
            "name-worth-echoing",
            cluster_arn(REGION, "a-name-a-message-would-quote"),
        ),
    ];

    for reader in &readers {
        for (reference_case, arn) in &references {
            let case = format!("{} {reference_case}", reader.case);
            let reply = emulator
                .call(
                    reader.method,
                    &(reader.uri)(arn),
                    reader.body.map(|build| build()),
                )
                .await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (reader.status, reader.code),
                "case {case}: expected {} {}, got {} {}",
                reader.status,
                reader.code,
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                reader.message,
                "case {case}: expected the whole fixed sentence, got {:?}",
                reply.error_message()
            );
            for leaked in [arn.as_str(), REGION, "eu-central-1", "fidelity"] {
                assert!(
                    !reply.error_message().contains(leaked),
                    "case {case}: the refusal must not state {leaked:?}, got {:?}",
                    reply.error_message()
                );
            }
        }
    }

    // The control: the reference that DOES resolve is still served, so the rows
    // above are about the reference and not about the reader being broken.
    let described = emulator.describe_v2(&stored).await;
    assert_eq!(
        described.status, 200,
        "a stored cluster must still be reported, got {} {}",
        described.status, described.body
    );
}

/// RULE: a tag key beginning with the prefix AWS reserves is refused with HTTP
/// 400 `BadRequestException` carrying [`RESERVED_TAG_KEY`] as its whole message,
/// and the prefix is matched WITHOUT regard to case — so no spelling of it is a
/// key a caller can set, and the offending key is not named in the refusal.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_in_the_services_fixed_words_whatever_its_case() {
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("fidelity-tagging"))
        .await;

    // Rows: the prefix in every case pattern, and a key that is only the prefix.
    let refused = [
        ("lower case", "aws:reserved"),
        ("upper case", "AWS:reserved"),
        ("title case", "Aws:Reserved"),
        ("mixed case", "aWs:reserved"),
        ("the prefix alone", "aws:"),
    ];
    for (case, key) in refused {
        let reply = emulator
            .call(
                "POST",
                &format!("/v1/tags/{}", pct(&cluster)),
                Some(json!({ "tags": { key: "value" } })),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case} ({key:?}): expected 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            RESERVED_TAG_KEY,
            "case {case} ({key:?}): expected the whole fixed sentence, got {:?}",
            reply.error_message()
        );
        // The sentence names the PREFIX, so only a key that carries more than
        // the prefix can show that nothing of the key reached the text.
        assert!(
            RESERVED_TAG_KEY.contains(key) || !reply.error_message().contains(key),
            "case {case}: the refusal must not name the key {key:?}, got {:?}",
            reply.error_message()
        );
    }

    // Controls: a key that merely STARTS with the three letters, and one that
    // carries the prefix somewhere other than the start, are not reserved — so
    // the rule is about the prefix rather than about the letters.
    let accepted = [
        ("not the prefix", "awsome"),
        ("the prefix in the middle", "team:aws:owner"),
        ("the letters without the colon", "aws"),
    ];
    for (case, key) in accepted {
        let reply = emulator
            .call(
                "POST",
                &format!("/v1/tags/{}", pct(&cluster)),
                Some(json!({ "tags": { key: "value" } })),
            )
            .await;
        assert_eq!(
            reply.status, 204,
            "case {case} ({key:?}): expected the tag write to be accepted, got {} {}",
            reply.status, reply.body
        );
    }

    // The accepted keys are the ones a later read reports, and no refused key is.
    let listed = emulator
        .call("GET", &format!("/v1/tags/{}", pct(&cluster)), None)
        .await;
    let tags = listed.at("/tags").clone();
    for (_, key) in accepted {
        assert!(
            tags.get(key).is_some(),
            "expected the accepted key {key:?} to be stored, got {tags}"
        );
    }
    for (_, key) in refused {
        assert!(
            tags.get(key).is_none(),
            "a refused key must not be stored, got {tags}"
        );
    }
}

/// RULE: a `SecretArnList` entry that is not an AWS Secrets Manager secret ARN
/// is refused with HTTP 400 `BadRequestException` carrying [`SECRET_ARN_INVALID`]
/// as its whole message — the same sentence for both SCRAM batch operations,
/// whatever the entry was and wherever in the list it sat, and with the entry
/// itself not echoed back.
#[tokio::test]
async fn a_secret_arn_that_names_no_secret_is_refused_in_the_services_fixed_words() {
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("fidelity-scram"))
        .await;
    // A secret ARN of the right shape, so a row can place the offending entry
    // beside a usable one.
    let usable = format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:AmazonMSK_one-1a2b3c");

    // Rows: what makes the entry unusable — not an ARN at all, an ARN of another
    // vendor, an ARN of this very service, a secret ARN naming no secret, and an
    // empty entry — crossed with its position in the list.
    let unusable = [
        ("not an arn", "not-a-secrets-manager-arn".to_string()),
        (
            "another vendor",
            format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/1a2b3c4d"),
        ),
        ("this service's own arn", cluster.clone()),
        (
            "a secret arn naming no secret",
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:"),
        ),
        ("an empty entry", String::new()),
    ];
    let operations = [
        ("BatchAssociateScramSecret", "POST"),
        ("BatchDisassociateScramSecret", "PATCH"),
    ];

    for (operation, method) in operations {
        for (entry_case, entry) in &unusable {
            for (position_case, list) in [
                ("alone", vec![entry.clone()]),
                ("first of two", vec![entry.clone(), usable.clone()]),
                ("last of two", vec![usable.clone(), entry.clone()]),
            ] {
                let case = format!("{operation} {entry_case} {position_case}");
                let reply = emulator
                    .call(
                        method,
                        &format!("/v1/clusters/{}/scram-secrets", pct(&cluster)),
                        Some(json!({ "secretArnList": list })),
                    )
                    .await;
                assert_eq!(
                    (reply.status, reply.error_code()),
                    (400, "BadRequestException"),
                    "case {case}: expected 400 BadRequestException, got {} {}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.error_message(),
                    SECRET_ARN_INVALID,
                    "case {case}: expected the whole fixed sentence, got {:?}",
                    reply.error_message()
                );
                if !entry.is_empty() {
                    assert!(
                        !reply.error_message().contains(entry.as_str()),
                        "case {case}: the refusal must not echo {entry:?}, got {:?}",
                        reply.error_message()
                    );
                }
            }
        }
    }
}

/// One `KafkaClusters` entry of a `CreateReplicator` row, named by MSK cluster
/// ARN or by Apache Kafka cluster id — both kinds, because the rule is about the
/// cluster a request names and not about how it names it.
#[derive(Clone, Copy)]
enum Named {
    Msk(&'static str),
    Apache(&'static str),
}

impl Named {
    /// This entry as the request carries it, with the two client-VPC subnets MSK
    /// places a replicator's interfaces across — so a row the rule admits is
    /// otherwise well formed.
    fn to_json(self) -> Value {
        let mut entry = match self {
            Named::Msk(name) => json!({
                "amazonMskCluster": { "mskClusterArn": cluster_arn(REGION, name) },
            }),
            Named::Apache(id) => json!({
                "apacheKafkaCluster": {
                    "apacheKafkaClusterId": id,
                    "bootstrapBrokerString": format!("b-1.{id}.example.internal:9092"),
                },
            }),
        };
        entry["vpcConfig"] = json!({
            "subnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
        });
        entry
    }

    /// The `ReplicationInfo` member naming this cluster at `end`, and its value.
    fn flow_member(self, end: &str) -> (String, Value) {
        match self {
            Named::Msk(name) => (
                format!("{end}KafkaClusterArn"),
                json!(cluster_arn(REGION, name)),
            ),
            Named::Apache(id) => (format!("{end}KafkaClusterId"), json!(id)),
        }
    }
}

/// A `CreateReplicator` body naming `clusters` and one flow from `source` to
/// `target`.
fn replicator_body(name: &str, clusters: &[Named], source: Named, target: Named) -> Value {
    let mut flow = json!({
        "targetCompressionType": "NONE",
        "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
        "topicReplication": { "topicsToReplicate": [".*"] },
    });
    for (end, entry) in [("source", source), ("target", target)] {
        let (member, value) = entry.flow_member(end);
        flow[member] = value;
    }
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": clusters.iter().map(|entry| entry.to_json()).collect::<Vec<_>>(),
        "replicationInfoList": [flow],
    })
}

async fn create_replicator(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .call("POST", "/replication/v1/replicators", Some(body))
        .await
}

/// RULE: the Kafka clusters a replicator names are a SET — it replicates between
/// distinct clusters — so a request naming one cluster twice is refused with HTTP
/// 400 `BadRequestException` carrying [`DUPLICATE_CLUSTERS`] as its whole
/// message, registering nothing, however the repeated cluster is identified and
/// whether the repetition is in the cluster list or in a flow's two ends.
#[tokio::test]
async fn a_replicator_naming_one_kafka_cluster_twice_is_refused_in_the_services_fixed_words() {
    let emulator = Emulator::new();

    let msk_one = Named::Msk("fidelity-rep-one");
    let msk_two = Named::Msk("fidelity-rep-two");
    let apache_one = Named::Apache("fidelity-apache-one");
    let apache_two = Named::Apache("fidelity-apache-two");

    // Rows: the same MSK cluster in both list entries, the same Apache cluster in
    // both, and a list of two DISTINCT clusters whose flow names one of them at
    // both ends — the same condition reached from the other member.
    let refused: Vec<(&str, Value)> = vec![
        (
            "the same msk cluster twice in the list",
            replicator_body("fidelity-dup-msk", &[msk_one, msk_one], msk_one, msk_one),
        ),
        (
            "the same apache cluster twice in the list",
            replicator_body(
                "fidelity-dup-apache",
                &[apache_one, apache_one],
                apache_one,
                apache_one,
            ),
        ),
        (
            "a flow whose two ends are one msk cluster",
            replicator_body("fidelity-dup-flow", &[msk_one, msk_two], msk_one, msk_one),
        ),
        (
            "a flow whose two ends are one apache cluster",
            replicator_body(
                "fidelity-dup-flow-apache",
                &[apache_one, apache_two],
                apache_two,
                apache_two,
            ),
        ),
    ];

    for (case, body) in refused {
        let name = body["replicatorName"].as_str().unwrap_or_default().to_string();
        let reply = create_replicator(&emulator, body).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            DUPLICATE_CLUSTERS,
            "case {case}: expected the whole fixed sentence, got {:?}",
            reply.error_message()
        );
        // Nothing of the request is in the text, and nothing was registered.
        assert!(
            !reply.error_message().contains("fidelity"),
            "case {case}: the refusal must not state anything of the request, got {:?}",
            reply.error_message()
        );
        let listed = emulator
            .call("GET", "/replication/v1/replicators", None)
            .await;
        assert!(
            !listed.body.to_string().contains(name.as_str()),
            "case {case}: a refused create must register nothing, got {}",
            listed.body
        );
    }

    // The control: two distinct clusters, each named once, are admitted — so the
    // rows above are refused for the repetition and nothing else.
    let accepted = create_replicator(
        &emulator,
        replicator_body("fidelity-distinct", &[msk_one, msk_two], msk_one, msk_two),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "two distinct clusters must be admitted, got {} {}",
        accepted.status, accepted.body
    );
}

/// RULE: the reader of a cluster's bootstrap endpoints refuses a cluster that has
/// no brokers yet with HTTP 400 `BadRequestException` and MSK's sentence for that
/// state — fixed apart from the one variable part MSK's own template has, the
/// state the cluster is in, read from the cluster rather than from the request.
#[tokio::test]
async fn the_bootstrap_endpoint_reader_refuses_a_cluster_without_brokers_in_the_services_words() {
    let emulator = Emulator::new();

    // Rows: cluster shapes whose provisioning the refusal must not depend on.
    let rows = [
        ("three brokers", ClusterRequest::named("fidelity-boot-three")),
        ("one broker", {
            ClusterRequest {
                brokers: 1,
                ..ClusterRequest::named("fidelity-boot-one")
            }
        }),
        ("six brokers", {
            ClusterRequest {
                brokers: 6,
                ..ClusterRequest::named("fidelity-boot-six")
            }
        }),
    ];

    for (case, request) in rows {
        let arn = emulator.create_ok(&request).await;
        let reply = emulator.bootstrap_brokers(&arn).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException while the cluster is provisioning, \
             got {} {}",
            reply.status,
            reply.body
        );
        // The state is the ONE variable part: the sentence is otherwise fixed,
        // and the state is the one the cluster reports.
        let state = emulator
            .describe_v2(&arn)
            .await
            .at("/clusterInfo/state")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected =
            format!("You can't get bootstrap broker nodes for a cluster in {state} state.");
        assert_eq!(
            reply.error_message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.error_message()
        );
        assert!(
            !reply.error_message().contains(arn.as_str()),
            "case {case}: the refusal must not echo the ARN, got {:?}",
            reply.error_message()
        );
    }
}
