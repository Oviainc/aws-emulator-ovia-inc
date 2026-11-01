// SPDX-License-Identifier: CC0-1.0

//! WHEN a domain publishes the address its data plane is reached at.
//!
//! RULE: a resource that is still being provisioned publishes no address for its data
//! plane, because the endpoint does not exist until the cluster behind it does — so
//! `DomainStatus$Endpoint` (the "domain-specific endpoint used to submit index, search, and
//! data upload requests"), `EndpointV2` and `Endpoints` are all absent while the domain is
//! `Creating`, appear on every read once it is up, and stay published through a later
//! configuration change, which leaves the running cluster in place.
//!
//! The table spans every access mode a domain publishes an address in — public, public
//! dual-stack, inside a VPC and dual-stack inside a VPC — because which MEMBER carries the
//! address differs per mode while the rule about WHEN it is carried does not, and every
//! reader of a `DomainStatus` is checked on each row: the create's own answer, the
//! single-domain read and the batch read. The members each mode publishes come from
//! `smithy_query(query="shape",shape="DomainStatus")`:
//! `Endpoint` is the public host, `EndpointV2` the dual-stack public host provisioned when
//! `IPAddressType` is `dualstack`, and `Endpoints` "the key-value pair that exists if the
//! OpenSearch Service domain uses VPC endpoints".
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness is this crate's one test harness ([`super::domain_lifecycle`]).

use serde_json::{json, Value};

use super::domain_lifecycle::{Answer, Emulator, PREFIX, SETTLE_WAIT};

/// One access mode to publish an address in: how the domain is created, and which members of
/// a `DomainStatus` carry its address once it is up.
struct Row {
    case: &'static str,
    domain: &'static str,
    /// The `VPCOptions` the create carries, if any — a domain is reached inside a VPC
    /// exactly when it holds them.
    vpc_options: Option<Value>,
    /// The `IPAddressType` the create asks for, if any: one of `ipv4` and `dualstack`
    /// (`smithy_query(query="shape",shape="IPAddressType")`), or neither.
    ip_address_type: Option<&'static str>,
    /// The members that carry an address for this domain, as [`published_members`] names
    /// them, sorted.
    members: &'static [&'static str],
}

/// The table: both access modes, over the `IPAddressType` values that decide whether a
/// domain has a dual-stack address beside its IPv4 one.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a public domain naming no IPAddressType",
            domain: "pub-when-plain",
            vpc_options: None,
            ip_address_type: None,
            members: &["Endpoint"],
        },
        Row {
            case: "a public ipv4 domain",
            domain: "pub-when-ipv4",
            vpc_options: None,
            ip_address_type: Some("ipv4"),
            members: &["Endpoint"],
        },
        Row {
            case: "a public dualstack domain",
            domain: "pub-when-dual",
            vpc_options: None,
            ip_address_type: Some("dualstack"),
            members: &["Endpoint", "EndpointV2"],
        },
        Row {
            case: "a domain reached inside a VPC",
            domain: "pub-when-vpc",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-0123456789abcdef0"],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
            })),
            ip_address_type: Some("ipv4"),
            members: &["Endpoints.vpc"],
        },
        Row {
            case: "a dualstack domain reached inside a VPC",
            domain: "pub-when-vpcdual",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            })),
            ip_address_type: Some("dualstack"),
            members: &["Endpoints.vpc", "Endpoints.vpcv2"],
        },
    ]
}

/// A `CreateDomain` body for one row.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "DomainName": row.domain,
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

/// Every member of `status` that carries an address, as `(member, address)` pairs sorted by
/// member: the public endpoint, its dual-stack twin, and each host of the VPC-access map
/// named `Endpoints.<key>`.
///
/// A member present but not a string is a failure rather than a skip, so a malformed answer
/// cannot read as an absent address.
fn published_members(status: &Value, case: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for member in ["Endpoint", "EndpointV2"] {
        match &status[member] {
            Value::Null => {}
            Value::String(address) => found.push((member.to_string(), address.clone())),
            other => panic!("case {case:?}: expected {member} to be a string, got {other:?}"),
        }
    }
    match &status["Endpoints"] {
        Value::Null => {}
        Value::Object(map) => {
            for (key, host) in map {
                match host {
                    Value::String(host) => {
                        found.push((format!("Endpoints.{key}"), host.clone()));
                    }
                    other => panic!(
                        "case {case:?}: expected a host string under Endpoints[{key:?}], got \
                         {other:?}"
                    ),
                }
            }
        }
        other => panic!("case {case:?}: expected an Endpoints map or none, got {other:?}"),
    }
    found.sort();
    found
}

/// `DescribeDomains` over one domain — the batch reader of a `DomainStatus`.
async fn describe_batch(emulator: &Emulator, domain: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain-info"),
            Some(json!({ "DomainNames": [domain] })),
        )
        .await
}

/// RULE: the address a domain's data plane is reached at is published only once the domain is
/// up — absent from every reader while it is still `Creating`, carried by every reader
/// afterwards, in exactly the members its access mode gives it, and still carried while a
/// later configuration change is in flight.
#[tokio::test]
async fn an_address_is_published_only_once_the_domain_is_up() {
    let emulator = Emulator::new();
    let table = rows();

    // While the domain is being provisioned: no reader carries an address, whatever access
    // mode the create asked for. Every row is created before the one wait below, so the
    // provisioning span is waited out once rather than per row.
    for row in &table {
        let case = row.case;
        let created = emulator.create(create_body(row)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateDomain, got {} with {:?}",
            created.status, created.body
        );
        let status = &created.body["DomainStatus"];
        assert_eq!(
            (
                status["Created"].as_bool(),
                status["Processing"].as_bool(),
                status["DomainProcessingStatus"].as_str(),
            ),
            (Some(true), Some(true), Some("Creating")),
            "case {case:?}: a domain the create has just answered for is created and still \
             processing, got {status:?}"
        );
        let carried = published_members(status, case);
        assert!(
            carried.is_empty(),
            "case {case:?}: CreateDomain must publish no address while the domain is \
             Creating, got {carried:?}"
        );

        for (reader, answer) in [
            ("DescribeDomain", emulator.describe(row.domain).await),
            ("DescribeDomains", describe_batch(&emulator, row.domain).await),
        ] {
            let status = match reader {
                "DescribeDomains" => answer.body["DomainStatusList"][0].clone(),
                _ => answer.body["DomainStatus"].clone(),
            };
            let carried = published_members(&status, case);
            assert!(
                carried.is_empty(),
                "case {case:?} ({reader}): must publish no address while the domain is \
                 Creating, got {carried:?}"
            );
        }
    }

    tokio::time::sleep(SETTLE_WAIT).await;

    // Once the domain is up: every reader carries an address, in exactly the members the
    // access mode gives it, and they all carry the SAME ones.
    for row in &table {
        let case = row.case;
        let settled = emulator.describe(row.domain).await;
        assert_eq!(
            settled.status, 200,
            "case {case:?}: expected 200 from DescribeDomain, got {} with {:?}",
            settled.status, settled.body
        );
        let status = &settled.body["DomainStatus"];
        assert_eq!(
            status["DomainProcessingStatus"].as_str(),
            Some("Active"),
            "case {case:?}: expected the domain to be Active once its provisioning is due, \
             got {status:?}"
        );
        let published = published_members(status, case);
        let members: Vec<&str> = published.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            members, row.members,
            "case {case:?}: expected the members {:?} to publish an address, got {published:?}",
            row.members
        );
        for (member, address) in &published {
            assert!(
                !address.is_empty(),
                "case {case:?} ({member}): expected a non-empty address, got {address:?}"
            );
        }

        // A read does not change what the next caller sees, and the batch reader reports the
        // same record as the single-domain one.
        let again = emulator.describe(row.domain).await;
        assert_eq!(
            published_members(&again.body["DomainStatus"], case),
            published,
            "case {case:?}: expected two reads to publish the same addresses"
        );
        let batch = describe_batch(&emulator, row.domain).await;
        assert_eq!(
            published_members(&batch.body["DomainStatusList"][0], case),
            published,
            "case {case:?} (DescribeDomains): expected the batch read to publish the same \
             addresses"
        );

        // A configuration change leaves the running cluster in place, so the address stays
        // published while that change is in flight.
        let updated = emulator
            .update_config(
                row.domain,
                json!({ "SnapshotOptions": { "AutomatedSnapshotStartHour": 7 } }),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateDomainConfig, got {} with {:?}",
            updated.status, updated.body
        );
        let modifying = emulator.describe(row.domain).await;
        assert_eq!(
            modifying.body["DomainStatus"]["DomainProcessingStatus"].as_str(),
            Some("Modifying"),
            "case {case:?}: expected the domain to be Modifying while the change is in \
             flight, got {:?}",
            modifying.body["DomainStatus"]
        );
        assert_eq!(
            published_members(&modifying.body["DomainStatus"], case),
            published,
            "case {case:?}: a domain applying a configuration change keeps the address it \
             publishes"
        );
    }
}
