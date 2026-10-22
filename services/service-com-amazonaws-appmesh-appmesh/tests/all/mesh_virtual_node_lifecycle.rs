// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the mesh -> virtual-node slice of the App Mesh emulator.
//!
//! Every test drives real restJson1 requests through the same `MultiServiceRouter`
//! the production router serves, so what is asserted here is the wire behaviour a
//! client sees — not a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. A create persists the resource and the create, describe and list responses are
//!    all rendered from that one stored record (same arn, uid, version, timestamps and
//!    spec).
//! 2. A resource is created at `version` 1 with `createdAt == lastUpdatedAt`; an update
//!    increments `version` and moves `lastUpdatedAt` while `arn`, `uid` and `createdAt`
//!    stay fixed.
//! 3. A name already taken conflicts (409), except for a replay of the create's
//!    `@idempotencyToken`, which returns the resource the first call made.
//! 4. A list returns items in name order, caps a page at `limit`, and resumes exactly
//!    where its `nextToken` says.
//! 5. A mesh cannot be deleted while it still contains a child resource (409); once the
//!    child is gone the delete succeeds and the mesh is no longer readable (404).
//! 6. A reference to a resource that does not exist is a 404, and a value outside a
//!    modelled bound is refused with a 400 before any handler runs.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the tests sign for. Deliberately NOT the emulator's fallback
/// (`us-east-1`), so an ARN built from the request's SigV4 scope is distinguishable
/// from one built from the default.
const REGION: &str = "us-west-2";

/// The App Mesh API's URI prefix, from the model's `@http` bindings.
const PREFIX: &str = "/v20190125";

/// One App Mesh emulator, served exactly the way the production router serves it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        // The SINGLE shared engine, as `router/src/lib.rs` builds it: every request
        // this emulator serves observes the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_appmesh_appmesh::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Make one call and return its HTTP status and parsed JSON body.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match body {
            Some(value) => {
                Bytes::from(serde_json::to_vec(&value).expect("the request body must serialize"))
            }
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("appmesh.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            // The SigV4 credential scope is both how the router picks this service and
            // how a handler learns which region to mint ARNs in.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTAPPMESH/20260101/{REGION}/appmesh/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(payload)
            .expect("the request must build");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                json!({ "unparsed_body": String::from_utf8_lossy(&bytes) })
            })
        };
        (status, parsed)
    }

    /// `PUT /v20190125/meshes`. `mesh_name` is named at the call site only to keep the
    /// request readable; the wire value is the `meshName` member of `body`.
    async fn create_mesh(&self, mesh_name: &str, body: Value) -> (u16, Value) {
        debug_assert_eq!(body["meshName"], json!(mesh_name));
        self.call("PUT", &format!("{PREFIX}/meshes"), Some(body)).await
    }

    async fn describe_mesh(&self, name: &str) -> (u16, Value) {
        self.call("GET", &format!("{PREFIX}/meshes/{name}"), None).await
    }

    async fn delete_mesh(&self, name: &str) -> (u16, Value) {
        self.call("DELETE", &format!("{PREFIX}/meshes/{name}"), None)
            .await
    }

    async fn list_meshes(&self, query: &str) -> (u16, Value) {
        self.call("GET", &format!("{PREFIX}/meshes{query}"), None).await
    }

    async fn create_virtual_node(&self, mesh: &str, body: Value) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("{PREFIX}/meshes/{mesh}/virtualNodes"),
            Some(body),
        )
        .await
    }

    async fn describe_virtual_node(&self, mesh: &str, node: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("{PREFIX}/meshes/{mesh}/virtualNodes/{node}"),
            None,
        )
        .await
    }

    async fn delete_virtual_node(&self, mesh: &str, node: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("{PREFIX}/meshes/{mesh}/virtualNodes/{node}"),
            None,
        )
        .await
    }

    async fn list_virtual_nodes(&self, mesh: &str, query: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("{PREFIX}/meshes/{mesh}/virtualNodes{query}"),
            None,
        )
        .await
    }
}

/// `arn:aws:appmesh:<region>:<account>:mesh/<meshName>` — the ARN shape the service
/// mints for a mesh, with the canonical emulator account id.
fn expected_mesh_arn(mesh: &str) -> String {
    format!(
        "arn:aws:appmesh:{REGION}:{}:mesh/{mesh}",
        engine::emulator_account_id()
    )
}

/// `arn:.../mesh/<meshName>/virtualNode/<virtualNodeName>`.
fn expected_virtual_node_arn(mesh: &str, node: &str) -> String {
    format!(
        "arn:aws:appmesh:{REGION}:{}:mesh/{mesh}/virtualNode/{node}",
        engine::emulator_account_id()
    )
}

/// A hyphenated UUID: 36 characters, 5 hyphen-separated hex groups. `ResourceMetadata$uid`
/// is "the unique identifier for the resource".
fn looks_like_a_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Assert the stored-metadata invariants every App Mesh resource shares.
fn assert_fresh_metadata(case: &str, metadata: &Value, expected_arn: &str) {
    assert_eq!(
        metadata["arn"],
        json!(expected_arn),
        "case {case}: expected arn {expected_arn}, got {:?}",
        metadata["arn"]
    );
    assert_eq!(
        metadata["version"],
        json!(1),
        "case {case}: a created resource is at version 1, got {:?}",
        metadata["version"]
    );
    let uid = metadata["uid"].as_str().unwrap_or_default();
    assert!(
        looks_like_a_uuid(uid),
        "case {case}: expected metadata.uid to be a hyphenated UUID, got {uid:?}"
    );
    assert_eq!(
        metadata["createdAt"], metadata["lastUpdatedAt"],
        "case {case}: a created resource has createdAt == lastUpdatedAt, got {:?} and {:?}",
        metadata["createdAt"], metadata["lastUpdatedAt"]
    );
    let account = json!(engine::emulator_account_id());
    assert_eq!(
        metadata["meshOwner"], account,
        "case {case}: expected meshOwner {account:?}, got {:?}",
        metadata["meshOwner"]
    );
    assert_eq!(
        metadata["resourceOwner"], account,
        "case {case}: expected resourceOwner {account:?}, got {:?}",
        metadata["resourceOwner"]
    );
}

/// RULE 1 for meshes, over the whole class of `MeshSpec`s and of names the model
/// admits: whatever is created is what describe and list report.
#[tokio::test]
async fn create_describe_and_list_report_the_same_stored_mesh() {
    // Rows vary along what the rule generalises over: whether a spec is sent at all,
    // which member of `MeshSpec` it sets, the enum value inside it, and the length of
    // the name. `EgressFilterType` is ALLOW_ALL | DROP_ALL and `IpPreference` is
    // IPv4_ONLY | IPv4_PREFERRED | IPv6_ONLY | IPv6_PREFERRED — smithy_query(query="shape",
    // shape="EgressFilterType") and the generated `IP_PREFERENCE_VALUES`.
    let cases: Vec<(String, Option<Value>)> = vec![
        // The shortest name `ResourceName` admits (length 1..255).
        ("a".to_string(), None),
        ("demo".to_string(), Some(json!({}))),
        (
            "egress-allow".to_string(),
            Some(json!({"egressFilter": {"type": "ALLOW_ALL"}})),
        ),
        (
            "egress-drop".to_string(),
            Some(json!({"egressFilter": {"type": "DROP_ALL"}})),
        ),
        (
            "discovery-ipv6".to_string(),
            Some(json!({"serviceDiscovery": {"ipPreference": "IPv6_PREFERRED"}})),
        ),
        (
            "discovery-ipv4-only".to_string(),
            Some(json!({"serviceDiscovery": {"ipPreference": "IPv4_ONLY"}})),
        ),
        // The longest name `ResourceName` admits.
        ("z".repeat(255), Some(json!({"egressFilter": {"type": "DROP_ALL"}}))),
    ];

    let emulator = Emulator::new();
    let mut uids = BTreeSet::new();

    for (name, spec) in &cases {
        let case = format!("mesh {name:?} spec {spec:?}");
        let mut request = json!({"meshName": name});
        if let Some(spec) = spec {
            request["spec"] = spec.clone();
        }

        let (status, created) = emulator.create_mesh(name, request).await;
        assert_eq!(status, 200, "case {case}: CreateMesh answered {status}: {created:?}");
        assert_eq!(
            created["meshName"],
            json!(name),
            "case {case}: expected meshName {name:?}, got {:?}",
            created["meshName"]
        );
        assert_eq!(
            created["status"]["status"],
            json!("ACTIVE"),
            "case {case}: expected status ACTIVE, got {:?}",
            created["status"]["status"]
        );
        // An absent `spec` is stored as the empty `MeshSpec`; a sent one round-trips
        // member for member.
        assert_eq!(
            created["spec"],
            spec.clone().unwrap_or_else(|| json!({})),
            "case {case}: expected the submitted spec back, got {:?}",
            created["spec"]
        );
        assert_fresh_metadata(&case, &created["metadata"], &expected_mesh_arn(name));

        let uid = created["metadata"]["uid"].as_str().unwrap_or_default().to_string();
        assert!(
            uids.insert(uid.clone()),
            "case {case}: uid {uid} was already minted for another mesh"
        );

        // RULE 1: the describe is the stored record, field for field.
        let (status, described) = emulator.describe_mesh(name).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeMesh answered {status}: {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case}: DescribeMesh must return the stored record; expected {created:?}, got {described:?}"
        );
    }

    // ... and the list is built from the same records, in name order.
    let (status, listed) = emulator.list_meshes("").await;
    assert_eq!(status, 200, "ListMeshes answered {status}: {listed:?}");
    let meshes = listed["meshes"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        meshes.len(),
        cases.len(),
        "ListMeshes must list every created mesh; got {listed:?}"
    );
    for (name, _) in &cases {
        let (_, described) = emulator.describe_mesh(name).await;
        let reference = meshes
            .iter()
            .find(|entry| entry["meshName"] == json!(name))
            .unwrap_or_else(|| panic!("case mesh {name:?}: not listed by ListMeshes: {listed:?}"));
        for field in ["arn", "version", "createdAt", "lastUpdatedAt", "meshOwner", "resourceOwner"] {
            assert_eq!(
                reference[field], described["metadata"][field],
                "case mesh {name:?}: MeshRef.{field} must come from the stored record; expected {:?}, got {:?}",
                described["metadata"][field], reference[field]
            );
        }
    }
}

/// RULE 1 for virtual nodes, over the class of `VirtualNodeSpec`s — including every
/// value of the `PortProtocol` enum and both published bounds of `PortNumber`.
#[tokio::test]
async fn create_describe_and_list_report_the_same_stored_virtual_node() {
    // `PortProtocol` is http | tcp | http2 | grpc and `PortNumber` has range 1..65535 —
    // smithy_query(query="shape", shape="PortProtocol,PortNumber").
    let protocols = ["http", "tcp", "http2", "grpc"];
    let mut cases: Vec<(String, Value)> = vec![
        // A virtual node with no ingress needs no spec members at all.
        ("bare".to_string(), json!({})),
        (
            "dns-only".to_string(),
            json!({"serviceDiscovery": {"dns": {"hostname": "orders.local"}}}),
        ),
        (
            "backend".to_string(),
            json!({"backends": [{"virtualService": {"virtualServiceName": "orders.local"}}]}),
        ),
        // Each published bound of `PortNumber`.
        (
            "port-min".to_string(),
            json!({"listeners": [{"portMapping": {"port": 1, "protocol": "http"}}]}),
        ),
        (
            "port-max".to_string(),
            json!({"listeners": [{"portMapping": {"port": 65535, "protocol": "http"}}]}),
        ),
    ];
    for protocol in protocols {
        cases.push((
            format!("listener-{protocol}"),
            json!({
                "serviceDiscovery": {"dns": {"hostname": format!("{protocol}.local")}},
                "listeners": [{"portMapping": {"port": 8080, "protocol": protocol}}],
            }),
        ));
    }

    let emulator = Emulator::new();
    let mesh = "lifecycle";
    let (status, _) = emulator.create_mesh(mesh, json!({"meshName": mesh})).await;
    assert_eq!(status, 200, "the parent mesh must be creatable, got {status}");

    let mut uids = BTreeSet::new();
    for (node, spec) in &cases {
        let case = format!("virtual node {node:?} spec {spec:?}");
        let (status, created) = emulator
            .create_virtual_node(
                mesh,
                json!({"virtualNodeName": node, "spec": spec}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateVirtualNode answered {status}: {created:?}"
        );
        assert_eq!(
            created["virtualNodeName"],
            json!(node),
            "case {case}: expected virtualNodeName {node:?}, got {:?}",
            created["virtualNodeName"]
        );
        assert_eq!(
            created["meshName"],
            json!(mesh),
            "case {case}: expected meshName {mesh:?}, got {:?}",
            created["meshName"]
        );
        assert_eq!(
            created["status"]["status"],
            json!("ACTIVE"),
            "case {case}: expected status ACTIVE, got {:?}",
            created["status"]["status"]
        );
        assert_eq!(
            created["spec"], *spec,
            "case {case}: expected the submitted spec back, got {:?}",
            created["spec"]
        );
        assert_fresh_metadata(
            &case,
            &created["metadata"],
            &expected_virtual_node_arn(mesh, node),
        );
        let uid = created["metadata"]["uid"].as_str().unwrap_or_default().to_string();
        assert!(
            uids.insert(uid.clone()),
            "case {case}: uid {uid} was already minted for another virtual node"
        );

        let (status, described) = emulator.describe_virtual_node(mesh, node).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeVirtualNode answered {status}: {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case}: DescribeVirtualNode must return the stored record; expected {created:?}, got {described:?}"
        );
    }

    let (status, listed) = emulator.list_virtual_nodes(mesh, "").await;
    assert_eq!(status, 200, "ListVirtualNodes answered {status}: {listed:?}");
    let nodes = listed["virtualNodes"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        nodes.len(),
        cases.len(),
        "ListVirtualNodes must list every created node; got {listed:?}"
    );
    for (node, _) in &cases {
        let reference = nodes
            .iter()
            .find(|entry| entry["virtualNodeName"] == json!(node))
            .unwrap_or_else(|| panic!("case node {node:?}: not listed: {listed:?}"));
        assert_eq!(
            reference["arn"],
            json!(expected_virtual_node_arn(mesh, node)),
            "case node {node:?}: expected arn {}, got {:?}",
            expected_virtual_node_arn(mesh, node),
            reference["arn"]
        );
        assert_eq!(
            reference["meshName"],
            json!(mesh),
            "case node {node:?}: expected meshName {mesh:?}, got {:?}",
            reference["meshName"]
        );
    }
}

/// RULE 3: a taken name conflicts, and a replay of the create's idempotency token
/// returns the resource the first call made — asserted for both resource kinds.
#[tokio::test]
async fn a_duplicate_create_conflicts_but_an_idempotency_token_replay_returns_the_first_resource() {
    let emulator = Emulator::new();

    let (status, first) = emulator
        .create_mesh("twice", json!({"meshName": "twice", "clientToken": "token-mesh"}))
        .await;
    assert_eq!(status, 200, "case mesh first create: answered {status}: {first:?}");

    let (status, conflict) = emulator
        .create_mesh("twice", json!({"meshName": "twice"}))
        .await;
    assert_eq!(
        status, 409,
        "case mesh duplicate without a token: expected 409, got {status}: {conflict:?}"
    );
    assert_eq!(
        conflict["__type"],
        json!("ConflictException"),
        "case mesh duplicate without a token: expected a ConflictException, got {conflict:?}"
    );

    let (status, replay) = emulator
        .create_mesh("twice", json!({"meshName": "twice", "clientToken": "token-mesh"}))
        .await;
    assert_eq!(
        status, 200,
        "case mesh token replay: expected 200, got {status}: {replay:?}"
    );
    assert_eq!(
        replay, first,
        "case mesh token replay: expected the first mesh {first:?}, got {replay:?}"
    );

    let node_request = json!({
        "virtualNodeName": "node",
        "spec": {},
        "clientToken": "token-node",
    });
    let (status, first_node) = emulator
        .create_virtual_node("twice", node_request.clone())
        .await;
    assert_eq!(
        status, 200,
        "case node first create: answered {status}: {first_node:?}"
    );

    let (status, conflict) = emulator
        .create_virtual_node("twice", json!({"virtualNodeName": "node", "spec": {}}))
        .await;
    assert_eq!(
        status, 409,
        "case node duplicate without a token: expected 409, got {status}: {conflict:?}"
    );

    let (status, replay) = emulator.create_virtual_node("twice", node_request).await;
    assert_eq!(
        status, 200,
        "case node token replay: expected 200, got {status}: {replay:?}"
    );
    assert_eq!(
        replay, first_node,
        "case node token replay: expected the first node {first_node:?}, got {replay:?}"
    );

    // The model documents ConflictException for "a client token that was used for a
    // previous ... call with different specifications", so the SAME token with a
    // DIFFERENT spec is a conflict, not a replay. One row per resource kind.
    let (status, conflict) = emulator
        .create_mesh(
            "twice",
            json!({
                "meshName": "twice",
                "clientToken": "token-mesh",
                "spec": {"egressFilter": {"type": "DROP_ALL"}},
            }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case mesh token reused with a different spec: expected 409, got {status}: {conflict:?}"
    );
    let (status, conflict) = emulator
        .create_virtual_node(
            "twice",
            json!({
                "virtualNodeName": "node",
                "clientToken": "token-node",
                "spec": {"listeners": [{"portMapping": {"port": 80, "protocol": "http"}}]},
            }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case node token reused with a different spec: expected 409, got {status}: {conflict:?}"
    );
}

/// The store is bounded: past each quota a create is refused with the modelled
/// `LimitExceededException`, and a list never renders more than one page.
#[tokio::test]
async fn the_store_is_bounded_by_a_quota_and_a_maximum_page_size() {
    // The ceilings the service enforces, matching App Mesh's published adjustable
    // defaults: 15 meshes per account, 200 virtual nodes per mesh. The page ceiling is
    // the model's own `limit` maximum of 100.
    const MAX_MESHES: usize = 15;
    const MAX_NODES: usize = 200;
    const MAX_PAGE: usize = 100;

    let emulator = Emulator::new();
    for index in 0..MAX_MESHES {
        let name = format!("mesh-{index:03}");
        let (status, body) = emulator.create_mesh(&name, json!({"meshName": name})).await;
        assert_eq!(
            status, 200,
            "case mesh {index} of the quota: answered {status}: {body:?}"
        );
    }
    let over = format!("mesh-{MAX_MESHES:03}");
    let (status, refused) = emulator.create_mesh(&over, json!({"meshName": over})).await;
    assert_eq!(
        status, 400,
        "case one mesh past the quota: expected 400, got {status}: {refused:?}"
    );
    assert_eq!(
        refused["__type"],
        json!("LimitExceededException"),
        "case one mesh past the quota: expected a LimitExceededException, got {refused:?}"
    );

    // Filling a mesh to its virtual-node quota also takes the store past one page, so
    // the same run shows both bounds.
    let mesh = "mesh-000";
    for index in 0..MAX_NODES {
        let node = format!("node-{index:03}");
        let (status, body) = emulator
            .create_virtual_node(mesh, json!({"virtualNodeName": node, "spec": {}}))
            .await;
        assert_eq!(
            status, 200,
            "case node {index} of the quota: answered {status}: {body:?}"
        );
    }
    let over = format!("node-{MAX_NODES:03}");
    let (status, refused) = emulator
        .create_virtual_node(mesh, json!({"virtualNodeName": over, "spec": {}}))
        .await;
    assert_eq!(
        status, 400,
        "case one virtual node past the quota: expected 400, got {status}: {refused:?}"
    );
    assert_eq!(
        refused["__type"],
        json!("LimitExceededException"),
        "case one virtual node past the quota: expected a LimitExceededException, got {refused:?}"
    );

    // A list with NO `limit` still returns at most one page, and says there is more.
    let (status, page) = emulator.list_virtual_nodes(mesh, "").await;
    assert_eq!(status, 200, "case unlimited list: answered {status}: {page:?}");
    let items = page["virtualNodes"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        MAX_PAGE,
        "case unlimited list: a page holds at most {MAX_PAGE} items, got {}",
        items.len()
    );
    assert!(
        page["nextToken"].is_string(),
        "case unlimited list: a truncated page must carry a nextToken, got {page:?}"
    );
}

/// RULE 5: a mesh that still contains a child resource cannot be deleted.
#[tokio::test]
async fn delete_mesh_refuses_while_a_child_remains_and_succeeds_once_it_is_gone() {
    let emulator = Emulator::new();
    let mesh = "demo";
    let (status, _) = emulator.create_mesh(mesh, json!({"meshName": mesh})).await;
    assert_eq!(status, 200, "the parent mesh must be creatable, got {status}");

    let (status, node) = emulator
        .create_virtual_node(mesh, json!({"virtualNodeName": "node1", "spec": {}}))
        .await;
    assert_eq!(status, 200, "case create child: answered {status}: {node:?}");

    let (status, refused) = emulator.delete_mesh(mesh).await;
    assert_eq!(
        status, 409,
        "case delete with a child: expected 409, got {status}: {refused:?}"
    );
    assert_eq!(
        refused["__type"],
        json!("ResourceInUseException"),
        "case delete with a child: expected a ResourceInUseException, got {refused:?}"
    );

    // Refused means refused: the mesh is still there, unchanged.
    let (status, still_there) = emulator.describe_mesh(mesh).await;
    assert_eq!(
        status, 200,
        "case describe after a refused delete: expected 200, got {status}: {still_there:?}"
    );

    let (status, deleted_node) = emulator.delete_virtual_node(mesh, "node1").await;
    assert_eq!(
        status, 200,
        "case delete child: expected 200, got {status}: {deleted_node:?}"
    );
    assert_eq!(
        deleted_node, node,
        "case delete child: DeleteVirtualNode must return the last-known record; expected {node:?}, got {deleted_node:?}"
    );

    let (status, gone) = emulator.describe_virtual_node(mesh, "node1").await;
    assert_eq!(
        status, 404,
        "case describe a deleted child: expected 404, got {status}: {gone:?}"
    );
    let (status, empty) = emulator.list_virtual_nodes(mesh, "").await;
    assert_eq!(status, 200, "case list after child delete: got {status}: {empty:?}");
    assert_eq!(
        empty["virtualNodes"],
        json!([]),
        "case list after child delete: expected no nodes, got {empty:?}"
    );

    let (status, deleted_mesh) = emulator.delete_mesh(mesh).await;
    assert_eq!(
        status, 200,
        "case delete empty mesh: expected 200, got {status}: {deleted_mesh:?}"
    );
    assert_eq!(
        deleted_mesh["meshName"],
        json!(mesh),
        "case delete empty mesh: expected the deleted mesh back, got {deleted_mesh:?}"
    );

    let (status, gone) = emulator.describe_mesh(mesh).await;
    assert_eq!(
        status, 404,
        "case describe a deleted mesh: expected 404, got {status}: {gone:?}"
    );
    let (status, listed) = emulator.list_meshes("").await;
    assert_eq!(status, 200, "case list after mesh delete: got {status}: {listed:?}");
    assert_eq!(
        listed["meshes"],
        json!([]),
        "case list after mesh delete: expected no meshes, got {listed:?}"
    );
}

/// RULE 4: pagination, over every page size from the minimum to beyond the item count.
#[tokio::test]
async fn a_list_caps_each_page_at_limit_and_resumes_where_next_token_says() {
    let emulator = Emulator::new();
    // Created out of order on purpose: the list must come back in name order whatever
    // order the creates arrived in.
    let names = ["mesh-d", "mesh-a", "mesh-g", "mesh-c", "mesh-f", "mesh-b", "mesh-e"];
    for name in names {
        let (status, body) = emulator.create_mesh(name, json!({"meshName": name})).await;
        assert_eq!(status, 200, "case setup {name}: answered {status}: {body:?}");
    }
    let mut expected: Vec<&str> = names.to_vec();
    expected.sort_unstable();

    // 1 and 100 are the published bounds of `ListMeshesLimit` (range 1..100).
    for limit in [1_usize, 2, 3, 7, 8, 100] {
        let case = format!("limit {limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            let query = match &token {
                Some(token) => format!("?limit={limit}&nextToken={token}"),
                None => format!("?limit={limit}"),
            };
            let (status, page) = emulator.list_meshes(&query).await;
            assert_eq!(status, 200, "case {case}: ListMeshes answered {status}: {page:?}");
            let items = page["meshes"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= limit,
                "case {case}: a page may hold at most {limit} items, got {}",
                items.len()
            );
            for item in &items {
                seen.push(item["meshName"].as_str().unwrap_or_default().to_string());
            }
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging must yield every mesh exactly once in name order; expected {expected:?}, got {seen:?}"
        );
    }

    // The same rule for the nested list.
    let mesh = "mesh-a";
    let nodes = ["n3", "n1", "n2"];
    for node in nodes {
        let (status, body) = emulator
            .create_virtual_node(mesh, json!({"virtualNodeName": node, "spec": {}}))
            .await;
        assert_eq!(status, 200, "case setup node {node}: answered {status}: {body:?}");
    }
    let (status, page) = emulator.list_virtual_nodes(mesh, "?limit=2").await;
    assert_eq!(status, 200, "case node page 1: answered {status}: {page:?}");
    let first: Vec<String> = page["virtualNodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["virtualNodeName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        first,
        vec!["n1".to_string(), "n2".to_string()],
        "case node page 1: expected the first two nodes in name order, got {first:?}"
    );
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case node page 1: expected a nextToken, got {page:?}"))
        .to_string();
    let (status, page) = emulator
        .list_virtual_nodes(mesh, &format!("?limit=2&nextToken={token}"))
        .await;
    assert_eq!(status, 200, "case node page 2: answered {status}: {page:?}");
    let second: Vec<String> = page["virtualNodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["virtualNodeName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        second,
        vec!["n3".to_string()],
        "case node page 2: expected the remaining node, got {second:?}"
    );
    assert_eq!(
        page["nextToken"],
        Value::Null,
        "case node page 2: the last page carries no nextToken, got {page:?}"
    );
}

/// RULE 2: an update is a new revision of the SAME resource.
#[tokio::test]
async fn an_update_increments_the_version_and_moves_only_last_updated_at() {
    let emulator = Emulator::new();
    let mesh = "versioned";
    let (status, created) = emulator
        .create_mesh(
            mesh,
            json!({"meshName": mesh, "spec": {"egressFilter": {"type": "DROP_ALL"}}}),
        )
        .await;
    assert_eq!(status, 200, "case create: answered {status}: {created:?}");

    // Each row is a further revision, so `version` must climb 2, 3, 4 ...
    let revisions = [
        json!({"egressFilter": {"type": "ALLOW_ALL"}}),
        json!({"serviceDiscovery": {"ipPreference": "IPv4_PREFERRED"}}),
        json!({}),
    ];
    for (index, spec) in revisions.iter().enumerate() {
        let case = format!("mesh revision {} spec {spec:?}", index + 2);
        let (status, updated) = emulator
            .call(
                "PUT",
                &format!("{PREFIX}/meshes/{mesh}"),
                Some(json!({"spec": spec})),
            )
            .await;
        assert_eq!(status, 200, "case {case}: UpdateMesh answered {status}: {updated:?}");
        let expected_version = json!(index as i64 + 2);
        assert_eq!(
            updated["metadata"]["version"], expected_version,
            "case {case}: expected version {expected_version:?}, got {:?}",
            updated["metadata"]["version"]
        );
        assert_eq!(
            updated["metadata"]["uid"], created["metadata"]["uid"],
            "case {case}: uid must not change on update; expected {:?}, got {:?}",
            created["metadata"]["uid"], updated["metadata"]["uid"]
        );
        assert_eq!(
            updated["metadata"]["createdAt"], created["metadata"]["createdAt"],
            "case {case}: createdAt must not change on update; expected {:?}, got {:?}",
            created["metadata"]["createdAt"], updated["metadata"]["createdAt"]
        );
        assert_eq!(
            updated["metadata"]["arn"], created["metadata"]["arn"],
            "case {case}: arn must not change on update; expected {:?}, got {:?}",
            created["metadata"]["arn"], updated["metadata"]["arn"]
        );
        assert!(
            updated["metadata"]["lastUpdatedAt"].as_f64()
                >= updated["metadata"]["createdAt"].as_f64(),
            "case {case}: lastUpdatedAt must not precede createdAt, got {:?}",
            updated["metadata"]
        );
        assert_eq!(
            updated["spec"], *spec,
            "case {case}: expected the new spec back, got {:?}",
            updated["spec"]
        );

        // RULE 1 again: the revision is what is stored, not just what was answered.
        let (_, described) = emulator.describe_mesh(mesh).await;
        assert_eq!(
            described, updated,
            "case {case}: DescribeMesh must report the updated record; expected {updated:?}, got {described:?}"
        );
    }

    let (status, node) = emulator
        .create_virtual_node(mesh, json!({"virtualNodeName": "n", "spec": {}}))
        .await;
    assert_eq!(status, 200, "case create node: answered {status}: {node:?}");
    let new_spec = json!({"listeners": [{"portMapping": {"port": 9000, "protocol": "grpc"}}]});
    let (status, updated) = emulator
        .call(
            "PUT",
            &format!("{PREFIX}/meshes/{mesh}/virtualNodes/n"),
            Some(json!({"spec": new_spec})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case node revision 2: UpdateVirtualNode answered {status}: {updated:?}"
    );
    assert_eq!(
        updated["metadata"]["version"],
        json!(2),
        "case node revision 2: expected version 2, got {:?}",
        updated["metadata"]["version"]
    );
    assert_eq!(
        updated["metadata"]["uid"], node["metadata"]["uid"],
        "case node revision 2: uid must not change; expected {:?}, got {:?}",
        node["metadata"]["uid"], updated["metadata"]["uid"]
    );
    assert_eq!(
        updated["spec"], new_spec,
        "case node revision 2: expected the new spec back, got {:?}",
        updated["spec"]
    );
    let (_, described) = emulator.describe_virtual_node(mesh, "n").await;
    assert_eq!(
        described, updated,
        "case node revision 2: DescribeVirtualNode must report the updated record; expected {updated:?}, got {described:?}"
    );

    // An update that changes nothing is not a new revision: a retry (what the
    // operation's `@idempotencyToken` exists for) must not advance the version again.
    // One row per resource kind, plus `UpdateMesh` with no `spec` at all.
    let retries: Vec<(&str, String, Option<Value>)> = vec![
        (
            "UpdateVirtualNode re-applying the stored spec",
            format!("{PREFIX}/meshes/{mesh}/virtualNodes/n"),
            Some(json!({"spec": new_spec, "clientToken": "retry"})),
        ),
        (
            "UpdateMesh re-applying the stored spec",
            format!("{PREFIX}/meshes/{mesh}"),
            Some(json!({"spec": revisions[2], "clientToken": "retry"})),
        ),
        (
            "UpdateMesh with no spec at all",
            format!("{PREFIX}/meshes/{mesh}"),
            Some(json!({})),
        ),
    ];
    for (case, path, body) in retries {
        let before = match case.starts_with("UpdateVirtualNode") {
            true => emulator.describe_virtual_node(mesh, "n").await.1,
            false => emulator.describe_mesh(mesh).await.1,
        };
        let (status, after) = emulator.call("PUT", &path, body).await;
        assert_eq!(status, 200, "case {case}: answered {status}: {after:?}");
        assert_eq!(
            after, before,
            "case {case}: a no-op update must record no revision; expected {before:?}, got {after:?}"
        );
    }
}

/// RULE 6, first half: every reference to a resource that is not there is a 404 —
/// never an empty success and never a 501.
#[tokio::test]
async fn a_reference_to_a_missing_resource_is_not_found() {
    let emulator = Emulator::new();
    // One mesh exists, so "missing child of an existing mesh" is covered too.
    let (status, _) = emulator
        .create_mesh("present", json!({"meshName": "present"}))
        .await;
    assert_eq!(status, 200, "the mesh must be creatable, got {status}");

    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("DescribeMesh", "GET", "/v20190125/meshes/absent", None),
        ("DeleteMesh", "DELETE", "/v20190125/meshes/absent", None),
        (
            "UpdateMesh",
            "PUT",
            "/v20190125/meshes/absent",
            Some(json!({"spec": {}})),
        ),
        (
            "CreateVirtualNode in a missing mesh",
            "PUT",
            "/v20190125/meshes/absent/virtualNodes",
            Some(json!({"virtualNodeName": "x", "spec": {}})),
        ),
        (
            "ListVirtualNodes of a missing mesh",
            "GET",
            "/v20190125/meshes/absent/virtualNodes",
            None,
        ),
        (
            "DescribeVirtualNode of a missing mesh",
            "GET",
            "/v20190125/meshes/absent/virtualNodes/x",
            None,
        ),
        (
            "DescribeVirtualNode of a missing node",
            "GET",
            "/v20190125/meshes/present/virtualNodes/absent",
            None,
        ),
        (
            "DeleteVirtualNode of a missing node",
            "DELETE",
            "/v20190125/meshes/present/virtualNodes/absent",
            None,
        ),
        (
            "UpdateVirtualNode of a missing node",
            "PUT",
            "/v20190125/meshes/present/virtualNodes/absent",
            Some(json!({"spec": {}})),
        ),
        // A mesh owned by another account is not visible either.
        (
            "DescribeMesh with a foreign meshOwner",
            "GET",
            "/v20190125/meshes/present?meshOwner=999999999999",
            None,
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, response) = emulator.call(method, path, body).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status}: {response:?}"
        );
        assert_eq!(
            response["__type"],
            json!("NotFoundException"),
            "case {case}: expected a NotFoundException, got {response:?}"
        );
    }
}

/// RULE 6, second half: a value one step outside a modelled bound is refused with a
/// 400 — the bounds themselves are exercised in the tests above.
#[tokio::test]
async fn a_value_outside_a_modelled_bound_is_refused() {
    let emulator = Emulator::new();
    let (status, _) = emulator
        .create_mesh("bounds", json!({"meshName": "bounds"}))
        .await;
    assert_eq!(status, 200, "the mesh must be creatable, got {status}");

    // `ResourceName` has length 1..255 and `ListMeshesLimit`/`ListVirtualNodesLimit`
    // range 1..100 — smithy_query(query="shape",
    // shape="ResourceName,ListMeshesLimit,ListVirtualNodesLimit").
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "meshName shorter than the minimum length",
            "PUT",
            format!("{PREFIX}/meshes"),
            Some(json!({"meshName": ""})),
        ),
        (
            "meshName longer than the maximum length",
            "PUT",
            format!("{PREFIX}/meshes"),
            Some(json!({"meshName": "x".repeat(256)})),
        ),
        (
            "ListMeshes limit below the minimum",
            "GET",
            format!("{PREFIX}/meshes?limit=0"),
            None,
        ),
        (
            "ListMeshes limit above the maximum",
            "GET",
            format!("{PREFIX}/meshes?limit=101"),
            None,
        ),
        (
            "ListVirtualNodes limit above the maximum",
            "GET",
            format!("{PREFIX}/meshes/bounds/virtualNodes?limit=101"),
            None,
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, response) = emulator.call(method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status}: {response:?}"
        );
    }
}

/// A `nextToken` is opaque: only one this service minted is a valid cursor, and the
/// one it minted resumes exactly where it says.
#[tokio::test]
async fn a_pagination_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    for name in ["cursor-a", "cursor-b"] {
        let (status, body) = emulator.create_mesh(name, json!({"meshName": name})).await;
        assert_eq!(status, 200, "case setup {name}: answered {status}: {body:?}");
    }
    let (status, _) = emulator
        .create_virtual_node("cursor-a", json!({"virtualNodeName": "n1", "spec": {}}))
        .await;
    assert_eq!(status, 200, "case setup node n1: answered {status}");
    let (status, _) = emulator
        .create_virtual_node("cursor-a", json!({"virtualNodeName": "n2", "spec": {}}))
        .await;
    assert_eq!(status, 200, "case setup node n2: answered {status}");

    // Rows span the ways a token can be wrong, plus one that is well-formed: a plain
    // resource name (the shape a non-opaque cursor would have taken), a non-hex string,
    // an odd-length hex string, a token for the nested list, and finally a hex string
    // this service could have minted (accepted as the start key it decodes to).
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "a bare mesh name",
            format!("{PREFIX}/meshes?limit=1&nextToken=cursor-b"),
            400,
        ),
        (
            "not hex at all",
            format!("{PREFIX}/meshes?limit=1&nextToken=zzzz"),
            400,
        ),
        (
            "odd-length hex",
            format!("{PREFIX}/meshes?limit=1&nextToken=abc"),
            400,
        ),
        (
            "a bare node name on the nested list",
            format!("{PREFIX}/meshes/cursor-a/virtualNodes?limit=1&nextToken=n2"),
            400,
        ),
        (
            // hex of "cursor-b": a token with the shape this service mints.
            "well-formed hex",
            format!("{PREFIX}/meshes?limit=1&nextToken=637572736f722d62"),
            200,
        ),
    ];
    for (case, path, expected) in cases {
        let (status, response) = emulator.call("GET", &path, None).await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status}: {response:?}"
        );
        if expected == 400 {
            assert_eq!(
                response["__type"],
                json!("BadRequestException"),
                "case {case}: expected a BadRequestException, got {response:?}"
            );
        }
    }

    // And the token this service DID mint resumes at the next mesh.
    let (status, page) = emulator.list_meshes("?limit=1").await;
    assert_eq!(status, 200, "case minted cursor page 1: answered {status}: {page:?}");
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case minted cursor page 1: expected a nextToken, got {page:?}"))
        .to_string();
    assert_ne!(
        token, "cursor-b",
        "case minted cursor: the token must not be the bare resource name, got {token:?}"
    );
    let (status, page) = emulator
        .list_meshes(&format!("?limit=1&nextToken={token}"))
        .await;
    assert_eq!(status, 200, "case minted cursor page 2: answered {status}: {page:?}");
    assert_eq!(
        page["meshes"][0]["meshName"],
        json!("cursor-b"),
        "case minted cursor page 2: expected to resume at cursor-b, got {page:?}"
    );
}
