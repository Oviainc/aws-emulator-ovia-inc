// SPDX-License-Identifier: CC0-1.0

//! The ONE wire harness this crate's attach-point and refusal suites drive the
//! emulator through.
//!
//! Every suite calls the service the way a client does — `POST /` with
//! `X-Amz-Target: Interconnect.<Operation>` on `awsJson1_0`, through the same
//! `MultiServiceRouter::dispatch` the socket front door uses — so the mounting rules
//! live here once rather than in each suite.
//!
//! Direct Connect is mounted beside Interconnect on the SAME engine, as the assembled
//! emulator mounts them: an attach point IS a Direct Connect gateway of the account, so
//! a suite that needs one creates it through that service ([`Emulator::dx`]) and then
//! reads it back through `ListAttachPoints`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The signing name the model gives this service; also its ARN namespace.
pub(crate) const SIGNING_NAME: &str = "interconnect";

/// The region the harness signs for unless a suite names another one.
pub(crate) const DEFAULT_SIGNING_REGION: &str = "us-east-1";

/// The signing name and `endpointPrefix` the Direct Connect model gives that service
/// (`smithy_query(query="service_info")` on its model: sigv4Name `directconnect`), whose
/// `awsJson1_1` target prefix is its service shape name, `OvertureService`.
pub(crate) const DIRECT_CONNECT_SIGNING_NAME: &str = "directconnect";
pub(crate) const DIRECT_CONNECT_TARGET_PREFIX: &str = "OvertureService";

/// How long a wait for a Direct Connect gateway to reach `available` runs before the
/// case fails. That service owes the clock a `pending` -> `available` transition, so a
/// freshly created gateway becomes a valid attach point a moment later.
pub(crate) const GATEWAY_READY_BUDGET: Duration = Duration::from_secs(5);

/// How often that wait re-reads.
pub(crate) const GATEWAY_READY_INTERVAL: Duration = Duration::from_millis(20);

/// The ONLY value `AttachPointType` models
/// (`smithy_query(query="shape",shape="AttachPointType")` -> values:
/// ["DirectConnectGateway"]), so it is the only `type` a descriptor can report.
pub(crate) const ATTACH_POINT_TYPE: &str = "DirectConnectGateway";

/// The `directConnectGatewayName` the harness gives the gateway it creates for a suite
/// that needs an attach point but is not itself about the attach-point listing.
pub(crate) const HARNESS_GATEWAY_NAME: &str = "ic-harness-attach-point";

/// The `DirectConnectGatewayState` Direct Connect documents as "ready for use", which
/// is the only state a gateway is an attach point in.
pub(crate) const GATEWAY_READY_STATE: &str = "available";

/// An email address of the shape an Environment declaring the `email`
/// `RemoteAccountIdentifierType` asks a create for.
pub(crate) const PARTNER_EMAIL: &str = "interconnect-partner@example.test";

/// One emulator: a router with this service and Direct Connect mounted over one shared
/// engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pinned off so the suites do not depend on the process-wide default; the
        // Authorization header below is still what carries the credential scope.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_interconnect_interconnect::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        // An attach point is a Direct Connect gateway, so the service that owns those
        // gateways is mounted on the same engine the way the assembled router mounts it.
        router.register_service(
            service_com_amazonaws_directconnect_overtureservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// `POST /` for one operation of `signing_name`, with that service's target and
    /// JSON protocol version, signed for the default region.
    async fn post(
        &self,
        signing_name: &str,
        target: &str,
        json_version: &str,
        body: Value,
    ) -> (u16, Value) {
        self.post_in(
            DEFAULT_SIGNING_REGION,
            signing_name,
            target,
            json_version,
            body,
        )
        .await
    }

    /// [`Self::post`], signed for `region` rather than for the default one.
    async fn post_in(
        &self,
        region: &str,
        signing_name: &str,
        target: &str,
        json_version: &str,
        body: Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-target, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.localhost"))
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", target)
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let decoded = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, decoded)
    }

    /// One Interconnect operation (`awsJson1_0`), as a `(status, body)` pair.
    pub(crate) async fn ic(&self, operation: &str, body: Value) -> (u16, Value) {
        self.ic_in(DEFAULT_SIGNING_REGION, operation, body).await
    }

    /// One Interconnect operation signed for `region`, so a suite's rows can differ in
    /// the Region the request addresses (which is the Region every ARN it mints
    /// carries, and the Region whose connections it sees).
    pub(crate) async fn ic_in(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        self.post_in(
            region,
            SIGNING_NAME,
            &format!("Interconnect.{operation}"),
            "1.0",
            body,
        )
        .await
    }

    /// One Interconnect operation, required to succeed.
    pub(crate) async fn ic_ok(&self, operation: &str, body: Value) -> Value {
        self.ic_ok_in(DEFAULT_SIGNING_REGION, operation, body).await
    }

    /// One Interconnect operation signed for `region`, required to succeed.
    pub(crate) async fn ic_ok_in(&self, region: &str, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.ic_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} in {region}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// One Direct Connect operation (`awsJson1_1`), as a `(status, body)` pair.
    pub(crate) async fn dx(&self, operation: &str, body: Value) -> (u16, Value) {
        self.post(
            DIRECT_CONNECT_SIGNING_NAME,
            &format!("{DIRECT_CONNECT_TARGET_PREFIX}.{operation}"),
            "1.1",
            body,
        )
        .await
    }

    /// One Direct Connect operation, required to succeed.
    pub(crate) async fn dx_ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.dx(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// Create one Direct Connect gateway and answer its `directConnectGatewayId`.
    ///
    /// `amazon_side_asn` is left to the caller so a table's rows can differ in it; the
    /// id is the server-minted one, never a value the test chose.
    pub(crate) async fn create_gateway(&self, name: &str, amazon_side_asn: Option<i64>) -> String {
        let mut request = Map::new();
        request.insert("directConnectGatewayName".to_string(), json!(name));
        if let Some(asn) = amazon_side_asn {
            request.insert("amazonSideAsn".to_string(), json!(asn));
        }
        let created = self
            .dx_ok("CreateDirectConnectGateway", Value::Object(request))
            .await;
        text(
            &created["directConnectGateway"],
            "directConnectGatewayId",
        )
    }

    /// The `directConnectGatewayState` Direct Connect reports for `gateway_id`, or `""`
    /// once it reports the gateway no longer exists.
    pub(crate) async fn gateway_state(&self, gateway_id: &str) -> String {
        let (status, body) = self
            .dx(
                "DescribeDirectConnectGateways",
                json!({ "directConnectGatewayId": gateway_id }),
            )
            .await;
        if status != 200 {
            return String::new();
        }
        body["directConnectGateways"]
            .as_array()
            .and_then(|gateways| gateways.first().cloned())
            .map(|gateway| text(&gateway, "directConnectGatewayState"))
            .unwrap_or_default()
    }

    /// Create `names.len()` gateways and wait until Direct Connect reports each one
    /// `available` — the state its own documentation calls "ready for use" — answering
    /// their ids in the order created.
    pub(crate) async fn ready_gateways(&self, names: &[(&str, Option<i64>)]) -> Vec<String> {
        let mut ids = Vec::new();
        for (name, asn) in names {
            ids.push(self.create_gateway(name, *asn).await);
        }
        for id in &ids {
            let started = Instant::now();
            loop {
                let state = self.gateway_state(id).await;
                if state == GATEWAY_READY_STATE {
                    break;
                }
                assert!(
                    started.elapsed() < GATEWAY_READY_BUDGET,
                    "setup {id:?}: expected the gateway to become available within \
                     {GATEWAY_READY_BUDGET:?}, it is still {state:?}"
                );
                tokio::time::sleep(GATEWAY_READY_INTERVAL).await;
            }
        }
        ids
    }

    /// Every seeded Environment, as `ListEnvironments` reports it.
    pub(crate) async fn environments(&self) -> Vec<Value> {
        let listed = self.ic_ok("ListEnvironments", json!({})).await;
        listed["environments"]
            .as_array()
            .expect("environments is a list")
            .clone()
    }

    /// One seeded Environment, as `GetEnvironment` reports it — the record that says
    /// which bandwidths it offers and what identifying information it asks a create for.
    pub(crate) async fn environment(&self, environment_id: &str) -> Value {
        let fetched = self
            .ic_ok("GetEnvironment", json!({ "id": environment_id }))
            .await;
        fetched["environment"].clone()
    }

    /// A Direct Connect gateway of the account that is ready for use, as
    /// `CreateConnection$attachPoint` names one.
    ///
    /// An attach point IS a gateway of the account, so a suite that needs one to attach
    /// a Connection to takes it from here — the account's first ready gateway, created
    /// on the first call and reused afterwards — rather than inventing a UUID no
    /// gateway of the account answers to.
    pub(crate) async fn attach_point(&self) -> String {
        if let Some(ready) = self.first_ready_gateway().await {
            return ready;
        }
        self.ready_gateways(&[(HARNESS_GATEWAY_NAME, None)])
            .await
            .remove(0)
    }

    /// The first gateway of the account Direct Connect reports ready for use, in id
    /// order so the answer does not depend on how the account's gateways are stored —
    /// or `None` when it holds none.
    async fn first_ready_gateway(&self) -> Option<String> {
        let described = self
            .dx_ok("DescribeDirectConnectGateways", json!({}))
            .await;
        let mut ready: Vec<String> = described["directConnectGateways"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|gateway| text(gateway, "directConnectGatewayState") == GATEWAY_READY_STATE)
            .map(|gateway| text(gateway, "directConnectGatewayId"))
            .collect();
        ready.sort();
        ready.into_iter().next()
    }

    /// Every attach point `ListAttachPoints` reports for `environment_id`, walking the
    /// pages so the answer is the whole listing however it is windowed.
    pub(crate) async fn attach_points(&self, environment_id: &str) -> Vec<Value> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = Map::new();
            request.insert("environmentId".to_string(), json!(environment_id));
            if let Some(token) = &token {
                request.insert("nextToken".to_string(), json!(token));
            }
            let listed = self.ic_ok("ListAttachPoints", Value::Object(request)).await;
            all.extend(
                listed["attachPoints"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            token = listed["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                return all;
            }
        }
    }

    /// Create a connection on `environment_id` with `bandwidth`, returning the stored
    /// `Connection`.
    pub(crate) async fn create_connection(
        &self,
        environment_id: &str,
        bandwidth: &str,
        attach_point: &str,
        description: &str,
    ) -> Value {
        let mut request = Map::new();
        request.insert("environmentId".to_string(), json!(environment_id));
        request.insert("bandwidth".to_string(), json!(bandwidth));
        request.insert(
            "attachPoint".to_string(),
            json!({ "directConnectGateway": attach_point }),
        );
        request.insert("description".to_string(), json!(description));
        if let Some(remote_account) = remote_account_for(&self.environment(environment_id).await) {
            request.insert("remoteAccount".to_string(), remote_account);
        }
        let created = self
            .ic_ok("CreateConnection", Value::Object(request))
            .await;
        created["connection"].clone()
    }

    /// `GetConnection`, as a `(status, body)` pair so the not-found case is readable.
    pub(crate) async fn get_connection(&self, identifier: &str) -> (u16, Value) {
        self.ic("GetConnection", json!({ "identifier": identifier }))
            .await
    }
}

/// The `(environmentId, bandwidth)` pairs a connection can be created on, taken
/// from the live catalog rather than from a copy of it, so a suite's rows can differ
/// in the environment and the bandwidth as well as in what they are testing.
pub(crate) async fn placements(emulator: &Emulator) -> Vec<(String, String)> {
    let catalog = emulator.environments().await;
    let mut out = Vec::new();
    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        for bandwidth in available_bandwidths(&environment) {
            out.push((environment_id.clone(), bandwidth));
        }
    }
    assert!(
        !out.is_empty(),
        "the seeded catalog must offer at least one environment that takes connections"
    );
    out
}

/// The `remoteAccount` an Environment asks a `CreateConnection` for: identifying
/// information of the kind its `remoteIdentifierType` declares ("the type of
/// identifying information that should be supplied to the `remoteAccount` parameter of
/// a `CreateConnection` call for this specific Environment"), or nothing at all for an
/// Environment that declares no type and so asks for none.
///
/// The two kinds are the values `RemoteAccountIdentifierType` models: an AWS account
/// number, and an email address.
pub(crate) fn remote_account_for(environment: &Value) -> Option<Value> {
    match environment["remoteIdentifierType"].as_str()? {
        "account" => Some(json!({ "identifier": engine::EMULATOR_ACCOUNT_ID })),
        "email" => Some(json!({ "identifier": PARTNER_EMAIL })),
        _ => None,
    }
}

/// Create a connection on `placement` carrying `tags`, and return the stored
/// `Connection` as `CreateConnection` reported it. `seed` keeps each row's description
/// distinct; the attach point is one of the account's own gateways, because an attach
/// point a Connection is provisioned onto has to be one that exists.
pub(crate) async fn create_with_tags(
    emulator: &Emulator,
    placement: &(String, String),
    seed: usize,
    tags: &std::collections::BTreeMap<String, String>,
) -> Value {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(placement.0));
    request.insert("bandwidth".to_string(), json!(placement.1));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": emulator.attach_point().await }),
    );
    request.insert("description".to_string(), json!(format!("tag case {seed}")));
    if let Some(remote_account) = remote_account_for(&emulator.environment(&placement.0).await) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    if !tags.is_empty() {
        request.insert("tags".to_string(), json!(tags));
    }
    let created = emulator
        .ic_ok("CreateConnection", Value::Object(request))
        .await;
    created["connection"].clone()
}

/// The `tags` member of a `Connection` or of a `ListTagsForResource` response, with
/// absent and empty treated as the same "no tags" — which is what the store holds
/// once the last key is removed.
pub(crate) fn tags_of(value: &Value) -> std::collections::BTreeMap<String, String> {
    value["tags"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A `Connection` with its tag set projected out, so two records can be compared on
/// everything a tag change must NOT touch.
pub(crate) fn without_tags(connection: &Value) -> Value {
    let mut copy = connection.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("tags");
    }
    copy
}

/// A string member, or `""` when absent — so a missing member fails an assertion
/// rather than panicking the harness.
pub(crate) fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The wire error code of an error body.
pub(crate) fn code(body: &Value) -> String {
    text(body, "__type")
}

/// The `message` an error body carries, under either casing the envelope may use.
pub(crate) fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// `Environment$bandwidths.available` for one listed environment.
pub(crate) fn available_bandwidths(environment: &Value) -> Vec<String> {
    environment["bandwidths"]["available"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|bandwidth| bandwidth.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The environments a connection can be created on: one offering no available
/// bandwidth (the `unavailable` state) takes none.
pub(crate) fn creatable_environments(catalog: &[Value]) -> Vec<Value> {
    catalog
        .iter()
        .filter(|environment| !available_bandwidths(environment).is_empty())
        .cloned()
        .collect()
}
