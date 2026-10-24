// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Verified Access ENDPOINT slice:
//! `CreateVerifiedAccessEndpoint` and `DescribeVerifiedAccessEndpoints`, driven over the
//! wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateVerifiedAccessEndpoint` MINTS AN ENDPOINT FROM THE REQUEST AND THE STORE and
//!    `DescribeVerifiedAccessEndpoints` REPORTS THE STORE: every member the request sets
//!    round-trips, the members the service owns (the endpoint id, the generated endpoint
//!    domain, the status, the timestamps) are derived rather than taken, and the instance
//!    the endpoint reports is the one its GROUP belongs to;
//! 2. EVERY EC2 RESOURCE THE REQUEST REFERENCES MUST BE ONE THE ACCOUNT HOLDS: an id that
//!    resolves to nothing is refused at HTTP 400 with that resource's own
//!    `Invalid<Kind>.NotFound` code and NOTHING is stored;
//! 3. each KIND of endpoint takes its OWN options block: the block the model calls required
//!    for that kind is required, and another kind's block is refused;
//! 4. `SecurityGroupIds` is required when `AttachmentType` is `vpc`, and a port range whose
//!    `FromPort` is above its `ToPort` is refused;
//! 5. one `ClientToken` names one endpoint, and a `DryRun` reports the outcome the request
//!    would have had and WRITES NOTHING;
//! 6. an endpoint is answered in the non-terminal `pending` status and reaches `active` one
//!    `TRANSITION_DELAY` later, whether or not anybody calls again;
//! 7. `DescribeVerifiedAccessEndpoints` narrows by `VerifiedAccessEndpointIds`, by the group
//!    and the instance the endpoint belongs to, and by every filter it answers, and pages by
//!    `MaxResults`/`NextToken`.
//!
//! Each table row is a distinct input along what the rule generalises over — which endpoint
//! kind, which protocol, which reference is unresolvable, which options block is missing or
//! surplus, which filter, how many values, which page size — so no row is the specification.
//! The kind catalog is the model's own `VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES` and the
//! attachment catalog its `VERIFIED_ACCESS_ENDPOINT_ATTACHMENT_TYPE_VALUES`, so every
//! published kind is covered.
//!
//! The groups the endpoints hang off are seeded through
//! [`ec2::state::ComAmazonawsEc2Amazonec2State::insert_verified_access_group`], the writer
//! seam `CreateVerifiedAccessGroup` will land on — that operation is still a scaffold stub
//! with its own open issue — the way the subnet and image suites seed through their own
//! insert seams.

use std::time::Duration;

use ec2::ops::types::{
    VerifiedAccessGroup, VERIFIED_ACCESS_ENDPOINT_ATTACHMENT_TYPE_VALUES,
    VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES,
};
use ec2::state::{
    availability_zones_in_region, ComAmazonawsEc2Amazonec2State as Ec2State, NetworkInterfaceSpec,
    SecurityGroupSpec, SubnetSpec, MAX_VERIFIED_ACCESS_ENDPOINTS_PAGE_SIZE,
    MAX_VERIFIED_ACCESS_ENDPOINT_PORT_RANGES, TRANSITION_DELAY,
    VERIFIED_ACCESS_ENDPOINT_STATUS_ACTIVE, VERIFIED_ACCESS_ENDPOINT_STATUS_PENDING,
    VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR, VERIFIED_ACCESS_ENDPOINT_TYPE_LOAD_BALANCER,
    VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE, VERIFIED_ACCESS_ENDPOINT_TYPE_RDS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for: the generated endpoint domain names it, so a
/// Region that is not the emulator's default is the stronger case.
const REGION: &str = "us-west-2";

/// The Verified Access instance the seeded group belongs to. Chosen by the test rather than
/// created: nothing in this emulator stores Verified Access instances yet
/// (`CreateVerifiedAccessInstance` is a scaffold stub with its own open issue), and an
/// endpoint reports the instance of its GROUP whatever that id is.
const INSTANCE_ID: &str = "vai-0123456789abcdef0";

/// A second instance, for the group that is not the one under test — so narrowing by
/// instance has something to leave out.
const OTHER_INSTANCE_ID: &str = "vai-00000000000000001";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a status and, when it is a refusal, an error code:
/// `(case, the parameters under test, the HTTP status, the error code)`.
type StatusRow = (&'static str, Params, u16, &'static str);

/// What the seeded store holds, for the tables to name rows by.
struct Fixture {
    /// The group on [`INSTANCE_ID`] every endpoint under test hangs off.
    group_id: String,
    /// A second group, on [`OTHER_INSTANCE_ID`].
    other_group_id: String,
    /// Two security groups the endpoint may be associated with.
    security_group_ids: Vec<String>,
    /// Two subnets the options blocks may name.
    subnet_ids: Vec<String>,
    /// A network interface a `network-interface` endpoint may target.
    network_interface_id: String,
}

/// Stand up an emulator and seed the resources a create references, the way the creating
/// operations would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Fixture) {
    let (router, engine) = super::ec2_query::emulator();
    let fixture = seed(&engine);
    (router, engine, fixture)
}

/// Write the referenced resources into the engine's store and report the ids they were
/// given.
fn seed(engine: &engine::InMemoryEngine) -> Fixture {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let vpc_id = guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
    let zones = availability_zones_in_region(REGION);
    let subnet_ids: Vec<String> = (0..2)
        .map(|index| {
            let zone = &zones[index];
            guard.insert_subnet(SubnetSpec {
                vpc_id: vpc_id.clone(),
                cidr_block: format!("10.0.{index}.0/24"),
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id.clone(),
                region: REGION.to_string(),
                state: "available".to_string(),
                default_for_az: false,
                map_public_ip_on_launch: false,
                tags: Vec::new(),
            })
        })
        .collect();
    let security_group_ids: Vec<String> = ["verified-access-a", "verified-access-b"]
        .into_iter()
        .map(|group_name| {
            guard.insert_security_group(SecurityGroupSpec {
                vpc_id: vpc_id.clone(),
                group_name: group_name.to_string(),
                description: "reaches the application".to_string(),
                region: REGION.to_string(),
                tags: Vec::new(),
                ip_permissions: Vec::new(),
                ip_permissions_egress: Vec::new(),
            })
        })
        .collect();
    let network_interface_id = guard
        .insert_network_interface(NetworkInterfaceSpec {
            subnet_id: subnet_ids[0].clone(),
            vpc_id: vpc_id.clone(),
            availability_zone: zones[0].zone_name.clone(),
            availability_zone_id: zones[0].zone_id.clone(),
            region: REGION.to_string(),
            description: "the application's interface".to_string(),
            groups: Vec::new(),
            private_ip_addresses: Vec::new(),
            interface_type: "interface".to_string(),
            attachment: None,
            tags: Vec::new(),
        })
        .expect("a seeded network interface");
    let group_id = guard.insert_verified_access_group(
        VerifiedAccessGroup {
            verified_access_instance_id: Some(INSTANCE_ID.to_string()),
            description: Some("the group under test".to_string()),
            ..VerifiedAccessGroup::default()
        },
        REGION,
    );
    let other_group_id = guard.insert_verified_access_group(
        VerifiedAccessGroup {
            verified_access_instance_id: Some(OTHER_INSTANCE_ID.to_string()),
            description: Some("another group".to_string()),
            ..VerifiedAccessGroup::default()
        },
        REGION,
    );

    Fixture {
        group_id,
        other_group_id,
        security_group_ids,
        subnet_ids,
        network_interface_id,
    }
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateVerifiedAccessEndpoint", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVerifiedAccessEndpoints", params).await
}

/// How many endpoints the store holds.
fn held(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_endpoints
        .len()
}

/// A `VerifiedAccessEndpointId` is `vae-` followed by 8 or 17 lowercase hex digits, the
/// short and the long EC2 id form.
fn is_endpoint_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("vae-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// An RFC 3339 UTC timestamp, as the service renders `CreationTime` and `LastUpdatedTime`:
/// `YYYY-MM-DDThh:mm:ssZ`.
fn is_iso8601_utc(candidate: &str) -> bool {
    candidate.len() >= 20
        && candidate.ends_with('Z')
        && candidate.as_bytes()[4] == b'-'
        && candidate.as_bytes()[7] == b'-'
        && candidate.as_bytes()[10] == b'T'
        && candidate.as_bytes()[13] == b':'
}

/// The options block one KIND of endpoint takes, as its form parameters: the block the model
/// calls required for that kind, filled from `fixture`.
fn kind_params(kind: &str, fixture: &Fixture) -> Params {
    match kind {
        VERIFIED_ACCESS_ENDPOINT_TYPE_LOAD_BALANCER => [
            vec![
                p("LoadBalancerOptions.Protocol", "https"),
                p("LoadBalancerOptions.Port", "443"),
                p(
                    "LoadBalancerOptions.LoadBalancerArn",
                    "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/web/0a1b",
                ),
                p("LoadBalancerOptions.PortRange.1.FromPort", "8000"),
                p("LoadBalancerOptions.PortRange.1.ToPort", "8100"),
            ],
            indexed(
                "LoadBalancerOptions.SubnetId",
                &[fixture.subnet_ids[0].as_str(), fixture.subnet_ids[1].as_str()],
            ),
        ]
        .concat(),
        VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE => vec![
            p(
                "NetworkInterfaceOptions.NetworkInterfaceId",
                fixture.network_interface_id.clone(),
            ),
            p("NetworkInterfaceOptions.Protocol", "http"),
            p("NetworkInterfaceOptions.Port", "80"),
        ],
        VERIFIED_ACCESS_ENDPOINT_TYPE_RDS => [
            vec![
                p("RdsOptions.Protocol", "tcp"),
                p("RdsOptions.Port", "5432"),
                p(
                    "RdsOptions.RdsDbInstanceArn",
                    "arn:aws:rds:us-west-2:000000000000:db:ledger",
                ),
                p("RdsOptions.RdsEndpoint", "ledger.example.test"),
            ],
            indexed("RdsOptions.SubnetId", &[fixture.subnet_ids[0].as_str()]),
        ]
        .concat(),
        // `cidr`, the remaining member of the catalog.
        _ => [
            vec![
                p("CidrOptions.Protocol", "tcp"),
                p("CidrOptions.Cidr", "10.0.0.0/24"),
                p("CidrOptions.PortRange.1.FromPort", "5000"),
                p("CidrOptions.PortRange.1.ToPort", "5000"),
            ],
            indexed("CidrOptions.SubnetId", &[fixture.subnet_ids[1].as_str()]),
        ]
        .concat(),
    }
}

/// `count` port ranges for a `cidr` endpoint's options block, as its form parameters: one
/// single-port range per entry, each inside the modelled `range 1..65535`.
fn port_range_params(count: usize) -> Params {
    (1..=count)
        .flat_map(|index| {
            let port = (1000 + index).to_string();
            [
                p(&format!("CidrOptions.PortRange.{index}.FromPort"), port.clone()),
                p(&format!("CidrOptions.PortRange.{index}.ToPort"), port),
            ]
        })
        .collect()
}

/// `base` with every parameter `overrides` names REPLACED rather than repeated: an ec2Query
/// body that carries one key twice is read as the first occurrence, so a row that changes a
/// member of an otherwise valid request has to take the original key out.
fn override_params(base: Params, overrides: Params) -> Params {
    let mut result: Params = base
        .into_iter()
        .filter(|(key, _)| !overrides.iter().any(|(other, _)| other == key))
        .collect();
    result.extend(overrides);
    result
}

/// The parameters of a whole create: the required members, the kind's own options block, and
/// whatever else the case carries. The ONE place a create's parameters are assembled.
fn create_params(kind: &str, fixture: &Fixture, extra: Params) -> Params {
    let base = [
        vec![
            p("VerifiedAccessGroupId", fixture.group_id.clone()),
            p("EndpointType", kind),
            p("AttachmentType", "vpc"),
        ],
        indexed(
            "SecurityGroupId",
            &[fixture.security_group_ids[0].as_str()],
        ),
        kind_params(kind, fixture),
    ]
    .concat();
    override_params(base, extra)
}

/// Create one endpoint that must succeed, and answer the id it was minted.
async fn seed_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    kind: &str,
    fixture: &Fixture,
    extra: Params,
) -> String {
    let (status, body) = create(router, &create_params(kind, fixture, extra)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVerifiedAccessEndpoint, got {status} \
         with body {body}"
    );
    let endpoint_id = element(&body, "verifiedAccessEndpointId").unwrap_or_default();
    assert!(
        is_endpoint_id(&endpoint_id),
        "case {case:?}: expected a minted endpoint id, got {endpoint_id:?}"
    );
    endpoint_id
}

/// The endpoint ids `DescribeVerifiedAccessEndpoints` reports for `params`, in the order it
/// reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVerifiedAccessEndpoints, got {status} \
         with body {body}"
    );
    elements(&body, "verifiedAccessEndpointId")
}

/// RULE 1 over the kind catalog: every published kind of endpoint is created from the
/// request, is given the members the service owns, and reads back out of the store through
/// `DescribeVerifiedAccessEndpoints` with every member it was created with.
#[tokio::test]
async fn every_published_kind_round_trips_through_the_describe() {
    for kind in VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES {
        for attachment_type in VERIFIED_ACCESS_ENDPOINT_ATTACHMENT_TYPE_VALUES {
            let (router, _engine, fixture) = emulator();
            let case = format!("{kind}/{attachment_type}");
            let (status, body) = create(
                &router,
                &create_params(
                    kind,
                    &fixture,
                    vec![
                        p("AttachmentType", *attachment_type),
                        p("Description", format!("the {kind} endpoint")),
                        p("ApplicationDomain", format!("{kind}.example.test")),
                        p(
                            "DomainCertificateArn",
                            "arn:aws:acm:us-west-2:000000000000:certificate/0a1b2c3d",
                        ),
                        p("EndpointDomainPrefix", "shop"),
                        p("PolicyDocument", "permit(principal, action, resource);"),
                        p("TagSpecification.1.ResourceType", "verified-access-endpoint"),
                        p("TagSpecification.1.Tag.1.Key", "Team"),
                        p("TagSpecification.1.Tag.1.Value", "platform"),
                    ],
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );

            let endpoint_id = element(&body, "verifiedAccessEndpointId").unwrap_or_default();
            assert!(
                is_endpoint_id(&endpoint_id),
                "case {case:?}: expected a minted endpoint id, got {endpoint_id:?}"
            );
            let creation_time = element(&body, "creationTime").unwrap_or_default();
            assert!(
                is_iso8601_utc(&creation_time),
                "case {case:?}: expected an ISO 8601 creationTime, got {creation_time:?}"
            );
            assert_eq!(
                element(&body, "lastUpdatedTime").as_deref(),
                Some(creation_time.as_str()),
                "case {case:?}: a freshly created endpoint was last updated when it was \
                 created, got {:?}",
                element(&body, "lastUpdatedTime")
            );
            assert!(
                element(&body, "deletionTime").is_none(),
                "case {case:?}: nothing deleted this endpoint, so it reports no deletion \
                 time, got {body}"
            );

            // The generated DNS name carries the prefix the request asked to prepend, names
            // the endpoint and the Region, and is not the application domain.
            let endpoint_domain = element(&body, "endpointDomain").unwrap_or_default();
            assert!(
                endpoint_domain.starts_with("shop.")
                    && endpoint_domain.contains(&endpoint_id)
                    && endpoint_domain.contains(REGION),
                "case {case:?}: expected a generated endpoint domain carrying the prefix, the \
                 endpoint id and {REGION}, got {endpoint_domain:?}"
            );

            // Every member the request set, as the response reports it, plus the ones the
            // service derives.
            let mut expected = vec![
                ("endpointType", (*kind).to_string()),
                ("attachmentType", (*attachment_type).to_string()),
                ("verifiedAccessGroupId", fixture.group_id.clone()),
                // The instance comes from the GROUP, not from the request.
                ("verifiedAccessInstanceId", INSTANCE_ID.to_string()),
                ("description", format!("the {kind} endpoint")),
                ("applicationDomain", format!("{kind}.example.test")),
                (
                    "domainCertificateArn",
                    "arn:aws:acm:us-west-2:000000000000:certificate/0a1b2c3d".to_string(),
                ),
                // An endpoint AWS hands back is still being set up.
                ("code", VERIFIED_ACCESS_ENDPOINT_STATUS_PENDING.to_string()),
                // No SSE specification was named, so no customer managed key is in use.
                ("customerManagedKeyEnabled", "false".to_string()),
                ("key", "Team".to_string()),
                ("value", "platform".to_string()),
            ];
            match *kind {
                VERIFIED_ACCESS_ENDPOINT_TYPE_LOAD_BALANCER => expected.extend([
                    ("protocol", "https".to_string()),
                    ("port", "443".to_string()),
                    ("fromPort", "8000".to_string()),
                    ("toPort", "8100".to_string()),
                ]),
                VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE => expected.extend([
                    ("protocol", "http".to_string()),
                    ("port", "80".to_string()),
                    (
                        "networkInterfaceId",
                        fixture.network_interface_id.clone(),
                    ),
                ]),
                VERIFIED_ACCESS_ENDPOINT_TYPE_RDS => expected.extend([
                    ("protocol", "tcp".to_string()),
                    ("port", "5432".to_string()),
                    ("rdsEndpoint", "ledger.example.test".to_string()),
                ]),
                _ => expected.extend([
                    ("protocol", "tcp".to_string()),
                    ("cidr", "10.0.0.0/24".to_string()),
                    ("fromPort", "5000".to_string()),
                ]),
            }
            for (member, value) in &expected {
                assert_eq!(
                    element(&body, member).as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected {member}={value:?} from the create, got {:?}",
                    element(&body, member)
                );
            }
            // The security group the request associated is reported in the endpoint's own
            // set, and only that one.
            let reported_groups = element(&body, "securityGroupIdSet").unwrap_or_default();
            assert!(
                reported_groups.contains(&fixture.security_group_ids[0])
                    && !reported_groups.contains(&fixture.security_group_ids[1]),
                "case {case:?}: expected the associated security group in the reported set, \
                 got {reported_groups:?}"
            );

            // ROUND TRIP: the endpoint is in the store, not just echoed.
            let (status, described) =
                describe(&router, &indexed("VerifiedAccessEndpointId", &[&endpoint_id])).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the describe, got {status} / {described}"
            );
            assert_eq!(
                elements(&described, "verifiedAccessEndpointId"),
                vec![endpoint_id.clone()],
                "case {case:?}: expected the describe to report the created endpoint, got \
                 {described}"
            );
            for (member, value) in &expected {
                assert_eq!(
                    element(&described, member).as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected the stored {member}={value:?}, got {:?}",
                    element(&described, member)
                );
            }
            assert_eq!(
                element(&described, "creationTime").as_deref(),
                Some(creation_time.as_str()),
                "case {case:?}: the creation time is stored, not re-derived per read, got {:?}",
                element(&described, "creationTime")
            );
            assert_eq!(
                element(&described, "endpointDomain").as_deref(),
                Some(endpoint_domain.as_str()),
                "case {case:?}: the generated endpoint domain is stored, not re-derived per \
                 read, got {:?}",
                element(&described, "endpointDomain")
            );
            // The policy document the create took is not a member of the reported endpoint
            // shape (`GetVerifiedAccessEndpointPolicy` is what reports it), so no read
            // leaks it.
            assert!(
                !described.contains("permit(principal"),
                "case {case:?}: the endpoint shape carries no policy document, got {described}"
            );
        }
    }
}

/// RULE 1's derived DNS name, across the PARTITIONS: the generated endpoint domain carries
/// the DNS suffix of the partition the Region the request was signed for belongs to.
///
/// The expected suffixes are the AWS SDK partition metadata the module's own
/// `NON_COMMERCIAL_PARTITIONS` catalog records (`aws-cn` `amazonaws.com.cn`, `aws-iso`
/// `c2s.ic.gov`, `aws-iso-b` `sc2s.sgov.gov`, `aws-us-gov` and the commercial partition
/// `amazonaws.com`), written out here rather than read back from the code under test, and
/// covering a Region the issue did not cite.
#[tokio::test]
async fn the_generated_endpoint_domain_carries_its_partitions_dns_suffix() {
    let rows: Vec<(&str, &str)> = vec![
        ("us-west-2", "amazonaws.com"),
        ("cn-north-1", "amazonaws.com.cn"),
        ("us-gov-west-1", "amazonaws.com"),
        ("us-iso-east-1", "c2s.ic.gov"),
        ("us-isob-east-1", "sc2s.sgov.gov"),
    ];
    for (region, suffix) in rows {
        let (router, _engine, fixture) = emulator();
        let params = create_params(
            VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
            &fixture,
            vec![p("EndpointDomainPrefix", "shop")],
        );
        let (status, body) = call(
            &router,
            region,
            "CreateVerifiedAccessEndpoint",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with body {body}"
        );
        let endpoint_id = element(&body, "verifiedAccessEndpointId").unwrap_or_default();
        let endpoint_domain = element(&body, "endpointDomain").unwrap_or_default();
        assert_eq!(
            endpoint_domain,
            format!("shop.{endpoint_id}.prod.verified-access.{region}.{suffix}"),
            "case {region:?}: expected the generated domain to carry the prefix, the endpoint \
             id, the Region and the {suffix:?} suffix of its partition, got \
             {endpoint_domain:?}"
        );
    }
}

/// RULE 2: every EC2 resource the request references has to be one the account holds, and a
/// create that names one it does not is refused with that resource's own code and stores
/// nothing.
#[tokio::test]
async fn an_unresolvable_reference_is_refused_and_stores_nothing() {
    let (router, engine, fixture) = emulator();

    // Each row replaces one reference of an otherwise valid create with an id that resolves
    // to nothing — a well-formed id of the right kind, and one that is not an id at all.
    let rows: Vec<StatusRow> = vec![
        (
            "a group id no group of the account has",
            vec![p("VerifiedAccessGroupId", "vagr-0123456789abcdef0")],
            400,
            "InvalidVerifiedAccessGroupId.NotFound",
        ),
        (
            "a group id that is not an id at all",
            vec![p("VerifiedAccessGroupId", "not-a-group")],
            400,
            "InvalidVerifiedAccessGroupId.NotFound",
        ),
        (
            "a security group the account does not hold",
            indexed("SecurityGroupId", &["sg-0123456789abcdef0"]),
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "a subnet the account does not hold",
            indexed("CidrOptions.SubnetId", &["subnet-0123456789abcdef0"]),
            400,
            "InvalidSubnetID.NotFound",
        ),
        (
            "every reference resolvable",
            Vec::new(),
            200,
            "",
        ),
    ];

    let before = held(&engine);
    let mut created = 0;
    for (case, extra, expected_status, expected_code) in rows {
        // The replacement parameters come LAST, so they override the base create's own.
        let params = create_params(VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR, &fixture, extra);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            created += 1;
            continue;
        }
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {body}"
        );
        assert!(
            unescaped(&element(&body, "Message").unwrap_or_default()).contains("does not exist"),
            "case {case:?}: expected a message saying the named resource does not exist, got \
             {body}"
        );
    }
    assert_eq!(
        held(&engine),
        before + created,
        "a refused create stores nothing: expected {} endpoints, got {}",
        before + created,
        held(&engine)
    );
}

/// RULE 2 for the one reference an endpoint of the `network-interface` kind names.
#[tokio::test]
async fn an_unresolvable_network_interface_is_refused() {
    let (router, engine, fixture) = emulator();
    let params = create_params(
        VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE,
        &fixture,
        vec![p(
            "NetworkInterfaceOptions.NetworkInterfaceId",
            "eni-0123456789abcdef0",
        )],
    );
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "case \"an interface the account does not hold\": expected HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNetworkInterfaceID.NotFound",
        "case \"an interface the account does not hold\": expected \
         InvalidNetworkInterfaceID.NotFound, got {body}"
    );
    assert_eq!(
        held(&engine),
        0,
        "case \"an interface the account does not hold\": a refused create stores nothing, \
         got {} endpoints",
        held(&engine)
    );
}

/// RULE 3 and RULE 4: each kind requires its OWN options block and refuses another kind's,
/// `SecurityGroupIds` is required for a `vpc` attachment, and a port range runs from its
/// lower bound to its upper one.
#[tokio::test]
async fn each_kind_requires_its_own_options_and_refuses_the_others() {
    let (router, engine, fixture) = emulator();

    let mut rows: Vec<(String, Params, u16, &'static str)> = Vec::new();
    for kind in VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES {
        // The kind's own block, which is what a correct create carries.
        rows.push((
            format!("a {kind} endpoint with its own options"),
            Vec::new(),
            200,
            "",
        ));
        // The same create with its own block taken away.
        rows.push((
            format!("a {kind} endpoint with no options at all"),
            Vec::new(),
            400,
            "MissingParameter",
        ));
        // And with ANOTHER kind's block beside its own.
        for other in VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES.iter().filter(|other| *other != kind) {
            rows.push((
                format!("a {kind} endpoint carrying the {other} options too"),
                kind_params(other, &fixture),
                400,
                "InvalidParameterCombination",
            ));
        }
    }
    rows.push((
        "a vpc attachment with no security groups".to_string(),
        Vec::new(),
        400,
        "MissingParameter",
    ));
    rows.push((
        "a port range whose fromPort is above its toPort".to_string(),
        vec![
            p("CidrOptions.PortRange.1.FromPort", "9000"),
            p("CidrOptions.PortRange.1.ToPort", "8000"),
        ],
        400,
        "InvalidParameterValue",
    ));
    // The caller chooses how many port ranges the block carries, so the count is bounded:
    // the ceiling itself is accepted and one past it is refused.
    rows.push((
        "a cidr endpoint carrying every port range the block admits".to_string(),
        port_range_params(MAX_VERIFIED_ACCESS_ENDPOINT_PORT_RANGES),
        200,
        "",
    ));
    rows.push((
        "a cidr endpoint carrying one port range too many".to_string(),
        port_range_params(MAX_VERIFIED_ACCESS_ENDPOINT_PORT_RANGES + 1),
        400,
        "InvalidParameterValue",
    ));

    let before = held(&engine);
    let mut created = 0;
    for (case, extra, expected_status, expected_code) in rows {
        let kind = VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES
            .iter()
            .find(|kind| case.contains(&format!("a {kind} endpoint")))
            .copied()
            .unwrap_or(VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR);
        let params = if case.contains("no options at all") {
            // Everything a create needs EXCEPT the kind's own options block.
            [
                vec![
                    p("VerifiedAccessGroupId", fixture.group_id.clone()),
                    p("EndpointType", kind),
                    p("AttachmentType", "vpc"),
                ],
                indexed("SecurityGroupId", &[fixture.security_group_ids[0].as_str()]),
            ]
            .concat()
        } else if case.contains("no security groups") {
            [
                vec![
                    p("VerifiedAccessGroupId", fixture.group_id.clone()),
                    p("EndpointType", VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR),
                    p("AttachmentType", "vpc"),
                ],
                kind_params(VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR, &fixture),
            ]
            .concat()
        } else {
            create_params(kind, &fixture, Vec::new())
        };
        let params = override_params(params, extra);

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            created += 1;
            continue;
        }
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {body}"
        );
    }
    assert_eq!(
        held(&engine),
        before + created,
        "a refused create stores nothing: expected {} endpoints, got {}",
        before + created,
        held(&engine)
    );
}

/// RULE 1 and RULE 2 for the tag specification: the endpoint is the only resource this call
/// tags, so a specification naming another resource type is refused.
#[tokio::test]
async fn a_tag_specification_names_the_endpoint_it_creates() {
    let (router, engine, fixture) = emulator();
    let rows: Vec<StatusRow> = vec![
        (
            "a specification naming the endpoint",
            vec![
                p("TagSpecification.1.ResourceType", "verified-access-endpoint"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            200,
            "",
        ),
        (
            "a specification naming no resource type",
            vec![
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            200,
            "",
        ),
        (
            "a specification naming another resource type",
            vec![
                p("TagSpecification.1.ResourceType", "verified-access-group"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    let mut created = 0;
    for (case, extra, expected_status, expected_code) in rows {
        let (status, body) = create(
            &router,
            &create_params(VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR, &fixture, extra),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            created += 1;
            assert_eq!(
                element(&body, "key").as_deref(),
                Some("Team"),
                "case {case:?}: expected the tag the request set, got {body}"
            );
            continue;
        }
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {body}"
        );
    }
    assert_eq!(
        held(&engine),
        created,
        "a refused create stores nothing: expected {created} endpoints, got {}",
        held(&engine)
    );
}

/// RULE 5: one `ClientToken` names one endpoint, and a `DryRun` reports the outcome the
/// request would have had and writes nothing.
#[tokio::test]
async fn one_client_token_names_one_endpoint_and_a_dry_run_writes_nothing() {
    let (router, engine, fixture) = emulator();

    // A dry run of a request that WOULD have succeeded.
    let (status, body) = create(
        &router,
        &create_params(
            VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
            &fixture,
            vec![p("DryRun", "true")],
        ),
    )
    .await;
    assert_eq!(
        status, 412,
        "case \"a dry run that would have succeeded\": expected HTTP 412, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"a dry run that would have succeeded\": expected DryRunOperation, got {body}"
    );
    // A dry run of a request that would have been REFUSED is refused with that reason.
    let (status, body) = create(
        &router,
        &create_params(
            VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
            &fixture,
            vec![
                p("DryRun", "true"),
                p("VerifiedAccessGroupId", "vagr-00000000000000002"),
            ],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a dry run that would have been refused\": expected HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVerifiedAccessGroupId.NotFound",
        "case \"a dry run that would have been refused\": expected the refusal the request \
         would have met, got {body}"
    );
    assert_eq!(
        held(&engine),
        0,
        "case \"a dry run\": expected a dry run to store nothing, got {} endpoints",
        held(&engine)
    );

    // One token, three calls: one endpoint.
    let token = "token-of-one-endpoint";
    let mut answered = Vec::new();
    for attempt in 1..=3 {
        let case = format!("replay {attempt} of the same client token");
        answered.push(
            seed_endpoint(
                &router,
                &case,
                VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
                &fixture,
                vec![p("ClientToken", token)],
            )
            .await,
        );
    }
    assert!(
        answered.windows(2).all(|pair| pair[0] == pair[1]),
        "case \"one client token\": expected every replay to answer the same endpoint, got \
         {answered:?}"
    );
    assert_eq!(
        held(&engine),
        1,
        "case \"one client token\": expected one stored endpoint, got {}",
        held(&engine)
    );

    // A DIFFERENT token is a different endpoint.
    let other = seed_endpoint(
        &router,
        "a second client token",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
        vec![p("ClientToken", "token-of-another-endpoint")],
    )
    .await;
    assert_ne!(
        other, answered[0],
        "case \"a second client token\": expected a second endpoint, got {other:?} twice"
    );
    assert_eq!(
        held(&engine),
        2,
        "case \"a second client token\": expected two stored endpoints, got {}",
        held(&engine)
    );
}

/// RULE 6: an endpoint is answered `pending` — a caller acting straight after the create
/// still sees the transient status — and reaches `active` one `TRANSITION_DELAY` later,
/// whether or not anybody calls again.
#[tokio::test]
async fn a_pending_endpoint_reaches_active_when_the_clock_says_so() {
    let (router, engine, fixture) = emulator();
    let endpoint_id = seed_endpoint(
        &router,
        "the endpoint under test",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
        Vec::new(),
    )
    .await;

    let (status, body) =
        describe(&router, &indexed("VerifiedAccessEndpointId", &[&endpoint_id])).await;
    assert_eq!(
        status, 200,
        "case \"straight after the create\": expected HTTP 200, got {status} / {body}"
    );
    assert_eq!(
        element(&body, "code").as_deref(),
        Some(VERIFIED_ACCESS_ENDPOINT_STATUS_PENDING),
        "case \"straight after the create\": expected the transient status, got {body}"
    );
    let creation_time = element(&body, "creationTime").unwrap_or_default();

    // Nobody calls while the move falls due.
    tokio::time::sleep(past_the_transition()).await;

    let (status, body) =
        describe(&router, &indexed("VerifiedAccessEndpointId", &[&endpoint_id])).await;
    assert_eq!(
        status, 200,
        "case \"past the transition\": expected HTTP 200, got {status} / {body}"
    );
    assert_eq!(
        element(&body, "code").as_deref(),
        Some(VERIFIED_ACCESS_ENDPOINT_STATUS_ACTIVE),
        "case \"past the transition\": expected the endpoint to have reached active, got {body}"
    );
    assert_eq!(
        element(&body, "creationTime").as_deref(),
        Some(creation_time.as_str()),
        "case \"past the transition\": the creation time does not move, got {:?}",
        element(&body, "creationTime")
    );
    assert_ne!(
        element(&body, "lastUpdatedTime").unwrap_or_default(),
        creation_time,
        "case \"past the transition\": the endpoint changed when its move fell due, so its \
         last updated time moved too, got {body}"
    );
    // The move is applied once, and the store — not the read — holds the new status.
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .verified_access_endpoints
            .get(&endpoint_id)
            .and_then(|record| record.view.status.as_ref())
            .and_then(|status| status.code.clone())
            .unwrap_or_default(),
        VERIFIED_ACCESS_ENDPOINT_STATUS_ACTIVE,
        "case \"past the transition\": expected the stored endpoint to be active"
    );
}

/// RULE 7: the describe narrows by ids, by the group and the instance the endpoint belongs
/// to, and by every filter it answers.
#[tokio::test]
async fn the_describe_narrows_by_every_selector_it_answers() {
    let (router, _engine, fixture) = emulator();

    // Three endpoints that differ along what the selectors narrow on: the kind, the group
    // (and so the instance), the security group, the application domain and the tags.
    let first = seed_endpoint(
        &router,
        "the cidr endpoint",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
        vec![
            p("Description", "the first endpoint"),
            p("ApplicationDomain", "first.example.test"),
            p("EndpointDomainPrefix", "first"),
            p("TagSpecification.1.ResourceType", "verified-access-endpoint"),
            p("TagSpecification.1.Tag.1.Key", "Team"),
            p("TagSpecification.1.Tag.1.Value", "platform"),
        ],
    )
    .await;
    let second = seed_endpoint(
        &router,
        "the load-balancer endpoint",
        VERIFIED_ACCESS_ENDPOINT_TYPE_LOAD_BALANCER,
        &fixture,
        vec![
            p("Description", "the second endpoint"),
            p("ApplicationDomain", "second.example.test"),
            p(
                "DomainCertificateArn",
                "arn:aws:acm:us-west-2:000000000000:certificate/2222",
            ),
        ],
    )
    .await;
    // The third hangs off the OTHER group, so it belongs to the other instance.
    let third = {
        let params = create_params(
            VERIFIED_ACCESS_ENDPOINT_TYPE_RDS,
            &fixture,
            [
                vec![
                    p("VerifiedAccessGroupId", fixture.other_group_id.clone()),
                    p("Description", "the third endpoint"),
                ],
                indexed(
                    "SecurityGroupId",
                    &[fixture.security_group_ids[1].as_str()],
                ),
            ]
            .concat(),
        );
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case \"the rds endpoint on the other group\": expected HTTP 200, got {status} / \
             {body}"
        );
        element(&body, "verifiedAccessEndpointId").unwrap_or_default()
    };

    let all = vec![first.clone(), second.clone(), third.clone()];
    let mut sorted = all.clone();
    sorted.sort();

    // The creation and update times of the first endpoint, for the time filters to select
    // on: read back rather than guessed.
    let first_body = describe(&router, &indexed("VerifiedAccessEndpointId", &[&first]))
        .await
        .1;
    let first_creation_time = element(&first_body, "creationTime").unwrap_or_default();
    let first_endpoint_domain = element(&first_body, "endpointDomain").unwrap_or_default();

    let rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "no selector at all".to_string(),
            Vec::new(),
            sorted.clone(),
        ),
        (
            "one id".to_string(),
            indexed("VerifiedAccessEndpointId", &[&second]),
            vec![second.clone()],
        ),
        (
            "two ids".to_string(),
            indexed("VerifiedAccessEndpointId", &[&first, &third]),
            {
                let mut expected = vec![first.clone(), third.clone()];
                expected.sort();
                expected
            },
        ),
        (
            "the group the endpoints hang off".to_string(),
            vec![p("VerifiedAccessGroupId", fixture.other_group_id.clone())],
            vec![third.clone()],
        ),
        (
            "the instance that group belongs to".to_string(),
            vec![p("VerifiedAccessInstanceId", OTHER_INSTANCE_ID)],
            vec![third.clone()],
        ),
        (
            "an instance no group of the account names".to_string(),
            vec![p("VerifiedAccessInstanceId", "vai-ffffffffffffffff0")],
            Vec::new(),
        ),
        (
            "filter application-domain".to_string(),
            filters(&[("application-domain", &["second.example.test"])]),
            vec![second.clone()],
        ),
        (
            "filter attachment-type".to_string(),
            filters(&[("attachment-type", &["vpc"])]),
            sorted.clone(),
        ),
        (
            "filter creation-time".to_string(),
            filters(&[("creation-time", &[first_creation_time.as_str()])]),
            // Each endpoint carries its own creation instant, so the exact time of one
            // selects that one.
            vec![first.clone()],
        ),
        (
            "filter deletion-time, which no live endpoint carries".to_string(),
            filters(&[("deletion-time", &["2026-01-01T00:00:00Z"])]),
            Vec::new(),
        ),
        (
            "filter description".to_string(),
            filters(&[("description", &["the third endpoint"])]),
            vec![third.clone()],
        ),
        (
            "filter device-validation-domain, which no endpoint carries".to_string(),
            filters(&[("device-validation-domain", &["device.example.test"])]),
            Vec::new(),
        ),
        (
            "filter domain-certificate-arn".to_string(),
            filters(&[(
                "domain-certificate-arn",
                &["arn:aws:acm:us-west-2:000000000000:certificate/2222"],
            )]),
            vec![second.clone()],
        ),
        (
            "filter endpoint-domain".to_string(),
            filters(&[("endpoint-domain", &[first_endpoint_domain.as_str()])]),
            vec![first.clone()],
        ),
        (
            "filter endpoint-type".to_string(),
            filters(&[("endpoint-type", &[VERIFIED_ACCESS_ENDPOINT_TYPE_RDS])]),
            vec![third.clone()],
        ),
        (
            "filter endpoint-type with two values".to_string(),
            filters(&[(
                "endpoint-type",
                &[
                    VERIFIED_ACCESS_ENDPOINT_TYPE_RDS,
                    VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
                ],
            )]),
            {
                let mut expected = vec![first.clone(), third.clone()];
                expected.sort();
                expected
            },
        ),
        (
            "filter last-updated-time".to_string(),
            filters(&[("last-updated-time", &[first_creation_time.as_str()])]),
            // A freshly created endpoint was last updated when it was created.
            vec![first.clone()],
        ),
        (
            "filter security-group-id".to_string(),
            filters(&[("security-group-id", &[fixture.security_group_ids[1].as_str()])]),
            vec![third.clone()],
        ),
        (
            "filter status-code".to_string(),
            filters(&[("status-code", &[VERIFIED_ACCESS_ENDPOINT_STATUS_PENDING])]),
            sorted.clone(),
        ),
        (
            "filter tag-key".to_string(),
            filters(&[("tag-key", &["Team"])]),
            vec![first.clone()],
        ),
        (
            "filter tag:Team".to_string(),
            filters(&[("tag:Team", &["platform"])]),
            vec![first.clone()],
        ),
        (
            "filter verified-access-endpoint-id".to_string(),
            filters(&[("verified-access-endpoint-id", &[second.as_str()])]),
            vec![second.clone()],
        ),
        (
            "filter verified-access-group-id".to_string(),
            filters(&[("verified-access-group-id", &[fixture.group_id.as_str()])]),
            {
                let mut expected = vec![first.clone(), second.clone()];
                expected.sort();
                expected
            },
        ),
        (
            "filter verified-access-instance-id".to_string(),
            filters(&[("verified-access-instance-id", &[INSTANCE_ID])]),
            {
                let mut expected = vec![first.clone(), second.clone()];
                expected.sort();
                expected
            },
        ),
        (
            "two filters, which must BOTH match".to_string(),
            filters(&[
                ("endpoint-type", &[VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR]),
                ("description", &["the third endpoint"]),
            ]),
            Vec::new(),
        ),
        (
            "a filter value matched with a wildcard".to_string(),
            filters(&[("description", &["the * endpoint"])]),
            sorted.clone(),
        ),
    ];

    for (case, params, expected) in rows {
        let reported = described_ids(&router, &case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the describe to report {expected:?}, got {reported:?}"
        );
    }

    // An id that names no endpoint, a group that names none, and a filter this operation
    // does not answer are refusals rather than empty pages.
    let refusals: Vec<StatusRow> = vec![
        (
            "an endpoint id the account holds no endpoint for",
            indexed("VerifiedAccessEndpointId", &["vae-0123456789abcdef0"]),
            400,
            "InvalidVerifiedAccessEndpointId.NotFound",
        ),
        (
            "a group id the account holds no group for",
            vec![p("VerifiedAccessGroupId", "vagr-0123456789abcdef0")],
            400,
            "InvalidVerifiedAccessGroupId.NotFound",
        ),
        (
            "a filter this operation does not answer",
            filters(&[("endpoint-colour", &["blue"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a page token this operation did not mint",
            vec![p("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
    ];
    for (case, params, expected_status, expected_code) in refusals {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {body}"
        );
    }
}

/// RULE 7's paging half: the report is cut into `MaxResults`-sized pages, each page naming
/// the endpoint the next one starts at, and the last page naming none.
#[tokio::test]
async fn the_describe_pages_by_max_results() {
    let (router, _engine, fixture) = emulator();

    // Six endpoints, so the smallest page the model admits (`range 5..1000`) leaves a second
    // page behind.
    let mut ids = Vec::new();
    for index in 0..6 {
        ids.push(
            seed_endpoint(
                &router,
                &format!("endpoint {index}"),
                VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
                &fixture,
                vec![p("Description", format!("endpoint {index}"))],
            )
            .await,
        );
    }
    ids.sort();

    let (status, body) = describe(&router, &[p("MaxResults", "5")]).await;
    assert_eq!(
        status, 200,
        "case \"the first page of five\": expected HTTP 200, got {status} / {body}"
    );
    assert_eq!(
        elements(&body, "verifiedAccessEndpointId"),
        ids[..5].to_vec(),
        "case \"the first page of five\": expected the first five endpoints in id order, got \
         {body}"
    );
    let next_token = element(&body, "nextToken").unwrap_or_default();
    assert!(
        !next_token.is_empty(),
        "case \"the first page of five\": expected a token naming the next page, got {body}"
    );

    let (status, body) = describe(&router, &[p("MaxResults", "5"), p("NextToken", &next_token)]).await;
    assert_eq!(
        status, 200,
        "case \"the second page\": expected HTTP 200, got {status} / {body}"
    );
    assert_eq!(
        elements(&body, "verifiedAccessEndpointId"),
        ids[5..].to_vec(),
        "case \"the second page\": expected the remaining endpoint, got {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case \"the second page\": the last page names no next page, got {body}"
    );

    // A request that names no page size gets the largest page this operation can be asked
    // for, which is well above the six endpoints held.
    let reported = described_ids(&router, "no page size at all", &[]).await;
    assert_eq!(
        reported, ids,
        "case \"no page size at all\": expected every endpoint, got {reported:?}"
    );
    assert!(
        MAX_VERIFIED_ACCESS_ENDPOINTS_PAGE_SIZE >= reported.len(),
        "case \"no page size at all\": the default page holds every endpoint, got {} for {}",
        MAX_VERIFIED_ACCESS_ENDPOINTS_PAGE_SIZE,
        reported.len()
    );
}
