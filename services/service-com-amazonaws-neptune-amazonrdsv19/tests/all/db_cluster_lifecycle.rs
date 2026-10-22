// SPDX-License-Identifier: CC0-1.0

//! The Neptune DB cluster / DB instance lifecycle, end to end over the awsQuery wire.
//!
//! Every test here drives the real HTTP surface: a `MultiServiceRouter` with this one
//! service mounted, fed form-urlencoded `Action=...` bodies whose credential scope names
//! this service (`neptune` — the `rds` signing name Neptune's model carries is SHARED
//! with Amazon RDS and Amazon DocumentDB and resolves to Amazon RDS; see `ROUTE_KEY` in
//! `src/service.rs`), and asserted against the XML response body — not against the
//! handler's Rust types. That is the only way to prove the wire contract (status codes,
//! awsQuery error codes, member names) rather than just the handler's internals.
//!
//! The rule under test: every member of a `DBCluster` / `DBInstance` response is computed
//! from the persisted record, so a create followed by a describe returns the same bytes.
//! Each test states that over a TABLE of inputs; the identifiers the issue happened to
//! use are one row.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_neptune_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The API version this service's model declares.
const API_VERSION: &str = "2014-10-31";
/// The region the test's SigV4 credential scope names; ARNs and endpoints must use it.
const REGION: &str = "us-east-1";

/// Characters a form-urlencoded value would have to escape. No parameter this suite sends
/// contains one, so values go on the wire verbatim and [`Neptune::call`] asserts that
/// stays true — rather than this crate taking a dependency on a percent encoder (and a
/// Cargo.lock entry) for test scaffolding.
const FORM_RESERVED: [char; 5] = ['&', '=', '+', '%', ' '];

/// One element of a parsed response body: its name, its own text, and its children.
///
/// Read by the small reader below rather than by an XML crate: the awsQuery bodies this
/// service emits are elements, text and the five predefined entities, with no attributes
/// to read, no CDATA and no comments, and keeping the suite dependency-free keeps this
/// change's Cargo.lock diff empty. The reader is deliberately strict — a malformed body
/// panics the test rather than silently yielding an empty tree.
#[derive(Debug, Default, Clone)]
struct Element {
    name: String,
    text: String,
    children: Vec<Element>,
}

impl Element {
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The element reached by following `path` from here.
    fn at(&self, path: &[&str]) -> Option<&Element> {
        let mut current = self;
        for segment in path {
            current = current.child(segment)?;
        }
        Some(current)
    }

    /// The text of the direct child element `name`, or the empty string when absent.
    fn field(&self, name: &str) -> String {
        self.child(name)
            .map(|found| found.text.clone())
            .unwrap_or_default()
    }

    /// Every leaf below here, keyed by its indexed element path, so two renderings of one
    /// record can be compared field by field.
    fn leaves(&self, prefix: &str, out: &mut BTreeMap<String, String>) {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for child in &self.children {
            let index = seen.entry(child.name.as_str()).or_insert(0);
            *index += 1;
            let path = format!("{prefix}/{}[{index}]", child.name);
            if child.children.is_empty() {
                out.insert(path, child.text.clone());
            } else {
                child.leaves(&path, out);
            }
        }
    }
}

/// Parse `body` and return its single root element.
fn parse_xml(body: &str) -> Element {
    let mut roots: Vec<Element> = Vec::new();
    let mut stack: Vec<Element> = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('<') {
            let (tag, after) = tail.split_once('>').expect("every tag is closed");
            rest = after;
            // `<?xml ...?>` prologue and `<!...>` declarations carry no element.
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                let closed = stack.pop().expect("a close tag has an open tag");
                assert_eq!(closed.name, name, "mismatched close tag in {body}");
                match stack.last_mut() {
                    Some(parent) => parent.children.push(closed),
                    None => roots.push(closed),
                }
            } else {
                // `<Name attr=".."/>` — an empty element with no close tag.
                let (tag, empty) = match tag.strip_suffix('/') {
                    Some(tag) => (tag, true),
                    None => (tag, false),
                };
                let element = Element {
                    name: tag.split_whitespace().next().unwrap_or_default().to_string(),
                    ..Element::default()
                };
                if empty {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(element),
                        None => roots.push(element),
                    }
                } else {
                    stack.push(element);
                }
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let (text, after) = rest.split_at(end);
            rest = after;
            if let Some(current) = stack.last_mut() {
                current.text.push_str(&decode_entities(text));
            }
        }
    }
    assert!(stack.is_empty(), "unclosed element in {body}");
    assert_eq!(roots.len(), 1, "expected one root element in {body}");
    roots.pop().unwrap_or_default()
}

/// The five entities XML predefines; `&amp;` is decoded last so `&amp;lt;` stays `&lt;`.
fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// One response: the HTTP status, the raw XML body (for assertion messages) and its
/// parsed root element.
struct Reply {
    status: u16,
    body: String,
    root: Element,
}

impl Reply {
    /// The text of the element at `path`, descending from the response root.
    fn field(&self, path: &[&str]) -> String {
        self.root
            .at(path)
            .map(|found| found.text.clone())
            .unwrap_or_default()
    }

    /// Whether the element at `path` is present at all.
    fn has(&self, path: &[&str]) -> bool {
        self.root.at(path).is_some()
    }

    /// The texts of every element child of the element at `path` (an XML list).
    fn list(&self, path: &[&str]) -> Vec<String> {
        self.root
            .at(path)
            .map(|found| {
                found
                    .children
                    .iter()
                    .map(|child| child.text.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every leaf under the element at `path`, keyed by its indexed element path.
    fn leaves(&self, path: &[&str]) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        if let Some(found) = self.root.at(path) {
            found.leaves("", &mut out);
        }
        out
    }

    /// The awsQuery `<Error><Code>` of a failure response.
    fn error_code(&self) -> String {
        self.field(&["Error", "Code"])
    }

    /// The awsQuery `<Error><Message>` of a failure response.
    fn error_message(&self) -> String {
        self.field(&["Error", "Message"])
    }
}

/// One emulator with this service mounted, driven over HTTP in-process.
struct Neptune {
    router: MultiServiceRouter,
}

impl Neptune {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The test asserts service behaviour, not SigV4 verification.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, action: &str, params: &[(&str, &str)]) -> Reply {
        let mut body = format!("Action={action}&Version={API_VERSION}");
        for (key, value) in params {
            assert!(
                !value.contains(FORM_RESERVED),
                "case {key:?}={value:?}: this suite sends parameter values verbatim, so a \
                 value carrying one of {FORM_RESERVED:?} needs percent-encoding first"
            );
            body.push('&');
            body.push_str(key);
            body.push('=');
            body.push_str(value);
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-www-form-urlencoded")
            // This model's sigv4 name is `rds`, which it SHARES with the Amazon RDS and
            // Amazon DocumentDB views of the same Query front door; the shared `rds` key
            // resolves to the Amazon RDS crate and this service answers under its own
            // service prefix, so the scope names that key here. See `src/service.rs`.
            .header(
                // The routing key every front door reads: the SigV4 credential scope.
                // Neptune SHARES the `rds` signing name with Amazon RDS and Amazon
                // DocumentDB, and that shared name resolves to Amazon RDS, so a request
                // for THIS service names it: `neptune` (see `ROUTE_KEY` in
                // `src/service.rs`).
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/neptune/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        let root = parse_xml(&body);
        Reply { status, body, root }
    }
}

/// The request members a table row sends beyond `Action` / `Version`.
type Params = &'static [(&'static str, &'static str)];

/// One row of the modelled-fault table: case name, action, parameters, the HTTP status
/// the model declares, and the awsQuery `<Code>` the fault must carry.
type FaultRow = (&'static str, &'static str, Params, u16, &'static str);

/// One row of the refused-input table: case name, action, parameters, and a fragment the
/// refusal's message must name.
type RefusalRow = (&'static str, &'static str, Params, &'static str);

/// Where a `DBCluster` sits in each operation's response envelope.
fn cluster_path(action: &str) -> Vec<String> {
    vec![format!("{action}Result"), "DBCluster".to_string()]
}

fn as_path(segments: &[String]) -> Vec<&str> {
    segments.iter().map(String::as_str).collect()
}

// ---------------------------------------------------------------------------
// Create -> describe round trip, over a table of cluster shapes.
// ---------------------------------------------------------------------------

/// One row of the create/describe round-trip table.
struct ClusterCase {
    name: &'static str,
    /// The `DBClusterIdentifier` the request sends, which may be mixed case.
    requested_identifier: &'static str,
    /// The identifier the service must store and report: the request's, lowercased.
    stored_identifier: &'static str,
    /// Request members beyond `DBClusterIdentifier` / `Engine`.
    extra: &'static [(&'static str, &'static str)],
    expected_port: &'static str,
    expected_engine_version: &'static str,
    expected_backup_retention: &'static str,
    expected_storage_encrypted: &'static str,
    expected_deletion_protection: &'static str,
    expected_network_type: &'static str,
    /// `None` means the response must omit `StorageType` entirely.
    expected_storage_type: Option<&'static str>,
    expected_availability_zones: &'static [&'static str],
}

/// The table spans the members the rule generalises over: value (port, engine version,
/// retention days), type (boolean flags), count (how many Availability Zones), form (the
/// identifier's letter case) and presence (a `StorageType` the API deliberately omits).
const CLUSTER_CASES: &[ClusterCase] = &[
    ClusterCase {
        name: "defaults only",
        requested_identifier: "plain-graph",
        stored_identifier: "plain-graph",
        extra: &[],
        // Port — "Default: 8182".
        expected_port: "8182",
        expected_engine_version: "1.3.2.1",
        // BackupRetentionPeriod — "Default: 1".
        expected_backup_retention: "1",
        expected_storage_encrypted: "false",
        // "when you create a new cluster using CreateDBCluster directly, deletion
        // protection is disabled by default".
        expected_deletion_protection: "false",
        // NetworkType — "IPV4 – ( the default )".
        expected_network_type: "IPV4",
        expected_storage_type: None,
        expected_availability_zones: &["us-east-1a", "us-east-1b", "us-east-1c"],
    },
    ClusterCase {
        name: "mixed-case identifier is stored lowercase",
        requested_identifier: "MyGraph-TWO",
        stored_identifier: "mygraph-two",
        extra: &[],
        expected_port: "8182",
        expected_engine_version: "1.3.2.1",
        expected_backup_retention: "1",
        expected_storage_encrypted: "false",
        expected_deletion_protection: "false",
        expected_network_type: "IPV4",
        expected_storage_type: None,
        expected_availability_zones: &["us-east-1a", "us-east-1b", "us-east-1c"],
    },
    ClusterCase {
        name: "every optional member supplied",
        requested_identifier: "loaded-graph",
        stored_identifier: "loaded-graph",
        extra: &[
            ("Port", "9999"),
            ("EngineVersion", "1.2.1.0"),
            ("BackupRetentionPeriod", "35"),
            ("StorageEncrypted", "true"),
            ("KmsKeyId", "arn:aws:kms:us-east-1:000000000000:key/abc"),
            ("StorageType", "iopt1"),
            ("NetworkType", "DUAL"),
            ("DeletionProtection", "true"),
            ("AvailabilityZones.AvailabilityZone.1", "us-east-1d"),
            ("AvailabilityZones.AvailabilityZone.2", "us-east-1f"),
            ("Tags.Tag.1.Key", "team"),
            ("Tags.Tag.1.Value", "graphs"),
        ],
        expected_port: "9999",
        expected_engine_version: "1.2.1.0",
        // The published upper bound of "Must be a value from 1 to 35".
        expected_backup_retention: "35",
        expected_storage_encrypted: "true",
        expected_deletion_protection: "true",
        expected_network_type: "DUAL",
        expected_storage_type: Some("iopt1"),
        expected_availability_zones: &["us-east-1d", "us-east-1f"],
    },
    ClusterCase {
        name: "standard storage type is not reported",
        requested_identifier: "standard-graph",
        stored_identifier: "standard-graph",
        // "When set to standard, the storage type is not returned in the response."
        extra: &[("StorageType", "standard"), ("BackupRetentionPeriod", "1")],
        expected_port: "8182",
        expected_engine_version: "1.3.2.1",
        // The published lower bound of "Must be a value from 1 to 35".
        expected_backup_retention: "1",
        expected_storage_encrypted: "false",
        expected_deletion_protection: "false",
        expected_network_type: "IPV4",
        expected_storage_type: None,
        expected_availability_zones: &["us-east-1a", "us-east-1b", "us-east-1c"],
    },
];

#[tokio::test]
async fn create_db_cluster_persists_every_member_and_describe_returns_the_same_bytes() {
    let neptune = Neptune::new();

    for case in CLUSTER_CASES {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", case.requested_identifier),
            ("Engine", "neptune"),
        ];
        params.extend_from_slice(case.extra);
        let created = neptune.call("CreateDBCluster", &params).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200, got {} with body {}",
            case.name, created.status, created.body
        );

        let create_path = cluster_path("CreateDBCluster");
        let create_path = as_path(&create_path);
        let field = |path: &str| {
            let mut full = create_path.clone();
            full.push(path);
            created.field(&full)
        };

        assert_eq!(
            field("DBClusterIdentifier"),
            case.stored_identifier,
            "case {:?}: expected the identifier stored lowercase, got {:?}",
            case.name,
            field("DBClusterIdentifier")
        );
        assert_eq!(
            field("Engine"),
            "neptune",
            "case {:?}: expected Engine neptune, got {:?}",
            case.name,
            field("Engine")
        );
        assert_eq!(
            field("Port"),
            case.expected_port,
            "case {:?}: expected Port {}, got {:?}",
            case.name,
            case.expected_port,
            field("Port")
        );
        assert_eq!(
            field("EngineVersion"),
            case.expected_engine_version,
            "case {:?}: expected EngineVersion {}, got {:?}",
            case.name,
            case.expected_engine_version,
            field("EngineVersion")
        );
        assert_eq!(
            field("BackupRetentionPeriod"),
            case.expected_backup_retention,
            "case {:?}: expected BackupRetentionPeriod {}, got {:?}",
            case.name,
            case.expected_backup_retention,
            field("BackupRetentionPeriod")
        );
        assert_eq!(
            field("StorageEncrypted"),
            case.expected_storage_encrypted,
            "case {:?}: expected StorageEncrypted {}, got {:?}",
            case.name,
            case.expected_storage_encrypted,
            field("StorageEncrypted")
        );
        assert_eq!(
            field("DeletionProtection"),
            case.expected_deletion_protection,
            "case {:?}: expected DeletionProtection {}, got {:?}",
            case.name,
            case.expected_deletion_protection,
            field("DeletionProtection")
        );
        assert_eq!(
            field("NetworkType"),
            case.expected_network_type,
            "case {:?}: expected NetworkType {}, got {:?}",
            case.name,
            case.expected_network_type,
            field("NetworkType")
        );
        let mut storage_type_path = create_path.clone();
        storage_type_path.push("StorageType");
        match case.expected_storage_type {
            Some(expected) => assert_eq!(
                created.field(&storage_type_path),
                expected,
                "case {:?}: expected StorageType {expected}, got {:?}",
                case.name,
                created.field(&storage_type_path)
            ),
            None => assert!(
                !created.has(&storage_type_path),
                "case {:?}: expected no StorageType member, got {:?}",
                case.name,
                created.field(&storage_type_path)
            ),
        }

        // Identity members are built from the identifier and the request's region.
        assert_eq!(
            field("DBClusterArn"),
            format!("arn:aws:rds:{REGION}:000000000000:cluster:{}", case.stored_identifier),
            "case {:?}: unexpected DBClusterArn {:?}",
            case.name,
            field("DBClusterArn")
        );
        let endpoint = field("Endpoint");
        let reader_endpoint = field("ReaderEndpoint");
        assert!(
            endpoint.starts_with(&format!("{}.cluster-", case.stored_identifier))
                && endpoint.ends_with(&format!(".{REGION}.neptune.amazonaws.com")),
            "case {:?}: unexpected Endpoint {endpoint:?}",
            case.name
        );
        assert!(
            reader_endpoint.starts_with(&format!("{}.cluster-ro-", case.stored_identifier))
                && reader_endpoint.ends_with(&format!(".{REGION}.neptune.amazonaws.com")),
            "case {:?}: unexpected ReaderEndpoint {reader_endpoint:?}",
            case.name
        );
        let resource_id = field("DbClusterResourceId");
        assert!(
            resource_id.starts_with("cluster-")
                && resource_id.len() == "cluster-".len() + 26
                && resource_id["cluster-".len()..]
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c)),
            "case {:?}: unexpected DbClusterResourceId {resource_id:?}",
            case.name
        );
        assert_eq!(
            field("AllocatedStorage"),
            "1",
            "case {:?}: expected AllocatedStorage 1, got {:?}",
            case.name,
            field("AllocatedStorage")
        );
        assert!(
            !field("ClusterCreateTime").is_empty(),
            "case {:?}: expected a ClusterCreateTime",
            case.name
        );

        let mut zones_path = create_path.clone();
        zones_path.push("AvailabilityZones");
        assert_eq!(
            created.list(&zones_path),
            case.expected_availability_zones,
            "case {:?}: unexpected AvailabilityZones {:?}",
            case.name,
            created.list(&zones_path)
        );

        // A cluster has no members until an instance joins it.
        let mut members_path = create_path.clone();
        members_path.push("DBClusterMembers");
        assert!(
            created.list(&members_path).is_empty(),
            "case {:?}: a fresh cluster must have no DBClusterMembers, got {:?}",
            case.name,
            created.list(&members_path)
        );

        // ROUND TRIP: the describe must render the same record, field for field. Only
        // `Status` may differ, because the clock owes the creating -> available move.
        let described = neptune
            .call(
                "DescribeDBClusters",
                &[("DBClusterIdentifier", case.requested_identifier)],
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected HTTP 200 from DescribeDBClusters, got {} with body {}",
            case.name, described.status, described.body
        );
        let mut created_leaves = created.leaves(&create_path);
        let mut described_leaves = described.leaves(&[
            "DescribeDBClustersResult",
            "DBClusters",
            "DBCluster",
        ]);
        created_leaves.remove("/Status[1]");
        described_leaves.remove("/Status[1]");
        assert_eq!(
            created_leaves, described_leaves,
            "case {:?}: DescribeDBClusters must return exactly what CreateDBCluster stored",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// The lifecycle the issue's "Done when" names, with cluster membership.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_cluster_and_instance_lifecycle_runs_end_to_end() {
    let neptune = Neptune::new();

    let created_cluster = neptune
        .call(
            "CreateDBCluster",
            &[("DBClusterIdentifier", "my-graph"), ("Engine", "neptune")],
        )
        .await;
    assert_eq!(
        created_cluster.status, 200,
        "CreateDBCluster must succeed, got {} with body {}",
        created_cluster.status, created_cluster.body
    );
    let cluster_endpoint =
        created_cluster.field(&["CreateDBClusterResult", "DBCluster", "Endpoint"]);
    let cluster_hosted_zone =
        created_cluster.field(&["CreateDBClusterResult", "DBCluster", "HostedZoneId"]);
    // `<id>.cluster-<shard>.<region>...` — the instance endpoints share the shard label.
    let shard = cluster_endpoint
        .split('.')
        .nth(1)
        .and_then(|label| label.strip_prefix("cluster-"))
        .expect("the cluster endpoint carries a shard label")
        .to_string();

    // Two instances: the first joins as the writer, the second as a reader.
    for instance in ["my-graph-1", "my-graph-2"] {
        let created = neptune
            .call(
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", instance),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "neptune"),
                    ("DBClusterIdentifier", "my-graph"),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {instance:?}: CreateDBInstance must succeed, got {} with body {}",
            created.status, created.body
        );
        let base = ["CreateDBInstanceResult", "DBInstance"];
        let field = |name: &str| {
            let mut path = base.to_vec();
            path.push(name);
            created.field(&path)
        };
        assert_eq!(
            field("DBInstanceIdentifier"),
            instance,
            "case {instance:?}: unexpected DBInstanceIdentifier {:?}",
            field("DBInstanceIdentifier")
        );
        assert_eq!(
            field("DBClusterIdentifier"),
            "my-graph",
            "case {instance:?}: unexpected DBClusterIdentifier {:?}",
            field("DBClusterIdentifier")
        );
        assert_eq!(
            field("DBInstanceArn"),
            format!("arn:aws:rds:{REGION}:000000000000:db:{instance}"),
            "case {instance:?}: unexpected DBInstanceArn {:?}",
            field("DBInstanceArn")
        );
        assert_eq!(
            created.field(&["CreateDBInstanceResult", "DBInstance", "Endpoint", "Address"]),
            format!("{instance}.{shard}.{REGION}.neptune.amazonaws.com"),
            "case {instance:?}: the instance endpoint must share the cluster's shard label"
        );
        assert_eq!(
            created.field(&["CreateDBInstanceResult", "DBInstance", "Endpoint", "Port"]),
            "8182",
            "case {instance:?}: the instance must inherit the cluster's port"
        );
        assert_eq!(
            created.field(&[
                "CreateDBInstanceResult",
                "DBInstance",
                "Endpoint",
                "HostedZoneId"
            ]),
            cluster_hosted_zone,
            "case {instance:?}: the instance must inherit the cluster's hosted zone"
        );
        let resource_id = field("DbiResourceId");
        assert!(
            resource_id.starts_with("db-") && resource_id.len() == "db-".len() + 26,
            "case {instance:?}: unexpected DbiResourceId {resource_id:?}"
        );
        assert!(
            !field("InstanceCreateTime").is_empty(),
            "case {instance:?}: expected an InstanceCreateTime"
        );
        assert_eq!(
            field("AvailabilityZone"),
            "us-east-1a",
            "case {instance:?}: unexpected AvailabilityZone {:?}",
            field("AvailabilityZone")
        );
    }

    // The cluster now reports both members, with exactly one writer.
    let clusters = neptune
        .call("DescribeDBClusters", &[("DBClusterIdentifier", "my-graph")])
        .await;
    assert_eq!(clusters.status, 200, "body {}", clusters.body);
    let members = clusters
        .root
        .at(&[
            "DescribeDBClustersResult",
            "DBClusters",
            "DBCluster",
            "DBClusterMembers",
        ])
        .expect("the described cluster carries DBClusterMembers");
    let mut rendered: Vec<(String, String)> = Vec::new();
    for member in &members.children {
        let id = member.field("DBInstanceIdentifier");
        let writer = member.field("IsClusterWriter");
        assert_eq!(
            member.field("DBClusterParameterGroupStatus"),
            "in-sync",
            "case {id:?}: expected DBClusterParameterGroupStatus in-sync"
        );
        assert_eq!(
            member.field("PromotionTier"),
            "1",
            "case {id:?}: expected the documented default PromotionTier 1"
        );
        rendered.push((id, writer));
    }
    assert_eq!(
        rendered,
        vec![
            ("my-graph-1".to_string(), "true".to_string()),
            ("my-graph-2".to_string(), "false".to_string()),
        ],
        "the first member must be the writer and the second a reader"
    );

    // DescribeDBInstances lists both, and names one on request.
    let all_instances = neptune.call("DescribeDBInstances", &[]).await;
    assert_eq!(all_instances.status, 200, "body {}", all_instances.body);
    assert_eq!(
        instance_ids(&all_instances, "DescribeDBInstancesResult"),
        vec!["my-graph-1".to_string(), "my-graph-2".to_string()],
        "DescribeDBInstances with no identifier must list every persisted instance"
    );
    let one = neptune
        .call(
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "my-graph-2")],
        )
        .await;
    assert_eq!(
        instance_ids(&one, "DescribeDBInstancesResult"),
        vec!["my-graph-2".to_string()],
        "DescribeDBInstances with an identifier must return exactly that instance"
    );

    // Deleting the writer reports `deleting`, detaches it, and promotes the survivor.
    let deleted = neptune
        .call(
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "my-graph-1"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(deleted.status, 200, "body {}", deleted.body);
    assert_eq!(
        deleted.field(&["DeleteDBInstanceResult", "DBInstance", "DBInstanceStatus"]),
        "deleting",
        "DeleteDBInstance must report the instance as deleting"
    );
    let after_delete = neptune
        .call("DescribeDBClusters", &[("DBClusterIdentifier", "my-graph")])
        .await;
    let members = after_delete
        .root
        .at(&[
            "DescribeDBClustersResult",
            "DBClusters",
            "DBCluster",
            "DBClusterMembers",
        ])
        .expect("DBClusterMembers");
    let surviving: Vec<(String, String)> = members
        .children
        .iter()
        .map(|member| {
            (
                member.field("DBInstanceIdentifier"),
                member.field("IsClusterWriter"),
            )
        })
        .collect();
    assert_eq!(
        surviving,
        vec![("my-graph-2".to_string(), "true".to_string())],
        "deleting the writer must detach it and promote the remaining member"
    );
    let gone = neptune
        .call(
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "my-graph-1")],
        )
        .await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (404, "DBInstanceNotFound"),
        "a deleted instance must be gone from state, got body {}",
        gone.body
    );

    // The cluster still has a member, so it cannot be deleted yet.
    let refused = neptune
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "my-graph"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "InvalidDBClusterStateFault"),
        "a cluster that still contains instances must not be deletable, got body {}",
        refused.body
    );

    let _ = neptune
        .call(
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "my-graph-2"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    let deleted_cluster = neptune
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "my-graph"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(deleted_cluster.status, 200, "body {}", deleted_cluster.body);
    assert_eq!(
        deleted_cluster.field(&["DeleteDBClusterResult", "DBCluster", "Status"]),
        "deleting",
        "DeleteDBCluster must report the cluster as deleting"
    );
    assert_eq!(
        deleted_cluster.field(&["DeleteDBClusterResult", "DBCluster", "Endpoint"]),
        cluster_endpoint,
        "DeleteDBCluster must answer from the record it removed"
    );

    let after = neptune
        .call("DescribeDBClusters", &[("DBClusterIdentifier", "my-graph")])
        .await;
    assert_eq!(
        (after.status, after.error_code().as_str()),
        (404, "DBClusterNotFoundFault"),
        "a deleted cluster must be gone from state, got body {}",
        after.body
    );
}

fn instance_ids(reply: &Reply, result: &str) -> Vec<String> {
    reply
        .root
        .at(&[result, "DBInstances"])
        .map(|list| {
            list.children
                .iter()
                .map(|instance| instance.field("DBInstanceIdentifier"))
                .collect()
        })
        .unwrap_or_default()
}

fn cluster_ids(reply: &Reply) -> Vec<String> {
    reply
        .root
        .at(&["DescribeDBClustersResult", "DBClusters"])
        .map(|list| {
            list.children
                .iter()
                .map(|cluster| cluster.field("DBClusterIdentifier"))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The clock-owed creating -> available transition.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_resource_reports_creating_and_then_becomes_available() {
    let neptune = Neptune::new();

    let created = neptune
        .call(
            "CreateDBCluster",
            &[("DBClusterIdentifier", "slow-graph"), ("Engine", "neptune")],
        )
        .await;
    assert_eq!(
        created.field(&["CreateDBClusterResult", "DBCluster", "Status"]),
        "creating",
        "a cluster AWS has not finished provisioning must report creating, body {}",
        created.body
    );
    let instance = neptune
        .call(
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "slow-graph-1"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", "slow-graph"),
            ],
        )
        .await;
    assert_eq!(
        instance.field(&["CreateDBInstanceResult", "DBInstance", "DBInstanceStatus"]),
        "creating",
        "a new instance must report creating, body {}",
        instance.body
    );

    // Nothing in the handler sleeps: the due instant is stored and applied on the next
    // touch, so waiting past it is enough.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let settled = neptune
        .call(
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "slow-graph")],
        )
        .await;
    assert_eq!(
        settled.field(&["DescribeDBClustersResult", "DBClusters", "DBCluster", "Status"]),
        "available",
        "after the provisioning span the cluster must be available, body {}",
        settled.body
    );
    let settled_instance = neptune
        .call(
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "slow-graph-1")],
        )
        .await;
    assert_eq!(
        settled_instance.field(&[
            "DescribeDBInstancesResult",
            "DBInstances",
            "DBInstance",
            "DBInstanceStatus"
        ]),
        "available",
        "after the provisioning span the instance must be available, body {}",
        settled_instance.body
    );
}

// ---------------------------------------------------------------------------
// Modelled faults: every one carries its modelled status and awsQuery code.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn modelled_faults_carry_their_modelled_status_and_query_code() {
    let neptune = Neptune::new();

    // Seed: one ordinary cluster with a member, and one protected empty cluster.
    for (identifier, extra) in [
        ("fault-graph", &[][..]),
        ("protected-graph", &[("DeletionProtection", "true")][..]),
    ] {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", identifier),
            ("Engine", "neptune"),
        ];
        params.extend_from_slice(extra);
        let reply = neptune.call("CreateDBCluster", &params).await;
        assert_eq!(reply.status, 200, "seeding {identifier}: body {}", reply.body);
    }
    let seeded = neptune
        .call(
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "fault-graph-1"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", "fault-graph"),
            ],
        )
        .await;
    assert_eq!(seeded.status, 200, "seeding an instance: body {}", seeded.body);
    let protected_instance = neptune
        .call(
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "fault-graph-protected"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", "fault-graph"),
                ("DeletionProtection", "true"),
            ],
        )
        .await;
    assert_eq!(
        protected_instance.status, 200,
        "seeding a protected instance: body {}",
        protected_instance.body
    );
    assert_eq!(
        protected_instance.field(&[
            "CreateDBInstanceResult",
            "DBInstance",
            "DeletionProtection"
        ]),
        "true",
        "the protected instance must report its persisted DeletionProtection"
    );

    let cases: &[FaultRow] = &[
        (
            "a second cluster with the same identifier",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "fault-graph"), ("Engine", "neptune")],
            400,
            "DBClusterAlreadyExistsFault",
        ),
        (
            "a second cluster with the same identifier in another letter case",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "Fault-Graph"), ("Engine", "neptune")],
            400,
            "DBClusterAlreadyExistsFault",
        ),
        (
            "an instance in a cluster that does not exist",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "orphan-1"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", "no-such-graph"),
            ],
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "a second instance with the same identifier",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "fault-graph-1"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", "fault-graph"),
            ],
            400,
            "DBInstanceAlreadyExists",
        ),
        (
            "describing a cluster that does not exist",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "no-such-graph")],
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "describing an instance that does not exist",
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "no-such-instance")],
            404,
            "DBInstanceNotFound",
        ),
        (
            "deleting an instance that does not exist",
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "no-such-instance"),
                ("SkipFinalSnapshot", "true"),
            ],
            404,
            "DBInstanceNotFound",
        ),
        (
            "deleting a cluster that does not exist",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "no-such-graph"),
                ("SkipFinalSnapshot", "true"),
            ],
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "deleting a cluster that does not exist, with no SkipFinalSnapshot",
            "DeleteDBCluster",
            &[("DBClusterIdentifier", "no-such-graph")],
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "deleting an instance that does not exist, with no SkipFinalSnapshot",
            "DeleteDBInstance",
            &[("DBInstanceIdentifier", "no-such-instance")],
            404,
            "DBInstanceNotFound",
        ),
        (
            "deleting an instance with deletion protection enabled",
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "fault-graph-protected"),
                ("SkipFinalSnapshot", "true"),
            ],
            400,
            "InvalidDBInstanceState",
        ),
        (
            "deleting a protected instance, with no SkipFinalSnapshot",
            "DeleteDBInstance",
            &[("DBInstanceIdentifier", "fault-graph-protected")],
            400,
            "InvalidDBInstanceState",
        ),
        (
            "deleting a cluster with deletion protection enabled, with no SkipFinalSnapshot",
            "DeleteDBCluster",
            &[("DBClusterIdentifier", "protected-graph")],
            400,
            "InvalidDBClusterStateFault",
        ),
        (
            "deleting a cluster that still has members, with no SkipFinalSnapshot",
            "DeleteDBCluster",
            &[("DBClusterIdentifier", "fault-graph")],
            400,
            "InvalidDBClusterStateFault",
        ),
        (
            "deleting a cluster with deletion protection enabled",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "protected-graph"),
                ("SkipFinalSnapshot", "true"),
            ],
            400,
            "InvalidDBClusterStateFault",
        ),
        (
            "deleting a cluster that still has members",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "fault-graph"),
                ("SkipFinalSnapshot", "true"),
            ],
            400,
            "InvalidDBClusterStateFault",
        ),
    ];

    for (case, action, params, expected_status, expected_code) in cases {
        let reply = neptune.call(action, params).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (*expected_status, *expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got body {}",
            reply.body
        );
        assert!(
            !reply.error_message().is_empty(),
            "case {case:?}: the fault must carry a message, got body {}",
            reply.body
        );
    }

    // Every refusal above left state untouched: the protected resources are still there.
    for (case, action, params) in [
        (
            "the protected cluster",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "protected-graph")][..],
        ),
        (
            "the protected instance",
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "fault-graph-protected")][..],
        ),
    ] {
        let reply = neptune.call(action, params).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a refused delete must leave the resource in state, body {}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// Documented input constraints the model cannot express.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn documented_input_constraints_are_refused_with_a_message_naming_the_member() {
    let neptune = Neptune::new();
    let seeded = neptune
        .call(
            "CreateDBCluster",
            &[("DBClusterIdentifier", "rules-graph"), ("Engine", "neptune")],
        )
        .await;
    assert_eq!(seeded.status, 200, "seeding: body {}", seeded.body);

    let cases: &[RefusalRow] = &[
        (
            "an identifier that does not start with a letter",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "9graph"), ("Engine", "neptune")],
            "first character must be a letter",
        ),
        (
            "an identifier ending with a hyphen",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "graph-"), ("Engine", "neptune")],
            "cannot end with a hyphen",
        ),
        (
            "an identifier with two consecutive hyphens",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "gr--aph"), ("Engine", "neptune")],
            "two consecutive hyphens",
        ),
        (
            "an identifier with a character outside the grammar",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "graph_one"), ("Engine", "neptune")],
            "letters, numbers, or hyphens",
        ),
        (
            "an identifier one character past the published maximum",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", LONG_IDENTIFIER_64),
                ("Engine", "neptune"),
            ],
            "1 to 63",
        ),
        (
            "an engine the model does not list",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "mysql-graph"), ("Engine", "mysql")],
            "Engine",
        ),
        (
            "a backup retention period one below the published minimum",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "short-graph"),
                ("Engine", "neptune"),
                ("BackupRetentionPeriod", "0"),
            ],
            "BackupRetentionPeriod",
        ),
        (
            "a backup retention period one above the published maximum",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "long-graph"),
                ("Engine", "neptune"),
                ("BackupRetentionPeriod", "36"),
            ],
            "BackupRetentionPeriod",
        ),
        (
            "an instance identifier outside the grammar",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "-bad"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", "rules-graph"),
            ],
            "DBInstanceIdentifier",
        ),
        (
            "MaxRecords one below the published minimum",
            "DescribeDBClusters",
            &[("MaxRecords", "19")],
            "MaxRecords",
        ),
        (
            "MaxRecords one above the published maximum",
            "DescribeDBInstances",
            &[("MaxRecords", "101")],
            "MaxRecords",
        ),
        (
            "a filter name the API does not support",
            "DescribeDBClusters",
            &[
                ("Filters.Filter.1.Name", "db-instance-id"),
                ("Filters.Filter.1.Values.Value.1", "x"),
            ],
            "Unrecognized filter name",
        ),
        (
            "a delete that asks for a final snapshot without naming it",
            "DeleteDBCluster",
            &[("DBClusterIdentifier", "rules-graph")],
            "FinalDBSnapshotIdentifier",
        ),
    ];

    for (case, action, params, fragment) in cases {
        let reply = neptune.call(action, params).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got body {}",
            reply.body
        );
        assert!(
            reply.error_message().contains(fragment),
            "case {case:?}: expected the message to name {fragment:?}, got {:?}",
            reply.error_message()
        );
    }

    // The published bounds themselves are accepted.
    for (case, value) in [("the minimum", "20"), ("the maximum", "100")] {
        let reply = neptune
            .call("DescribeDBClusters", &[("MaxRecords", value)])
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: MaxRecords={value} is a published bound and must be accepted, body {}",
            reply.body
        );
    }
}

/// 64 characters: one past "Must contain from 1 to 63 letters, numbers, or hyphens".
const LONG_IDENTIFIER_64: &str =
    "aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgggg";

// ---------------------------------------------------------------------------
// Paging and filtering.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_pages_with_max_records_and_resumes_from_the_marker() {
    let neptune = Neptune::new();

    // 45 clusters: more than two pages at the published minimum page size.
    let identifiers: Vec<String> = (0..45).map(|n| format!("page-graph-{n:03}")).collect();
    for identifier in &identifiers {
        let reply = neptune
            .call(
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier.as_str()),
                    ("Engine", "neptune"),
                ],
            )
            .await;
        assert_eq!(reply.status, 200, "seeding {identifier}: body {}", reply.body);
    }
    let mut sorted = identifiers.clone();
    sorted.sort();

    for page_size in ["20", "100"] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params: Vec<(&str, &str)> = vec![("MaxRecords", page_size)];
            if let Some(from) = marker.as_deref() {
                params.push(("Marker", from));
            }
            let reply = neptune.call("DescribeDBClusters", &params).await;
            assert_eq!(
                reply.status, 200,
                "case MaxRecords={page_size:?}: body {}",
                reply.body
            );
            let ids = cluster_ids(&reply);
            let expected_len = page_size.parse::<usize>().unwrap().min(sorted.len() - seen.len());
            assert_eq!(
                ids.len(),
                expected_len,
                "case MaxRecords={page_size:?}: expected a page of {expected_len}, got {:?}",
                ids
            );
            seen.extend(ids);
            pages += 1;
            marker = Some(reply.field(&["DescribeDBClustersResult", "Marker"]))
                .filter(|value| !value.is_empty());
            if marker.is_none() {
                break;
            }
            assert!(
                pages < 10,
                "case MaxRecords={page_size:?}: paging did not terminate"
            );
        }
        assert_eq!(
            seen, sorted,
            "case MaxRecords={page_size:?}: paging must visit every cluster exactly once, in order"
        );
        let expected_pages = sorted.len().div_ceil(page_size.parse::<usize>().unwrap());
        assert_eq!(
            pages, expected_pages,
            "case MaxRecords={page_size:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn describe_filters_by_the_documented_filter_names() {
    let neptune = Neptune::new();

    let mut cluster_arns: BTreeMap<&str, String> = BTreeMap::new();
    for cluster in ["filter-graph-a", "filter-graph-b"] {
        let reply = neptune
            .call(
                "CreateDBCluster",
                &[("DBClusterIdentifier", cluster), ("Engine", "neptune")],
            )
            .await;
        assert_eq!(reply.status, 200, "seeding {cluster}: body {}", reply.body);
        cluster_arns.insert(
            cluster,
            reply.field(&["CreateDBClusterResult", "DBCluster", "DBClusterArn"]),
        );
        for suffix in ["1", "2"] {
            let instance = format!("{cluster}-{suffix}");
            let reply = neptune
                .call(
                    "CreateDBInstance",
                    &[
                        ("DBInstanceIdentifier", instance.as_str()),
                        ("DBInstanceClass", "db.r5.large"),
                        ("Engine", "neptune"),
                        ("DBClusterIdentifier", cluster),
                    ],
                )
                .await;
            assert_eq!(reply.status, 200, "seeding {instance}: body {}", reply.body);
        }
    }

    // (case, the filter value, the instances it must select)
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "by cluster identifier",
            "filter-graph-a".to_string(),
            vec!["filter-graph-a-1".to_string(), "filter-graph-a-2".to_string()],
        ),
        (
            "by cluster identifier in another letter case",
            "FILTER-GRAPH-B".to_string(),
            vec!["filter-graph-b-1".to_string(), "filter-graph-b-2".to_string()],
        ),
        (
            "by cluster ARN",
            cluster_arns["filter-graph-b"].clone(),
            vec!["filter-graph-b-1".to_string(), "filter-graph-b-2".to_string()],
        ),
        (
            "by a cluster that owns nothing here",
            "no-such-graph".to_string(),
            Vec::new(),
        ),
    ];

    for (case, value, expected) in &cases {
        let reply = neptune
            .call(
                "DescribeDBInstances",
                &[
                    ("Filters.Filter.1.Name", "db-cluster-id"),
                    ("Filters.Filter.1.Values.Value.1", value.as_str()),
                ],
            )
            .await;
        assert_eq!(reply.status, 200, "case {case:?}: body {}", reply.body);
        assert_eq!(
            &instance_ids(&reply, "DescribeDBInstancesResult"),
            expected,
            "case {case:?}: unexpected selection"
        );
    }

    // The `engine` filter accepts the engine name and rejects everything else.
    for (case, value, expected_len) in [("the matching engine", "neptune", 4), ("another engine", "mysql", 0)] {
        let reply = neptune
            .call(
                "DescribeDBInstances",
                &[
                    ("Filters.Filter.1.Name", "engine"),
                    ("Filters.Filter.1.Values.Value.1", value),
                ],
            )
            .await;
        assert_eq!(reply.status, 200, "case {case:?}: body {}", reply.body);
        assert_eq!(
            instance_ids(&reply, "DescribeDBInstancesResult").len(),
            expected_len,
            "case {case:?}: unexpected selection size"
        );
    }
}

#[tokio::test]
async fn a_cluster_refuses_more_instances_than_it_can_hold() {
    let neptune = Neptune::new();
    let seeded = neptune
        .call(
            "CreateDBCluster",
            &[("DBClusterIdentifier", "full-graph"), ("Engine", "neptune")],
        )
        .await;
    assert_eq!(seeded.status, 200, "seeding: body {}", seeded.body);

    // A Neptune cluster holds a writer plus up to 15 read replicas, so the 17th instance
    // must be refused with the modelled quota fault rather than stored.
    for n in 1..=16 {
        let identifier = format!("full-graph-{n:02}");
        let reply = neptune
            .call(
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "neptune"),
                    ("DBClusterIdentifier", "full-graph"),
                ],
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {identifier:?}: member {n} of 16 must be accepted, body {}",
            reply.body
        );
    }
    let refused = neptune
        .call(
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "full-graph-17"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", "full-graph"),
            ],
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "InstanceQuotaExceeded"),
        "case \"the 17th instance\": expected the modelled quota fault, got body {}",
        refused.body
    );
    let listed = neptune
        .call(
            "DescribeDBInstances",
            &[
                ("Filters.Filter.1.Name", "db-cluster-id"),
                ("Filters.Filter.1.Values.Value.1", "full-graph"),
                ("MaxRecords", "100"),
            ],
        )
        .await;
    assert_eq!(
        instance_ids(&listed, "DescribeDBInstancesResult").len(),
        16,
        "case \"the 17th instance\": a refused create must store nothing, body {}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// The paths this slice cannot serve faithfully answer 501, never a fake success.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_that_needs_a_final_snapshot_answers_501_and_keeps_the_resource() {
    let neptune = Neptune::new();
    let seeded = neptune
        .call(
            "CreateDBCluster",
            &[("DBClusterIdentifier", "snap-graph"), ("Engine", "neptune")],
        )
        .await;
    assert_eq!(seeded.status, 200, "seeding: body {}", seeded.body);

    let refused = neptune
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "snap-graph"),
                ("SkipFinalSnapshot", "false"),
                ("FinalDBSnapshotIdentifier", "snap-graph-final"),
            ],
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (501, "NotImplemented"),
        "a delete that must take a snapshot this emulator cannot create must answer 501, body {}",
        refused.body
    );

    let still_there = neptune
        .call(
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "snap-graph")],
        )
        .await;
    assert_eq!(
        still_there.status, 200,
        "a refused delete must leave the cluster in state, body {}",
        still_there.body
    );
}
