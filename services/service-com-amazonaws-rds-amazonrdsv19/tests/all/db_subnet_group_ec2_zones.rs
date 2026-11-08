// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE fact a DB subnet group is about: which Availability Zones
//! the subnets a request names lie in.
//!
//! A subnet is not a resource of Amazon RDS — it is Amazon EC2's — so the zone of each
//! `SubnetIds` entry is read off the record the service that OWNS subnets answers with, and
//! never derived from the identifier. The rules asserted here, each over a TABLE spanning
//! what it generalises over:
//!
//! * a group reports, for every subnet it holds, the Availability Zone EC2 reports that
//!   subnet in — on the create, on the read back and on the modification;
//! * a list whose subnets cover fewer than the two zones a group must cover is refused with
//!   `DBSubnetGroupDoesNotCoverEnoughAZs`, whose message names the coverage THOSE subnets
//!   add up to; the coverage is counted in ZONES, so two subnets of one zone cover one zone
//!   and are refused, and two subnets of two zones are served;
//! * an identifier the account has no subnet for has no zone this service knows, so the
//!   group reports none for it rather than one of its own choosing;
//! * the zones a STORED group covers are what a cluster-placement precondition counts: a
//!   Multi-AZ DB cluster create against a group covering fewer than three zones is refused
//!   with the zones of the Region that group does not cover yet.
//!
//! EC2 is mounted beside Amazon RDS on ONE engine and driven through the AWS SDK over the
//! router's own in-process callback — the very seam the resolution under test reaches EC2
//! through. Every subnet id below is one EC2 MINTED, and every expected zone is one EC2
//! reported, so no value in an assertion is computed the way the code under test computes
//! it.

use std::sync::Arc;

use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::db_instances::{call_in, expect_error, expect_ok, subtrees, text, REGION};

/// A well-formed EC2 subnet identifier the account has no subnet for: EC2 mints none in this
/// suite's network, so it resolves to nothing however often it is named.
const ABSENT_SUBNET: &str = "subnet-00000000deadbeef0";

/// The other published identifier form — `subnet-` and 8 hexadecimal characters — also
/// naming no subnet of the account.
const SHORT_ABSENT_SUBNET: &str = "subnet-0000beef";

/// The CIDR block of the VPC this suite builds its own subnets in, outside the default VPC's
/// own block so both can exist.
const VPC_CIDR: &str = "10.52.0.0/16";

/// One emulator over one shared engine: Amazon RDS the way the router mounts it in
/// production, PLUS Amazon EC2, the service that owns subnets. The EC2 client comes with it,
/// over the router's in-process callback.
///
/// `pub(crate)` so a sibling suite over the same fact — what a DB INSTANCE reports about
/// the group it was created in — drives one network through the same seam rather than
/// standing up a second copy of it.
pub(crate) struct Account {
    pub(crate) router: MultiServiceRouter,
    ec2: ::aws_sdk_ec2::Client,
}

impl Account {
    /// Both services on one engine, with an EC2 client over the router's own callback.
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject of these tests; the requests still carry the SigV4
        // credential scope, which is what routes them and names the region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        Self {
            router,
            ec2: ::aws_sdk_ec2::Client::from_conf(config),
        }
    }

    /// The zones of [`REGION`] the ACCOUNT has, as EC2 reports them — asked, never assumed,
    /// so a case places a subnet in a zone the Region really has.
    async fn zones(&self) -> Vec<String> {
        let reported = self
            .ec2
            .describe_availability_zones()
            .send()
            .await
            .expect("EC2 must report the Region's Availability Zones");
        let mut zones: Vec<String> = reported
            .availability_zones()
            .iter()
            .filter_map(|zone| zone.zone_name().map(str::to_string))
            .filter(|zone| zone.starts_with(REGION))
            .collect();
        zones.sort();
        assert!(
            zones.len() >= 2,
            "the Region must have at least two Availability Zones for these cases, got \
             {zones:?}"
        );
        zones
    }

    /// The VPC `ec2:CreateVpc` mints for `cidr_block`, as the id EC2 reported.
    async fn create_vpc(&self, cidr_block: &str) -> String {
        self.ec2
            .create_vpc()
            .cidr_block(cidr_block)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must create the VPC {cidr_block}: {error:?}"))
            .vpc()
            .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
            .expect("CreateVpc must report the VPC it minted")
            .to_string()
    }

    /// The subnet `ec2:CreateSubnet` mints in `vpc_id`, in `zone`, as the id EC2 reported.
    async fn create_subnet(&self, vpc_id: &str, cidr: &str, zone: &str) -> String {
        self.ec2
            .create_subnet()
            .vpc_id(vpc_id)
            .cidr_block(cidr)
            .availability_zone(zone)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("EC2 must create the subnet {cidr} in {zone}: {error:?}")
            })
            .subnet()
            .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
            .expect("CreateSubnet must report the subnet it minted")
            .to_string()
    }

    /// The Availability Zone EC2 reports for `subnet_id`: what every assertion below is
    /// made against, read from the owner of subnets rather than computed here.
    pub(crate) async fn zone_of(&self, subnet_id: &str) -> String {
        self.ec2
            .describe_subnets()
            .subnet_ids(subnet_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must report the subnet {subnet_id}: {error:?}"))
            .subnets()
            .iter()
            .find(|subnet| subnet.subnet_id() == Some(subnet_id))
            .and_then(::aws_sdk_ec2::types::Subnet::availability_zone)
            .unwrap_or_else(|| panic!("EC2 must report a zone for the subnet {subnet_id}"))
            .to_string()
    }

    /// `CreateDBSubnetGroup` over `subnets`.
    pub(crate) async fn create_group(&self, name: &str, subnets: &[String]) -> (u16, String) {
        let mut params: Vec<(String, String)> = vec![
            ("DBSubnetGroupName".to_string(), name.to_string()),
            (
                "DBSubnetGroupDescription".to_string(),
                "zones read from the owner of subnets".to_string(),
            ),
        ];
        params.extend(subnet_members(subnets));
        call_in(&self.router, REGION, "CreateDBSubnetGroup", &params).await
    }

    /// `ModifyDBSubnetGroup` leaving the group holding exactly `subnets`.
    pub(crate) async fn modify_group(&self, name: &str, subnets: &[String]) -> (u16, String) {
        let mut params: Vec<(String, String)> =
            vec![("DBSubnetGroupName".to_string(), name.to_string())];
        params.extend(subnet_members(subnets));
        call_in(&self.router, REGION, "ModifyDBSubnetGroup", &params).await
    }

    /// `DescribeDBSubnetGroups` for one group.
    pub(crate) async fn describe_group(&self, name: &str) -> (u16, String) {
        call_in(
            &self.router,
            REGION,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName".to_string(), name.to_string())],
        )
        .await
    }
}

/// The `SubnetIds.SubnetIdentifier.<n>` members for `subnets`, in the order given.
pub(crate) fn subnet_members(subnets: &[String]) -> Vec<(String, String)> {
    subnets
        .iter()
        .enumerate()
        .map(|(index, subnet)| {
            (
                format!("SubnetIds.SubnetIdentifier.{}", index + 1),
                subnet.clone(),
            )
        })
        .collect()
}

/// The network one case runs against: two subnets in the Region's first zone and one in its
/// second, every id minted by EC2.
pub(crate) struct Network {
    pub(crate) account: Account,
    /// The Region's zones, as EC2 reports them.
    zones: Vec<String>,
    /// The VPC EC2 minted this network's subnets in.
    vpc: String,
    /// Two subnets of the FIRST zone, so a list can name two subnets and cover one zone.
    pub(crate) first_zone: [String; 2],
    /// One subnet of the SECOND zone.
    pub(crate) second_zone: String,
}

impl Network {
    pub(crate) async fn build() -> Self {
        let account = Account::new();
        let zones = account.zones().await;
        let vpc = account.create_vpc(VPC_CIDR).await;
        let first_zone = [
            account
                .create_subnet(&vpc, "10.52.1.0/24", &zones[0])
                .await,
            account
                .create_subnet(&vpc, "10.52.2.0/24", &zones[0])
                .await,
        ];
        let second_zone = account
            .create_subnet(&vpc, "10.52.3.0/24", &zones[1])
            .await;
        Self {
            account,
            zones,
            vpc,
            first_zone,
            second_zone,
        }
    }

    /// The zone EC2 reports for each of `subnets`, in the order given.
    pub(crate) async fn zones_of(&self, subnets: &[String]) -> Vec<String> {
        let mut reported = Vec::with_capacity(subnets.len());
        for subnet in subnets {
            reported.push(self.account.zone_of(subnet).await);
        }
        reported
    }
}

/// Assert one `DBSubnetGroup` envelope reports each of `subnets` with the zone `zones` says
/// EC2 holds it in, in the order the group names them.
pub(crate) fn assert_reported_zones(
    case: &str,
    label: &str,
    group: &str,
    subnets: &[String],
    zones: &[String],
) {
    let reported = subtrees(group, "Subnet");
    assert_eq!(
        reported.len(),
        subnets.len(),
        "case {case:?} ({label}): expected {} subnet(s), got {reported:?}",
        subnets.len()
    );
    for ((subnet, expected_id), expected_zone) in reported.iter().zip(subnets).zip(zones) {
        let identifier = text(subnet, "SubnetIdentifier");
        assert_eq!(
            identifier, *expected_id,
            "case {case:?} ({label}): expected SubnetIdentifier {expected_id:?}, got \
             {identifier:?}"
        );
        let zone = text(subnet, "Name");
        assert_eq!(
            zone, *expected_zone,
            "case {case:?} ({label}): expected the zone EC2 reports for {expected_id:?} \
             ({expected_zone:?}), got {zone:?}"
        );
    }
}

/// The `DBSubnetGroupDoesNotCoverEnoughAZs` sentence pair for `coverage`, as Amazon RDS
/// words it: the requirement, the coverage the request's own subnets add up to, and what to
/// do about it.
fn coverage_message(coverage: &str) -> String {
    format!(
        "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. \
         Current AZ coverage: {coverage}. Add subnets to cover at least 2 AZs."
    )
}

/// RULE: a DB subnet group reports, for each subnet it holds, the Availability Zone the
/// service that owns subnets reports that subnet in — on the create, on the read back and
/// after a modification.
#[tokio::test]
async fn a_group_reports_the_zone_ec2_holds_each_of_its_subnets_in() {
    let network = Network::build().await;

    // Rows differ along how many subnets the group names, which zones they lie in and in
    // which order — all of which the reported zones must follow.
    let rows: [(&str, &str, Vec<String>); 3] = [
        (
            "two subnets in two zones",
            "emu-ec2zone-two",
            vec![
                network.first_zone[0].clone(),
                network.second_zone.clone(),
            ],
        ),
        (
            "the same two subnets named in the other order",
            "emu-ec2zone-reversed",
            vec![
                network.second_zone.clone(),
                network.first_zone[0].clone(),
            ],
        ),
        (
            "three subnets, two of them in one zone",
            "emu-ec2zone-three",
            vec![
                network.first_zone[0].clone(),
                network.first_zone[1].clone(),
                network.second_zone.clone(),
            ],
        ),
    ];

    for (case, name, subnets) in rows {
        let expected = network.zones_of(&subnets).await;
        let created = expect_ok(case, network.account.create_group(name, &subnets).await);
        assert_reported_zones(case, "CreateDBSubnetGroup", &created, &subnets, &expected);

        let read_back = expect_ok(case, network.account.describe_group(name).await);
        assert_reported_zones(
            case,
            "DescribeDBSubnetGroups",
            &read_back,
            &subnets,
            &expected,
        );

        // The modification leaves the group holding a DIFFERENT list, and the zones it
        // reports follow that list rather than the one the create named.
        let modified_subnets = vec![
            network.second_zone.clone(),
            network.first_zone[1].clone(),
        ];
        let modified_zones = network.zones_of(&modified_subnets).await;
        let modified = expect_ok(
            case,
            network.account.modify_group(name, &modified_subnets).await,
        );
        assert_reported_zones(
            case,
            "ModifyDBSubnetGroup",
            &modified,
            &modified_subnets,
            &modified_zones,
        );
        let read_back = expect_ok(case, network.account.describe_group(name).await);
        assert_reported_zones(
            case,
            "DescribeDBSubnetGroups after the modification",
            &read_back,
            &modified_subnets,
            &modified_zones,
        );
    }
}

/// RULE: the coverage a `SubnetIds` list has is the set of ZONES its subnets lie in, as EC2
/// reports them: a list covering fewer than two is refused with the
/// `DBSubnetGroupDoesNotCoverEnoughAZs` sentence pair naming exactly those zones, and a list
/// covering two or more is served.
#[tokio::test]
async fn the_coverage_refusal_names_the_zones_the_requests_own_subnets_cover() {
    let network = Network::build().await;
    let [first, second_in_first_zone] = network.first_zone.clone();
    let second_zone = network.second_zone.clone();

    // Rows spanning what the rule generalises over: how many subnets the list names, how
    // many DISTINCT subnets those are, and how many distinct ZONES they lie in.
    let refused: [(&str, Vec<String>, Vec<String>); 3] = [
        (
            "one subnet of the first zone",
            vec![first.clone()],
            vec![network.zones[0].clone()],
        ),
        (
            "the same subnet twice",
            vec![first.clone(), first.clone()],
            vec![network.zones[0].clone()],
        ),
        (
            "two DISTINCT subnets that lie in ONE zone",
            vec![first.clone(), second_in_first_zone.clone()],
            vec![network.zones[0].clone()],
        ),
    ];

    for (index, (case, subnets, covered)) in refused.into_iter().enumerate() {
        let name = format!("emu-ec2zone-thin-{index}");
        let body = expect_error(
            case,
            network.account.create_group(&name, &subnets).await,
            400,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        );
        let message = text(&body, "Message");
        let expected = coverage_message(&covered.join(", "));
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The refused create stored nothing: the group it would have made is not there.
        expect_error(
            case,
            network.account.describe_group(&name).await,
            404,
            "DBSubnetGroupNotFoundFault",
        );
    }

    // And the same request is SERVED once its subnets cover two zones, which is the other
    // side of the one rule.
    let served: [(&str, Vec<String>); 2] = [
        (
            "two subnets in two zones",
            vec![first.clone(), second_zone.clone()],
        ),
        (
            "three subnets in two zones",
            vec![first, second_in_first_zone, second_zone],
        ),
    ];
    for (index, (case, subnets)) in served.into_iter().enumerate() {
        let name = format!("emu-ec2zone-served-{index}");
        let expected = network.zones_of(&subnets).await;
        let created = expect_ok(case, network.account.create_group(&name, &subnets).await);
        assert_reported_zones(case, "CreateDBSubnetGroup", &created, &subnets, &expected);
    }

    // A modification is held to the same rule as a create: the group it would leave behind
    // has to cover the two zones, and the refusal names the coverage the NEW list has.
    let name = "emu-ec2zone-modified-thin";
    expect_ok(
        "the group to modify",
        network
            .account
            .create_group(
                name,
                &[
                    network.first_zone[0].clone(),
                    network.second_zone.clone(),
                ],
            )
            .await,
    );
    let body = expect_error(
        "a modification down to one zone",
        network
            .account
            .modify_group(name, std::slice::from_ref(&network.second_zone))
            .await,
        400,
        "DBSubnetGroupDoesNotCoverEnoughAZs",
    );
    let message = text(&body, "Message");
    let expected = coverage_message(&network.zones[1]);
    assert_eq!(
        message, expected,
        "case \"a modification down to one zone\": expected {expected:?}, got {message:?}"
    );
}

/// RULE: a subnet the account has no record of has no zone this service knows, so the group
/// reports NO `SubnetAvailabilityZone` for it — never one of this service's own choosing —
/// and the zone it DOES report for a subnet EC2 answered for is the one EC2 answered with,
/// kept with the group from then on.
#[tokio::test]
async fn a_subnet_the_account_has_no_record_of_is_reported_with_no_zone() {
    let network = Network::build().await;

    // Rows spanning where in the list the unknown identifier sits, how many there are and
    // which published identifier form it takes (8 and 17 hexadecimal characters).
    let rows: [(&str, Vec<String>, Vec<&str>); 3] = [
        (
            "an identifier EC2 minted no subnet for, named first",
            vec![ABSENT_SUBNET.to_string(), network.second_zone.clone()],
            vec![ABSENT_SUBNET],
        ),
        (
            "the same identifier after a subnet the account does have",
            vec![network.first_zone[0].clone(), ABSENT_SUBNET.to_string()],
            vec![ABSENT_SUBNET],
        ),
        (
            "two identifiers the account has no subnet for",
            vec![ABSENT_SUBNET.to_string(), SHORT_ABSENT_SUBNET.to_string()],
            vec![ABSENT_SUBNET, SHORT_ABSENT_SUBNET],
        ),
    ];

    for (index, (case, subnets, unknown)) in rows.into_iter().enumerate() {
        let name = format!("emu-ec2zone-absent-{index}");
        // Nothing is known about the zone of an identifier the account has no subnet for,
        // which is not evidence that the list covers too few zones: the group is created,
        // and it reports the zone of the subnets EC2 did answer for and none for the rest.
        let created = expect_ok(case, network.account.create_group(&name, &subnets).await);
        let reported = subtrees(&created, "Subnet");
        assert_eq!(
            reported.len(),
            subnets.len(),
            "case {case:?}: expected {} subnet(s), got {reported:?}",
            subnets.len()
        );
        for (subnet, identifier) in reported.iter().zip(&subnets) {
            let zone = subtrees(subnet, "SubnetAvailabilityZone");
            if unknown.contains(&identifier.as_str()) {
                assert!(
                    zone.is_empty(),
                    "case {case:?}: no zone is known for {identifier:?}, so none may be \
                     reported, got {zone:?}"
                );
            } else {
                let expected = network.account.zone_of(identifier).await;
                let reported_zone = text(subnet, "Name");
                assert_eq!(
                    reported_zone, expected,
                    "case {case:?}: expected the zone EC2 reports for {identifier:?}, got \
                     {reported_zone:?}"
                );
            }
        }
    }

    // A subnet the account HELD and holds no longer: the group created while it existed
    // keeps reporting the zone EC2 answered with for it — that is the zone it was placed in
    // — while a group created after the subnet was removed knows no zone for it.
    let vpc = network.account.create_vpc("10.53.0.0/16").await;
    let doomed = network
        .account
        .create_subnet(&vpc, "10.53.1.0/24", &network.zones[0])
        .await;
    let case = "a subnet EC2 minted and then deleted";
    let before = [doomed.clone(), network.second_zone.clone()];
    let zones_before = network.zones_of(&before).await;
    expect_ok(
        case,
        network
            .account
            .create_group("emu-ec2zone-deleted", &before)
            .await,
    );
    network
        .account
        .ec2
        .delete_subnet()
        .subnet_id(&doomed)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: EC2 must delete the subnet: {error:?}"));
    let kept = expect_ok(case, network.account.describe_group("emu-ec2zone-deleted").await);
    assert_reported_zones(
        case,
        "DescribeDBSubnetGroups after the subnet was deleted",
        &kept,
        &before,
        &zones_before,
    );
    let created_after = expect_ok(
        case,
        network
            .account
            .create_group("emu-ec2zone-deleted-2", &before)
            .await,
    );
    let reported = subtrees(&created_after, "Subnet");
    let zone = subtrees(
        reported.first().expect("the group reports its subnets"),
        "SubnetAvailabilityZone",
    );
    assert!(
        zone.is_empty(),
        "case {case:?}: the account has no subnet {doomed} any more, so no zone may be \
         reported for it, got {zone:?}"
    );
}

/// RULE: the zones a stored group covers are what a cluster-placement precondition counts: a
/// Multi-AZ DB cluster needs a subnet in three Availability Zones, so a create against a
/// group whose subnets cover fewer is refused with the zones of the Region that group does
/// not cover yet, and one against a group covering three is served.
#[tokio::test]
async fn a_multi_az_cluster_is_placed_by_the_zones_its_subnet_group_covers() {
    let network = Network::build().await;
    let zones = network.account.zones().await;
    assert!(
        zones.len() >= 3,
        "the Region must have three Availability Zones for these cases, got {zones:?}"
    );
    // A third subnet, in the Region's third zone, so a group can be made that covers two
    // zones and one that covers three.
    let third_zone = network
        .account
        .create_subnet(&network.vpc, "10.52.4.0/24", &zones[2])
        .await;

    // The settings `CreateDBCluster` documents as required for a Multi-AZ DB cluster, with
    // an `Iops` inside the ratio `Iops` documents against `AllocatedStorage`.
    let multi_az: &[(&str, String)] = &[
        ("AllocatedStorage", "100".to_string()),
        ("DBClusterInstanceClass", "db.m6gd.xlarge".to_string()),
        ("StorageType", "io1".to_string()),
        ("Iops", "1000".to_string()),
    ];

    // Rows differ along how many zones the group covers: two (one short, and the row that
    // the old distinct-SUBNET reading got wrong, since three subnets in two zones still
    // cover two) and three.
    //
    // `uncovered` is the zones of the Region the group does not cover, which the refusal
    // names; a row that covers them all is served instead.
    struct MultiAzRow {
        case: &'static str,
        subnets: Vec<String>,
        uncovered: Vec<String>,
    }
    let rows: [MultiAzRow; 3] = [
        MultiAzRow {
            case: "a group covering two zones",
            subnets: vec![network.first_zone[0].clone(), network.second_zone.clone()],
            uncovered: vec![zones[2].clone()],
        },
        MultiAzRow {
            case: "a group of three subnets covering two zones",
            subnets: vec![
                network.first_zone[0].clone(),
                network.first_zone[1].clone(),
                network.second_zone.clone(),
            ],
            uncovered: vec![zones[2].clone()],
        },
        MultiAzRow {
            case: "a group covering three zones",
            subnets: vec![
                network.first_zone[0].clone(),
                network.second_zone.clone(),
                third_zone.clone(),
            ],
            uncovered: Vec::new(),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let group = format!("emu-ec2zone-multiaz-{index}");
        expect_ok(case, network.account.create_group(&group, &row.subnets).await);
        let identifier = format!("emu-ec2zone-multiaz-cl-{index}");
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", identifier.clone()),
            ("Engine", "mysql".to_string()),
            ("DBSubnetGroupName", group.clone()),
        ];
        params.extend(multi_az.iter().cloned());
        let response = call_in(&network.account.router, REGION, "CreateDBCluster", &params).await;

        if row.uncovered.is_empty() {
            expect_ok(case, response);
        } else {
            let body = expect_error(case, response, 400, "InvalidVPCNetworkStateFault");
            let message = text(&body, "Message");
            let expected = format!(
                "Can't create a Multi-AZ DB cluster because there aren't enough subnets \
                 available. At least three subnets with available IP addresses in \
                 different Availability Zones are required. You can add subnets to a DB \
                 subnet group in the following Availability Zones: {}.",
                row.uncovered.join(", ")
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
