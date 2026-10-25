// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `LicenseServers` an RDS license server endpoint reports,
//! driven over the real wire path: every call goes through the router's `dispatch`, so
//! the request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one request: a
//! license server endpoint reports the license-server hosts it was provisioned with —
//! one per subnet the registration it was created for provisions into — each carrying
//! the provisioning status of the endpoint that provisioned it, a health status that is
//! `HEALTHY` once it is provisioned and `NOT_APPLICABLE` while there is no serving host,
//! and the addresses it was given, reported from the instant it is provisioned.
//!
//! The spec this is read off (`smithy_query(query="shape",
//! shape="LicenseServerEndpoint,LicenseServer,LicenseServerHealthStatus")`):
//! `LicenseServerEndpoint$LicenseServers` is "an array of LicenseServer resources that
//! represent the license servers that are accessed through this endpoint", a
//! `LicenseServer` carries `ProvisioningStatus`, `HealthStatus` (HEALTHY | UNHEALTHY |
//! NOT_APPLICABLE) and `Ipv4Address`/`Ipv6Address`, and `Settings$Subnets` is "the
//! subnets defined for the registered identity provider".
//!
//! The rows differ along what the rule generalises over: how many subnets the
//! registration provisions into (one, two, three), whether a subnet is named twice,
//! whether the directory is an AWS-managed one or a self-managed domain, and whether
//! that domain uses IPv6. Two further suites pin the edges: a registration with no
//! subnet at all, and that the hosts an endpoint holds are the ones IT was provisioned
//! with rather than a view recomputed from its registration's current subnets.

use std::net::{Ipv4Addr, Ipv6Addr};

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, harness, provider_union, register, Harness, PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use super::license_server_endpoint_deletion::delete_endpoint;
use super::license_server_endpoints::{create_endpoint, list_endpoints, SERVER_TYPE};

/// One registration to provision an endpoint for.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The `IdentityProvider` union: an AWS-managed `DirectoryId` the harness created, or
    /// a self-managed domain named by `ActiveDirectorySettings.DomainName`.
    directory_id: Option<&'static str>,
    /// The self-managed domain's name, when the row registers one.
    domain_name: Option<&'static str>,
    /// `ActiveDirectorySettings$DomainIpv6List` — a domain that uses IPv6, so its hosts
    /// are reachable over IPv6 too.
    domain_ipv6: Option<&'static str>,
    product: &'static str,
    /// `Settings$Subnets` exactly as the registration supplies them, duplicates and all
    /// (`^subnet-[a-z0-9]{8,17}`).
    subnets: &'static [&'static str],
    /// How many DISTINCT subnets `subnets` names: the hosts the endpoint gets.
    hosts: usize,
}

/// The registrations under test. Every `d-` id is one `harness()` creates in the
/// account's directory fleet.
const ROWS: &[Row] = &[
    Row {
        case: "aws-managed directory, one subnet",
        directory_id: Some("d-0123456789"),
        domain_name: None,
        domain_ipv6: None,
        product: PRODUCTS[4],
        subnets: &["subnet-0123456789abcdef0"],
        hosts: 1,
    },
    Row {
        case: "aws-managed directory, two subnets",
        directory_id: Some("d-00000000ff"),
        domain_name: None,
        domain_ipv6: None,
        product: PRODUCTS[3],
        subnets: &["subnet-1111111111111111a", "subnet-2222222222222222b"],
        hosts: 2,
    },
    Row {
        case: "aws-managed directory, three subnets with one named twice",
        directory_id: Some("d-123456789a"),
        domain_name: None,
        domain_ipv6: None,
        product: PRODUCTS[0],
        subnets: &[
            "subnet-3333333333333333c",
            "subnet-4444444444444444d",
            "subnet-3333333333333333c",
            "subnet-5555555555555555e",
        ],
        hosts: 3,
    },
    Row {
        case: "self-managed ipv4-only domain, one subnet",
        directory_id: None,
        domain_name: Some("corp.example.com"),
        domain_ipv6: None,
        product: PRODUCTS[1],
        subnets: &["subnet-6666666666666666f"],
        hosts: 1,
    },
    Row {
        case: "self-managed dual-stack domain, two subnets",
        directory_id: None,
        domain_name: Some("dual.example.com"),
        domain_ipv6: Some("2001:db8:1:2::1"),
        product: PRODUCTS[2],
        subnets: &["subnet-7777777777777777a", "subnet-8888888888888888b"],
        hosts: 2,
    },
];

impl Row {
    /// The provider union this row registers.
    fn provider(&self) -> Value {
        match (self.directory_id, self.domain_name) {
            (Some(directory_id), _) => provider_union(directory_id),
            (None, Some(domain_name)) => {
                let mut settings = json!({
                    "DomainName": domain_name,
                    // `ActiveDirectorySettings$DomainIpv4List` (@length(min: 1, max: 2)).
                    "DomainIpv4List": ["10.200.0.10"],
                });
                if let Some(ipv6) = self.domain_ipv6 {
                    settings["DomainIpv6List"] = json!([ipv6]);
                }
                json!({
                    "ActiveDirectoryIdentityProvider": {
                        "ActiveDirectorySettings": settings,
                        "ActiveDirectoryType": "SELF_MANAGED",
                    }
                })
            }
            // Every row names one of the two, so the provider always identifies a
            // directory; a union naming neither is refused by the service itself.
            (None, None) => unreachable!("a row registers a directory id or a domain"),
        }
    }

    fn registration(&self) -> Value {
        json!({
            "IdentityProvider": self.provider(),
            "Product": self.product,
            "Settings": {
                "Subnets": self.subnets,
                "SecurityGroupId": "sg-0123abcd",
            },
        })
    }

    /// Whether this row's hosts are reachable over IPv6: exactly when its domain recorded
    /// an IPv6 address.
    fn dual_stack(&self) -> bool {
        self.domain_ipv6.is_some()
    }
}

/// The create request for an RDS SAL endpoint of `identity_provider_arn`.
fn endpoint_request(identity_provider_arn: &str) -> Value {
    json!({
        "IdentityProviderArn": identity_provider_arn,
        "LicenseServerSettings": {
            "ServerType": SERVER_TYPE,
            "ServerSettings": {
                "RdsSalSettings": {
                    "RdsSalCredentialsProvider": {
                        "SecretsManagerCredentialsProvider": { "SecretId": "rds-sal-admin" },
                    },
                },
            },
        },
    })
}

/// Register `row`'s identity provider and create its endpoint, answering the
/// `(IdentityProviderArn, LicenseServerEndpointArn)` the two calls minted.
async fn provision(h: &Harness, row: &Row) -> (String, String) {
    let registered = register(h, &row.registration()).await;
    let provider_arn = as_str(
        &registered.ok(row.case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        row.case,
    )
    .to_string();
    let created = create_endpoint(h, &endpoint_request(&provider_arn)).await;
    let endpoint_arn = as_str(created.ok(row.case), "LicenseServerEndpointArn", row.case).to_string();
    (provider_arn, endpoint_arn)
}

/// The `LicenseServers` the list reports for `endpoint_arn`.
async fn listed_servers(h: &Harness, endpoint_arn: &str, case: &str) -> Vec<Value> {
    let reply = list_endpoints(h, &json!({})).await;
    let body = reply.ok(case);
    let endpoint = body["LicenseServerEndpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: LicenseServerEndpoints is reported, got {body:?}"))
        .iter()
        .find(|endpoint| {
            endpoint.get("LicenseServerEndpointArn").and_then(Value::as_str) == Some(endpoint_arn)
        })
        .unwrap_or_else(|| panic!("case {case:?}: the list reports {endpoint_arn}, got {body:?}"))
        .clone();
    endpoint["LicenseServers"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: LicenseServers is reported for every endpoint, got {endpoint:?}")
        })
        .clone()
}

/// Assert every server in `servers` reports `status`, the health that status implies and
/// the addresses that status implies, and answer the addresses reported.
///
/// The addresses are the service's own allocation, so what is asserted is their FORM: an
/// IPv4 address in the private block a host in the account's VPC has, and — exactly when
/// the domain uses IPv6 — a unique-local IPv6 address (RFC 4193 `fc00::/7`).
fn assert_servers(
    servers: &[Value],
    expected_hosts: usize,
    status: &str,
    dual_stack: bool,
    case: &str,
) -> Vec<String> {
    assert_eq!(
        servers.len(),
        expected_hosts,
        "case {case:?}: one license server per subnet the registration provisions into, \
         expected {expected_hosts}, got {servers:?}"
    );
    let provisioned = status == "PROVISIONED";
    let mut addresses = Vec::new();
    for server in servers {
        assert_eq!(
            server.get("ProvisioningStatus").and_then(Value::as_str),
            Some(status),
            "case {case:?}: a license server carries the status of the endpoint that \
             provisioned it, got {server:?}"
        );
        assert_eq!(
            server.get("HealthStatus").and_then(Value::as_str),
            Some(if provisioned {
                "HEALTHY"
            } else {
                "NOT_APPLICABLE"
            }),
            "case {case:?}: health is a verdict about a serving host, got {server:?}"
        );
        let ipv4 = server.get("Ipv4Address").and_then(Value::as_str);
        let ipv6 = server.get("Ipv6Address").and_then(Value::as_str);
        if !provisioned {
            assert_eq!(
                (ipv4, ipv6),
                (None, None),
                "case {case:?}: there is no host to reach before provisioning completed, \
                 got {server:?}"
            );
            continue;
        }
        let ipv4 = ipv4
            .unwrap_or_else(|| panic!("case {case:?}: a provisioned host has an address: {server:?}"))
            .to_string();
        let parsed: Ipv4Addr = ipv4
            .parse()
            .unwrap_or_else(|error| panic!("case {case:?}: {ipv4:?} is not an IPv4 address: {error}"));
        assert!(
            parsed.is_private() && parsed.octets()[0] == 10,
            "case {case:?}: a host in the account's VPC has a private address, got {ipv4:?}"
        );
        assert!(
            parsed.octets()[3] != 0 && parsed.octets()[3] != 255,
            "case {case:?}: an allocation is neither the network nor the broadcast \
             address of its /24, got {ipv4:?}"
        );
        addresses.push(ipv4);
        match (dual_stack, ipv6) {
            (true, Some(ipv6)) => {
                let parsed: Ipv6Addr = ipv6.parse().unwrap_or_else(|error| {
                    panic!("case {case:?}: {ipv6:?} is not an IPv6 address: {error}")
                });
                assert_eq!(
                    parsed.segments()[0] & 0xfe00,
                    0xfc00,
                    "case {case:?}: a host address is a unique-local one, got {ipv6:?}"
                );
                addresses.push(ipv6.to_string());
            }
            (true, None) => panic!(
                "case {case:?}: a host in a domain that uses IPv6 has an IPv6 address, \
                 got {server:?}"
            ),
            (false, reported) => assert_eq!(
                reported, None,
                "case {case:?}: an IPv4-only domain reports no IPv6 address, got {server:?}"
            ),
        }
    }
    addresses
}

/// RULE: an endpoint reports one license server per subnet its registration provisions
/// into, each in the endpoint's own provisioning status — so a client polling the list
/// watches the hosts behind the endpoint come up, and reads their health and addresses
/// once they are provisioned.
#[tokio::test]
async fn an_endpoint_reports_one_license_server_per_subnet_it_provisions_into() {
    let h = harness();
    let mut provisioned = Vec::new();
    for row in ROWS {
        let (_, endpoint_arn) = provision(&h, row).await;
        provisioned.push((row, endpoint_arn));
    }

    // A caller reading straight after the create still sees the transient status, so the
    // hosts are reported in it: no health verdict and no address yet.
    for (row, endpoint_arn) in &provisioned {
        let servers = listed_servers(&h, endpoint_arn, row.case).await;
        assert_servers(
            &servers,
            row.hosts,
            "PROVISIONING",
            row.dual_stack(),
            row.case,
        );
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let mut every_address: Vec<String> = Vec::new();
    for (row, endpoint_arn) in &provisioned {
        let servers = listed_servers(&h, endpoint_arn, row.case).await;
        let addresses = assert_servers(
            &servers,
            row.hosts,
            "PROVISIONED",
            row.dual_stack(),
            row.case,
        );
        // A read must not change what a later caller sees: the same read reports the same
        // addresses.
        let again = listed_servers(&h, endpoint_arn, row.case).await;
        let repeated = assert_servers(
            &again,
            row.hosts,
            "PROVISIONED",
            row.dual_stack(),
            row.case,
        );
        assert_eq!(
            addresses, repeated,
            "case {:?}: a host keeps the address it was allocated, got {again:?}",
            row.case
        );
        every_address.extend(addresses);
    }

    let mut unique = every_address.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        every_address.len(),
        "no two hosts of the account share an address, got {every_address:?}"
    );
}

/// RULE: the hosts come from the subnets the registration provisions into — a
/// registration that supplied no `Settings` provisions into none, so its endpoint reports
/// no license server rather than a host in a subnet nobody named.
#[tokio::test]
async fn a_registration_with_no_subnet_provisions_no_license_server() {
    let h = harness();
    let case = "a registration that supplied no Settings";
    // `Settings` is optional on the wire, so this registration carries no subnet at all.
    let registered = register(
        &h,
        &json!({
            "IdentityProvider": provider_union("d-0123456789"),
            "Product": PRODUCTS[4],
        }),
    )
    .await;
    let provider_arn = as_str(
        &registered.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string();
    let created = create_endpoint(&h, &endpoint_request(&provider_arn)).await;
    let endpoint_arn = as_str(created.ok(case), "LicenseServerEndpointArn", case).to_string();

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let servers = listed_servers(&h, &endpoint_arn, case).await;
    assert_servers(&servers, 0, "PROVISIONED", false, case);
}

/// RULE: an endpoint reports the hosts IT was provisioned with — changing its
/// registration's subnets afterwards does not re-provision an endpoint that already
/// exists, so the hosts and their addresses are what the create persisted.
#[tokio::test]
async fn the_hosts_an_endpoint_was_provisioned_with_outlive_a_settings_update() {
    let h = harness();
    let row = &ROWS[0];
    let (provider_arn, endpoint_arn) = provision(&h, row).await;

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let before = listed_servers(&h, &endpoint_arn, row.case).await;
    let addresses = assert_servers(
        &before,
        row.hosts,
        "PROVISIONED",
        row.dual_stack(),
        row.case,
    );

    let updated = call(
        &h,
        "/identity-provider/UpdateIdentityProviderSettings",
        &json!({
            "IdentityProviderArn": provider_arn,
            "UpdateSettings": {
                "AddSubnets": ["subnet-9999999999999999a", "subnet-aaaaaaaaaaaaaaaab"],
                "RemoveSubnets": [],
            },
        }),
    )
    .await;
    updated.ok("two subnets added to the registration");

    let after = listed_servers(&h, &endpoint_arn, row.case).await;
    let unchanged = assert_servers(
        &after,
        row.hosts,
        "PROVISIONED",
        row.dual_stack(),
        row.case,
    );
    assert_eq!(
        addresses, unchanged,
        "case {:?}: the endpoint keeps the hosts it was provisioned with, got {after:?}",
        row.case
    );
}

/// RULE: a license server is torn down WITH its endpoint — a deletion moves every host
/// into the endpoint's `DELETING` and then `DELETED` status, and the addresses stop being
/// reported because there is no host left to reach.
#[tokio::test]
async fn deleting_an_endpoint_tears_down_the_license_servers_behind_it() {
    let h = harness();
    for row in ROWS {
        let (_, endpoint_arn) = provision(&h, row).await;
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        assert_servers(
            &listed_servers(&h, &endpoint_arn, row.case).await,
            row.hosts,
            "PROVISIONED",
            row.dual_stack(),
            row.case,
        );

        let deleted = delete_endpoint(&h, &endpoint_arn).await;
        let answered = deleted.ok(row.case)["LicenseServerEndpoint"]["LicenseServers"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the deletion answers the endpoint's license servers, got {:?}",
                    row.case, deleted.body
                )
            })
            .clone();
        assert_servers(
            &answered,
            row.hosts,
            "DELETING",
            row.dual_stack(),
            row.case,
        );
        assert_servers(
            &listed_servers(&h, &endpoint_arn, row.case).await,
            row.hosts,
            "DELETING",
            row.dual_stack(),
            row.case,
        );

        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        assert_servers(
            &listed_servers(&h, &endpoint_arn, row.case).await,
            row.hosts,
            "DELETED",
            row.dual_stack(),
            row.case,
        );
    }
}
