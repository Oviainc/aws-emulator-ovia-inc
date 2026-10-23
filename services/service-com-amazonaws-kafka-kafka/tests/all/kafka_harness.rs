// SPDX-License-Identifier: CC0-1.0

//! The shared harness every suite in this crate drives the service through: the
//! kafka service mounted in a `MultiServiceRouter` and called over HTTP exactly as
//! the emulator serves it, plus the request builders a suite writes its table rows
//! in (`ClusterRequest` for `CreateCluster`, `ConnectionRequest` for
//! `CreateVpcConnection`).
//!
//! One harness rather than one per suite, so every suite dispatches through the
//! same path and a new suite adds only its rule.

// A suite reaches for the part of the harness its rule needs, so an item only one
// suite uses is not dead code — it is this harness's surface.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_kafka_kafka::state::{
    CLUSTER_ACTIVATION_DELAY, CLUSTER_UPDATE_DELAY, VPC_CONNECTION_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The signing region every request below is scoped to; it is the region the
/// service must mint its ARNs and endpoints in.
pub const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT: &str = "000000000000";

/// The kafka service mounted in a router, driven over HTTP exactly as the emulator
/// serves it.
pub struct Emulator {
    router: MultiServiceRouter,
}

/// One HTTP answer, parsed.
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    /// The wire error code (`__type` of the JSON error envelope), or `""` on success.
    pub fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// A member of the response body, or `Value::Null` when absent.
    pub fn at(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    /// The error envelope's message, or `""` when there is none.
    pub fn error_message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the core's business, not this suite's: the
        // requests below carry a SigV4 scope so the region and route key resolve,
        // but they are not signed.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    pub async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.call_in(REGION, method, uri, body).await
    }

    /// The same call, signed for `region` — which is where the service must mint
    /// the ARNs and endpoints of anything it creates.
    pub async fn call_in(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("kafka.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/kafka/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(payload))
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

    pub async fn create(&self, request: &ClusterRequest) -> Reply {
        self.call("POST", "/v1/clusters", Some(request.to_json())).await
    }

    pub async fn describe(&self, arn: &str) -> Reply {
        self.call("GET", &format!("/v1/clusters/{}", pct(arn)), None)
            .await
    }

    pub async fn bootstrap_brokers(&self, arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v1/clusters/{}/bootstrap-brokers", pct(arn)),
            None,
        )
        .await
    }

    pub async fn list(&self, query: &str) -> Reply {
        self.call("GET", &format!("/v1/clusters{query}"), None).await
    }

    /// `ListClustersV2`, which the model binds to `GET /api/v2/clusters`.
    pub async fn list_v2(&self, query: &str) -> Reply {
        self.list_v2_in(REGION, query).await
    }

    /// The same listing, as an endpoint for `region` serves it.
    pub async fn list_v2_in(&self, region: &str, query: &str) -> Reply {
        self.call_in(region, "GET", &format!("/api/v2/clusters{query}"), None)
            .await
    }

    /// Create `request`, assert it succeeded, and hand back the minted ARN.
    pub async fn create_ok(&self, request: &ClusterRequest) -> String {
        self.create_ok_in(REGION, request).await
    }

    /// The same create, signed for `region`, which is the region the cluster and
    /// everything addressed under it then live in.
    pub async fn create_ok_in(&self, region: &str, request: &ClusterRequest) -> String {
        let reply = self
            .call_in(region, "POST", "/v1/clusters", Some(request.to_json()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected CreateCluster in {region} to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {request:?}: CreateCluster returned no clusterArn"))
            .to_string()
    }

    /// `CreateVpcConnection`, which the model binds to `POST /v1/vpc-connection`.
    pub async fn create_vpc_connection(&self, request: &ConnectionRequest) -> Reply {
        self.call("POST", "/v1/vpc-connection", Some(request.to_json()))
            .await
    }

    /// `DescribeVpcConnection`, whose `Arn` is an `@httpLabel`.
    pub async fn describe_vpc_connection(&self, arn: &str) -> Reply {
        self.call("GET", &vpc_connection_uri(arn), None).await
    }

    /// `DeleteVpcConnection`, on the same resource URI.
    pub async fn delete_vpc_connection(&self, arn: &str) -> Reply {
        self.call("DELETE", &vpc_connection_uri(arn), None).await
    }

    /// Create `request`, assert it succeeded, and hand back the minted ARN.
    pub async fn create_vpc_connection_ok(&self, request: &ConnectionRequest) -> String {
        let reply = self.create_vpc_connection(request).await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected CreateVpcConnection to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["vpcConnectionArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {request:?}: CreateVpcConnection returned no vpcConnectionArn")
            })
            .to_string()
    }

    /// `ListVpcConnections`, which the model binds to `GET /v1/vpc-connections`
    /// with `MaxResults` and `NextToken` as query parameters; `query` is the
    /// whole query string including its `?`, or empty.
    pub async fn list_vpc_connections(&self, query: &str) -> Reply {
        self.list_vpc_connections_in(REGION, query).await
    }

    /// The same listing, as the endpoint of `region` serves it.
    pub async fn list_vpc_connections_in(&self, region: &str, query: &str) -> Reply {
        self.call_in(region, "GET", &format!("/v1/vpc-connections{query}"), None)
            .await
    }

    /// Create `request` under `region`'s endpoint, assert it succeeded, and hand
    /// back the minted ARN — the regional counterpart of
    /// [`Self::create_vpc_connection_ok`].
    pub async fn create_vpc_connection_ok_in(
        &self,
        region: &str,
        request: &ConnectionRequest,
    ) -> String {
        let reply = self
            .call_in(
                region,
                "POST",
                "/v1/vpc-connection",
                Some(request.to_json()),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region} {request:?}: expected CreateVpcConnection to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["vpcConnectionArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {region} {request:?}: CreateVpcConnection returned no vpcConnectionArn")
            })
            .to_string()
    }
}

/// The resource URI of one MSK VPC connection, with the ARN percent-encoded into
/// the single `@httpLabel` path segment the model declares.
pub fn vpc_connection_uri(arn: &str) -> String {
    format!("/v1/vpc-connection/{}", pct(arn))
}

/// Percent-encode a URI component. An MSK ARN is an `@httpLabel`, so its `:` and
/// `/` must be encoded to keep it one path segment; a `NextToken` is base64,
/// whose `+` would otherwise decode to a space. Everything that is not an ASCII
/// alphanumeric is encoded — the conservative set, and over-encoding an
/// unreserved byte is still correct (RFC 3986 §2.1).
pub fn pct(value: &str) -> String {
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

/// One row of a `CreateCluster` table: the required members plus whichever
/// optional ones the row exercises. `None` means "the request omits it", which is
/// how a row tests a documented default.
#[derive(Debug, Clone)]
pub struct ClusterRequest {
    pub name: String,
    pub kafka_version: &'static str,
    pub brokers: i64,
    pub instance_type: &'static str,
    pub subnets: Vec<&'static str>,
    pub client_broker: Option<&'static str>,
    pub sasl_scram: Option<bool>,
    pub sasl_iam: Option<bool>,
    pub storage_mode: Option<&'static str>,
    pub enhanced_monitoring: Option<&'static str>,
    pub public_access: Option<&'static str>,
    pub network_type: Option<&'static str>,
    pub vpc_tls: Option<bool>,
    pub vpc_scram: Option<bool>,
    pub vpc_iam: Option<bool>,
    pub tags: Vec<(&'static str, &'static str)>,
}

impl ClusterRequest {
    /// A minimal request: only the four required members, so every optional
    /// member's documented default applies.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            kafka_version: "3.6.0",
            brokers: 3,
            instance_type: "kafka.m5.large",
            subnets: vec!["subnet-a", "subnet-b", "subnet-c"],
            client_broker: None,
            sasl_scram: None,
            sasl_iam: None,
            storage_mode: None,
            enhanced_monitoring: None,
            public_access: None,
            network_type: None,
            vpc_tls: None,
            vpc_scram: None,
            vpc_iam: None,
            tags: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut broker_group = json!({
            "instanceType": self.instance_type,
            "clientSubnets": self.subnets,
        });

        let mut connectivity = Map::new();
        if let Some(access) = self.public_access {
            connectivity.insert("publicAccess".to_string(), json!({ "type": access }));
        }
        if let Some(network) = self.network_type {
            connectivity.insert("networkType".to_string(), json!(network));
        }
        let mut vpc_auth = Map::new();
        if let Some(enabled) = self.vpc_tls {
            vpc_auth.insert("tls".to_string(), json!({ "enabled": enabled }));
        }
        let mut vpc_sasl = Map::new();
        if let Some(enabled) = self.vpc_scram {
            vpc_sasl.insert("scram".to_string(), json!({ "enabled": enabled }));
        }
        if let Some(enabled) = self.vpc_iam {
            vpc_sasl.insert("iam".to_string(), json!({ "enabled": enabled }));
        }
        if !vpc_sasl.is_empty() {
            vpc_auth.insert("sasl".to_string(), Value::Object(vpc_sasl));
        }
        if !vpc_auth.is_empty() {
            connectivity.insert(
                "vpcConnectivity".to_string(),
                json!({ "clientAuthentication": Value::Object(vpc_auth) }),
            );
        }
        if !connectivity.is_empty() {
            broker_group["connectivityInfo"] = Value::Object(connectivity);
        }

        let mut body = json!({
            "clusterName": self.name,
            "kafkaVersion": self.kafka_version,
            "numberOfBrokerNodes": self.brokers,
            "brokerNodeGroupInfo": broker_group,
        });
        if let Some(client_broker) = self.client_broker {
            body["encryptionInfo"] =
                json!({ "encryptionInTransit": { "clientBroker": client_broker } });
        }
        let mut sasl = Map::new();
        if let Some(enabled) = self.sasl_scram {
            sasl.insert("scram".to_string(), json!({ "enabled": enabled }));
        }
        if let Some(enabled) = self.sasl_iam {
            sasl.insert("iam".to_string(), json!({ "enabled": enabled }));
        }
        if !sasl.is_empty() {
            body["clientAuthentication"] = json!({ "sasl": Value::Object(sasl) });
        }
        if let Some(mode) = self.storage_mode {
            body["storageMode"] = json!(mode);
        }
        if let Some(level) = self.enhanced_monitoring {
            body["enhancedMonitoring"] = json!(level);
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }
        body
    }
}

/// Sleep past the emulator's own `CREATING` -> `ACTIVE` span, so a later read sees
/// a settled cluster. `std::thread::sleep` because the workspace's tokio carries no
/// timer feature; nothing else has to make progress here.
pub fn sleep_past_activation() {
    std::thread::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// One row of a `CreateVpcConnection` table: every member the request carries.
#[derive(Debug, Clone)]
pub struct ConnectionRequest {
    pub target_cluster_arn: String,
    pub authentication: &'static str,
    pub vpc_id: &'static str,
    pub client_subnets: Vec<&'static str>,
    pub security_groups: Vec<&'static str>,
    pub tags: Vec<(&'static str, &'static str)>,
}

impl ConnectionRequest {
    /// A minimal, valid request against the cluster `cluster` names. The EC2 ids
    /// are in their 8-hex-digit form; a row that exercises the 17-digit one says
    /// so.
    pub fn to(cluster: &str) -> Self {
        Self {
            target_cluster_arn: cluster.to_string(),
            authentication: "SASL_IAM",
            vpc_id: "vpc-0a1b2c3d",
            client_subnets: vec!["subnet-0a1b2c3d"],
            security_groups: vec!["sg-0a1b2c3d"],
            tags: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut body = json!({
            "targetClusterArn": self.target_cluster_arn,
            "authentication": self.authentication,
            "vpcId": self.vpc_id,
            "clientSubnets": self.client_subnets,
            "securityGroups": self.security_groups,
        });
        if !self.tags.is_empty() {
            body["tags"] = self.tag_json();
        }
        body
    }

    /// The tags as the JSON map a reader must report.
    pub fn tag_json(&self) -> Value {
        Value::Object(
            self.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        )
    }
}

/// Sleep past the emulator's own `CREATING` -> `AVAILABLE` span, so a later read
/// sees a settled VPC connection.
pub fn sleep_past_vpc_connection_activation() {
    std::thread::sleep(VPC_CONNECTION_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// Sleep past the emulator's own `UPDATING` -> `ACTIVE` span, so a later read
/// sees the cluster update the clock now owes the cluster. Beside
/// [`sleep_past_activation`] so every suite that drives an update operation
/// waits through one helper.
pub fn sleep_past_update() {
    std::thread::sleep(CLUSTER_UPDATE_DELAY + Duration::from_millis(250));
}
