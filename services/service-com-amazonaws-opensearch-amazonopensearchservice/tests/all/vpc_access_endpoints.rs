// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE a domain publishes the hosts it is reachable on:
//! `DomainStatus$Endpoint` — "the domain-specific endpoint used to submit index,
//! search, and data upload requests" — for a domain reached on a public endpoint, and
//! `DomainStatus$Endpoints` — "the key-value pair that exists if the OpenSearch
//! Service domain uses VPC endpoints" — for a domain provisioned inside a VPC.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert the RULE over a set of inputs, not one recorded call:
//!
//! * which access members a domain reports is decided by whether it holds
//!   `VPCOptions` and by its `IPAddressType`, over a table spanning both axes and
//!   differing in how many subnets and security groups a VPC domain names — a public
//!   domain reports its IPv4 `Endpoint`, and `EndpointV2` beside it exactly when it is
//!   `dualstack`, while a VPC one reports its hosts in the `Endpoints` map instead;
//! * the hosts are per-domain and stable: two VPC domains publish different ones, and
//!   repeated reads — before and after the provisioning span settles — publish the
//!   same ones;
//! * the rule follows the domain rather than the call that created it: an update that
//!   moves a public domain into a VPC flips which members it publishes, a dry run of
//!   the same update flips nothing, and a later unrelated update leaves the hosts it
//!   published untouched.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK,
};

use super::domain_lifecycle::{Emulator, SETTLE_WAIT};

/// The Region every request of this suite is signed for, and so the one the hosts a
/// domain publishes name.
const REGION: &str = "us-east-1";

/// The tail an IPv4 host of this service carries: the Region and the service's
/// endpoint prefix.
const IPV4_SUFFIX: &str = ".us-east-1.es.amazonaws.com";

/// The tail a dual-stack host of this service carries, from
/// `DomainStatus$Endpoints`'s own `vpcv2` example
/// (`vpc-endpoint-<opaque-id>.aos.<region>.on.aws`).
const DUALSTACK_SUFFIX: &str = ".aos.us-east-1.on.aws";

/// `IPAddressType`'s `dualstack` value, the setting that gives a domain a second,
/// dual-stack host beside its IPv4 one.
const DUALSTACK: &str = "dualstack";

/// One domain to create: how it is reached, and the `Endpoints` keys the rule says it
/// publishes.
struct Row {
    case: &'static str,
    /// The `VPCOptions` container the create carries, if any — a domain is VPC-access
    /// exactly when it holds one.
    vpc_options: Option<Value>,
    /// The `IPAddressType` the create asks for, if any. From the model this is one of
    /// `ipv4` and `dualstack` (`smithy_query(query="shape",shape="IPAddressType")`),
    /// and a request may name neither.
    ip_address_type: Option<&'static str>,
    /// The keys the domain's `Endpoints` map must carry — empty for a domain that
    /// publishes no map at all, and so reports a public `Endpoint` instead.
    endpoints_keys: &'static [&'static str],
}

/// The table: both access modes, each over the `IPAddressType` values the model
/// declares and the absence of one, with the VPC rows differing in how many subnets
/// and security groups they name and whether egress is asked for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "public, no IPAddressType",
            vpc_options: None,
            ip_address_type: None,
            endpoints_keys: &[],
        },
        Row {
            case: "public, ipv4",
            vpc_options: None,
            ip_address_type: Some("ipv4"),
            endpoints_keys: &[],
        },
        Row {
            case: "public, dualstack",
            vpc_options: None,
            ip_address_type: Some(DUALSTACK),
            endpoints_keys: &[],
        },
        Row {
            case: "one subnet and one security group, no IPAddressType",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-0123456789abcdef0"],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
            })),
            ip_address_type: None,
            endpoints_keys: &[ENDPOINTS_KEY_VPC],
        },
        Row {
            case: "one subnet, no security group, ipv4",
            vpc_options: Some(json!({ "SubnetIds": ["subnet-00000000000000001"] })),
            ip_address_type: Some("ipv4"),
            endpoints_keys: &[ENDPOINTS_KEY_VPC],
        },
        Row {
            case: "three subnets, two security groups, egress enabled, ipv4",
            vpc_options: Some(json!({
                "SubnetIds": [
                    "subnet-00000000000000001",
                    "subnet-00000000000000002",
                    "subnet-00000000000000003",
                ],
                "SecurityGroupIds": ["sg-00000000000000001", "sg-00000000000000002"],
                "EgressEnabled": true,
            })),
            ip_address_type: Some("ipv4"),
            endpoints_keys: &[ENDPOINTS_KEY_VPC],
        },
        Row {
            case: "two subnets, dualstack",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-00000000000000004", "subnet-00000000000000005"],
                "SecurityGroupIds": ["sg-00000000000000003"],
            })),
            ip_address_type: Some(DUALSTACK),
            endpoints_keys: &[ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK],
        },
    ]
}

/// A `CreateDomain` body for one row.
fn create_body(name: &str, row: &Row) -> Value {
    let mut body = json!({
        "DomainName": name,
        "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
    });
    if let Some(vpc_options) = &row.vpc_options {
        body["VPCOptions"] = vpc_options.clone();
    }
    if let Some(ip_address_type) = row.ip_address_type {
        body["IPAddressType"] = json!(ip_address_type);
    }
    body
}

/// The `Endpoints` map of a `DomainStatus`, as the keys it carries paired with their
/// hosts — empty when the member is absent, which is what a public domain reports.
///
/// The pairs come back ordered by key, because `Endpoints` is a MAP: the document a
/// client reads states which host each key is reached on and nothing about the order
/// the members travel in, so two reads are compared by their entries rather than by
/// the order one serialization happened to emit them in.
fn endpoints(status: &Value, case: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = match &status["Endpoints"] {
        Value::Null => Vec::new(),
        Value::Object(map) => map
            .iter()
            .map(|(key, host)| {
                let host = host.as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a host string under Endpoints[{key:?}], got {host:?}")
                });
                (key.clone(), host.to_owned())
            })
            .collect::<Vec<(String, String)>>(),
        other => panic!("case {case:?}: expected an Endpoints map or none at all, got {other:?}"),
    };
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// One host's opaque per-domain id, with the tail the rule gives that host checked
/// first: `vpc-endpoint-<opaque-id>` under the Region's IPv4 or dual-stack suffix.
fn host_id(host: &str, tail: &str, case: &str) -> String {
    let id = host
        .strip_prefix("vpc-endpoint-")
        .and_then(|rest| rest.strip_suffix(tail))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected vpc-endpoint-<id>{tail}, got {host:?}")
        });
    assert!(
        !id.is_empty() && id.chars().all(|character| character.is_ascii_alphanumeric()),
        "case {case:?}: expected an opaque alphanumeric host id, got {id:?} in {host:?}"
    );
    id.to_owned()
}

/// Assert `status` publishes exactly what the rule says for `row`, and answer with the
/// hosts it published so a caller can compare two reads or two domains.
///
/// `Endpoints` is a MAP, so the order its members arrive in is no part of the answer —
/// the same reason the key-set assertion below sorts. The hosts are answered in key
/// order so a caller comparing two reads compares their CONTENTS and not the order one
/// serialization happened to put them in.
fn assert_rule(status: &Value, row: &Row, name: &str, case: &str) -> Vec<(String, String)> {
    let mut published = endpoints(status, case);
    published.sort();
    let mut keys: Vec<&str> = published.iter().map(|(key, _)| key.as_str()).collect();
    keys.sort_unstable();
    let mut expected: Vec<&str> = row.endpoints_keys.to_vec();
    expected.sort_unstable();
    assert_eq!(
        keys, expected,
        "case {case:?}: expected the Endpoints keys {expected:?}, got {published:?}"
    );

    if row.endpoints_keys.is_empty() {
        // A domain reached on a public endpoint publishes that address and no map. The
        // address is the one a client of this emulator reaches the domain at: this
        // emulator's own endpoint with the reserved path naming this service's data plane
        // and the domain behind it, because the host AWS names that endpoint by resolves
        // nowhere here.
        let endpoint = status["Endpoint"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a public Endpoint, got {:?}", status["Endpoint"])
        });
        assert_eq!(
            endpoint,
            format!(
                "{}/_aws/es/{name}",
                super::published_domain_address::emulator_authority()
            ),
            "case {case:?}: expected this emulator's own endpoint with the reserved path \
             naming this service's data plane and the domain"
        );
        // "If IPAddressType [is] set to dualstack, a version 2 domain endpoint is
        // provisioned. This endpoint functions like a normal endpoint, except that it
        // works with both IPv4 and IPv6 IP addresses" (`DomainStatus$EndpointV2`): so a
        // dual-stack public domain publishes that host — the label AWS names the domain's
        // endpoint by, under the service's dual-stack domain — and an IPv4 one publishes
        // none.
        let dual_stack = status["EndpointV2"].as_str();
        if row.ip_address_type == Some(DUALSTACK) {
            assert!(
                dual_stack.is_some_and(|host| {
                    host.starts_with(&format!("search-{name}-")) && host.ends_with(DUALSTACK_SUFFIX)
                }),
                "case {case:?}: expected the dual-stack host search-{name}-<suffix>\
                 {DUALSTACK_SUFFIX}, got {dual_stack:?}"
            );
        } else {
            assert_eq!(
                dual_stack, None,
                "case {case:?}: a domain that is not dualstack has no version 2 endpoint, \
                 got {dual_stack:?}"
            );
        }
    } else {
        // A VPC-access domain has no public host: the map is the only place its
        // reachable hosts are published.
        assert!(
            status["Endpoint"].is_null(),
            "case {case:?}: a VPC-access domain has no public Endpoint, got {:?}",
            status["Endpoint"]
        );
        let ipv4 = published
            .iter()
            .find(|(key, _)| key == ENDPOINTS_KEY_VPC)
            .map(|(_, host)| host.clone())
            .unwrap_or_else(|| panic!("case {case:?}: expected a {ENDPOINTS_KEY_VPC} host"));
        let id = host_id(&ipv4, IPV4_SUFFIX, case);
        assert!(
            !ipv4.contains(name),
            "case {case:?}: the host inside a VPC names the endpoint, not the domain, got {ipv4:?}"
        );
        if let Some((_, dualstack)) = published
            .iter()
            .find(|(key, _)| key == ENDPOINTS_KEY_VPC_DUALSTACK)
        {
            // The dual-stack host is the SAME endpoint reached over IPv6 as well as
            // IPv4, so it carries the same opaque id under the dual-stack domain.
            assert_eq!(
                host_id(dualstack, DUALSTACK_SUFFIX, case),
                id,
                "case {case:?}: expected the dual-stack host to carry the IPv4 host's id, \
                 got {dualstack:?} beside {ipv4:?}"
            );
        }
    }
    // Whatever it publishes, the host names the Region the request was signed for.
    for (key, host) in &published {
        assert!(
            host.contains(REGION),
            "case {case:?}: expected the {key} host in {REGION}, got {host:?}"
        );
    }
    published
}

/// Assert `status` publishes NO address at all — what a domain still being provisioned
/// reports, whatever access mode it was created with: the endpoint index and search requests
/// go to does not exist until the cluster behind it does.
fn assert_no_published_address(status: &Value, case: &str) {
    for member in ["Endpoint", "EndpointV2", "Endpoints"] {
        assert!(
            status[member].is_null(),
            "case {case:?}: a domain still Creating publishes no {member}, got {:?}",
            status[member]
        );
    }
}

#[tokio::test]
async fn a_domain_publishes_the_hosts_its_access_mode_gives_it() {
    let emulator = Emulator::new();
    let mut ipv4_hosts: Vec<(String, String)> = Vec::new();

    for (index, row) in rows().iter().enumerate() {
        let name = format!("access-{index}");
        let case = row.case;

        let created = emulator.create(create_body(&name, row)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateDomain, got {} with {:?}",
            created.status, created.body
        );
        // A domain is still being provisioned when the create answers, so it publishes no
        // address yet; the hosts its access mode gives it appear once it is up.
        assert_no_published_address(&created.body["DomainStatus"], case);
        tokio::time::sleep(SETTLE_WAIT).await;

        let read = emulator.describe(&name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from DescribeDomain, got {} with {:?}",
            read.status, read.body
        );
        let minted = assert_rule(&read.body["DomainStatus"], row, &name, case);

        // A read does not change what the next caller sees: the hosts a domain publishes
        // are the same on every later read.
        let settled = emulator.describe(&name).await;
        let settled = assert_rule(&settled.body["DomainStatus"], row, &name, case);
        assert_eq!(
            settled, minted,
            "case {case:?}: the hosts a domain publishes are stable across reads: {minted:?} \
             then {settled:?}"
        );
        if let Some((_, host)) = minted
            .iter()
            .find(|(key, _)| key == ENDPOINTS_KEY_VPC)
        {
            ipv4_hosts.push((name.clone(), host.clone()));
        }
    }

    // The host is per-domain: no two VPC-access domains are reached on one.
    for (left_index, (left_name, left_host)) in ipv4_hosts.iter().enumerate() {
        for (right_name, right_host) in ipv4_hosts.iter().skip(left_index + 1) {
            assert_ne!(
                left_host, right_host,
                "case two VPC domains: {left_name} and {right_name} must not share the host \
                 {left_host:?}"
            );
        }
    }
}

#[tokio::test]
async fn moving_a_domain_into_a_vpc_moves_where_its_hosts_are_published() {
    let emulator = Emulator::new();
    let public = Row {
        case: "public before the update",
        vpc_options: None,
        ip_address_type: Some("ipv4"),
        endpoints_keys: &[],
    };
    let in_vpc = Row {
        case: "VPC-access after the update",
        vpc_options: None,
        ip_address_type: Some("ipv4"),
        endpoints_keys: &[ENDPOINTS_KEY_VPC],
    };
    let name = "moved-domain";

    let created = emulator.create(create_body(name, &public)).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected 200 from CreateDomain, got {} with {:?}",
        public.case, created.status, created.body
    );
    assert_no_published_address(&created.body["DomainStatus"], public.case);
    tokio::time::sleep(SETTLE_WAIT).await;
    let up = emulator.describe(name).await;
    assert_rule(&up.body["DomainStatus"], &public, name, public.case);

    let vpc_options = json!({
        "SubnetIds": ["subnet-00000000000000009"],
        "SecurityGroupIds": ["sg-00000000000000009"],
    });

    // A dry run "returns the results of a dry run analysis without actually applying
    // the change": the domain is still reached on its public endpoint afterwards.
    let dry = emulator
        .update_config(
            name,
            json!({ "VPCOptions": vpc_options.clone(), "DryRun": true }),
        )
        .await;
    assert_eq!(
        dry.status, 200,
        "case dry run: expected 200 from UpdateDomainConfig, got {} with {:?}",
        dry.status, dry.body
    );
    let after_dry = emulator.describe(name).await;
    assert_rule(
        &after_dry.body["DomainStatus"],
        &public,
        name,
        "public after the dry run",
    );

    let applied = emulator
        .update_config(name, json!({ "VPCOptions": vpc_options }))
        .await;
    assert_eq!(
        applied.status, 200,
        "case {:?}: expected 200 from UpdateDomainConfig, got {} with {:?}",
        in_vpc.case, applied.status, applied.body
    );
    let read = emulator.describe(name).await;
    let moved = assert_rule(&read.body["DomainStatus"], &in_vpc, name, in_vpc.case);
    tokio::time::sleep(SETTLE_WAIT).await;

    // A later change to something else leaves the hosts the domain published where
    // they were: the id is minted once for the domain, not per call.
    let unrelated = emulator
        .update_config(name, json!({ "SnapshotOptions": { "AutomatedSnapshotStartHour": 4 } }))
        .await;
    assert_eq!(
        unrelated.status, 200,
        "case unrelated update: expected 200 from UpdateDomainConfig, got {} with {:?}",
        unrelated.status, unrelated.body
    );
    let read = emulator.describe(name).await;
    let still = assert_rule(
        &read.body["DomainStatus"],
        &in_vpc,
        name,
        "VPC-access after an unrelated update",
    );
    assert_eq!(
        still, moved,
        "case unrelated update: expected the hosts {moved:?} to be unchanged, got {still:?}"
    );
}
