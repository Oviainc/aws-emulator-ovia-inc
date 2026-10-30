// SPDX-License-Identifier: CC0-1.0

//! The COUNTS Amazon MSK admits in a `CreateReplicator` request, driven end to end
//! over the same router the emulator serves from.
//!
//! The RULE under test: `CreateReplicator` decides the cardinality of each list the
//! request sizes from the request alone, before any replicator is registered — the
//! `KafkaClusters` list names exactly the one source and the one target a replicator
//! replicates between, and EVERY one of those entries names a client VPC
//! configuration spanning the two or three Availability Zones MSK places the
//! replicator's network interfaces across (`KafkaClusterClientVpcConfig$SubnetIds`,
//! one subnet per Availability Zone). A count outside either is refused HTTP 400
//! `BadRequestException` carrying the member in its wire spelling (`kafkaClusters`,
//! `subnetIds`) and that refusal's whole fixed sentence, and nothing is minted: the
//! name the request asked for stays free and no replicator appears in a listing.
//!
//! Each suite asserts that over a TABLE rather than one request. The rows differ
//! along what the rule generalises over: the submitted count — each bound, one past
//! each bound, and well past it — which entry of `KafkaClusters` carries the
//! offending list, whether an entry names a client VPC configuration at all, and how
//! each Kafka cluster is identified (an Amazon MSK cluster ARN or an Apache Kafka
//! cluster id). The controls pin the other side: a request whose counts are inside
//! both ranges is admitted and reads back through `DescribeReplicator` carrying the
//! subnets it submitted.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the account and region
//! come from [`crate::kafka_harness`], the crate's shared harness.

use serde_json::{json, Value};

use crate::kafka_harness::{pct, Emulator, Reply, ACCOUNT, REGION};

/// The IAM role every request below hands the service. It is a role of the caller's
/// own account, so the pass-role decision admits it and the cardinality gates are
/// what answer.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds — `CreateReplicator` is asynchronous, so the
/// clusters it names need not exist for the request to be admitted.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// The HTTP status and error code MSK refuses an inadmissible request with, spelled
/// out here rather than read from the service so a drift in the service fails this
/// suite.
const BAD_REQUEST_STATUS: u16 = 400;
/// The wire error code those refusals carry.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The whole sentence MSK answers a `KafkaClusters` list that does not name the
/// source and the target a replicator spans: fixed text end to end, carrying neither
/// the submitted count nor the admitted one.
const CLUSTER_COUNT_MESSAGE: &str = "Too many or too few kafka clusters specified.";

/// The whole sentence MSK answers a client VPC configuration whose subnet list is
/// outside the admitted range: likewise fixed text end to end, carrying neither the
/// count, the subnet ids, nor the bounds.
const SUBNET_COUNT_MESSAGE: &str = "Too many or too few subnet ids specified.";

/// The subnet counts MSK admits in one client VPC configuration: one subnet per
/// Availability Zone, across two or three Availability Zones. Spelled out as the
/// bounds this suite asserts, so the rows below are a statement of the contract
/// rather than a reading of the service's own constants.
const SUBNET_BOUNDS: (usize, usize) = (2, 3);

/// `subnet-`-prefixed ids, `count` of them, each a distinct EC2 subnet id — the
/// account's own subnets are not what this rule is about, so the ids only have to be
/// well formed and distinct.
fn subnet_ids(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("subnet-{:017x}", index + 1))
        .collect()
}

/// How one `KafkaClusters` entry names its client VPC configuration.
#[derive(Debug, Clone, Copy)]
enum Placement {
    /// `vpcConfig.subnetIds` carries this many ids.
    Subnets(usize),
    /// The entry carries no `vpcConfig` at all, so it names no subnet.
    NoVpcConfig,
}

impl Placement {
    /// The number of subnets this placement names, which is what the rule decides
    /// on.
    fn count(self) -> usize {
        match self {
            Placement::Subnets(count) => count,
            Placement::NoVpcConfig => 0,
        }
    }

    /// Whether MSK admits it.
    fn admitted(self) -> bool {
        (SUBNET_BOUNDS.0..=SUBNET_BOUNDS.1).contains(&self.count())
    }
}

/// How one `KafkaClusters` entry identifies its Kafka cluster — the other thing the
/// rule has to hold over, since both kinds carry a client VPC configuration.
#[derive(Debug, Clone, Copy)]
enum Identity {
    /// An Amazon MSK cluster, named by ARN.
    Msk,
    /// A self-managed Apache Kafka cluster, named by the id the caller gave it.
    Apache,
}

/// One `KafkaClusters` entry: how it identifies its cluster and how it is placed.
#[derive(Debug, Clone, Copy)]
struct Entry {
    identity: Identity,
    placement: Placement,
}

impl Entry {
    fn msk(placement: Placement) -> Self {
        Self {
            identity: Identity::Msk,
            placement,
        }
    }

    fn apache(placement: Placement) -> Self {
        Self {
            identity: Identity::Apache,
            placement,
        }
    }

    /// The ARN or id this entry names its cluster by, derived from the replicator's
    /// name and the entry's position so two entries of one request never collide.
    fn reference(self, name: &str, index: usize) -> String {
        match self.identity {
            Identity::Msk => {
                format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{name}-c{index}/{GHOST_ID}")
            }
            Identity::Apache => format!("{name}-apache-{index}"),
        }
    }

    /// This entry as the request carries it.
    fn to_json(self, name: &str, index: usize) -> Value {
        let reference = self.reference(name, index);
        let mut entry = match self.identity {
            Identity::Msk => json!({ "amazonMskCluster": { "mskClusterArn": reference } }),
            Identity::Apache => json!({
                "apacheKafkaCluster": {
                    "apacheKafkaClusterId": reference,
                    "bootstrapBrokerString": format!("b-1.{reference}.example.internal:9092"),
                },
            }),
        };
        if let Placement::Subnets(count) = self.placement {
            entry["vpcConfig"] = json!({ "subnetIds": subnet_ids(count) });
        }
        entry
    }

    /// The member of a `ReplicationInfo` that names this cluster at `end`, and its
    /// value.
    fn flow_member(self, end: &str, name: &str, index: usize) -> (String, Value) {
        let reference = self.reference(name, index);
        match self.identity {
            Identity::Msk => (format!("{end}KafkaClusterArn"), json!(reference)),
            Identity::Apache => (format!("{end}KafkaClusterId"), json!(reference)),
        }
    }
}

/// Where in `KafkaClusters` the row's placement sits: a name for the case, and how
/// it builds the two entries a replicator spans around that placement.
struct Position(&'static str, fn(Placement) -> Vec<Entry>);

/// A whole `CreateReplicator` body for `entries`, with one replication running from
/// the first entry to the last — so a request whose counts are admitted is otherwise
/// well formed and is admitted as a whole.
fn create_body(name: &str, entries: &[Entry]) -> Value {
    let mut flow = json!({
        "targetCompressionType": "NONE",
        "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
        "topicReplication": { "topicsToReplicate": [".*"] },
    });
    if let (Some(source), Some(target)) = (entries.first(), entries.last()) {
        let last = entries.len().saturating_sub(1);
        for (end, entry, index) in [("source", source, 0), ("target", target, last)] {
            let (member, value) = entry.flow_member(end, name, index);
            flow[member] = value;
        }
    }
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": entries
            .iter()
            .enumerate()
            .map(|(index, entry)| entry.to_json(name, index))
            .collect::<Vec<_>>(),
        "replicationInfoList": [flow],
    })
}

async fn create(emulator: &Emulator, name: &str, entries: &[Entry]) -> Reply {
    emulator
        .call(
            "POST",
            "/replication/v1/replicators",
            Some(create_body(name, entries)),
        )
        .await
}

/// `ListReplicators`, which the model binds to `GET /replication/v1/replicators`.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    let reply = emulator
        .call("GET", "/replication/v1/replicators", None)
        .await;
    assert_eq!(
        reply.status, 200,
        "ListReplicators must answer 200, got {} {}",
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

/// `DescribeReplicator`, whose `ReplicatorArn` is an `@httpLabel`.
async fn describe(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(arn)),
            None,
        )
        .await
}

/// Assert `reply` is MSK's refusal for `member` carrying `message`, and that the
/// replicator the request asked for was not registered.
async fn assert_refused(
    emulator: &Emulator,
    case: &str,
    name: &str,
    reply: &Reply,
    member: &str,
    message: &str,
) {
    assert_eq!(
        reply.status, BAD_REQUEST_STATUS,
        "case {case:?}: expected HTTP {BAD_REQUEST_STATUS}, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        BAD_REQUEST_CODE,
        "case {case:?}: expected {BAD_REQUEST_CODE}, got {:?}",
        reply.error_code()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(member),
        "case {case:?}: expected invalidParameter {member:?}, got {}",
        reply.at("/invalidParameter")
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case:?}: expected the fixed refusal {message:?}, got {:?}",
        reply.error_message()
    );
    assert!(
        reply.at("/replicatorArn").is_null(),
        "case {case:?}: a refused create mints no ARN, got {}",
        reply.body
    );
    let listed = listed_names(emulator).await;
    assert!(
        !listed.contains(&name.to_string()),
        "case {case:?}: a refused create must register no replicator, but {name:?} is in {listed:?}"
    );
}

/// RULE: every `KafkaClusters` entry names a client VPC configuration whose subnet
/// count is one of the two or three MSK admits, so a create carrying any other count
/// — in any entry — is refused with HTTP 400 `BadRequestException`, `invalidParameter`
/// `subnetIds` and MSK's fixed sentence for that count, and registers nothing.
#[tokio::test]
async fn a_subnet_list_outside_the_admitted_count_is_refused() {
    let emulator = Emulator::new();

    // Rows span the counts around BOTH published bounds — one below the minimum
    // (and the two ways to name none at all), each bound itself, one past the
    // maximum, and well past it — crossed with WHICH entry carries the offending
    // list and how each entry identifies its cluster.
    let counts = [
        ("no-vpc-config", Placement::NoVpcConfig),
        ("none", Placement::Subnets(0)),
        ("one", Placement::Subnets(1)),
        ("two", Placement::Subnets(SUBNET_BOUNDS.0)),
        ("three", Placement::Subnets(SUBNET_BOUNDS.1)),
        ("four", Placement::Subnets(4)),
        ("eight", Placement::Subnets(8)),
    ];
    // Where the row's placement sits, against an entry that is itself admitted.
    let positions = [
        Position("first-entry", |row| {
            vec![Entry::msk(row), Entry::msk(Placement::Subnets(2))]
        }),
        Position("second-entry", |row| {
            vec![Entry::msk(Placement::Subnets(2)), Entry::msk(row)]
        }),
        Position("both-entries", |row| {
            vec![Entry::msk(row), Entry::apache(row)]
        }),
        Position("apache-second-entry", |row| {
            vec![Entry::msk(Placement::Subnets(3)), Entry::apache(row)]
        }),
    ];

    for (count_case, placement) in counts {
        for (index, Position(position, build)) in positions.iter().enumerate() {
            let entries = build(placement);
            let case = format!("{count_case} in the {position}");
            let name = format!("subnets{}{index}", count_case.replace('-', ""));
            let reply = create(&emulator, &name, &entries).await;
            if placement.admitted() {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: {} subnets per entry is admitted, got {} {}",
                    placement.count(),
                    reply.status,
                    reply.body
                );
                continue;
            }
            assert_refused(
                &emulator,
                &case,
                &name,
                &reply,
                "subnetIds",
                SUBNET_COUNT_MESSAGE,
            )
            .await;
        }
    }

    // The control at the other end: with every entry inside the range, the same
    // requests are admitted whichever admitted count they carry, so the refusal
    // rests on the count and on nothing else about the request.
    for count in SUBNET_BOUNDS.0..=SUBNET_BOUNDS.1 {
        let name = format!("subnets-admitted-{count}");
        let reply = create(
            &emulator,
            &name,
            &[
                Entry::msk(Placement::Subnets(count)),
                Entry::apache(Placement::Subnets(count)),
            ],
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected the create to be admitted, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/replicatorState").as_str(),
            Some("CREATING"),
            "case {name:?}: an admitted create reports CREATING, got {}",
            reply.body
        );
        // And the admitted placement is what the replicator holds: the subnets it
        // was created with read back through an independent `DescribeReplicator`.
        let arn = reply
            .at("/replicatorArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: an admitted create mints an ARN"))
            .to_string();
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected DescribeReplicator to answer 200, got {} {}",
            described.status, described.body
        );
        for index in 0..2 {
            let pointer = format!("/kafkaClusters/{index}/vpcConfig/subnetIds");
            assert_eq!(
                described.at(&pointer),
                &json!(subnet_ids(count)),
                "case {name:?}: entry {index} must report the {count} subnets it was created \
                 with, got {}",
                described.at(&pointer)
            );
        }
    }
}

/// RULE: a replicator replicates between one source and one target, so
/// `KafkaClusters` names exactly two clusters and a create naming any other number is
/// refused with HTTP 400 `BadRequestException`, `invalidParameter` `kafkaClusters` and
/// MSK's fixed sentence for that count, registering nothing.
#[tokio::test]
async fn a_kafka_cluster_list_that_is_not_a_source_and_a_target_is_refused() {
    let emulator = Emulator::new();
    let placement = Placement::Subnets(SUBNET_BOUNDS.0);

    // Rows span the counts on both sides of the admitted one — none, one, one past
    // it, and well past it — with the entries identified both ways, so the answer
    // rests on the count alone.
    let rows: Vec<(&str, Vec<Entry>)> = vec![
        ("no-entries", Vec::new()),
        ("one-msk-entry", vec![Entry::msk(placement)]),
        ("one-apache-entry", vec![Entry::apache(placement)]),
        (
            "three-entries",
            vec![
                Entry::msk(placement),
                Entry::msk(placement),
                Entry::apache(placement),
            ],
        ),
        (
            "four-entries",
            vec![
                Entry::msk(placement),
                Entry::apache(placement),
                Entry::msk(placement),
                Entry::apache(placement),
            ],
        ),
        (
            "nine-entries",
            (0..9).map(|_| Entry::msk(placement)).collect(),
        ),
    ];

    for (case, entries) in &rows {
        let name = format!("clusters{}", case.replace('-', ""));
        let reply = create(&emulator, &name, entries).await;
        assert_refused(
            &emulator,
            case,
            &name,
            &reply,
            "kafkaClusters",
            CLUSTER_COUNT_MESSAGE,
        )
        .await;
    }

    // The control: exactly a source and a target is admitted, in both ways of
    // identifying them.
    for (case, entries) in [
        ("two-msk", vec![Entry::msk(placement), Entry::msk(placement)]),
        (
            "msk-and-apache",
            vec![Entry::msk(placement), Entry::apache(placement)],
        ),
    ] {
        let name = format!("clusters-admitted-{case}");
        let reply = create(&emulator, &name, &entries).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a source and a target is admitted, got {} {}",
            reply.status, reply.body
        );
        let listed = listed_names(&emulator).await;
        assert!(
            listed.contains(&name),
            "case {case:?}: an admitted create registers the replicator, but {name:?} is not in \
             {listed:?}"
        );
    }
}

/// RULE (the ordering half): the counts are decided before anything else about the
/// request is, and the `KafkaClusters` count before the subnets of its entries — so a
/// request that is wrong in several ways at once is answered about the count first,
/// and a request whose counts are admitted reaches the checks it always did.
#[tokio::test]
async fn the_cardinality_gates_answer_before_the_rest_of_the_request() {
    let emulator = Emulator::new();

    // One entry, which names no subnet MSK admits either: the cluster count is what
    // answers.
    let reply = create(
        &emulator,
        "ordering-one-entry",
        &[Entry::msk(Placement::Subnets(1))],
    )
    .await;
    assert_refused(
        &emulator,
        "one entry whose subnet list is also out of range",
        "ordering-one-entry",
        &reply,
        "kafkaClusters",
        CLUSTER_COUNT_MESSAGE,
    )
    .await;

    // Two entries, the second of which identifies no cluster AND names too few
    // subnets: the entry's identity is settled first, since it is what the entry
    // IS, and the subnet count of an identified entry is what the gate then
    // answers.
    let unidentified = json!({
        "replicatorName": "ordering-unidentified",
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": [
            Entry::msk(Placement::Subnets(2)).to_json("ordering-unidentified", 0),
            json!({ "vpcConfig": { "subnetIds": subnet_ids(1) } }),
        ],
        "replicationInfoList": [{
            "sourceKafkaClusterArn":
                Entry::msk(Placement::Subnets(2)).reference("ordering-unidentified", 0),
            "targetKafkaClusterId": "ordering-unidentified-apache-1",
            "targetCompressionType": "NONE",
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
            "topicReplication": { "topicsToReplicate": [".*"] },
        }],
    });
    let reply = emulator
        .call(
            "POST",
            "/replication/v1/replicators",
            Some(unidentified.clone()),
        )
        .await;
    assert_eq!(
        (reply.status, reply.at("/invalidParameter").as_str()),
        (BAD_REQUEST_STATUS, Some("KafkaClusters[1]")),
        "case an entry that identifies nothing: expected the entry to be blamed, got {} {}",
        reply.status,
        reply.body
    );

    // A request with an out-of-range subnet list and a name ALREADY TAKEN is
    // answered about the subnets: the cardinality gates run before the registry is
    // consulted, so the conflict the name would raise is never reached — and the
    // replicator already holding that name is left exactly as it was.
    let taken = "ordering-taken";
    let first = create(
        &emulator,
        taken,
        &[
            Entry::msk(Placement::Subnets(2)),
            Entry::msk(Placement::Subnets(2)),
        ],
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case the first create under a name: expected 200, got {} {}",
        first.status, first.body
    );
    let arn = first
        .at("/replicatorArn")
        .as_str()
        .expect("the first create mints an ARN")
        .to_string();
    let before = describe(&emulator, &arn).await.body;
    let again = create(
        &emulator,
        taken,
        &[
            Entry::msk(Placement::Subnets(4)),
            Entry::msk(Placement::Subnets(2)),
        ],
    )
    .await;
    assert_eq!(
        (
            again.status,
            again.at("/invalidParameter").as_str(),
            again.error_message()
        ),
        (BAD_REQUEST_STATUS, Some("subnetIds"), SUBNET_COUNT_MESSAGE),
        "case a taken name with an out-of-range subnet list: expected the subnet refusal, got {} {}",
        again.status,
        again.body
    );
    assert_eq!(
        describe(&emulator, &arn).await.body,
        before,
        "case a taken name with an out-of-range subnet list: the registered replicator must be \
         untouched, got {}",
        describe(&emulator, &arn).await.body
    );
}
