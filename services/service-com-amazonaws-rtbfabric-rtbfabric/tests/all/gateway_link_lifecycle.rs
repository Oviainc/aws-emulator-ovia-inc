// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the gateway-pair + link lifecycle: `CreateRequesterGateway`,
//! `CreateResponderGateway`, `GetRequesterGateway`, `GetResponderGateway`,
//! `CreateLink`, `GetLink` and `DeleteLink`, driven over the wire through the
//! router the production emulator uses.
//!
//! Every assertion here is about state, not about one recorded request: a create is
//! only correct if a later read returns what it stored, and a delete is only correct
//! if the next reader sees it. The tables span the inputs the rules generalise over
//! (how many subnets and security groups, whether a description, tags, attributes or
//! the optional link members are given, the port and protocol, the published bounds
//! of the modelled ranges), so a handler that answered one shape of request would
//! fail on the next row.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::ops::types::{
    CONNECTIVITY_TYPE_VALUES, LINK_DIRECTION_VALUES, LINK_STATUS_VALUES, PROTOCOL_VALUES,
    REQUESTER_GATEWAY_STATUS_VALUES, RESPONDER_GATEWAY_STATUS_VALUES,
};
use service_com_amazonaws_rtbfabric_rtbfabric::state::{GATEWAY_QUOTA, PROVISIONING_DELAY};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; it is also the zone the gateways' assigned
/// domain names must sit under.
const REGION: &str = "us-west-2";

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate ->
/// handle -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one restJson1 request and return `(status, parsed body)`.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bytes = match body {
            Some(body) => {
                Bytes::from(serde_json::to_vec(&body).expect("serialize the request body"))
            }
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// How a row's responder gateway is reached. `CreateResponderGateway`'s own
/// documentation says "A domain name or managed endpoint is required", so each
/// row picks one of the two accepted forms.
enum ResponderEndpoint {
    DomainName(&'static str),
    ManagedEndpoint,
}

/// One row of the lifecycle table.
///
/// The rules under test ("a create persists; a read reports what was persisted;
/// the link counts are derived") hold for every input, so the rows differ along
/// each axis the inputs offer rather than only in the names of things.
struct Row {
    name: &'static str,
    vpc_id: &'static str,
    subnet_ids: &'static [&'static str],
    security_group_ids: &'static [&'static str],
    /// `GetRequesterGateway`/`GetResponderGateway` model `description` with the
    /// narrower `^[A-Za-z0-9 ]+$`, so the rows that set one keep to that.
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    port: i64,
    protocol: &'static str,
    responder_endpoint: ResponderEndpoint,
    /// A `GatewayType`, from `GATEWAY_TYPE_VALUES`.
    gateway_type: Option<&'static str>,
    /// A `ClientRoutingPolicy`, from `CLIENT_ROUTING_POLICY_VALUES`.
    client_routing_policy: Option<&'static str>,
    link_tags: &'static [(&'static str, &'static str)],
    http_responder_allowed: Option<bool>,
    timeout_in_millis: Option<i64>,
    customer_provided_id: Option<&'static str>,
}

/// Ids here satisfy the modelled member constraints, which are narrower than the
/// shorthand an issue report uses: `SubnetId` is `^subnet-\w{8,17}$` (len 15..24)
/// and `SecurityGroupId` is `^sg-[0-9a-f]{8,40}$` (len 11..43), from
/// `smithy_query(query="shape", shape="SubnetIdList,SecurityGroupIdList")`.
/// `port` sits inside the modelled range 1..65535 and `timeoutInMillis` inside
/// 100..5000, both exercised at their published bounds.
const ROWS: &[Row] = &[
    Row {
        name: "only the required members",
        vpc_id: "vpc-0123456789abcdef0",
        subnet_ids: &["subnet-0123456789abcdef0"],
        security_group_ids: &["sg-0123456789abcdef0"],
        description: None,
        tags: &[],
        port: 1,
        protocol: "HTTP",
        responder_endpoint: ResponderEndpoint::DomainName("responder.example.com"),
        gateway_type: None,
        client_routing_policy: None,
        link_tags: &[],
        http_responder_allowed: None,
        timeout_in_millis: None,
        customer_provided_id: None,
    },
    Row {
        name: "every optional member, several subnets and groups",
        vpc_id: "vpc-abcdef0123456789a",
        subnet_ids: &[
            "subnet-0123456789abcdef0",
            "subnet-00000000",
            "subnet-fedcba9876543210f",
        ],
        security_group_ids: &["sg-00000000", "sg-abcdef0123456789abcdef0123456789abcdef01"],
        description: Some("Spot bidding fabric 2"),
        tags: &[("env", "test"), ("team", "ads")],
        port: 65535,
        protocol: "HTTPS",
        responder_endpoint: ResponderEndpoint::ManagedEndpoint,
        gateway_type: Some("EXTERNAL"),
        client_routing_policy: Some("AVAILABILITY_ZONE_AFFINITY"),
        link_tags: &[("team", "ads")],
        http_responder_allowed: Some(true),
        timeout_in_millis: Some(100),
        customer_provided_id: Some("edge-fabric-1"),
    },
    Row {
        name: "the other end of each bound",
        vpc_id: "vpc-00000000",
        subnet_ids: &["subnet-11111111", "subnet-22222222"],
        security_group_ids: &["sg-99999999"],
        description: Some("Display exchange"),
        tags: &[("cost-center", "4711")],
        port: 443,
        protocol: "HTTPS",
        responder_endpoint: ResponderEndpoint::DomainName("bidder.ads.example.com"),
        gateway_type: Some("INTERNAL"),
        client_routing_policy: Some("ANY_AVAILABILITY_ZONE"),
        link_tags: &[("env", "test"), ("owner", "fabric")],
        http_responder_allowed: Some(false),
        timeout_in_millis: Some(5000),
        customer_provided_id: None,
    },
];

impl Row {
    fn requester_body(&self, client_token: &str) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("vpcId".to_string(), json!(self.vpc_id));
        body.insert("subnetIds".to_string(), json!(self.subnet_ids));
        body.insert(
            "securityGroupIds".to_string(),
            json!(self.security_group_ids),
        );
        body.insert("clientToken".to_string(), json!(client_token));
        if let Some(description) = self.description {
            body.insert("description".to_string(), json!(description));
        }
        if !self.tags.is_empty() {
            body.insert("tags".to_string(), tags_json(self.tags));
        }
        Value::Object(body)
    }

    fn responder_body(&self, client_token: &str) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("vpcId".to_string(), json!(self.vpc_id));
        body.insert("subnetIds".to_string(), json!(self.subnet_ids));
        body.insert(
            "securityGroupIds".to_string(),
            json!(self.security_group_ids),
        );
        body.insert("port".to_string(), json!(self.port));
        body.insert("protocol".to_string(), json!(self.protocol));
        body.insert(
            "listenerConfig".to_string(),
            json!({ "protocols": [self.protocol] }),
        );
        body.insert("clientToken".to_string(), json!(client_token));
        match self.responder_endpoint {
            ResponderEndpoint::DomainName(domain_name) => {
                body.insert("domainName".to_string(), json!(domain_name));
            }
            ResponderEndpoint::ManagedEndpoint => {
                body.insert(
                    "managedEndpointConfiguration".to_string(),
                    managed_endpoint(),
                );
            }
        }
        if let Some(gateway_type) = self.gateway_type {
            body.insert("gatewayType".to_string(), json!(gateway_type));
        }
        if let Some(policy) = self.client_routing_policy {
            body.insert("clientRoutingPolicy".to_string(), json!(policy));
        }
        if let Some(description) = self.description {
            body.insert("description".to_string(), json!(description));
        }
        if !self.tags.is_empty() {
            body.insert("tags".to_string(), tags_json(self.tags));
        }
        Value::Object(body)
    }

    /// What `GetResponderGateway` must report for `domainName`.
    fn expected_domain_name(&self) -> Value {
        match self.responder_endpoint {
            ResponderEndpoint::DomainName(domain_name) => json!(domain_name),
            ResponderEndpoint::ManagedEndpoint => Value::Null,
        }
    }

    /// What `GetResponderGateway` must report for `managedEndpointConfiguration`.
    fn expected_managed_endpoint(&self) -> Value {
        match self.responder_endpoint {
            ResponderEndpoint::DomainName(_) => Value::Null,
            ResponderEndpoint::ManagedEndpoint => managed_endpoint(),
        }
    }

    fn link_body(&self, peer_gateway_id: &str) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("peerGatewayId".to_string(), json!(peer_gateway_id));
        body.insert("logSettings".to_string(), log_settings());
        if !self.link_tags.is_empty() {
            body.insert("tags".to_string(), tags_json(self.link_tags));
        }
        if let Some(allowed) = self.http_responder_allowed {
            body.insert("httpResponderAllowed".to_string(), json!(allowed));
        }
        if let Some(timeout) = self.timeout_in_millis {
            body.insert("timeoutInMillis".to_string(), json!(timeout));
        }
        if let Some(customer_provided_id) = self.customer_provided_id {
            body.insert(
                "attributes".to_string(),
                json!({ "customerProvidedId": customer_provided_id }),
            );
        }
        Value::Object(body)
    }
}

fn tags_json(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings() -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } })
}

/// The `autoScalingGroups` arm of `ManagedEndpointConfiguration`, with the two
/// members `smithy_query(query="shape", shape="AutoScalingGroupsConfiguration")`
/// marks required (`roleArn` matching `^arn:aws:iam::[0-9]{12}:role/.{1,2048}$`).
fn managed_endpoint() -> Value {
    json!({
        "autoScalingGroups": {
            "autoScalingGroupNames": ["bidders-asg"],
            "roleArn": "arn:aws:iam::123456789012:role/rtbfabric-responder"
        }
    })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// `GatewayId`: len 8..32, `^rtb-gw-[a-z0-9-]{1,25}$`.
fn is_gateway_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("rtb-gw-") else {
        return false;
    };
    (8..=32).contains(&value.len())
        && (1..=25).contains(&suffix.len())
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `LinkId`: len 6..30, `^link-[a-z0-9-]{1,25}$`.
fn is_link_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("link-") else {
        return false;
    };
    (6..=30).contains(&value.len())
        && (1..=25).contains(&suffix.len())
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// One label of `DomainName`: `[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?`.
fn is_dns_label(label: &str) -> bool {
    (1..=63).contains(&label.len())
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

/// `DomainName`: len 1..255, two or more labels joined by dots.
fn is_domain_name(value: &str) -> bool {
    let labels: Vec<&str> = value.split('.').collect();
    (1..=255).contains(&value.len()) && labels.len() >= 2 && labels.iter().all(|l| is_dns_label(l))
}

#[tokio::test]
async fn the_gateway_pair_and_link_lifecycle_round_trips_through_state() {
    for row in ROWS {
        let case = row.name;
        let fabric = Fabric::new();

        let (status, requester) = fabric
            .send(
                "POST",
                "/requester-gateway",
                Some(row.requester_body("token-requester")),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRequesterGateway to answer 200, got {status} {requester:?}"
        );
        let requester_id = text(&requester, "gatewayId");
        assert!(
            is_gateway_id(&requester_id),
            "case {case:?}: expected a minted gatewayId matching ^rtb-gw-[a-z0-9-]{{1,25}}$, got {requester_id:?}"
        );
        let requester_domain = text(&requester, "domainName");
        assert!(
            is_domain_name(&requester_domain),
            "case {case:?}: expected a generated domainName matching the modelled pattern, got {requester_domain:?}"
        );
        assert!(
            requester_domain.contains(REGION),
            "case {case:?}: expected the domainName to sit under the addressed region {REGION}, got {requester_domain:?}"
        );
        let requester_status = text(&requester, "status");
        assert!(
            REQUESTER_GATEWAY_STATUS_VALUES.contains(&requester_status.as_str()),
            "case {case:?}: expected a modelled RequesterGatewayStatus, got {requester_status:?}"
        );

        let (status, responder) = fabric
            .send(
                "POST",
                "/responder-gateway",
                Some(row.responder_body("token-responder")),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateResponderGateway to answer 200, got {status} {responder:?}"
        );
        let responder_id = text(&responder, "gatewayId");
        assert!(
            is_gateway_id(&responder_id),
            "case {case:?}: expected a minted gatewayId for the responder, got {responder_id:?}"
        );
        assert_ne!(
            requester_id, responder_id,
            "case {case:?}: expected two distinct gateways, both answered {responder_id:?}"
        );
        let responder_status = text(&responder, "status");
        assert!(
            RESPONDER_GATEWAY_STATUS_VALUES.contains(&responder_status.as_str()),
            "case {case:?}: expected a modelled ResponderGatewayStatus, got {responder_status:?}"
        );
        assert_eq!(
            responder["listenerConfig"],
            json!({ "protocols": [row.protocol] }),
            "case {case:?}: expected the persisted listenerConfig back, got {:?}",
            responder["listenerConfig"]
        );
        assert_eq!(
            responder["clientRoutingPolicy"],
            row.client_routing_policy
                .map_or(Value::Null, |policy| json!(policy)),
            "case {case:?}: expected the persisted clientRoutingPolicy back, got {:?}",
            responder["clientRoutingPolicy"]
        );
        // Only an EXTERNAL gateway accepts inbound external links, so only it
        // has a public inbound endpoint.
        if row.gateway_type == Some("EXTERNAL") {
            let endpoint = text(&responder, "externalInboundEndpoint");
            assert!(
                is_domain_name(&endpoint),
                "case {case:?}: expected an EXTERNAL responder gateway to carry an externalInboundEndpoint matching the modelled pattern, got {endpoint:?}"
            );
        } else {
            assert!(
                responder["externalInboundEndpoint"].is_null(),
                "case {case:?}: expected no externalInboundEndpoint for gatewayType {:?}, got {:?}",
                row.gateway_type,
                responder["externalInboundEndpoint"]
            );
        }

        // The requester gateway reads back exactly what was stored, with no links yet.
        let (status, read) = fabric
            .send("GET", &format!("/requester-gateway/{requester_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRequesterGateway to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            text(&read, "gatewayId"),
            requester_id,
            "case {case:?}: expected GetRequesterGateway to report {requester_id:?}, got {:?}",
            read["gatewayId"]
        );
        assert_eq!(
            text(&read, "vpcId"),
            row.vpc_id,
            "case {case:?}: expected the stored vpcId, got {:?}",
            read["vpcId"]
        );
        assert_eq!(
            read["subnetIds"],
            json!(row.subnet_ids),
            "case {case:?}: expected the stored subnetIds, got {:?}",
            read["subnetIds"]
        );
        assert_eq!(
            read["securityGroupIds"],
            json!(row.security_group_ids),
            "case {case:?}: expected the stored securityGroupIds, got {:?}",
            read["securityGroupIds"]
        );
        assert_eq!(
            read["domainName"],
            json!(requester_domain),
            "case {case:?}: expected GetRequesterGateway to report the domainName the create answered, got {:?}",
            read["domainName"]
        );
        match row.description {
            Some(description) => assert_eq!(
                read["description"],
                json!(description),
                "case {case:?}: expected the stored description, got {:?}",
                read["description"]
            ),
            None => assert!(
                read["description"].is_null(),
                "case {case:?}: expected no description, got {:?}",
                read["description"]
            ),
        }
        if row.tags.is_empty() {
            assert!(
                read["tags"].is_null(),
                "case {case:?}: expected no tags, got {:?}",
                read["tags"]
            );
        } else {
            assert_eq!(
                read["tags"],
                tags_json(row.tags),
                "case {case:?}: expected the stored tags, got {:?}",
                read["tags"]
            );
        }
        assert_eq!(
            read["totalLinksCount"],
            json!(0),
            "case {case:?}: expected totalLinksCount 0 before any link, got {:?}",
            read["totalLinksCount"]
        );
        assert_eq!(
            read["activeLinksCount"],
            json!(0),
            "case {case:?}: expected activeLinksCount 0 before any link, got {:?}",
            read["activeLinksCount"]
        );
        assert!(
            read["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a createdAt timestamp, got {:?}",
            read["createdAt"]
        );

        // And so does the responder gateway, including its port and protocol.
        let (status, read_responder) = fabric
            .send("GET", &format!("/responder-gateway/{responder_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetResponderGateway to answer 200, got {status} {read_responder:?}"
        );
        assert_eq!(
            read_responder["port"],
            json!(row.port),
            "case {case:?}: expected the stored port, got {:?}",
            read_responder["port"]
        );
        assert_eq!(
            text(&read_responder, "protocol"),
            row.protocol,
            "case {case:?}: expected the stored protocol, got {:?}",
            read_responder["protocol"]
        );
        assert!(
            PROTOCOL_VALUES.contains(&text(&read_responder, "protocol").as_str()),
            "case {case:?}: expected a modelled Protocol, got {:?}",
            read_responder["protocol"]
        );
        assert_eq!(
            read_responder["domainName"],
            row.expected_domain_name(),
            "case {case:?}: expected GetResponderGateway to report the domainName exactly as it was given, got {:?}",
            read_responder["domainName"]
        );
        assert_eq!(
            read_responder["managedEndpointConfiguration"],
            row.expected_managed_endpoint(),
            "case {case:?}: expected the stored managedEndpointConfiguration, got {:?}",
            read_responder["managedEndpointConfiguration"]
        );
        assert_eq!(
            read_responder["gatewayType"],
            row.gateway_type.map_or(Value::Null, |kind| json!(kind)),
            "case {case:?}: expected the stored gatewayType, got {:?}",
            read_responder["gatewayType"]
        );
        assert_eq!(
            read_responder["clientRoutingPolicy"],
            row.client_routing_policy
                .map_or(Value::Null, |policy| json!(policy)),
            "case {case:?}: expected the stored clientRoutingPolicy, got {:?}",
            read_responder["clientRoutingPolicy"]
        );
        assert_eq!(
            read_responder["externalInboundEndpoint"], responder["externalInboundEndpoint"],
            "case {case:?}: expected GetResponderGateway to report the externalInboundEndpoint the create answered, got {:?}",
            read_responder["externalInboundEndpoint"]
        );

        // The link: a fresh request between the two stored gateways.
        let (status, link) = fabric
            .send(
                "POST",
                &format!("/gateway/{requester_id}/create-link"),
                Some(row.link_body(&responder_id)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLink to answer 200, got {status} {link:?}"
        );
        let link_id = text(&link, "linkId");
        assert!(
            is_link_id(&link_id),
            "case {case:?}: expected a minted linkId matching ^link-[a-z0-9-]{{1,25}}$, got {link_id:?}"
        );
        let link_status = text(&link, "status");
        assert!(
            LINK_STATUS_VALUES.contains(&link_status.as_str()),
            "case {case:?}: expected a modelled LinkStatus, got {link_status:?}"
        );
        assert!(
            link_status != "ACTIVE" && link_status != "DELETED" && link_status != "REJECTED",
            "case {case:?}: expected a fresh link request to be non-terminal, got {link_status:?}"
        );
        assert_eq!(
            text(&link, "gatewayId"),
            requester_id,
            "case {case:?}: expected CreateLink to report the calling gateway, got {:?}",
            link["gatewayId"]
        );
        assert_eq!(
            text(&link, "peerGatewayId"),
            responder_id,
            "case {case:?}: expected CreateLink to report the peer, got {:?}",
            link["peerGatewayId"]
        );
        assert_eq!(
            text(&link, "direction"),
            "REQUEST",
            "case {case:?}: expected a link opened from a requester gateway to be REQUEST, got {:?}",
            link["direction"]
        );
        assert!(
            LINK_DIRECTION_VALUES.contains(&text(&link, "direction").as_str()),
            "case {case:?}: expected a modelled LinkDirection, got {:?}",
            link["direction"]
        );
        assert!(
            CONNECTIVITY_TYPE_VALUES.contains(&text(&link, "connectivityType").as_str()),
            "case {case:?}: expected a modelled ConnectivityType, got {:?}",
            link["connectivityType"]
        );
        assert_eq!(
            link["customerProvidedId"],
            row.customer_provided_id.map_or(Value::Null, |id| json!(id)),
            "case {case:?}: expected the customerProvidedId carried in attributes, got {:?}",
            link["customerProvidedId"]
        );

        // GetLink proves the request was STORED, not echoed: `tags`,
        // `httpResponderAllowed` and `timeoutInMillis` are not in CreateLink's
        // response shape at all, so they can only come from state.
        let (status, read_link) = fabric
            .send(
                "GET",
                &format!("/gateway/{requester_id}/link/{link_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetLink to answer 200, got {status} {read_link:?}"
        );
        assert_eq!(
            text(&read_link, "linkId"),
            link_id,
            "case {case:?}: expected GetLink to report {link_id:?}, got {:?}",
            read_link["linkId"]
        );
        assert_eq!(
            read_link["httpResponderAllowed"],
            row.http_responder_allowed
                .map_or(Value::Null, |allowed| json!(allowed)),
            "case {case:?}: expected httpResponderAllowed read back from state, got {:?}",
            read_link["httpResponderAllowed"]
        );
        assert_eq!(
            read_link["timeoutInMillis"],
            row.timeout_in_millis
                .map_or(Value::Null, |timeout| json!(timeout)),
            "case {case:?}: expected timeoutInMillis read back from state, got {:?}",
            read_link["timeoutInMillis"]
        );
        if row.link_tags.is_empty() {
            assert!(
                read_link["tags"].is_null(),
                "case {case:?}: expected no link tags, got {:?}",
                read_link["tags"]
            );
        } else {
            assert_eq!(
                read_link["tags"],
                tags_json(row.link_tags),
                "case {case:?}: expected the link tags read back from state, got {:?}",
                read_link["tags"]
            );
        }
        assert_eq!(
            read_link["logSettings"],
            log_settings(),
            "case {case:?}: expected the stored logSettings, got {:?}",
            read_link["logSettings"]
        );

        // The link is addressable from its other end too: both gateways are ends
        // of the one stored record.
        let (status, from_peer) = fabric
            .send(
                "GET",
                &format!("/gateway/{responder_id}/link/{link_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetLink from the peer end to answer 200, got {status} {from_peer:?}"
        );
        assert_eq!(
            from_peer["peerGatewayId"], read_link["peerGatewayId"],
            "case {case:?}: expected the stored orientation from either end, got {:?}",
            from_peer["peerGatewayId"]
        );

        // Both gateways now count the one link.
        for (kind, path, gateway_id) in [
            (
                "requester",
                format!("/requester-gateway/{requester_id}"),
                &requester_id,
            ),
            (
                "responder",
                format!("/responder-gateway/{responder_id}"),
                &responder_id,
            ),
        ] {
            let (status, read) = fabric.send("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {kind} gateway {gateway_id} to answer 200, got {status} {read:?}"
            );
            assert_eq!(
                read["totalLinksCount"],
                json!(1),
                "case {case:?}: expected the {kind} gateway's totalLinksCount to be 1, got {:?}",
                read["totalLinksCount"]
            );
            assert_eq!(
                read["activeLinksCount"],
                json!(0),
                "case {case:?}: expected the {kind} gateway's activeLinksCount to be 0 while the request is outstanding, got {:?}",
                read["activeLinksCount"]
            );
        }

        // Delete it, and the next reader sees it gone.
        let (status, deleted) = fabric
            .send(
                "DELETE",
                &format!("/gateway/{requester_id}/link/{link_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteLink to answer 200, got {status} {deleted:?}"
        );
        assert_eq!(
            text(&deleted, "linkId"),
            link_id,
            "case {case:?}: expected DeleteLink to report {link_id:?}, got {:?}",
            deleted["linkId"]
        );
        assert_eq!(
            text(&deleted, "status"),
            "DELETED",
            "case {case:?}: expected the terminal DELETED status, got {:?}",
            deleted["status"]
        );

        let (status, gone) = fabric
            .send(
                "GET",
                &format!("/gateway/{requester_id}/link/{link_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetLink on a deleted link to answer 404, got {status} {gone:?}"
        );

        let (status, read) = fabric
            .send("GET", &format!("/requester-gateway/{requester_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRequesterGateway to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read["totalLinksCount"],
            json!(0),
            "case {case:?}: expected totalLinksCount back to 0 after the delete, got {:?}",
            read["totalLinksCount"]
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_gateway_and_a_changed_one_is_refused() {
    for row in ROWS {
        let case = row.name;
        let fabric = Fabric::new();

        for (operation, path, body) in [
            (
                "CreateRequesterGateway",
                "/requester-gateway",
                row.requester_body("token-a") as Value,
            ),
            (
                "CreateResponderGateway",
                "/responder-gateway",
                row.responder_body("token-a"),
            ),
        ] {
            let (status, first) = fabric.send("POST", path, Some(body.clone())).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {operation} to answer 200, got {status} {first:?}"
            );
            let (status, retry) = fabric.send("POST", path, Some(body)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {operation} retry to answer 200, got {status} {retry:?}"
            );
            assert_eq!(
                text(&retry, "gatewayId"),
                text(&first, "gatewayId"),
                "case {case:?}: expected the same clientToken to answer {operation}'s first gatewayId {:?}, got {:?}",
                first["gatewayId"],
                retry["gatewayId"]
            );
        }

        // Same token, different parameters: the documented contract is a failure,
        // and the only client-input error these operations model is
        // ValidationException (400).
        let mut changed = row.requester_body("token-a");
        changed["vpcId"] = json!("vpc-ffffffffffffffff0");
        let (status, refused) = fabric
            .send("POST", "/requester-gateway", Some(changed))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a reused clientToken with changed parameters to be refused with 400, got {status} {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_link_needs_two_stored_gateways_and_only_one_link_per_pair() {
    let row = &ROWS[1];
    let fabric = Fabric::new();

    let (_, requester) = fabric
        .send(
            "POST",
            "/requester-gateway",
            Some(row.requester_body("token-requester")),
        )
        .await;
    let requester_id = text(&requester, "gatewayId");
    let (_, responder) = fabric
        .send(
            "POST",
            "/responder-gateway",
            Some(row.responder_body("token-responder")),
        )
        .await;
    let responder_id = text(&responder, "gatewayId");

    // Neither end may be an id the service never minted, whichever end it is.
    let unknown = "rtb-gw-does-not-exist";
    for (case, gateway_id, peer_gateway_id) in [
        ("unknown peer", requester_id.as_str(), unknown),
        ("unknown caller", unknown, responder_id.as_str()),
        ("both unknown", unknown, unknown),
    ] {
        let (status, body) = fabric
            .send(
                "POST",
                &format!("/gateway/{gateway_id}/create-link"),
                Some(row.link_body(peer_gateway_id)),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected CreateLink against an unstored gateway to answer 404, got {status} {body:?}"
        );
    }

    // A gateway cannot link to itself.
    let (status, body) = fabric
        .send(
            "POST",
            &format!("/gateway/{requester_id}/create-link"),
            Some(row.link_body(&requester_id)),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a self-link to be refused with 400, got {status} {body:?}"
    );

    // The first link is created, a second one for the same pair conflicts.
    let (status, body) = fabric
        .send(
            "POST",
            &format!("/gateway/{requester_id}/create-link"),
            Some(row.link_body(&responder_id)),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the first CreateLink to answer 200, got {status} {body:?}"
    );
    let (status, body) = fabric
        .send(
            "POST",
            &format!("/gateway/{requester_id}/create-link"),
            Some(row.link_body(&responder_id)),
        )
        .await;
    assert_eq!(
        status, 409,
        "expected a second link for the same gateway pair to conflict with 409, got {status} {body:?}"
    );

    // A link is one record with two ends, so the reverse orientation is the same
    // pair and conflicts too.
    let (status, body) = fabric
        .send(
            "POST",
            &format!("/gateway/{responder_id}/create-link"),
            Some(row.link_body(&requester_id)),
        )
        .await;
    assert_eq!(
        status, 409,
        "expected a link for the same pair in the reverse orientation to conflict with 409, got {status} {body:?}"
    );
}

#[tokio::test]
async fn a_responder_gateway_needs_a_domain_name_or_a_managed_endpoint() {
    for row in ROWS {
        let case = row.name;
        let fabric = Fabric::new();

        // Neither member: refused, because the operation's documentation makes
        // one of the two required and neither is `@required` on its own.
        let mut neither = row.responder_body("token-neither");
        let object = neither.as_object_mut().expect("a JSON object body");
        object.remove("domainName");
        object.remove("managedEndpointConfiguration");
        let (status, refused) = fabric
            .send("POST", "/responder-gateway", Some(neither))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a responder gateway with neither domainName nor managedEndpointConfiguration to be refused with 400, got {status} {refused:?}"
        );

        // Either member alone: accepted.
        let mut with_domain = row.responder_body("token-domain");
        let object = with_domain.as_object_mut().expect("a JSON object body");
        object.remove("managedEndpointConfiguration");
        object.insert("domainName".to_string(), json!("responder.example.com"));
        let (status, accepted) = fabric
            .send("POST", "/responder-gateway", Some(with_domain))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a responder gateway with only a domainName to be accepted, got {status} {accepted:?}"
        );

        let mut with_endpoint = row.responder_body("token-endpoint");
        let object = with_endpoint.as_object_mut().expect("a JSON object body");
        object.remove("domainName");
        object.insert(
            "managedEndpointConfiguration".to_string(),
            managed_endpoint(),
        );
        let (status, accepted) = fabric
            .send("POST", "/responder-gateway", Some(with_endpoint))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a responder gateway with only a managedEndpointConfiguration to be accepted, got {status} {accepted:?}"
        );
    }
}

#[tokio::test]
async fn unknown_gateways_and_links_are_not_found_and_a_second_delete_is_not_a_server_error() {
    let row = &ROWS[0];
    let fabric = Fabric::new();

    for (case, method, path) in [
        (
            "requester gateway",
            "GET",
            "/requester-gateway/rtb-gw-unknown",
        ),
        (
            "responder gateway",
            "GET",
            "/responder-gateway/rtb-gw-unknown",
        ),
        (
            "link on an unknown gateway",
            "GET",
            "/gateway/rtb-gw-unknown/link/link-unknown",
        ),
        (
            "delete a link on an unknown gateway",
            "DELETE",
            "/gateway/rtb-gw-unknown/link/link-unknown",
        ),
    ] {
        let (status, body) = fabric.send(method, path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {method} {path} to answer 404, got {status} {body:?}"
        );
    }

    let (_, requester) = fabric
        .send(
            "POST",
            "/requester-gateway",
            Some(row.requester_body("token-requester")),
        )
        .await;
    let requester_id = text(&requester, "gatewayId");
    let (_, responder) = fabric
        .send(
            "POST",
            "/responder-gateway",
            Some(row.responder_body("token-responder")),
        )
        .await;
    let responder_id = text(&responder, "gatewayId");
    let (_, link) = fabric
        .send(
            "POST",
            &format!("/gateway/{requester_id}/create-link"),
            Some(row.link_body(&responder_id)),
        )
        .await;
    let link_id = text(&link, "linkId");

    let path = format!("/gateway/{requester_id}/link/{link_id}");
    let (status, _) = fabric.send("DELETE", &path, None).await;
    assert_eq!(status, 200, "expected the first delete to answer 200");
    let (status, body) = fabric.send("DELETE", &path, None).await;
    assert_eq!(
        status, 404,
        "expected a second delete of the same link to answer 404, never a server error, got {status} {body:?}"
    );
}

#[tokio::test]
async fn the_modelled_constraints_are_refused_at_their_bounds() {
    let row = &ROWS[0];
    let fabric = Fabric::new();

    // One past each published bound of the modelled ranges, plus an id that does
    // not match its pattern. All are refused before any handler runs.
    let mut port_low = row.responder_body("token-port-low");
    port_low["port"] = json!(0);
    let mut port_high = row.responder_body("token-port-high");
    port_high["port"] = json!(65536);
    let mut bad_vpc = row.requester_body("token-vpc");
    bad_vpc["vpcId"] = json!("vpc-not-hex");
    let mut bad_subnet = row.requester_body("token-subnet");
    bad_subnet["subnetIds"] = json!(["subnet-1"]);

    for (case, path, body) in [
        ("port 0", "/responder-gateway", port_low),
        ("port 65536", "/responder-gateway", port_high),
        ("vpcId off pattern", "/requester-gateway", bad_vpc),
        ("subnetId too short", "/requester-gateway", bad_subnet),
    ] {
        let (status, answered) = fabric.send("POST", path, Some(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a request outside the modelled constraints to answer 400, got {status} {answered:?}"
        );
    }

    let (_, requester) = fabric
        .send(
            "POST",
            "/requester-gateway",
            Some(row.requester_body("token-requester")),
        )
        .await;
    let requester_id = text(&requester, "gatewayId");
    let (_, responder) = fabric
        .send(
            "POST",
            "/responder-gateway",
            Some(row.responder_body("token-responder")),
        )
        .await;
    let responder_id = text(&responder, "gatewayId");

    for (case, timeout) in [("timeout 99", 99_i64), ("timeout 5001", 5001_i64)] {
        let mut body = row.link_body(&responder_id);
        body["timeoutInMillis"] = json!(timeout);
        let (status, answered) = fabric
            .send(
                "POST",
                &format!("/gateway/{requester_id}/create-link"),
                Some(body),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a timeoutInMillis outside 100..5000 to answer 400, got {status} {answered:?}"
        );
    }

    // And the bounds themselves are accepted.
    for (case, timeout) in [("timeout 100", 100_i64), ("timeout 5000", 5000_i64)] {
        let fabric = Fabric::new();
        let (_, requester) = fabric
            .send(
                "POST",
                "/requester-gateway",
                Some(row.requester_body("token-requester")),
            )
            .await;
        let requester_id = text(&requester, "gatewayId");
        let (_, responder) = fabric
            .send(
                "POST",
                "/responder-gateway",
                Some(row.responder_body("token-responder")),
            )
            .await;
        let responder_id = text(&responder, "gatewayId");
        let mut body = row.link_body(&responder_id);
        body["timeoutInMillis"] = json!(timeout);
        let (status, answered) = fabric
            .send(
                "POST",
                &format!("/gateway/{requester_id}/create-link"),
                Some(body),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a timeoutInMillis on the published bound to be accepted, got {status} {answered:?}"
        );
    }
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_own_quota() {
    let row = &ROWS[0];
    let fabric = Fabric::new();

    // Fill the requester-gateway store to the emulator's own bound. Nothing
    // releases a gateway record yet (DeleteRequesterGateway is still a stub), so
    // this quota is what keeps caller-driven state growth finite.
    for n in 0..GATEWAY_QUOTA {
        let (status, body) = fabric
            .send(
                "POST",
                "/requester-gateway",
                Some(row.requester_body(&format!("token-{n}"))),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {n}: expected a create below the quota to answer 200, got {status} {body:?}"
        );
    }

    let (status, refused) = fabric
        .send(
            "POST",
            "/requester-gateway",
            Some(row.requester_body("token-past-the-quota")),
        )
        .await;
    assert_eq!(
        status, 402,
        "expected the create past the store's quota of {GATEWAY_QUOTA} to be refused with the modelled ServiceQuotaExceededException (402), got {status} {refused:?}"
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "expected the modelled error code, got {:?}",
        refused["__type"]
    );

    // A retry of a token already recorded still answers from state: the quota
    // refuses new resources, not idempotent replays.
    let (status, replay) = fabric
        .send(
            "POST",
            "/requester-gateway",
            Some(row.requester_body("token-0")),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected an idempotent replay to answer 200 even at the quota, got {status} {replay:?}"
    );
}

#[tokio::test]
async fn a_pending_resource_settles_once_the_clock_has_made_the_transition_due() {
    let row = &ROWS[2];
    let fabric = Fabric::new();

    let (_, requester) = fabric
        .send(
            "POST",
            "/requester-gateway",
            Some(row.requester_body("token-requester")),
        )
        .await;
    let requester_id = text(&requester, "gatewayId");
    let (_, responder) = fabric
        .send(
            "POST",
            "/responder-gateway",
            Some(row.responder_body("token-responder")),
        )
        .await;
    let responder_id = text(&responder, "gatewayId");
    let (_, link) = fabric
        .send(
            "POST",
            &format!("/gateway/{requester_id}/create-link"),
            Some(row.link_body(&responder_id)),
        )
        .await;
    let link_id = text(&link, "linkId");

    // Straight after the create, the transient status is still observable.
    let (_, pending) = fabric
        .send("GET", &format!("/requester-gateway/{requester_id}"), None)
        .await;
    assert_eq!(
        text(&pending, "status"),
        "PENDING_CREATION",
        "expected the gateway to still be PENDING_CREATION straight after the create, got {:?}",
        pending["status"]
    );

    // Once the clock has made the transition due, every reader sees the settled
    // status — nothing had to be polled for it to become due.
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    for (case, path, expected) in [
        (
            "requester gateway",
            format!("/requester-gateway/{requester_id}"),
            "ACTIVE",
        ),
        (
            "responder gateway",
            format!("/responder-gateway/{responder_id}"),
            "ACTIVE",
        ),
        (
            "link",
            format!("/gateway/{requester_id}/link/{link_id}"),
            "REQUESTED",
        ),
    ] {
        let (status, body) = fabric.send("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            text(&body, "status"),
            expected,
            "case {case:?}: expected the settled status {expected:?}, got {:?}",
            body["status"]
        );
    }

    // The settled link now counts as requested against the responder gateway.
    let (_, read_responder) = fabric
        .send("GET", &format!("/responder-gateway/{responder_id}"), None)
        .await;
    assert_eq!(
        read_responder["linksRequestedCount"],
        json!(1),
        "expected the outstanding link request to be counted, got {:?}",
        read_responder["linksRequestedCount"]
    );
}
