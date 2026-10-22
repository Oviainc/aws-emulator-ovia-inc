// SPDX-License-Identifier: CC0-1.0

//! `ListReplicators`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over (replicator count,
//! page size, filter prefix, region, cluster kind, endpoint-naming form, clock
//! position). Where the rule is an invariant — pagination reporting every
//! replicator exactly once, a read leaving the registry as a later caller must
//! see it — the input space is small enough to walk EXHAUSTIVELY, which is
//! strictly stronger than sampling it.
//!
//! The records are put in the registry through
//! [`ComAmazonawsKafkaKafkaState::register_replicator`], the one insertion path
//! `CreateReplicator` will use once it is implemented: that operation is still a
//! scaffold (its own open issue,
//! `issues/things-that-do-not-work/20261022T171242Z-kafka-createreplicator-stub.md`),
//! so seeding the store is how a reader is exercised over a populated registry
//! today. Everything the suites assert is read back OVER HTTP.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_kafka_kafka::ops::types::{
    AmazonMskCluster, ApacheKafkaCluster, KafkaCluster, ReplicationInfo,
};
use service_com_amazonaws_kafka_kafka::state::{
    ComAmazonawsKafkaKafkaState, Replicator, ReplicatorCluster, REPLICATOR_ACTIVATION_DELAY,
    REPLICATOR_STATE_CREATING, REPLICATOR_STATE_RUNNING,
};
use smithy_server_runtime::MultiServiceRouter;

/// The signing region every request below is scoped to unless a row names
/// another; it is the region whose replicators the endpoint must report.
const REGION: &str = "us-west-2";

/// The kafka service mounted in a router, driven over HTTP exactly as the
/// emulator serves it, plus the engine whose store it reads.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer, parsed.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type` of the JSON error envelope), or `""` on
    /// success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// This page's `replicators`, which a successful answer always carries.
    fn page(&self) -> Vec<Value> {
        self.body["replicators"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// This page's `nextToken`, absent on the last page.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the core's business, not this suite's: the
        // requests below carry a SigV4 scope so the region and route key
        // resolve, but they are not signed.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// Put `replicator` in the registry through the service's own insertion
    /// path, and hand back its ARN.
    fn seed(&self, replicator: Replicator) -> String {
        let arn = replicator.arn.clone();
        self.engine
            .state::<ComAmazonawsKafkaKafkaState>()
            .lock()
            .register_replicator(replicator)
            .unwrap_or_else(|rejected| panic!("seeding {arn} was refused: {rejected:?}"));
        arn
    }

    /// The `ReplicatorState` the registry currently holds for `arn`, if any —
    /// read to prove what a reporting call did and did not write.
    fn stored_state(&self, arn: &str) -> Option<String> {
        self.engine
            .state::<ComAmazonawsKafkaKafkaState>()
            .lock()
            .replicators
            .get(arn)
            .map(|replicator| replicator.state.clone())
    }

    async fn list(&self, query: &str) -> Reply {
        self.list_in(REGION, query).await
    }

    /// `ListReplicators` against the endpoint of `region`, which is the region
    /// whose replicators it must report.
    async fn list_in(&self, region: &str, query: &str) -> Reply {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("/replication/v1/replicators{query}"))
            .header("host", format!("kafka.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/kafka/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::new())
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// Every page of `query`, concatenated by following `nextToken`, with the
    /// per-page sizes and the number of pages. Fails the case rather than
    /// looping forever if the list never closes.
    async fn walk(&self, query: &str, case: &str) -> (Vec<Value>, Vec<usize>, usize) {
        let mut all: Vec<Value> = Vec::new();
        let mut sizes: Vec<usize> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= 64,
                "case {case}: pagination did not end after {pages} pages"
            );
            let uri = match &token {
                Some(value) => format!("{query}{}nextToken={}", separator(query), pct(value)),
                None => query.to_string(),
            };
            let reply = self.list(&uri).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected ListReplicators to return 200, got {} {}",
                reply.status, reply.body
            );
            let page = reply.page();
            sizes.push(page.len());
            all.extend(page);
            match reply.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        (all, sizes, pages)
    }
}

/// `?` or `&`, whichever continues `query`'s query string.
fn separator(query: &str) -> &'static str {
    if query.contains('?') {
        "&"
    } else {
        "?"
    }
}

/// Percent-encode a URI component. A `NextToken` is base64, whose `+` would
/// otherwise decode to a space. Everything that is not an ASCII alphanumeric is
/// encoded — the conservative set, and over-encoding an unreserved byte is still
/// correct (RFC 3986 §2.1).
fn pct(value: &str) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(char::from(*byte));
        } else {
            // Infallible: `{:02X}` raises no formatting error, and a `String` is
            // the one `fmt::Write` that cannot fail for any other reason.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// A replicator record as its creator mints one: identity derived from the
/// region, the name and a per-record ordinal, no clusters yet, and the clock
/// owing it `CREATING` -> `RUNNING` after [`REPLICATOR_ACTIVATION_DELAY`]. A row
/// that exercises more overwrites the fields it cares about.
fn replicator(region: &str, name: &str, ordinal: u32) -> Replicator {
    let now = SystemTime::now();
    let arn = engine::emulator_arn("kafka", region, &format!("replicator/{name}/{ordinal}"));
    Replicator {
        resource_arn: arn.clone(),
        arn,
        name: name.to_string(),
        region: region.to_string(),
        // Shaped like an MSK version token; this suite only needs it to be
        // reported back unchanged.
        current_version: format!("KV{ordinal:012}"),
        creation_time: now,
        running_due: now + REPLICATOR_ACTIVATION_DELAY,
        state: REPLICATOR_STATE_CREATING.to_string(),
        is_replicator_reference: false,
        kafka_clusters: Vec::new(),
        replication_info: Vec::new(),
        // Only reported by `DescribeReplicator`, which this suite does not drive:
        // a `ReplicatorSummary` carries none of them.
        description: None,
        service_execution_role_arn: format!(
            "arn:aws:iam::{}:role/service-role/MskReplicator-{ordinal}",
            engine::emulator_account_id()
        ),
        log_delivery: None,
        tags: std::collections::BTreeMap::new(),
    }
}

/// An Amazon MSK cluster in a replicator's setup, under `alias`.
fn msk_cluster(alias: &str, arn: &str) -> ReplicatorCluster {
    ReplicatorCluster {
        alias: alias.to_string(),
        cluster: KafkaCluster {
            amazon_msk_cluster: Some(AmazonMskCluster {
                msk_cluster_arn: arn.to_string(),
            }),
            ..KafkaCluster::default()
        },
    }
}

/// A self-managed Apache Kafka cluster in a replicator's setup, under `alias`.
fn apache_cluster(alias: &str, id: &str, bootstrap: &str) -> ReplicatorCluster {
    ReplicatorCluster {
        alias: alias.to_string(),
        cluster: KafkaCluster {
            apache_kafka_cluster: Some(ApacheKafkaCluster {
                apache_kafka_cluster_id: id.to_string(),
                bootstrap_broker_string: bootstrap.to_string(),
            }),
            ..KafkaCluster::default()
        },
    }
}

/// How one end of a replication names its cluster: `ReplicationInfo` carries an
/// ARN for an Amazon MSK cluster and an id for an Apache Kafka one.
#[derive(Debug, Clone, Default)]
struct Endpoint {
    arn: Option<String>,
    id: Option<String>,
}

fn by_arn(arn: &str) -> Endpoint {
    Endpoint {
        arn: Some(arn.to_string()),
        ..Endpoint::default()
    }
}

fn by_id(id: &str) -> Endpoint {
    Endpoint {
        id: Some(id.to_string()),
        ..Endpoint::default()
    }
}

/// A replication between the two endpoints. The topic and consumer-group
/// configuration is left at its default: no member of it reaches
/// `ReplicationInfoSummary`.
fn replication(source: Endpoint, target: Endpoint) -> ReplicationInfo {
    ReplicationInfo {
        source_kafka_cluster_arn: source.arn,
        source_kafka_cluster_id: source.id,
        target_kafka_cluster_arn: target.arn,
        target_kafka_cluster_id: target.id,
        ..ReplicationInfo::default()
    }
}

/// The reported timestamp as seconds since the epoch, however the wire carried
/// it.
///
/// This emulator's serializer renders a timestamp member as an epoch-seconds
/// number; an ISO-8601 string is the other form the protocol allows for a
/// `@timestampFormat("date-time")` member. Which encoding the core picks is not
/// this operation's business, so the suite pins the INSTANT reported rather than
/// its spelling.
fn reported_epoch_seconds(value: &Value, case: &str) -> f64 {
    match value {
        Value::Number(seconds) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: timestamp {value} is not a number")),
        Value::String(text) => {
            ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
                .unwrap_or_else(|error| {
                    panic!("case {case}: timestamp {text:?} is not ISO-8601: {error}")
                })
                .as_secs_f64()
        }
        other => panic!("case {case}: expected a timestamp, got {other}"),
    }
}

/// The `replicatorName` of each entry of a page, in the order reported.
fn names(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|entry| {
            entry["replicatorName"]
                .as_str()
                .unwrap_or_else(|| panic!("every ReplicatorSummary carries a replicatorName"))
                .to_string()
        })
        .collect()
}

/// RULE: with no replicator registered in the endpoint's region, the operation
/// succeeds and reports an empty list, whatever the paging and filter arguments
/// — an empty account is not an error and closes the list with no `nextToken`.
#[tokio::test]
async fn an_account_with_no_replicators_reports_an_empty_list() {
    let emulator = Emulator::new();
    // A replicator of ANOTHER region is seeded, so the empty answer below is the
    // region scoping and not simply an empty store.
    emulator.seed(replicator("eu-central-1", "elsewhere", 1));

    for query in [
        "",
        "?maxResults=1",
        "?maxResults=100",
        "?replicatorNameFilter=elsewhere",
        "?replicatorNameFilter=a&maxResults=7",
    ] {
        let reply = emulator.list(query).await;
        assert_eq!(
            reply.status, 200,
            "case query={query:?}: expected 200, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.page().is_empty(),
            "case query={query:?}: expected no replicators, got {}",
            reply.body
        );
        assert_eq!(
            reply.next_token(),
            None,
            "case query={query:?}: an empty list must close, got {}",
            reply.body
        );
    }
}

/// RULE: paging reports every replicator of the endpoint's region exactly once
/// across the pages a caller follows, whatever page size it asks for, and the
/// last page carries no `nextToken`.
#[tokio::test]
async fn every_registered_replicator_is_reported_exactly_once_across_pages() {
    // Rows: the registry size, and the page size the caller asks for (`None` =
    // the request names none, so the model's maximum applies).
    for count in [0u32, 1, 2, 5, 17] {
        for page_size in [None, Some(1), Some(2), Some(5), Some(100)] {
            let emulator = Emulator::new();
            let mut expected: Vec<String> = Vec::new();
            for index in 0..count {
                let arn = emulator.seed(replicator(REGION, &format!("rep-{index:02}"), index));
                expected.push(arn);
            }
            expected.sort();

            let case = format!("count={count} maxResults={page_size:?}");
            let query = match page_size {
                Some(size) => format!("?maxResults={size}"),
                None => String::new(),
            };
            let (all, sizes, pages) = emulator.walk(&query, &case).await;

            let mut reported: Vec<String> = all
                .iter()
                .map(|entry| {
                    entry["replicatorArn"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: a summary carried no replicatorArn"))
                        .to_string()
                })
                .collect();
            assert_eq!(
                reported.len(),
                expected.len(),
                "case {case}: expected {} entries across {pages} pages, got {}",
                expected.len(),
                reported.len()
            );
            reported.sort();
            assert_eq!(
                reported, expected,
                "case {case}: the pages did not report every replicator exactly once"
            );
            if let Some(size) = page_size {
                for (index, reported_size) in sizes.iter().enumerate() {
                    assert!(
                        *reported_size <= size as usize,
                        "case {case}: page {index} carried {reported_size} entries, over the asked-for {size}"
                    );
                }
            }
        }
    }
}

/// RULE: `ReplicatorNameFilter` keeps exactly the replicators whose name starts
/// with the given value — a case-sensitive prefix test, not a substring or an
/// equality one — and the filter holds across a token round trip.
#[tokio::test]
async fn the_name_filter_keeps_exactly_the_prefix_matches() {
    let emulator = Emulator::new();
    // Names drawn along the model's `ReplicatorName` pattern
    // (`^[0-9A-Za-z][0-9A-Za-z-]{0,}$`): letters, digits and hyphens, in both
    // cases.
    for (index, name) in ["alpha", "alpha-two", "alphabet", "Alpha", "beta", "b2"]
        .into_iter()
        .enumerate()
    {
        emulator.seed(replicator(REGION, name, index as u32));
    }

    // Rows: the filter, and the names it must keep (sorted).
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("alpha", vec!["alpha", "alpha-two", "alphabet"]),
        ("alpha-", vec!["alpha-two"]),
        ("alphabet", vec!["alphabet"]),
        ("Alpha", vec!["Alpha"]),
        ("b", vec!["b2", "beta"]),
        ("beta", vec!["beta"]),
        ("2", vec![]),
        ("eta", vec![]),
        ("zeta", vec![]),
        (
            "",
            vec!["Alpha", "alpha", "alpha-two", "alphabet", "b2", "beta"],
        ),
    ];

    for (filter, expected) in rows {
        let case = format!("filter={filter:?}");
        // One page large enough for the whole registry, and a one-at-a-time
        // walk: the filter must survive the token round trip unchanged.
        for page_size in [None, Some(1)] {
            let query = match page_size {
                Some(size) => format!("?replicatorNameFilter={filter}&maxResults={size}"),
                None => format!("?replicatorNameFilter={filter}"),
            };
            let (all, _, _) = emulator.walk(&query, &case).await;
            let mut got = names(&all);
            got.sort();
            assert_eq!(
                got, expected,
                "case {case} maxResults={page_size:?}: expected {expected:?}, got {got:?}"
            );
        }
    }
}

/// RULE: `ListReplicators` is regional — an endpoint reports the replicators of
/// its own signing region and none of any other region's.
#[tokio::test]
async fn a_regional_endpoint_reports_only_its_own_regions_replicators() {
    let emulator = Emulator::new();
    // Rows: region, and the replicator names it holds.
    let layout: Vec<(&str, Vec<&str>)> = vec![
        ("us-west-2", vec!["west-one", "west-two"]),
        ("us-east-1", vec!["east-only"]),
        ("eu-central-1", vec!["eu-a", "eu-b", "eu-c"]),
        ("ap-southeast-2", vec![]),
    ];
    let mut ordinal = 0u32;
    for (region, replicators) in &layout {
        for name in replicators {
            emulator.seed(replicator(region, name, ordinal));
            ordinal += 1;
        }
    }

    for (region, replicators) in &layout {
        let reply = emulator.list_in(region, "?maxResults=100").await;
        assert_eq!(
            reply.status, 200,
            "case region={region}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let mut got = names(&reply.page());
        got.sort();
        let mut expected: Vec<String> = replicators.iter().map(ToString::to_string).collect();
        expected.sort();
        assert_eq!(
            got, expected,
            "case region={region}: expected {expected:?}, got {got:?}"
        );
    }
}

/// RULE: every member of a reported `ReplicatorSummary` is computed from the
/// stored replicator — the cluster summaries from its stored Kafka clusters, and
/// each replication's source and target ALIASES resolved against those clusters
/// by the ARN or id the replication names them with.
#[tokio::test]
async fn each_summary_is_derived_from_the_stored_replication_setup() {
    let emulator = Emulator::new();
    let msk_arn = engine::emulator_arn("kafka", REGION, "cluster/source-msk/abcd-1");
    let unknown_arn = engine::emulator_arn("kafka", REGION, "cluster/not-in-setup/ffff-9");

    // Row 1: a locally created replicator, naming one Amazon MSK cluster (by
    // ARN) and one Apache Kafka cluster (by id), with a replication each way and
    // one whose source is a cluster the setup does not hold.
    let mut local = replicator(REGION, "both-kinds", 1);
    local.kafka_clusters = vec![
        msk_cluster("source", &msk_arn),
        apache_cluster("target", "apache-1", "broker-1.example.com:9092"),
    ];
    local.replication_info = vec![
        replication(by_arn(&msk_arn), by_id("apache-1")),
        replication(by_id("apache-1"), by_arn(&msk_arn)),
        replication(by_arn(&unknown_arn), by_arn(&msk_arn)),
    ];
    let created_at = local.creation_time;
    let local_version = local.current_version.clone();
    let local_arn = emulator.seed(local);

    // Row 2: a replicator REFERENCE — the resource lives in the region it was
    // created in, so its `replicatorResourceArn` is not its `replicatorArn`.
    let mut reference = replicator(REGION, "a-reference", 2);
    reference.is_replicator_reference = true;
    reference.resource_arn =
        engine::emulator_arn("kafka", "us-east-1", "replicator/a-reference/2");
    let reference_resource_arn = reference.resource_arn.clone();
    let reference_arn = emulator.seed(reference);

    let reply = emulator.list("?maxResults=100").await;
    assert_eq!(
        reply.status, 200,
        "expected 200, got {} {}",
        reply.status, reply.body
    );
    let page = reply.page();
    assert_eq!(page.len(), 2, "expected both replicators, got {}", reply.body);

    let local_summary = page
        .iter()
        .find(|entry| entry["replicatorArn"] == Value::String(local_arn.clone()))
        .unwrap_or_else(|| panic!("case both-kinds: not reported in {}", reply.body));

    assert_eq!(
        local_summary["replicatorName"], "both-kinds",
        "case both-kinds: expected the stored name, got {local_summary}"
    );
    assert_eq!(
        local_summary["replicatorResourceArn"],
        Value::String(local_arn.clone()),
        "case both-kinds: a locally created replicator's resource ARN is its own ARN, got {local_summary}"
    );
    assert_eq!(
        local_summary["currentVersion"],
        Value::String(local_version.clone()),
        "case both-kinds: expected the stored currentVersion, got {local_summary}"
    );
    assert_eq!(
        local_summary["isReplicatorReference"],
        Value::Bool(false),
        "case both-kinds: expected isReplicatorReference=false, got {local_summary}"
    );
    // `CreationTime` reports the instant the stored record was created.
    let expected_creation = ::aws_smithy_types::DateTime::from(created_at).as_secs_f64();
    let reported_creation = reported_epoch_seconds(&local_summary["creationTime"], "both-kinds");
    assert!(
        (reported_creation - expected_creation).abs() < 0.001,
        "case both-kinds: expected creationTime {expected_creation}, got {reported_creation} in {local_summary}"
    );

    // The cluster summaries carry the alias and the stored cluster details, in
    // submission order.
    let clusters = local_summary["kafkaClustersSummary"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        clusters.len(),
        2,
        "case both-kinds: expected two cluster summaries, got {local_summary}"
    );
    assert_eq!(
        clusters[0]["kafkaClusterAlias"], "source",
        "case both-kinds: expected the first cluster's alias, got {}",
        clusters[0]
    );
    assert_eq!(
        clusters[0]["amazonMskCluster"]["mskClusterArn"],
        Value::String(msk_arn.clone()),
        "case both-kinds: expected the stored MSK cluster ARN, got {}",
        clusters[0]
    );
    assert!(
        clusters[0].get("apacheKafkaCluster").is_none(),
        "case both-kinds: an MSK cluster must not report an Apache cluster, got {}",
        clusters[0]
    );
    assert_eq!(
        clusters[1]["kafkaClusterAlias"], "target",
        "case both-kinds: expected the second cluster's alias, got {}",
        clusters[1]
    );
    assert_eq!(
        clusters[1]["apacheKafkaCluster"]["apacheKafkaClusterId"], "apache-1",
        "case both-kinds: expected the stored Apache cluster id, got {}",
        clusters[1]
    );
    assert_eq!(
        clusters[1]["apacheKafkaCluster"]["bootstrapBrokerString"],
        "broker-1.example.com:9092",
        "case both-kinds: expected the stored bootstrap brokers, got {}",
        clusters[1]
    );

    // Each replication is reported by the aliases its endpoints resolve to; an
    // endpoint naming a cluster the setup does not hold resolves to no alias.
    let replications = local_summary["replicationInfoSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let expected_aliases: Vec<(Option<&str>, Option<&str>)> = vec![
        (Some("source"), Some("target")),
        (Some("target"), Some("source")),
        (None, Some("source")),
    ];
    assert_eq!(
        replications.len(),
        expected_aliases.len(),
        "case both-kinds: expected {} replication summaries, got {local_summary}",
        expected_aliases.len()
    );
    for (index, (source, target)) in expected_aliases.into_iter().enumerate() {
        let got_source = replications[index]["sourceKafkaClusterAlias"].as_str();
        let got_target = replications[index]["targetKafkaClusterAlias"].as_str();
        assert_eq!(
            got_source, source,
            "case both-kinds replication {index}: expected source alias {source:?}, got {got_source:?}"
        );
        assert_eq!(
            got_target, target,
            "case both-kinds replication {index}: expected target alias {target:?}, got {got_target:?}"
        );
    }

    let reference_summary = page
        .iter()
        .find(|entry| entry["replicatorArn"] == Value::String(reference_arn.clone()))
        .unwrap_or_else(|| panic!("case a-reference: not reported in {}", reply.body));
    assert_eq!(
        reference_summary["isReplicatorReference"],
        Value::Bool(true),
        "case a-reference: expected isReplicatorReference=true, got {reference_summary}"
    );
    assert_eq!(
        reference_summary["replicatorResourceArn"],
        Value::String(reference_resource_arn.clone()),
        "case a-reference: expected the stored resource ARN, got {reference_summary}"
    );
    assert!(
        reference_summary["kafkaClustersSummary"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "case a-reference: a replicator with no clusters reports none, got {reference_summary}"
    );
}

/// RULE: a replicator is reported in the state the clock owes it — `CREATING`
/// until its activation instant has passed, `RUNNING` from then on — and
/// reporting it materialises only that already-due transition, so the state a
/// later caller sees never goes backwards and is never advanced early.
#[tokio::test]
async fn the_reported_state_follows_the_clock() {
    let emulator = Emulator::new();
    let now = SystemTime::now();

    // Rows: how the record's activation instant sits relative to now, and the
    // state the endpoint must report for it.
    let mut pending = replicator(REGION, "pending", 1);
    pending.running_due = now + Duration::from_secs(3600);
    let pending_arn = emulator.seed(pending);

    let mut due = replicator(REGION, "due", 2);
    due.running_due = now - Duration::from_secs(1);
    let due_arn = emulator.seed(due);

    let mut settled = replicator(REGION, "settled", 3);
    settled.state = REPLICATOR_STATE_RUNNING.to_string();
    settled.running_due = now - Duration::from_secs(3600);
    let settled_arn = emulator.seed(settled);

    let rows = [
        (&pending_arn, REPLICATOR_STATE_CREATING),
        (&due_arn, REPLICATOR_STATE_RUNNING),
        (&settled_arn, REPLICATOR_STATE_RUNNING),
    ];

    // Listed twice: the second answer must agree with the first, so a read is
    // stable rather than advancing anything of its own.
    for attempt in 1..=2 {
        let reply = emulator.list("?maxResults=100").await;
        assert_eq!(
            reply.status, 200,
            "attempt {attempt}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let page = reply.page();
        for (arn, expected) in rows {
            let summary = page
                .iter()
                .find(|entry| entry["replicatorArn"] == Value::String(arn.clone()))
                .unwrap_or_else(|| panic!("case {arn}: not reported in {}", reply.body));
            assert_eq!(
                summary["replicatorState"],
                Value::String(expected.to_string()),
                "case {arn} attempt {attempt}: expected {expected}, got {summary}"
            );
            assert_eq!(
                emulator.stored_state(arn).as_deref(),
                Some(expected),
                "case {arn} attempt {attempt}: the registry must hold the reported state"
            );
        }
    }

    // A replicator whose activation is owed at the project's activation delay is
    // still `CREATING` for a caller that lists straight away, and `RUNNING` for
    // one that waits: the span is never zero.
    let fresh_arn = emulator.seed(replicator(REGION, "fresh", 4));
    let immediate = emulator.list("?replicatorNameFilter=fresh").await;
    let immediate_state = immediate.page()[0]["replicatorState"].clone();
    assert_eq!(
        immediate_state,
        Value::String(REPLICATOR_STATE_CREATING.to_string()),
        "case fresh: a caller listing immediately must still see CREATING, got {}",
        immediate.body
    );
    tokio::time::sleep(REPLICATOR_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let later = emulator.list("?replicatorNameFilter=fresh").await;
    assert_eq!(
        later.page()[0]["replicatorState"],
        Value::String(REPLICATOR_STATE_RUNNING.to_string()),
        "case fresh: a caller listing after the activation delay must see RUNNING, got {}",
        later.body
    );
    assert_eq!(
        emulator.stored_state(&fresh_arn).as_deref(),
        Some(REPLICATOR_STATE_RUNNING),
        "case fresh: the due transition is materialised in the registry"
    );
}

/// RULE: a `NextToken` this operation did not mint is refused with HTTP 400
/// `BadRequestException` naming the parameter, while a token it did mint
/// resumes the list.
#[tokio::test]
async fn a_pagination_token_this_operation_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    for index in 0..3u32 {
        emulator.seed(replicator(REGION, &format!("rep-{index}"), index));
    }

    // Rows: not base64 at all, valid base64 of unrelated bytes, base64 of a
    // token minted for another list, and the empty token.
    let foreign = ::aws_smithy_types::base64::encode("kafka:clusters:arn:aws:kafka:::cluster/x/1");
    for token in [
        "!!!not-base64!!!".to_string(),
        "aGVsbG8gd29ybGQ=".to_string(),
        foreign,
        "%20".to_string(),
    ] {
        let reply = emulator.list(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case token={token:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case token={token:?}: expected BadRequestException, got {}",
            reply.body
        );
    }

    // The token the operation itself mints is accepted and resumes the list.
    let first = emulator.list("?maxResults=1").await;
    let token = first
        .next_token()
        .unwrap_or_else(|| panic!("a truncated page must carry a nextToken, got {}", first.body));
    let second = emulator
        .list(&format!("?maxResults=1&nextToken={}", pct(&token)))
        .await;
    assert_eq!(
        second.status, 200,
        "a token this operation minted must be accepted, got {} {}",
        second.status, second.body
    );
    assert_eq!(
        names(&second.page()),
        vec!["rep-1".to_string()],
        "the second page must resume after the first, got {}",
        second.body
    );
}

/// RULE: `MaxResults` is accepted on each published bound of the model's 1..100
/// range and refused outside it, as a client-input error rather than a server
/// fault.
#[tokio::test]
async fn max_results_is_bounded_by_the_models_range() {
    let emulator = Emulator::new();
    for index in 0..3u32 {
        emulator.seed(replicator(REGION, &format!("rep-{index}"), index));
    }

    // Each published bound, and one past it on either side.
    for size in [1, 100] {
        let reply = emulator.list(&format!("?maxResults={size}")).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={size}: a bound of the range must be accepted, got {} {}",
            reply.status, reply.body
        );
        assert!(
            !reply.page().is_empty(),
            "case maxResults={size}: expected a page of results, got {}",
            reply.body
        );
    }
    for size in [0, 101, -1] {
        let reply = emulator.list(&format!("?maxResults={size}")).await;
        assert_eq!(
            reply.status, 400,
            "case maxResults={size}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            matches!(
                reply.error_code(),
                "BadRequestException" | "ValidationException"
            ),
            "case maxResults={size}: expected a client-input refusal, got {}",
            reply.body
        );
    }
}
