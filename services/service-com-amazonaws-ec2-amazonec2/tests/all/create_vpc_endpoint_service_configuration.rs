// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateVpcEndpointServiceConfiguration` and the
//! `DescribeVpcEndpointServiceConfigurations` that reads back what it stored, driven over
//! `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a create stores the endpoint service the request configures — the load
//! balancers behind it, whether a connection to it must be accepted, the address types and the
//! Regions it is offered from, the private DNS name assigned to it and its tags — mints it a
//! `vpce-svc-` `ServiceId`, DERIVES its service name, its base endpoint DNS name and its private
//! DNS verification record from that id, reports it `Available`, and the describes then report
//! that stored service: in full through `DescribeVpcEndpointServiceConfigurations`, and as a
//! service a consumer can create an endpoint to through `DescribeVpcEndpointServices`; a request
//! the service cannot satisfy is refused with the EC2 code for its cause and stores nothing.
//!
//! Each table row is a distinct input along what that rule generalises over: which kind of load
//! balancer is behind the service, how many of them, whether acceptance was waived or defaulted,
//! which address types and Regions were asked for, whether a private DNS name was assigned, and
//! which member of the request is at fault.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed};
use super::wire::{Param, is_long_id, param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The emulator the tests drive. The store is reached only through the wire here, so the
/// engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The text of every `<item>` of the first set element named `set`.
///
/// Read through the XML parser rather than by substring, so the items of ONE set are never
/// confused with the identically-shaped items of another set of the same response.
fn set_items(case: &str, body: &str, set: &str) -> Vec<String> {
    let document = super::wire::document(case, body);
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set)
        .map(|node| {
            node.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `name` of every `<item>` of the first set element named `set` — for a set whose items are
/// structures (`serviceType`, `supportedRegionSet`, `tagSet`).
fn set_members(case: &str, body: &str, set: &str, name: &str) -> Vec<String> {
    let document = super::wire::document(case, body);
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set)
        .map(|node| {
            node.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .filter_map(|item| super::wire::child(item, name))
                .map(|found| found.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `name` member of the one `parent` structure a response carries.
fn nested(case: &str, body: &str, parent: &str, name: &str) -> Option<String> {
    let document = super::wire::document(case, body);
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == parent)
        .and_then(|node| super::wire::child_text(node, name))
}

/// The ARN of the load balancer `name` of `kind` — `net` for a Network Load Balancer and `gwy`
/// for a Gateway Load Balancer, the resource paths Elastic Load Balancing gives each.
///
/// A load balancer is a resource of another service, so the ARN is only ever a well-formed ARN
/// here: what this crate stores is the ARN the request named.
fn load_balancer_arn(kind: &str, name: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{account}:loadbalancer/{kind}/{name}/0123456789abcdef",
        account = engine::emulator_account_id(),
    )
}

/// The `ServiceId`s `DescribeVpcEndpointServices` reports for the services this ACCOUNT offers —
/// selected by the `owner` filter, which a service of the Region's own catalog answers with the
/// `amazon` alias rather than an account id.
async fn account_services(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeVpcEndpointServices",
        &filters(&[("owner", &[engine::emulator_account_id()])]),
    )
    .await;
    elements(&body, "serviceId")
}

/// The body `DescribeVpcEndpointServiceConfigurations` answers for `params`.
async fn describe_configurations(
    case: &str,
    router: &MultiServiceRouter,
    params: &[Param],
) -> String {
    ok(
        case,
        router,
        "DescribeVpcEndpointServiceConfigurations",
        params,
    )
    .await
}

// ---------------------------------------------------------------------------
// What a create stores
// ---------------------------------------------------------------------------

/// What one row of the create table asks for, and what the service it configures must report.
struct CreateCase {
    case: &'static str,
    /// The Network Load Balancers behind the service, by name.
    network: &'static [&'static str],
    /// The Gateway Load Balancers behind it, by name.
    gateway: &'static [&'static str],
    /// `AcceptanceRequired`, or `None` to leave the member out.
    acceptance_required: Option<bool>,
    /// `SupportedIpAddressTypes`, which may be left empty.
    ip_address_types: &'static [&'static str],
    /// `SupportedRegions`, which may be left empty.
    supported_regions: &'static [&'static str],
    /// `PrivateDnsName`, or `None` to leave the member out.
    private_dns_name: Option<&'static str>,
    /// The tags to assign, as key/value pairs.
    tags: &'static [(&'static str, &'static str)],
    /// The `ServiceType` the configured service must report.
    reported_type: &'static str,
    /// The `AcceptanceRequired` it must report.
    reported_acceptance: bool,
    /// The `SupportedIpAddressTypes` it must report.
    reported_ip_address_types: &'static [&'static str],
}

/// The rows: a service behind one Network Load Balancer with nothing else asked for, one behind
/// two of them with acceptance waived, both address types and tags, one behind a Gateway Load
/// Balancer, one with a private DNS name to prove ownership of, and one offered from other
/// Regions.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "one Network Load Balancer and nothing else",
            network: &["alpha"],
            gateway: &[],
            acceptance_required: None,
            ip_address_types: &[],
            supported_regions: &[],
            private_dns_name: None,
            tags: &[],
            reported_type: "Interface",
            reported_acceptance: true,
            reported_ip_address_types: &["ipv4"],
        },
        CreateCase {
            case: "two Network Load Balancers, acceptance waived, both address types, tagged",
            network: &["beta", "gamma"],
            gateway: &[],
            acceptance_required: Some(false),
            ip_address_types: &["ipv4", "ipv6"],
            supported_regions: &[],
            private_dns_name: None,
            tags: &[("Owner", "TeamA"), ("Tier", "shared")],
            reported_type: "Interface",
            reported_acceptance: false,
            reported_ip_address_types: &["ipv4", "ipv6"],
        },
        CreateCase {
            case: "a Gateway Load Balancer, which consumers reach as a route target",
            network: &[],
            gateway: &["delta"],
            acceptance_required: Some(true),
            ip_address_types: &[],
            supported_regions: &[],
            private_dns_name: None,
            tags: &[],
            reported_type: "GatewayLoadBalancer",
            reported_acceptance: true,
            reported_ip_address_types: &["ipv4"],
        },
        CreateCase {
            case: "a private DNS name the owner has still to prove it owns",
            network: &["epsilon"],
            gateway: &[],
            acceptance_required: Some(false),
            ip_address_types: &["ipv6"],
            supported_regions: &[],
            private_dns_name: Some("payments.example.com"),
            tags: &[],
            reported_type: "Interface",
            reported_acceptance: false,
            reported_ip_address_types: &["ipv6"],
        },
        CreateCase {
            case: "offered from two other Regions as well as its own",
            network: &["zeta"],
            gateway: &[],
            acceptance_required: Some(false),
            ip_address_types: &[],
            supported_regions: &["us-east-1", "eu-west-1"],
            private_dns_name: Some("shared.example.org"),
            tags: &[("Project", "privatelink")],
            reported_type: "Interface",
            reported_acceptance: false,
            reported_ip_address_types: &["ipv4"],
        },
    ]
}

/// The form fields one row of the create table sends.
fn create_params(row: &CreateCase, network: &[String], gateway: &[String]) -> Vec<Param> {
    let mut params: Vec<Param> = Vec::new();
    params.extend(indexed(
        "NetworkLoadBalancerArn",
        &network.iter().map(String::as_str).collect::<Vec<&str>>(),
    ));
    params.extend(indexed(
        "GatewayLoadBalancerArn",
        &gateway.iter().map(String::as_str).collect::<Vec<&str>>(),
    ));
    if let Some(acceptance_required) = row.acceptance_required {
        params.push(param("AcceptanceRequired", &acceptance_required.to_string()));
    }
    params.extend(indexed("SupportedIpAddressType", row.ip_address_types));
    params.extend(indexed("SupportedRegion", row.supported_regions));
    if let Some(private_dns_name) = row.private_dns_name {
        params.push(param("PrivateDnsName", private_dns_name));
    }
    for (index, (key, value)) in row.tags.iter().enumerate() {
        params.push(param("TagSpecification.1.ResourceType", "vpc-endpoint-service"));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// Every row is created, answered with the record the store holds, and read back through
/// `DescribeVpcEndpointServices` — so the answer is what was stored rather than values assembled
/// for the reply.
#[tokio::test]
async fn each_endpoint_service_is_configured_persisted_and_read_back() {
    let router = emulator();
    let zones = ec2::state::availability_zones_in_region(REGION);
    let expected_zones: Vec<String> = zones.iter().map(|zone| zone.zone_name.clone()).collect();
    let expected_zone_ids: Vec<String> = zones.iter().map(|zone| zone.zone_id.clone()).collect();

    let mut created: Vec<String> = Vec::new();
    for row in create_cases() {
        let case = row.case;
        let network: Vec<String> = row
            .network
            .iter()
            .map(|name| load_balancer_arn("net", name))
            .collect();
        let gateway: Vec<String> = row
            .gateway
            .iter()
            .map(|name| load_balancer_arn("gwy", name))
            .collect();
        let params = create_params(&row, &network, &gateway);

        let body = ok(
            case,
            &router,
            "CreateVpcEndpointServiceConfiguration",
            &params,
        )
        .await;
        let service_id = member(case, &body, "serviceId");
        assert!(
            is_long_id("vpce-svc", &service_id),
            "case {case:?}: expected a minted vpce-svc- id, got {service_id:?}"
        );
        assert!(
            !created.contains(&service_id),
            "case {case:?}: expected a fresh id, got {service_id:?} again"
        );
        created.push(service_id.clone());

        // The name, the base endpoint DNS name and the verification record are DERIVED from
        // the id this create minted, so each is that id's own string and no other.
        let service_name = member(case, &body, "serviceName");
        assert_eq!(
            service_name,
            format!("com.amazonaws.vpce.{REGION}.{service_id}"),
            "case {case:?}: expected the service name to carry its own id, got {body}"
        );
        for (element_name, expected) in [
            ("serviceState", "Available"),
            ("acceptanceRequired", &row.reported_acceptance.to_string()),
            ("managesVpcEndpoints", "false"),
            ("payerResponsibility", "ServiceOwner"),
        ] {
            assert_eq!(
                element(&body, element_name).as_deref(),
                Some(expected),
                "case {case:?}: expected <{element_name}> to carry {expected:?}, got {body}"
            );
        }
        assert_eq!(
            set_members(case, &body, "serviceType", "serviceType"),
            vec![row.reported_type.to_string()],
            "case {case:?}: expected the service type the load balancers decide, got {body}"
        );
        assert_eq!(
            set_items(case, &body, "availabilityZoneSet"),
            expected_zones,
            "case {case:?}: expected the zones of {REGION}, got {body}"
        );
        assert_eq!(
            set_items(case, &body, "availabilityZoneIdSet"),
            expected_zone_ids,
            "case {case:?}: expected the zone ids of {REGION}, got {body}"
        );
        assert_eq!(
            set_items(case, &body, "networkLoadBalancerArnSet"),
            network,
            "case {case:?}: expected the Network Load Balancers the request named, got {body}"
        );
        assert_eq!(
            set_items(case, &body, "gatewayLoadBalancerArnSet"),
            gateway,
            "case {case:?}: expected the Gateway Load Balancers the request named, got {body}"
        );
        assert_eq!(
            set_items(case, &body, "supportedIpAddressTypeSet"),
            row.reported_ip_address_types
                .iter()
                .map(|address_type| (*address_type).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the supported address types, got {body}"
        );
        // "The DNS names for the service": the base name an interface endpoint's own names sit
        // under, and nothing for a service behind a Gateway Load Balancer.
        let expected_base_names: Vec<String> = if row.reported_type == "Interface" {
            vec![format!("{service_id}.{REGION}.vpce.amazonaws.com")]
        } else {
            Vec::new()
        };
        assert_eq!(
            set_items(case, &body, "baseEndpointDnsNameSet"),
            expected_base_names,
            "case {case:?}: expected the base endpoint DNS names, got {body}"
        );
        // "The supported Regions", each in the state the service is in there.
        assert_eq!(
            set_members(case, &body, "supportedRegionSet", "region"),
            row.supported_regions
                .iter()
                .map(|region| (*region).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the supported Regions the request named, got {body}"
        );
        assert_eq!(
            element(&body, "remoteAccessEnabled").as_deref(),
            Some(if row.supported_regions.is_empty() {
                "false"
            } else {
                "true"
            }),
            "case {case:?}: remote access is enabled exactly when another Region was named, \
             got {body}"
        );
        // "If you set the private DNS name, you must prove that you own the private DNS domain
        // name": the name is stored and its verification is still pending, with the record the
        // owner has to publish.
        match row.private_dns_name {
            None => assert_eq!(
                element(&body, "privateDnsName"),
                None,
                "case {case:?}: expected no private DNS name at all, got {body}"
            ),
            Some(expected) => {
                assert_eq!(
                    element(&body, "privateDnsName").as_deref(),
                    Some(expected),
                    "case {case:?}: expected the private DNS name the request named, got {body}"
                );
                assert_eq!(
                    nested(case, &body, "privateDnsNameConfiguration", "state").as_deref(),
                    Some("pendingVerification"),
                    "case {case:?}: expected an unproven private DNS name, got {body}"
                );
                assert_eq!(
                    nested(case, &body, "privateDnsNameConfiguration", "type").as_deref(),
                    Some("TXT"),
                    "case {case:?}: expected a TXT verification record, got {body}"
                );
                let record = nested(case, &body, "privateDnsNameConfiguration", "name")
                    .unwrap_or_default();
                let value = nested(case, &body, "privateDnsNameConfiguration", "value")
                    .unwrap_or_default();
                assert!(
                    record.starts_with('_') && value.starts_with("vpce:") && record[1..] != value[5..],
                    "case {case:?}: expected a subdomain and a distinct value to publish, \
                     got {record:?} and {value:?}"
                );
            }
        }
        assert_eq!(
            set_members(case, &body, "tagSet", "key"),
            row.tags
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the tags the request assigned, got {body}"
        );

        // The round trip: the service the store now holds is one a consumer can create an
        // endpoint to, reported by the describe with the same id, owner and settings.
        let described = ok(
            case,
            &router,
            "DescribeVpcEndpointServices",
            &indexed("ServiceName", &[service_name.as_str()]),
        )
        .await;
        for (element_name, expected) in [
            ("serviceId", service_id.as_str()),
            ("serviceName", service_name.as_str()),
            ("owner", engine::emulator_account_id()),
            ("serviceRegion", REGION),
            ("acceptanceRequired", &row.reported_acceptance.to_string()),
        ] {
            assert_eq!(
                element(&described, element_name).as_deref(),
                Some(expected),
                "case {case:?}: expected the describe to report <{element_name}> as \
                 {expected:?}, got {described}"
            );
        }
        assert_eq!(
            set_members(case, &described, "serviceType", "serviceType"),
            vec![row.reported_type.to_string()],
            "case {case:?}: expected the describe to report the stored service type, \
             got {described}"
        );
        assert_eq!(
            element(&described, "privateDnsName").as_deref(),
            row.private_dns_name,
            "case {case:?}: expected the describe to report the stored private DNS name, \
             got {described}"
        );
        assert_eq!(
            element(&described, "privateDnsNameVerificationState").as_deref(),
            row.private_dns_name.map(|_| "pendingVerification"),
            "case {case:?}: expected the describe to report the stored verification state, \
             got {described}"
        );
        assert_eq!(
            set_items(case, &described, "supportedIpAddressTypeSet"),
            row.reported_ip_address_types
                .iter()
                .map(|address_type| (*address_type).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the describe to report the stored address types, \
             got {described}"
        );

        // The service's own describe reports the whole configuration, so every member the
        // create answered is read back off the record the store holds.
        let configuration = describe_configurations(
            case,
            &router,
            &indexed("ServiceId", &[service_id.as_str()]),
        )
        .await;
        for element_name in [
            "serviceId",
            "serviceName",
            "serviceState",
            "acceptanceRequired",
            "managesVpcEndpoints",
            "payerResponsibility",
            "remoteAccessEnabled",
        ] {
            assert_eq!(
                element(&configuration, element_name),
                element(&body, element_name),
                "case {case:?}: expected the configuration describe to report \
                 <{element_name}> as the create did, got {configuration}"
            );
        }
        for set in [
            "networkLoadBalancerArnSet",
            "gatewayLoadBalancerArnSet",
            "baseEndpointDnsNameSet",
            "supportedIpAddressTypeSet",
            "availabilityZoneSet",
        ] {
            assert_eq!(
                set_items(case, &configuration, set),
                set_items(case, &body, set),
                "case {case:?}: expected the configuration describe to report <{set}> as \
                 the create did, got {configuration}"
            );
        }
        assert_eq!(
            nested(case, &configuration, "privateDnsNameConfiguration", "value"),
            nested(case, &body, "privateDnsNameConfiguration", "value"),
            "case {case:?}: expected the stored verification record, got {configuration}"
        );
        assert_eq!(
            set_members(case, &configuration, "tagSet", "value"),
            row.tags
                .iter()
                .map(|(_, value)| (*value).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the stored tags, got {configuration}"
        );
    }

    // Every row is one stored service, and the store holds exactly those.
    assert_eq!(
        account_services("the create table", &router).await,
        {
            let mut ids = created.clone();
            ids.sort();
            ids
        },
        "expected the account to offer exactly the services the table created"
    );
}

// ---------------------------------------------------------------------------
// What a create refuses
// ---------------------------------------------------------------------------

/// One request that cannot be served, and the EC2 code it must be refused with.
struct RefusalCase {
    case: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// The rows: every member whose value or combination the service cannot honour, plus the dry run
/// of a request that would have succeeded.
fn refusal_cases() -> Vec<RefusalCase> {
    let network = load_balancer_arn("net", "alpha");
    let gateway = load_balancer_arn("gwy", "delta");
    vec![
        RefusalCase {
            case: "no load balancer behind the service at all",
            params: vec![param("AcceptanceRequired", "false")],
            status: 400,
            // There are two members one of which the request has to carry, and neither is
            // required on its own, so the refusal is about the VALUES the request named —
            // the code this operation is recorded answering such a request with.
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "both kinds of load balancer at once",
            params: vec![
                param("NetworkLoadBalancerArn.1", &network),
                param("GatewayLoadBalancerArn.1", &gateway),
            ],
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a load balancer named by something that is not an ARN",
            params: vec![param("NetworkLoadBalancerArn.1", "my-load-balancer")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "an ARN that addresses something other than a load balancer",
            params: vec![param(
                "NetworkLoadBalancerArn.1",
                "arn:aws:ec2:us-west-2:123456789012:vpc/vpc-01234567890abcdef",
            )],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "an address type that is neither ipv4 nor ipv6",
            params: vec![
                param("NetworkLoadBalancerArn.1", &network),
                param("SupportedIpAddressType.1", "ipv5"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a supported Region that is not a Region",
            params: vec![
                param("NetworkLoadBalancerArn.1", &network),
                param("SupportedRegion.1", "atlantis-1"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a private DNS name that is not a domain name",
            params: vec![
                param("NetworkLoadBalancerArn.1", &network),
                param("PrivateDnsName", "payments"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a private DNS name on a service behind a Gateway Load Balancer",
            params: vec![
                param("GatewayLoadBalancerArn.1", &gateway),
                param("PrivateDnsName", "payments.example.com"),
            ],
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a tag specification for another resource type",
            params: vec![
                param("NetworkLoadBalancerArn.1", &network),
                param("TagSpecification.1.ResourceType", "vpc-endpoint"),
                param("TagSpecification.1.Tag.1.Key", "Owner"),
                param("TagSpecification.1.Tag.1.Value", "TeamA"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a dry run of a request that would have succeeded",
            params: vec![
                param("NetworkLoadBalancerArn.1", &network),
                param("DryRun", "true"),
            ],
            status: 412,
            code: "DryRunOperation",
        },
    ]
}

/// Every refused row answers its own code and leaves the account offering exactly the services it
/// offered before — a refusal stores nothing.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_and_stores_nothing() {
    let router = emulator();
    let before = account_services("the refusal table", &router).await;

    for row in refusal_cases() {
        let case = row.case;
        let (status, body) = call(&router, "CreateVpcEndpointServiceConfiguration", &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {:?}, got {body}",
            row.code
        );
        assert_eq!(
            account_services(case, &router).await,
            before,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the request":
/// a repeat of one request answers the service it configured then and offers nothing a second
/// time, while the same token with a different request is refused.
#[tokio::test]
async fn one_client_token_configures_one_service() {
    let router = emulator();
    let network = load_balancer_arn("net", "alpha");
    let first = vec![
        param("NetworkLoadBalancerArn.1", &network),
        param("AcceptanceRequired", "false"),
        param("ClientToken", "token-alpha"),
    ];

    let case = "the first call";
    let body = ok(
        case,
        &router,
        "CreateVpcEndpointServiceConfiguration",
        &first,
    )
    .await;
    let service_id = member(case, &body, "serviceId");
    assert_eq!(
        element(&body, "clientToken").as_deref(),
        Some("token-alpha"),
        "case {case:?}: expected the client token echoed back, got {body}"
    );

    let case = "the same request again";
    let repeated = ok(
        case,
        &router,
        "CreateVpcEndpointServiceConfiguration",
        &first,
    )
    .await;
    assert_eq!(
        element(&repeated, "serviceId").as_deref(),
        Some(service_id.as_str()),
        "case {case:?}: expected the service the first call configured, got {repeated}"
    );
    assert_eq!(
        account_services(case, &router).await,
        vec![service_id.clone()],
        "case {case:?}: a repeat must configure no second service"
    );

    let case = "the same token for a different request";
    let (status, body) = call(
        &router,
        "CreateVpcEndpointServiceConfiguration",
        &[
            param("NetworkLoadBalancerArn.1", &network),
            param("AcceptanceRequired", "true"),
            param("ClientToken", "token-alpha"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IdempotentParameterMismatch",
        "case {case:?}: expected the idempotency refusal, got {body}"
    );
    assert_eq!(
        account_services(case, &router).await,
        vec![service_id],
        "case {case:?}: a refused repeat must configure nothing"
    );
}

// ---------------------------------------------------------------------------
// What the describe reports out of that store
// ---------------------------------------------------------------------------

/// How many services the paging rows below configure: one more than the smallest page this
/// describe serves, so one page cannot hold them all.
const CONFIGURED: usize = 6;

/// The index of the service the rows below tag, and of the one they offer over IPv6 — two
/// distinct services, so a filter on either selects exactly one.
const TAGGED: usize = 2;
const IPV6: usize = 3;

/// Configure [`CONFIGURED`] services through the API and answer their ids in `ServiceId` order,
/// which is the order the describe reports and pages over.
async fn configure_services(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::with_capacity(CONFIGURED);
    for index in 0..CONFIGURED {
        let arn = load_balancer_arn("net", &format!("lb-{index}"));
        let mut params = vec![param("NetworkLoadBalancerArn.1", &arn)];
        if index == TAGGED {
            params.push(param("TagSpecification.1.ResourceType", "vpc-endpoint-service"));
            params.push(param("TagSpecification.1.Tag.1.Key", "Owner"));
            params.push(param("TagSpecification.1.Tag.1.Value", "TeamA"));
        }
        if index == IPV6 {
            params.push(param("SupportedIpAddressType.1", "ipv6"));
        }
        let body = ok(
            case,
            router,
            "CreateVpcEndpointServiceConfiguration",
            &params,
        )
        .await;
        ids.push(member(case, &body, "serviceId"));
    }
    ids.sort();
    ids
}

/// The describe reports the account's own configurations, narrows them by `ServiceIds` and by
/// each documented filter, and pages them in `ServiceId` order.
#[tokio::test]
async fn the_describe_selects_filters_and_pages_the_stored_configurations() {
    let router = emulator();
    let ids = configure_services("the describe table", &router).await;
    let tagged = ids[TAGGED].clone();
    let over_ipv6 = ids[IPV6].clone();
    let named = describe_configurations(
        "the service name of one row",
        &router,
        &indexed("ServiceId", &[tagged.as_str()]),
    )
    .await;
    let tagged_name = member("the service name of one row", &named, "serviceName");

    // Each row: what the request selects on, and which of the configured services it must
    // report. `service-state` holds for every one of them, a state nothing is in holds for
    // none, and the two single-service filters select the row they were set on.
    let rows: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        ("no argument at all describes every one", Vec::new(), ids.clone()),
        (
            "the ids of two of them",
            indexed("ServiceId", &[ids[0].as_str(), ids[1].as_str()]),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "the service-id filter",
            filters(&[("service-id", &[tagged.as_str()])]),
            vec![tagged.clone()],
        ),
        (
            "the service-name filter",
            filters(&[("service-name", &[tagged_name.as_str()])]),
            vec![tagged.clone()],
        ),
        (
            "the service-state every configured service is in",
            filters(&[("service-state", &["Available"])]),
            ids.clone(),
        ),
        (
            "a service-state none of them is in",
            filters(&[("service-state", &["Deleted"])]),
            Vec::new(),
        ),
        (
            "the tag the one tagged service carries",
            filters(&[("tag:Owner", &["TeamA"])]),
            vec![tagged.clone()],
        ),
        (
            "the key of that tag, whatever its value",
            filters(&[("tag-key", &["Owner"])]),
            vec![tagged.clone()],
        ),
        (
            "the address type one service is offered over",
            filters(&[("supported-ip-address-types", &["ipv6"])]),
            vec![over_ipv6.clone()],
        ),
        (
            "two filters that cannot both hold",
            filters(&[("tag-key", &["Owner"]), ("supported-ip-address-types", &["ipv6"])]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let body = describe_configurations(case, &router, &params).await;
        assert_eq!(
            elements(&body, "serviceId"),
            expected,
            "case {case:?}: expected exactly those services, got {body}"
        );
    }

    // Paging: the smallest page this describe serves holds one fewer than the store does, so
    // the first page carries a token and the second finishes the walk.
    let case = "the first page of five";
    let first = describe_configurations(case, &router, &[param("MaxResults", "5")]).await;
    assert_eq!(
        elements(&first, "serviceId"),
        ids[..5].to_vec(),
        "case {case:?}: expected the first five in ServiceId order, got {first}"
    );
    let token = member(case, &first, "nextToken");

    let case = "the page that token names";
    let second = describe_configurations(
        case,
        &router,
        &[param("MaxResults", "5"), param("NextToken", &token)],
    )
    .await;
    assert_eq!(
        elements(&second, "serviceId"),
        ids[5..].to_vec(),
        "case {case:?}: expected the rest of the walk, got {second}"
    );
    assert_eq!(
        element(&second, "nextToken"),
        None,
        "case {case:?}: the last page names no next page, got {second}"
    );
}

/// What the describe refuses: a page smaller than it serves, a filter it does not document, an
/// id of no service of the account, a value that is no service id at all, a token it did not
/// mint, and the dry run of a request that would have succeeded.
#[tokio::test]
async fn the_describe_refuses_what_it_cannot_serve() {
    let router = emulator();
    let ids = configure_services("the describe refusal table", &router).await;

    let rows: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a page smaller than the documented minimum",
            vec![param("MaxResults", "4")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a filter this operation does not document",
            filters(&[("service-owner", &["amazon"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key at all",
            filters(&[("tag:", &["TeamA"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a well-formed id of no service of this account",
            indexed("ServiceId", &["vpce-svc-0123456789abcdef0"]),
            400,
            "InvalidVpcEndpointServiceId.NotFound",
        ),
        (
            "a value that is not a service id at all",
            indexed("ServiceId", &["not-a-service"]),
            400,
            "InvalidVpcEndpointServiceId.Malformed",
        ),
        (
            "a token this service did not mint",
            vec![param("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
        (
            "the dry run of a request that would have succeeded",
            vec![param("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];
    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = call(&router, "DescribeVpcEndpointServiceConfigurations", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {body}"
        );
    }

    // A read changes nothing: the store still holds exactly the services it was given.
    assert_eq!(
        elements(
            &describe_configurations("after the refusals", &router, &[]).await,
            "serviceId"
        ),
        ids,
        "a describe, refused or served, must leave the store as it found it"
    );
}
