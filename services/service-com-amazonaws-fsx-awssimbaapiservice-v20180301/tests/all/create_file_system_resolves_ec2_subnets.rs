// SPDX-License-Identifier: CC0-1.0

//! `CreateFileSystem` places a file system in the subnets the ACCOUNT has, as the service
//! that owns subnets reports them.
//!
//! `SubnetIds` is "the IDs of the subnets that the file system will be accessible from" and
//! a subnet is an Amazon EC2 resource a client creates with `ec2:CreateSubnet` in a VPC it
//! owns, so the rule under test is:
//!
//! * a subnet identifier a create names resolves against what EC2 holds for the account in
//!   the Region the request is signed for, so a subnet the client just minted is one a file
//!   system can be placed in: the call answers HTTP 200 with `Lifecycle` `CREATING` and a
//!   `VpcId` read off the subnet's own record — the VPC EC2 put it in;
//! * the modelled `InvalidNetworkSettings` (HTTP 400) is reserved for an identifier that
//!   resolves to NO subnet of the account — one EC2 never minted, or one it minted and then
//!   deleted — and such a create stores nothing.
//!
//! EC2 is mounted beside FSx on ONE engine and driven through the AWS SDK over the same
//! in-process callback this service's own resolution reaches it through. Nothing here
//! reaches into either store: a subnet exists only because EC2 answered for it, and a file
//! system only because a later `DescribeFileSystems` reported it. Every id a row names was
//! MINTED by EC2 (or is deliberately absent), so no id this suite writes can pass itself off
//! as one the account holds.
//!
//! The rows differ along what the rule generalises over: the modelled `FileSystemType` and
//! its per-type configuration, the Availability Zone the subnet sits in, how many subnets
//! the request names, which of the account's two VPCs they belong to, whether the identifier
//! is also named as a `PreferredSubnetId`, and — for the refusals — whether the identifier
//! was never minted or was minted and deleted, and where in the request it sits.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// ONE harness per crate: FSx mounted beside the owner of subnets on one engine, with an EC2
// client over the router's own in-process callback — the very seam this service's subnet
// resolution reaches EC2 through — comes from the lifecycle suite rather than a second copy.
use crate::file_system_lifecycle::{
    call, emulator, ok, with_client_request_token, Emulator, PAST_TRANSITION,
};

/// The lifecycle a create answers with ("The CreateFileSystem call returns while the file
/// system's lifecycle state is still CREATING") and the one the clock owes it. Spelled out
/// here rather than read from the production constants, so a change to them fails a test
/// instead of silently changing what this suite accepts.
const LIFECYCLE_CREATING: &str = "CREATING";
const LIFECYCLE_AVAILABLE: &str = "AVAILABLE";

/// The Availability Zones the account's subnets are created in, one per subnet of the first
/// VPC. Three zones, because "a Region has at least three Availability Zones".
const ZONES: [&str; 3] = ["us-west-2a", "us-west-2b", "us-west-2c"];

/// Identifiers of the published `SubnetId` form (`length 15..24`, pattern
/// `^(subnet-[0-9a-f]{8,})$`) that EC2 mints no subnet for, so each passes the boundary
/// validator and reaches the handler's resolution. Both ends of the published length appear.
const ABSENT_SUBNET: &str = "subnet-00000000deadbeef";
const SHORT_ABSENT_SUBNET: &str = "subnet-0000beef";

/// The VPC `ec2:CreateVpc` mints for `cidr_block`, as the id EC2 reported.
pub(crate) async fn create_vpc(account: &Emulator, case: &str, cidr_block: &str) -> String {
    let created = account
        .ec2
        .create_vpc()
        .cidr_block(cidr_block)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must create the VPC {cidr_block}: {error:?}")
        });
    created
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .unwrap_or_else(|| panic!("case {case:?}: CreateVpc must report the VPC it minted"))
        .to_string()
}

/// The subnet `ec2:CreateSubnet` mints in `vpc_id`, as the id EC2 reported.
pub(crate) async fn create_subnet(
    account: &Emulator,
    case: &str,
    vpc_id: &str,
    cidr: &str,
    zone: &str,
) -> String {
    let created = account
        .ec2
        .create_subnet()
        .vpc_id(vpc_id)
        .cidr_block(cidr)
        .availability_zone(zone)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must create the subnet {cidr} in {zone}: {error:?}")
        });
    created
        .subnet()
        .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
        .unwrap_or_else(|| panic!("case {case:?}: CreateSubnet must report the subnet it minted"))
        .to_string()
}

/// Remove a subnet again through EC2's own API: an id the account HELD and holds no longer,
/// which is a different thing from one it never had.
pub(crate) async fn delete_subnet(account: &Emulator, case: &str, subnet_id: &str) {
    account
        .ec2
        .delete_subnet()
        .subnet_id(subnet_id)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must delete the subnet {subnet_id}: {error:?}")
        });
}

/// The network of one account: two VPCs EC2 minted, the subnets it minted in each, and one
/// more subnet of the first VPC that it minted and then deleted.
struct Network {
    account: Emulator,
    here: String,
    here_subnets: Vec<String>,
    elsewhere: String,
    elsewhere_subnet: String,
    deleted_subnet: String,
}

/// Build that network, every id as EC2 minted it.
///
/// The blocks are RFC 1918 ranges that overlap nothing else; a /24 per subnet leaves room for
/// every subnet a row names.
async fn network(case: &str) -> Network {
    let account = emulator().await;
    let here = create_vpc(&account, case, "10.0.0.0/16").await;
    let mut here_subnets = Vec::with_capacity(ZONES.len());
    for (index, zone) in ZONES.iter().enumerate() {
        here_subnets.push(
            create_subnet(
                &account,
                case,
                &here,
                &format!("10.0.{}.0/24", index + 1),
                zone,
            )
            .await,
        );
    }
    // One more subnet of the first VPC, minted and then deleted again.
    let deleted_subnet = create_subnet(&account, case, &here, "10.0.9.0/24", ZONES[0]).await;
    delete_subnet(&account, case, &deleted_subnet).await;

    let elsewhere = create_vpc(&account, case, "10.1.0.0/16").await;
    let elsewhere_subnet =
        create_subnet(&account, case, &elsewhere, "10.1.1.0/24", ZONES[0]).await;

    Network {
        account,
        here,
        here_subnets,
        elsewhere,
        elsewhere_subnet,
        deleted_subnet,
    }
}

/// Which subnet a row names, and where it came from.
#[derive(Debug, Clone, Copy)]
enum Subnet {
    /// The subnet at this index of the first VPC's minted subnets.
    Here(usize),
    /// The one subnet of the second VPC.
    Elsewhere,
    /// The subnet EC2 minted in the first VPC and then deleted.
    Deleted,
    /// A well-formed identifier EC2 minted no subnet for.
    Absent(&'static str),
}

impl Network {
    /// The identifier a `Subnet` of a row resolves to.
    fn subnet(&self, named: Subnet) -> &str {
        match named {
            Subnet::Here(index) => &self.here_subnets[index],
            Subnet::Elsewhere => &self.elsewhere_subnet,
            Subnet::Deleted => &self.deleted_subnet,
            Subnet::Absent(id) => id,
        }
    }

    /// The identifiers a row's `SubnetIds` carries.
    fn subnets(&self, named: &[Subnet]) -> Vec<String> {
        named
            .iter()
            .map(|subnet| self.subnet(*subnet).to_string())
            .collect()
    }
}

/// How many file systems the store holds, read back through the implemented reader.
async fn stored_count(router: &MultiServiceRouter) -> usize {
    ok(router, "DescribeFileSystems", &json!({})).await["FileSystems"]
        .as_array()
        .map_or(0, Vec::len)
}

/// The `__type` of an error body, with the namespace an `awsJson` client strips.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One per-type configuration a row carries: the request member it goes under, and the body
/// it carries.
type Configuration = (&'static str, fn() -> Value);

/// One create: the modelled type, its per-type configuration, and the subnets it is placed
/// in.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    file_system_type: &'static str,
    storage_capacity: i64,
    /// The per-type configuration member and the body it carries, with
    /// `"PreferredSubnetId"` filled in from `preferred` when that is `Some`.
    configuration: Option<Configuration>,
    /// Which of the row's own `subnets` is also named as the configuration's
    /// `PreferredSubnetId`.
    preferred: Option<usize>,
    subnets: &'static [Subnet],
    /// Which VPC the created file system must report: the first one, or the second.
    in_second_vpc: bool,
}

/// The per-type configuration bodies the rows carry. Each names only members the model
/// declares for that type (`smithy_query(query="operation",operation="CreateFileSystem")`),
/// with the required ones filled in.
fn open_zfs() -> Value {
    json!({
        "DeploymentType": "SINGLE_AZ_1",
        "ThroughputCapacity": 64,
        "AutomaticBackupRetentionDays": 0,
        "RootVolumeConfiguration": { "DataCompressionType": "ZSTD", "RecordSizeKiB": 128 },
    })
}

fn lustre() -> Value {
    json!({ "DeploymentType": "SCRATCH_2" })
}

fn windows() -> Value {
    json!({ "ThroughputCapacity": 32, "DeploymentType": "MULTI_AZ_1" })
}

fn ontap() -> Value {
    json!({ "DeploymentType": "MULTI_AZ_1", "ThroughputCapacity": 128 })
}

/// Creates that must be ACCEPTED, because every identifier they name is a subnet EC2 minted
/// for this account.
const ACCEPTED: &[Row] = &[
    Row {
        case: "OpenZFS SINGLE_AZ_1 in the first zone's minted subnet",
        file_system_type: "OPENZFS",
        storage_capacity: 64,
        configuration: Some(("OpenZFSConfiguration", open_zfs)),
        preferred: None,
        subnets: &[Subnet::Here(0)],
        in_second_vpc: false,
    },
    Row {
        case: "Lustre SCRATCH_2 in the second zone's minted subnet",
        file_system_type: "LUSTRE",
        storage_capacity: 1200,
        configuration: Some(("LustreConfiguration", lustre)),
        preferred: None,
        subnets: &[Subnet::Here(1)],
        in_second_vpc: false,
    },
    Row {
        case: "Lustre SCRATCH_2 in the third zone's minted subnet",
        file_system_type: "LUSTRE",
        storage_capacity: 1200,
        configuration: Some(("LustreConfiguration", lustre)),
        preferred: None,
        subnets: &[Subnet::Here(2)],
        in_second_vpc: false,
    },
    Row {
        case: "Windows MULTI_AZ_1 across two minted subnets, the first preferred",
        file_system_type: "WINDOWS",
        storage_capacity: 32,
        configuration: Some(("WindowsConfiguration", windows)),
        preferred: Some(0),
        subnets: &[Subnet::Here(0), Subnet::Here(1)],
        in_second_vpc: false,
    },
    Row {
        case: "ONTAP MULTI_AZ_1 across two minted subnets, the second preferred",
        file_system_type: "ONTAP",
        storage_capacity: 1024,
        configuration: Some(("OntapConfiguration", ontap)),
        preferred: Some(1),
        subnets: &[Subnet::Here(0), Subnet::Here(1)],
        in_second_vpc: false,
    },
    Row {
        case: "every minted subnet of the first VPC, one per zone",
        file_system_type: "WINDOWS",
        storage_capacity: 32,
        configuration: None,
        preferred: None,
        subnets: &[Subnet::Here(0), Subnet::Here(1), Subnet::Here(2)],
        in_second_vpc: false,
    },
    Row {
        case: "the minted subnet of the OTHER VPC",
        file_system_type: "OPENZFS",
        storage_capacity: 64,
        configuration: Some(("OpenZFSConfiguration", open_zfs)),
        preferred: None,
        subnets: &[Subnet::Elsewhere],
        in_second_vpc: true,
    },
];

/// One create that must be REFUSED, and which identifier the refusal has to name: the index
/// into the row's own `subnets`, or the `PreferredSubnetId` when `preferred_absent` is set.
#[derive(Debug, Clone, Copy)]
struct RefusedRow {
    case: &'static str,
    subnets: &'static [Subnet],
    /// A `PreferredSubnetId` that names no subnet of the account, when the row carries one.
    preferred_absent: Option<&'static str>,
    /// The position in `subnets` of the identifier the refusal must name; ignored when
    /// `preferred_absent` is set.
    offending: usize,
}

const REFUSED: &[RefusedRow] = &[
    RefusedRow {
        case: "an identifier EC2 minted no subnet for",
        subnets: &[Subnet::Absent(ABSENT_SUBNET)],
        preferred_absent: None,
        offending: 0,
    },
    RefusedRow {
        case: "the shortest published form, which EC2 minted no subnet for",
        subnets: &[Subnet::Absent(SHORT_ABSENT_SUBNET)],
        preferred_absent: None,
        offending: 0,
    },
    RefusedRow {
        case: "a subnet EC2 minted and then deleted",
        subnets: &[Subnet::Deleted],
        preferred_absent: None,
        offending: 0,
    },
    RefusedRow {
        case: "an absent identifier after two minted ones",
        subnets: &[
            Subnet::Here(0),
            Subnet::Here(1),
            Subnet::Absent(ABSENT_SUBNET),
        ],
        preferred_absent: None,
        offending: 2,
    },
    RefusedRow {
        case: "an absent identifier before a minted one",
        subnets: &[Subnet::Absent(ABSENT_SUBNET), Subnet::Here(0)],
        preferred_absent: None,
        offending: 0,
    },
    RefusedRow {
        case: "a minted subnet and a deleted one together",
        subnets: &[Subnet::Here(0), Subnet::Deleted],
        preferred_absent: None,
        offending: 1,
    },
    RefusedRow {
        case: "minted subnets and a PreferredSubnetId EC2 minted nothing for",
        subnets: &[Subnet::Here(0), Subnet::Here(1)],
        preferred_absent: Some(ABSENT_SUBNET),
        offending: 0,
    },
];

/// The request body one row sends.
fn request(row: &Row, named: &[String]) -> Value {
    let mut body = json!({
        "FileSystemType": row.file_system_type,
        "StorageCapacity": row.storage_capacity,
        "SubnetIds": named,
    });
    if let Some((member, configuration)) = row.configuration {
        let mut configuration = configuration();
        if let Some(preferred) = row.preferred {
            configuration["PreferredSubnetId"] = json!(named[preferred]);
        }
        body[member] = configuration;
    }
    with_client_request_token(&body)
}

/// RULE: a create naming subnets the account's own Amazon EC2 minted for it is accepted —
/// answered with `Lifecycle` `CREATING` and the VPC EC2 put those subnets in — and the
/// stored record is what a later read reports, settling into `AVAILABLE` when the clock owes
/// it.
#[tokio::test]
async fn a_create_is_accepted_in_the_subnets_ec2_minted_for_the_account() {
    for row in ACCEPTED {
        // A fresh emulator per row, so the network EC2 minted is exactly this row's.
        let net = network(row.case).await;
        let named = net.subnets(row.subnets);
        let expected_vpc = if row.in_second_vpc {
            &net.elsewhere
        } else {
            &net.here
        };

        let created = ok(&net.account.router, "CreateFileSystem", &request(row, &named)).await
            ["FileSystem"]
            .clone();
        assert_eq!(
            created["SubnetIds"],
            json!(named),
            "case {:?}: expected the requested SubnetIds on the created file system, got {:?}",
            row.case,
            created["SubnetIds"]
        );
        assert_eq!(
            created["VpcId"].as_str(),
            Some(expected_vpc.as_str()),
            "case {:?}: expected VpcId {expected_vpc} (the VPC EC2 minted these subnets in), \
             got {:?}",
            row.case,
            created["VpcId"]
        );
        // The span is never zero: a caller acting straight after the call still sees the
        // file system being created.
        assert_eq!(
            created["Lifecycle"].as_str(),
            Some(LIFECYCLE_CREATING),
            "case {:?}: expected Lifecycle {LIFECYCLE_CREATING:?}, got {:?}",
            row.case,
            created["Lifecycle"]
        );

        let id = created["FileSystemId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no FileSystemId in {created:?}", row.case))
            .to_string();

        // Read back through the implemented reader: the response was the stored record.
        let described = ok(
            &net.account.router,
            "DescribeFileSystems",
            &json!({ "FileSystemIds": [id.clone()] }),
        )
        .await["FileSystems"][0]
            .clone();
        for member in ["SubnetIds", "VpcId", "FileSystemId"] {
            assert_eq!(
                described[member], created[member],
                "case {:?}: DescribeFileSystems must report the stored {member}; create said \
                 {:?} and describe said {:?}",
                row.case, created[member], described[member]
            );
        }

        // And the transition the clock owes it is the one AWS documents a create as being on
        // its way to.
        tokio::time::sleep(PAST_TRANSITION).await;
        let settled = ok(
            &net.account.router,
            "DescribeFileSystems",
            &json!({ "FileSystemIds": [id] }),
        )
        .await["FileSystems"][0]
            .clone();
        assert_eq!(
            settled["Lifecycle"].as_str(),
            Some(LIFECYCLE_AVAILABLE),
            "case {:?}: expected the file system to settle {LIFECYCLE_AVAILABLE:?}, got {:?}",
            row.case,
            settled["Lifecycle"]
        );
    }
}

/// RULE: an identifier that resolves to no subnet of the account — one EC2 never minted, or
/// one it minted and then deleted — is refused with the modelled `InvalidNetworkSettings`
/// (HTTP 400) naming that identifier in `InvalidSubnetId`, and the create stores nothing.
#[tokio::test]
async fn a_create_is_refused_for_a_subnet_ec2_does_not_hold() {
    for row in REFUSED {
        // A fresh emulator per row, so the network EC2 minted is exactly this row's.
        let net = network(row.case).await;
        let named = net.subnets(row.subnets);
        let mut body = json!({
            "FileSystemType": "WINDOWS",
            "StorageCapacity": 32,
            "SubnetIds": named,
        });
        let offending = match row.preferred_absent {
            Some(preferred) => {
                body["WindowsConfiguration"] =
                    json!({ "ThroughputCapacity": 32, "PreferredSubnetId": preferred });
                preferred.to_string()
            }
            None => named[row.offending].clone(),
        };

        let (status, refused) = call(
            &net.account.router,
            "CreateFileSystem",
            &with_client_request_token(&body),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for a subnet the account does not have, got {status} \
             with body {refused}",
            row.case
        );
        assert_eq!(
            error_code(&refused),
            "InvalidNetworkSettings",
            "case {:?}: expected the modelled InvalidNetworkSettings, got {refused}",
            row.case
        );
        assert_eq!(
            refused["InvalidSubnetId"].as_str(),
            Some(offending.as_str()),
            "case {:?}: expected InvalidSubnetId {offending}, got {:?}",
            row.case,
            refused["InvalidSubnetId"]
        );
        let message = refused["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(offending.as_str()),
            "case {:?}: expected a Message naming {offending}, got {message:?}",
            row.case
        );
        assert_eq!(
            stored_count(&net.account.router).await,
            0,
            "case {:?}: a refused create must store nothing",
            row.case
        );

        // And the same request succeeds once it names only subnets EC2 did mint, so the
        // refusal was about those identifiers and nothing else.
        let repaired = ok(
            &net.account.router,
            "CreateFileSystem",
            &with_client_request_token(&json!({
                "FileSystemType": "WINDOWS",
                "StorageCapacity": 32,
                "SubnetIds": [net.here_subnets[0].clone()],
            })),
        )
        .await;
        assert_eq!(
            repaired["FileSystem"]["VpcId"].as_str(),
            Some(net.here.as_str()),
            "case {:?}: the repaired create must report the VPC EC2 minted, got {:?}",
            row.case,
            repaired["FileSystem"]["VpcId"]
        );
    }
}
