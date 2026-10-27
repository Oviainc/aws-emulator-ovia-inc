// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeSecondaryInterfaces`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeSecondaryInterfaces`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's secondary interfaces, narrowed by the `SecondaryInterfaceIds` and `Filters`
//! the request names and cut into `MaxResults`-sized pages, with each interface's own
//! `Status` following from the attachment it holds. Every table row is a distinct input along
//! what that generalises over — which documented filter, which member of the model's own
//! catalogs, how many values, how many filters, how many private addresses, whether an
//! instance holds the interface, which ids, which page size — so no single recorded request
//! is the specification.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{
    ATTACHMENT_STATUS_VALUES, SECONDARY_INTERFACE_STATUS_VALUES, SECONDARY_INTERFACE_TYPE_VALUES,
    SECONDARY_NETWORK_TYPE_VALUES, Tag,
};
use ec2::state::{
    ATTACHMENT_STATUS_ATTACHED, ATTACHMENT_STATUS_DETACHED,
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_SOURCE_DEST_CHECK, MAX_FILTER_VALUE_BYTES,
    MAX_FILTER_VALUES, MAX_FILTERS, MAX_RESOURCE_IDS, SECONDARY_INTERFACE_RESOURCE,
    SECONDARY_INTERFACE_STATUS_AVAILABLE, SECONDARY_INTERFACE_STATUS_IN_USE,
    SECONDARY_INTERFACE_TYPE_SECONDARY, SECONDARY_NETWORK_TYPE_RDMA,
    SECONDARY_SUBNET_STATE_CREATE_COMPLETE, SecondaryInterfaceAttachmentSpec,
    SecondaryInterfaceSpec, SecondaryNetworkSpec, SecondarySubnetSpec, availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, error_code, filters, indexed, unescaped};

/// The `status` of each secondary interface a response reports: the `<status>` element that
/// is a DIRECT child of a reported interface, rather than the one inside its `<attachment>`.
/// Read with the XML parser, because the modelled `SecondaryInterface` carries its
/// `Attachment` before its own `Status`, so a substring read would answer the attachment's.
fn reported_statuses(case: &str, body: &str) -> Vec<String> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .filter_map(|item| {
            item.children()
                .find(|child| child.is_element() && child.tag_name().name() == "status")
        })
        .map(|status| status.text().unwrap_or_default().to_string())
        .collect()
}

/// The Region every call here is signed for, and the Region the seeded interfaces' ARNs name.
const REGION: &str = "us-east-1";

/// The instant the seeded attachments were made at, as a fixed point rather than
/// `SystemTime::now()`, so the timestamp a response reports can be compared exactly.
fn attach_base() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// One `DescribeSecondaryInterfaces` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecondaryInterfaces", params).await
}

/// The secondary interface ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "secondaryInterfaceId")
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded secondary interfaces carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded attachment is asked for: the instance that holds the interface, its
/// `AttachmentStatus`, and the device and network card indices.
struct AttachmentFixture {
    instance_id: &'static str,
    status: &'static str,
    device_index: i32,
    network_card_index: i32,
}

/// What one seeded secondary interface is created with, beyond the facts the store derives
/// itself.
struct InterfaceFixture {
    /// Which of the seeded secondary subnets — and so which secondary network and zone — the
    /// interface is in.
    subnet_index: usize,
    /// The private IPv4 addresses the interface holds: none, one, or several.
    private_ipv4_addresses: Vec<&'static str>,
    /// Whether source/destination checking is on for this interface.
    source_dest_check: bool,
    /// The instance holding the interface, for the ones an instance holds.
    attachment: Option<AttachmentFixture>,
    tags: Vec<Tag>,
}

/// An instance id of this account, in the long EC2 form.
const INSTANCE_A: &str = "i-0000000000000000a";
const INSTANCE_B: &str = "i-0000000000000000b";
const INSTANCE_C: &str = "i-0000000000000000c";

/// The secondary interfaces the filter and paging tables are written against. They differ
/// along every attribute the documented filters select on — the subnet and network, the
/// addresses, the attachment and its status, and the tags — and along how many addresses they
/// carry and whether anything holds them at all.
fn seeded_fixtures() -> Vec<InterfaceFixture> {
    vec![
        InterfaceFixture {
            subnet_index: 0,
            private_ipv4_addresses: vec!["10.0.0.10"],
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            attachment: Some(AttachmentFixture {
                instance_id: INSTANCE_A,
                status: "attached",
                device_index: 1,
                network_card_index: 0,
            }),
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        },
        InterfaceFixture {
            subnet_index: 0,
            // Several addresses on one interface: a filter on either selects it.
            private_ipv4_addresses: vec!["10.0.0.11", "10.0.0.12"],
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            attachment: Some(AttachmentFixture {
                instance_id: INSTANCE_A,
                status: "attaching",
                device_index: 2,
                network_card_index: 0,
            }),
            tags: vec![tag("Name", "batch")],
        },
        InterfaceFixture {
            subnet_index: 1,
            // An interface holding no address at all, so every
            // `private-ipv4-addresses.*` filter must pass it over.
            private_ipv4_addresses: Vec::new(),
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            attachment: None,
            tags: Vec::new(),
        },
        InterfaceFixture {
            subnet_index: 1,
            private_ipv4_addresses: vec!["10.0.1.10"],
            source_dest_check: false,
            attachment: Some(AttachmentFixture {
                instance_id: INSTANCE_B,
                status: "detaching",
                device_index: 1,
                network_card_index: 1,
            }),
            tags: Vec::new(),
        },
        InterfaceFixture {
            subnet_index: 1,
            private_ipv4_addresses: vec!["10.0.1.11"],
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            // An attachment that has been taken away holds nothing, so this interface is
            // `available` though it carries an attachment.
            attachment: Some(AttachmentFixture {
                instance_id: INSTANCE_B,
                status: "detached",
                device_index: 3,
                network_card_index: 0,
            }),
            tags: vec![tag("Name", "web")],
        },
        InterfaceFixture {
            subnet_index: 2,
            private_ipv4_addresses: vec!["10.0.2.10", "10.0.2.11", "10.0.2.12"],
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            attachment: Some(AttachmentFixture {
                instance_id: INSTANCE_C,
                status: "attached",
                device_index: 1,
                network_card_index: 0,
            }),
            tags: vec![tag("Owner", "TeamB")],
        },
        InterfaceFixture {
            subnet_index: 2,
            private_ipv4_addresses: vec!["10.0.2.20"],
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            attachment: None,
            tags: Vec::new(),
        },
        InterfaceFixture {
            subnet_index: 2,
            private_ipv4_addresses: vec!["10.0.2.21"],
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            attachment: Some(AttachmentFixture {
                instance_id: INSTANCE_C,
                status: "attached",
                device_index: 2,
                network_card_index: 1,
            }),
            tags: Vec::new(),
        },
    ]
}

/// The ids the seeded secondary interfaces, their subnets and their networks were given.
struct Seeded {
    /// The `SecondaryInterfaceId` of each fixture, in fixture order — which is also store
    /// order, the ids being minted in that order.
    ids: Vec<String>,
    /// The `SecondarySubnetId` of each seeded secondary subnet.
    subnet_ids: Vec<String>,
    /// The `SecondaryNetworkId` of each seeded secondary network.
    network_ids: Vec<String>,
    /// The attachment id reported for each fixture that has one, in fixture order.
    attachment_ids: Vec<Option<String>>,
}

/// Create one secondary network, and one secondary subnet inside it, for the seeded
/// interfaces to sit in — so an interface's `SecondaryNetworkId` and `SecondarySubnetId` name
/// resources this store really holds. `ordinal` keeps their CIDR blocks and zones apart.
fn create_network_and_subnet(guard: &mut Ec2State, ordinal: usize) -> (String, String) {
    let zones = availability_zones_in_region(REGION);
    let zone = &zones[ordinal % zones.len()];
    let network_id = guard.insert_secondary_network(SecondaryNetworkSpec {
        ipv4_cidr_block: format!("10.{ordinal}.0.0/16"),
        network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        // Already past its `create-in-progress` -> `create-complete` move: nothing here
        // reads the network's own state, the interfaces are what is under test.
        create_complete_at: std::time::Instant::now(),
    });
    let subnet_id = guard
        .insert_secondary_subnet(SecondarySubnetSpec {
            secondary_network_id: network_id.clone(),
            secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
            availability_zone: zone.zone_name.clone(),
            availability_zone_id: zone.zone_id.clone(),
            region: REGION.to_string(),
            ipv4_cidr_blocks: Vec::new(),
            state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
            state_reason: None,
            tags: Vec::new(),
        })
        .expect("the seeded store is far below its secondary subnet ceiling");
    (network_id, subnet_id)
}

/// Write the seeded secondary interfaces into the engine's store, the way an operation
/// creating one would, and report the ids they were given.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let zones = availability_zones_in_region(REGION);
    let mut network_ids = Vec::new();
    let mut subnet_ids = Vec::new();
    for ordinal in 0..3 {
        let (network_id, subnet_id) = create_network_and_subnet(&mut guard, ordinal);
        network_ids.push(network_id);
        subnet_ids.push(subnet_id);
    }

    let mut ids = Vec::new();
    let mut attachment_ids = Vec::new();
    for (index, fixture) in seeded_fixtures().into_iter().enumerate() {
        let zone = &zones[fixture.subnet_index % zones.len()];
        let id = guard
            .insert_secondary_interface(SecondaryInterfaceSpec {
                secondary_subnet_id: subnet_ids[fixture.subnet_index].clone(),
                secondary_network_id: network_ids[fixture.subnet_index].clone(),
                secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id.clone(),
                region: REGION.to_string(),
                private_ipv4_addresses: fixture
                    .private_ipv4_addresses
                    .iter()
                    .map(|address| (*address).to_string())
                    .collect(),
                source_dest_check: fixture.source_dest_check,
                attachment: fixture
                    .attachment
                    .as_ref()
                    .map(|attachment| SecondaryInterfaceAttachmentSpec {
                        instance_id: attachment.instance_id.to_string(),
                        device_index: attachment.device_index,
                        network_card_index: attachment.network_card_index,
                        delete_on_termination: true,
                        status: attachment.status.to_string(),
                        // Each attachment a second apart, so a reported timestamp cannot
                        // match by accident.
                        attach_time: attach_base()
                            + Duration::from_secs(u64::try_from(index).unwrap_or_default()),
                    }),
                tags: fixture.tags,
            })
            .expect("the seeded store is far below its secondary interface ceiling");
        let attachment_id = guard
            .secondary_interfaces
            .get(&id)
            .and_then(|interface| interface.attachment.as_ref())
            .and_then(|attachment| attachment.attachment_id.clone());
        ids.push(id);
        attachment_ids.push(attachment_id);
    }

    Seeded {
        ids,
        subnet_ids,
        network_ids,
        attachment_ids,
    }
}

/// The ids of the fixtures at `indices`, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account holds no secondary interface: the report is empty, and describing is a
/// READ — two calls answer the same and the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_no_secondary_interface_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "secondaryInterfaceId").is_empty(),
        "fresh account: expected no secondary interface, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: nothing follows an empty report, so no token is due, got {body}"
    );

    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second,
        Vec::<String>::new(),
        "a read must not change what a later caller sees, got {second:?}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .secondary_interfaces
            .is_empty(),
        "describing secondary interfaces must not write to the store"
    );
}

/// Every secondary interface the store holds is reported, in store order, with the members it
/// was stored with — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_secondary_interface_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all secondary interfaces", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all secondary interfaces: expected every stored interface in store order, got \
         {reported:?}"
    );

    // One interface, member by member: everything the write set round-trips through the
    // describe, including all three addresses and the whole attachment.
    let zones = availability_zones_in_region(REGION);
    let (status, body) = describe(
        &router,
        &indexed("SecondaryInterfaceId", &[seeded.ids[5].as_str()]),
    )
    .await;
    assert_eq!(status, 200, "one interface: got {status} with body {body}");
    let expected_members = [
        ("secondaryInterfaceId", seeded.ids[5].clone()),
        (
            "secondaryInterfaceArn",
            engine::emulator_arn(
                "ec2",
                REGION,
                &format!("{SECONDARY_INTERFACE_RESOURCE}/{}", seeded.ids[5]),
            ),
        ),
        (
            "secondaryInterfaceType",
            SECONDARY_INTERFACE_TYPE_SECONDARY.to_string(),
        ),
        ("secondarySubnetId", seeded.subnet_ids[2].clone()),
        ("secondaryNetworkId", seeded.network_ids[2].clone()),
        (
            "secondaryNetworkType",
            SECONDARY_NETWORK_TYPE_RDMA.to_string(),
        ),
        ("ownerId", engine::emulator_account_id().to_string()),
        ("availabilityZone", zones[2].zone_name.clone()),
        ("availabilityZoneId", zones[2].zone_id.clone()),
        ("sourceDestCheck", "true".to_string()),
        (
            "attachmentId",
            seeded.attachment_ids[5]
                .clone()
                .expect("fixture 5 was stored with an attachment"),
        ),
        ("instanceId", INSTANCE_C.to_string()),
        ("instanceOwnerId", engine::emulator_account_id().to_string()),
        ("deviceIndex", "1".to_string()),
        ("networkCardIndex", "0".to_string()),
        ("deleteOnTermination", "true".to_string()),
    ];
    for (member, value) in expected_members {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "one interface: expected <{member}>{value}</{member}>, got {body}"
        );
    }
    assert_eq!(
        elements(&body, "privateIpAddress"),
        vec![
            "10.0.2.10".to_string(),
            "10.0.2.11".to_string(),
            "10.0.2.12".to_string()
        ],
        "one interface: expected every stored address in stored order, got {body}"
    );
    // The attachment's own status is the word it was stored with, beside the interface's
    // derived one.
    assert!(
        elements(&body, "status").contains(&ATTACHMENT_STATUS_ATTACHED.to_string()),
        "one interface: expected the attachment to report {ATTACHMENT_STATUS_ATTACHED:?}, got \
         {body}"
    );
    assert_eq!(
        reported_statuses("one interface", &body),
        vec![SECONDARY_INTERFACE_STATUS_IN_USE.to_string()],
        "one interface: expected the interface itself to report \
         {SECONDARY_INTERFACE_STATUS_IN_USE:?}, got {body}"
    );
    let attach_time = element(&body, "attachTime")
        .and_then(|value| as_timestamp(&value))
        .unwrap_or_else(|| panic!("one interface: expected an attachTime timestamp in {body}"));
    let expected_attach_time = attach_base()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        + 5.0;
    assert!(
        (attach_time - expected_attach_time).abs() < 0.001,
        "one interface: expected attachTime {expected_attach_time}, got {attach_time}"
    );

    // The MAC address is derived by the store, one per interface and never shared.
    let (status, all) = describe(&router, &[]).await;
    assert_eq!(status, 200, "every MAC: got {status} with body {all}");
    let macs = elements(&all, "macAddress");
    assert_eq!(
        macs.len(),
        seeded.ids.len(),
        "every MAC: each interface reports one MAC address, got {macs:?}"
    );
    for mac in &macs {
        assert!(
            mac.starts_with("02:") && mac.len() == 17,
            "every MAC: expected a locally administered MAC address, got {mac:?}"
        );
    }
    let mut unique = macs.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        macs.len(),
        "every MAC: no two interfaces may share one, got {macs:?}"
    );

    // An interface whose source/destination checking was turned off reports that.
    let (status, body) = describe(
        &router,
        &indexed("SecondaryInterfaceId", &[seeded.ids[3].as_str()]),
    )
    .await;
    assert_eq!(status, 200, "sourceDestCheck=false: got {status} {body}");
    assert_eq!(
        element(&body, "sourceDestCheck"),
        Some("false".to_string()),
        "sourceDestCheck=false: expected the stored value, got {body}"
    );

    // An interface stored after the first call is reported by the next one.
    let late_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard
            .insert_secondary_interface(SecondaryInterfaceSpec {
                secondary_subnet_id: seeded.subnet_ids[0].clone(),
                secondary_network_id: seeded.network_ids[0].clone(),
                secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
                availability_zone: zones[0].zone_name.clone(),
                availability_zone_id: zones[0].zone_id.clone(),
                region: REGION.to_string(),
                private_ipv4_addresses: vec!["10.0.0.99".to_string()],
                source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
                attachment: None,
                tags: vec![tag("Name", "late")],
            })
            .expect("the seeded store is far below its secondary interface ceiling")
    };

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

/// The words this crate stores for a secondary interface are members of the model's own
/// catalogs, so a filter value a caller reads out of the API documentation can match them.
#[test]
fn the_stored_catalog_words_are_members_of_the_modelled_enums() {
    let cases: Vec<(&str, &str, &[&str])> = vec![
        (
            "SecondaryInterfaceStatus",
            SECONDARY_INTERFACE_STATUS_AVAILABLE,
            SECONDARY_INTERFACE_STATUS_VALUES,
        ),
        (
            "SecondaryInterfaceStatus",
            SECONDARY_INTERFACE_STATUS_IN_USE,
            SECONDARY_INTERFACE_STATUS_VALUES,
        ),
        (
            "SecondaryInterfaceType",
            SECONDARY_INTERFACE_TYPE_SECONDARY,
            SECONDARY_INTERFACE_TYPE_VALUES,
        ),
        (
            "SecondaryNetworkType",
            SECONDARY_NETWORK_TYPE_RDMA,
            SECONDARY_NETWORK_TYPE_VALUES,
        ),
        (
            "AttachmentStatus",
            ATTACHMENT_STATUS_ATTACHED,
            ATTACHMENT_STATUS_VALUES,
        ),
        (
            "AttachmentStatus",
            ATTACHMENT_STATUS_DETACHED,
            ATTACHMENT_STATUS_VALUES,
        ),
    ];
    for (enum_name, stored, catalog) in cases {
        assert!(
            catalog.contains(&stored),
            "case {enum_name:?}: {stored:?} is not a member of {catalog:?}"
        );
    }
    // Every attachment status the fixtures above seed is a member of that same catalog, so
    // the tables below are written against the model rather than against free text.
    for fixture in seeded_fixtures() {
        if let Some(attachment) = fixture.attachment {
            assert!(
                ATTACHMENT_STATUS_VALUES.contains(&attachment.status),
                "case {:?}: a seeded attachment status must be a member of \
                 {ATTACHMENT_STATUS_VALUES:?}",
                attachment.status
            );
        }
    }
}

/// An interface's `Status` follows from its attachment: an instance holding it makes it
/// `in-use`, and an attachment that has reached `detached` — or no attachment at all —
/// leaves it `available`. The rows are the whole modelled `AttachmentStatus` catalog plus the
/// unattached case, not just the statuses the fixtures above happen to use.
#[tokio::test]
async fn the_status_follows_from_the_attachment_for_every_modelled_attachment_status() {
    let (router, engine) = super::ec2_query::emulator();

    // `(the case, the interface id, the status the report must carry)`.
    let mut expected: Vec<(String, String, &str)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let (network_id, subnet_id) = create_network_and_subnet(&mut guard, 0);
        let zones = availability_zones_in_region(REGION);
        let interface = |guard: &mut Ec2State,
                         attachment: Option<SecondaryInterfaceAttachmentSpec>|
         -> String {
            guard
                .insert_secondary_interface(SecondaryInterfaceSpec {
                    secondary_subnet_id: subnet_id.clone(),
                    secondary_network_id: network_id.clone(),
                    secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
                    availability_zone: zones[0].zone_name.clone(),
                    availability_zone_id: zones[0].zone_id.clone(),
                    region: REGION.to_string(),
                    private_ipv4_addresses: Vec::new(),
                    source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
                    attachment,
                    tags: Vec::new(),
                })
                .expect("the seeded store is far below its secondary interface ceiling")
        };

        for status in ATTACHMENT_STATUS_VALUES {
            let id = interface(
                &mut guard,
                Some(SecondaryInterfaceAttachmentSpec {
                    instance_id: INSTANCE_A.to_string(),
                    device_index: 1,
                    network_card_index: 0,
                    delete_on_termination: false,
                    status: (*status).to_string(),
                    attach_time: attach_base(),
                }),
            );
            let interface_status = if *status == ATTACHMENT_STATUS_DETACHED {
                SECONDARY_INTERFACE_STATUS_AVAILABLE
            } else {
                SECONDARY_INTERFACE_STATUS_IN_USE
            };
            expected.push((format!("attachment {status:?}"), id, interface_status));
        }
        let unattached = interface(&mut guard, None);
        expected.push((
            "no attachment".to_string(),
            unattached,
            SECONDARY_INTERFACE_STATUS_AVAILABLE,
        ));
    }

    for (case, id, interface_status) in &expected {
        let (status, body) = describe(&router, &indexed("SecondaryInterfaceId", &[id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            reported_statuses(case, &body),
            vec![(*interface_status).to_string()],
            "case {case:?}: expected the interface to report {interface_status:?}, got {body}"
        );
        // And the `status` filter selects it under that word.
        let selected =
            described_ids(&router, case, &filters(&[("status", &[*interface_status])])).await;
        assert!(
            selected.contains(id),
            "case {case:?}: the status filter {interface_status:?} must select {id:?}, got \
             {selected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects the secondary interfaces whose own attribute matches a
/// value; values within one filter are OR'd and the comparison is case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_interfaces_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.ids.len()).collect();
    let attached = vec![0, 1, 3, 4, 5, 7];

    // One row per filter name `DescribeSecondaryInterfacesRequest$Filters` documents, with
    // extra rows for value counts, wildcards, case sensitivity and the empty value list.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "attachment.attachment-id",
            vec![
                seeded.attachment_ids[0]
                    .clone()
                    .expect("fixture 0 has an attachment"),
            ],
            vec![0],
        ),
        (
            "attachment.attachment-id",
            vec![
                seeded.attachment_ids[7]
                    .clone()
                    .expect("fixture 7 has an attachment"),
            ],
            vec![7],
        ),
        // An interface no instance holds carries no attachment id, so no value of this
        // filter can select it.
        (
            "attachment.attachment-id",
            vec!["seni-attach-00000000000000000".into()],
            vec![],
        ),
        ("attachment.instance-id", vec![INSTANCE_A.into()], vec![0, 1]),
        ("attachment.instance-id", vec![INSTANCE_B.into()], vec![3, 4]),
        (
            "attachment.instance-id",
            vec![INSTANCE_A.into(), INSTANCE_C.into()],
            vec![0, 1, 5, 7],
        ),
        (
            "attachment.instance-owner-id",
            vec![account.clone()],
            attached.clone(),
        ),
        (
            "attachment.instance-owner-id",
            vec!["210987654321".into()],
            vec![],
        ),
        ("attachment.status", vec!["attached".into()], vec![0, 5, 7]),
        ("attachment.status", vec!["attaching".into()], vec![1]),
        ("attachment.status", vec!["detaching".into()], vec![3]),
        ("attachment.status", vec!["detached".into()], vec![4]),
        (
            "attachment.status",
            vec!["attaching".into(), "detaching".into()],
            vec![1, 3],
        ),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("attachment.status", vec!["Attached".into()], vec![]),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["210987654321".into()], vec![]),
        (
            "private-ipv4-addresses.private-ip-address",
            vec!["10.0.0.10".into()],
            vec![0],
        ),
        (
            "private-ipv4-addresses.private-ip-address",
            // The second address of the interface that holds two.
            vec!["10.0.0.12".into()],
            vec![1],
        ),
        (
            "private-ipv4-addresses.private-ip-address",
            vec!["10.0.1.10".into(), "10.0.1.11".into()],
            vec![3, 4],
        ),
        // EC2's own `*` wildcard — and the one interface stored with no address at all
        // matches none of these.
        (
            "private-ipv4-addresses.private-ip-address",
            vec!["10.0.2.*".into()],
            vec![5, 6, 7],
        ),
        (
            "private-ipv4-addresses.private-ip-address",
            vec!["10.0.*".into()],
            vec![0, 1, 3, 4, 5, 6, 7],
        ),
        (
            "secondary-interface-arn",
            vec![engine::emulator_arn(
                "ec2",
                REGION,
                &format!("{SECONDARY_INTERFACE_RESOURCE}/{}", seeded.ids[6]),
            )],
            vec![6],
        ),
        // Every ARN of this account's secondary interfaces, by wildcard.
        (
            "secondary-interface-arn",
            vec![format!(
                "arn:aws:ec2:{REGION}:{}:{SECONDARY_INTERFACE_RESOURCE}/*",
                engine::emulator_account_id()
            )],
            all.clone(),
        ),
        ("secondary-interface-id", vec![seeded.ids[2].clone()], vec![2]),
        (
            "secondary-interface-id",
            vec![seeded.ids[0].clone(), seeded.ids[7].clone()],
            vec![0, 7],
        ),
        ("secondary-interface-type", vec!["secondary".into()], all.clone()),
        ("secondary-interface-type", vec!["trunk".into()], vec![]),
        (
            "secondary-network-id",
            vec![seeded.network_ids[0].clone()],
            vec![0, 1],
        ),
        (
            "secondary-network-id",
            vec![seeded.network_ids[1].clone()],
            vec![2, 3, 4],
        ),
        (
            "secondary-network-id",
            vec![
                seeded.network_ids[0].clone(),
                seeded.network_ids[2].clone(),
            ],
            vec![0, 1, 5, 6, 7],
        ),
        ("secondary-network-type", vec!["rdma".into()], all.clone()),
        ("secondary-network-type", vec!["efa".into()], vec![]),
        (
            "secondary-subnet-id",
            vec![seeded.subnet_ids[1].clone()],
            vec![2, 3, 4],
        ),
        (
            "secondary-subnet-id",
            vec![
                seeded.subnet_ids[0].clone(),
                seeded.subnet_ids[1].clone(),
            ],
            vec![0, 1, 2, 3, 4],
        ),
        ("status", vec!["in-use".into()], vec![0, 1, 3, 5, 7]),
        ("status", vec!["available".into()], vec![2, 4, 6]),
        (
            "status",
            vec!["available".into(), "in-use".into()],
            all.clone(),
        ),
        ("tag:Name", vec!["web".into()], vec![0, 4]),
        ("tag:Name", vec!["batch".into()], vec![1]),
        ("tag:Owner", vec!["TeamA".into(), "TeamB".into()], vec![0, 5]),
        ("tag:Owner", vec!["TeamC".into()], vec![]),
        ("tag-key", vec!["Name".into()], vec![0, 1, 4]),
        ("tag-key", vec!["Owner".into()], vec![0, 5]),
        (
            "tag-key",
            vec!["Name".into(), "Owner".into()],
            vec![0, 1, 4, 5],
        ),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("status", vec![], vec![]),
        ("secondary-interface-id", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the interfaces that
/// must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the interfaces matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let network_two = seeded.network_ids[2].clone();
    let network_two_values = [network_two.as_str()];
    let subnet_one = seeded.subnet_ids[1].clone();
    let subnet_one_values = [subnet_one.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("secondary-network-id", &network_two_values[..]),
                ("attachment.status", &["attached"][..]),
            ],
            vec![5, 7],
        ),
        (
            vec![
                ("secondary-subnet-id", &subnet_one_values[..]),
                ("status", &["available"][..]),
            ],
            vec![2, 4],
        ),
        (
            vec![
                ("secondary-network-type", &["rdma"][..]),
                ("tag-key", &["Name"][..]),
                ("status", &["in-use"][..]),
            ],
            vec![0, 1],
        ),
        // Each filter matches something, but no interface matches both.
        (
            vec![
                ("attachment.status", &["attached"][..]),
                ("status", &["available"][..]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: near-misses of two documented names, the underscore spelling, a documented
    // name with the wrong case, filters of other describes, the tag filter without its key
    // separator, the tag filter with a separator but no key, and an empty name.
    let cases = [
        "secondary-interface-ids",
        "attachment.instance-ids",
        "secondary_interface_id",
        "Secondary-Interface-Id",
        "attachment-status",
        "mac-address",
        "source-dest-check",
        "availability-zone",
        "subnet-id",
        "tag",
        "tag:",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
        assert!(
            elements(&body, "secondaryInterfaceId").is_empty(),
            "case {name:?}: a refused request reports no interface, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// SecondaryInterfaceIds
// ---------------------------------------------------------------------------

/// `SecondaryInterfaceIds` narrows the report to the ids named, in store order whatever order
/// they were asked for, and composes with a filter.
#[tokio::test]
async fn secondary_interface_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: one id, several in store order, the same several reversed, every id, and a
    // repeated id.
    let cases: Vec<(Vec<usize>, Vec<usize>)> = vec![
        (vec![3], vec![3]),
        (vec![1, 5, 6], vec![1, 5, 6]),
        (vec![6, 5, 1], vec![1, 5, 6]),
        (
            (0..seeded.ids.len()).collect(),
            (0..seeded.ids.len()).collect(),
        ),
        (vec![2, 2], vec![2]),
    ];
    for (asked, expected_indices) in cases {
        let asked_ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = asked_ids.iter().map(String::as_str).collect();
        let case = format!("SecondaryInterfaceIds={borrowed:?}");
        let reported =
            described_ids(&router, &case, &indexed("SecondaryInterfaceId", &borrowed)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // The ids and the filters are joined: only the named interfaces that also match.
    let asked = ids_at(&seeded, &[0, 1, 2, 3]);
    let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
    let mut params = indexed("SecondaryInterfaceId", &borrowed);
    params.extend(filters(&[("status", &["in-use"])]));
    let reported = described_ids(&router, "ids and a filter", &params).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[0, 1, 3]),
        "ids and a filter: expected the named interfaces that match, got {reported:?}"
    );
}

/// An id that names no secondary interface of the account is refused with
/// `InvalidSecondaryInterfaceID.NotFound`, rather than reported as an absent row.
#[tokio::test]
async fn an_unknown_secondary_interface_id_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // Rows: a well-formed id nothing has, a shorter id form, something that is not an id
    // at all, an elastic network interface id, and an unknown id carried beside a known one
    // in each order.
    let cases: Vec<Vec<String>> = vec![
        vec!["seni-0ffffffffffffffff".to_string()],
        vec!["seni-0abc1234".to_string()],
        vec!["not-a-secondary-interface-id".to_string()],
        vec!["eni-00000000000000001".to_string()],
        vec![known.clone(), "seni-0ffffffffffffffff".to_string()],
        vec!["seni-0ffffffffffffffff".to_string(), known],
    ];
    for ids in cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("SecondaryInterfaceIds={borrowed:?}");
        let (status, body) = describe(&router, &indexed("SecondaryInterfaceId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSecondaryInterfaceID.NotFound",
            "case {case:?}: expected InvalidSecondaryInterfaceID.NotFound, got {body}"
        );
        assert!(
            elements(&body, "secondaryInterfaceId").is_empty(),
            "case {case:?}: a refused request reports no interface, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The caps on what one request may carry
// ---------------------------------------------------------------------------

/// One row of the request-size cap table: the case name, the form fields to send, and the
/// HTTP status they must answer with.
type CapCase = (String, Vec<(String, String)>, u16);

/// Each cap this operation puts on a request-controlled factor — how many filters, how many
/// values in one filter, how many bytes in one value, how many ids — is accepted at its
/// bound and refused one past it with `InvalidParameterValue`. The model bounds none of the
/// four, so the handler is the only place they exist.
#[tokio::test]
async fn the_request_size_caps_are_accepted_at_their_bound_and_refused_past_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // `count` copies of one documented filter.
    let filter_count = |count: usize| -> Vec<(String, String)> {
        (0..count)
            .flat_map(|index| {
                vec![
                    (format!("Filter.{}.Name", index + 1), "status".to_string()),
                    (format!("Filter.{}.Value.1", index + 1), "in-use".to_string()),
                ]
            })
            .collect()
    };
    // One documented filter carrying `count` values.
    let value_count = |count: usize| -> Vec<(String, String)> {
        let mut params = vec![("Filter.1.Name".to_string(), "status".to_string())];
        params.extend((0..count).map(|index| {
            (
                format!("Filter.1.Value.{}", index + 1),
                format!("status-{index}"),
            )
        }));
        params
    };
    // One filter value of `bytes` bytes.
    let value_bytes = |bytes: usize| -> Vec<(String, String)> {
        vec![
            (
                "Filter.1.Name".to_string(),
                "secondary-interface-id".to_string(),
            ),
            ("Filter.1.Value.1".to_string(), "a".repeat(bytes)),
        ]
    };
    // `count` ids, each naming an interface the store really holds, so the only thing that
    // can refuse the request is the cap itself.
    let id_count = |count: usize| -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("SecondaryInterfaceId.{}", index + 1), known.clone()))
            .collect()
    };

    let cases: Vec<CapCase> = vec![
        (
            format!("{MAX_FILTERS} filters"),
            filter_count(MAX_FILTERS),
            200,
        ),
        (
            format!("{} filters", MAX_FILTERS + 1),
            filter_count(MAX_FILTERS + 1),
            400,
        ),
        (
            format!("{MAX_FILTER_VALUES} values"),
            value_count(MAX_FILTER_VALUES),
            200,
        ),
        (
            format!("{} values", MAX_FILTER_VALUES + 1),
            value_count(MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            format!("a {MAX_FILTER_VALUE_BYTES}-byte value"),
            value_bytes(MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            format!("a {}-byte value", MAX_FILTER_VALUE_BYTES + 1),
            value_bytes(MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
        (
            format!("{MAX_RESOURCE_IDS} ids"),
            id_count(MAX_RESOURCE_IDS),
            200,
        ),
        (
            format!("{} ids", MAX_RESOURCE_IDS + 1),
            id_count(MAX_RESOURCE_IDS + 1),
            400,
        ),
    ];

    for (case, params, expected_status) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                elements(&body, "secondaryInterfaceId").is_empty(),
                "case {case:?}: a refused request reports no interface, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every secondary interface exactly once,
/// in store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_interface_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeSecondaryInterfacesMaxResults` is modelled `range 5..1000`, so the rows are
    // its smallest published size, one that does not divide the store, one that holds it
    // exactly, and the largest published size.
    for max_results in [5, 6, 8, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "secondaryInterfaceId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every interface exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is cut from the FILTERED set, not from the whole store: `MaxResults` and a filter
/// compose, and the token resumes within the filtered set.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[
        ("secondary-network-type", &["rdma"]),
        ("private-ipv4-addresses.private-ip-address", &["10.0.*"]),
    ]);
    params.extend(max_results_param(5));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    assert_eq!(
        elements(&body, "secondaryInterfaceId"),
        ids_at(&seeded, &[0, 1, 3, 4, 5]),
        "filtered first page: expected the first five interfaces holding an address, got {body}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut resumed = filters(&[(
        "private-ipv4-addresses.private-ip-address",
        &["10.0.*"],
    )]);
    resumed.extend(max_results_param(5));
    resumed.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &resumed).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[6, 7]),
        "filtered second page: expected the interfaces left after the first page, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 5..1000`) and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded (the shape the
    // token takes, without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
        ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0])),
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` on a request that would have succeeded answers `DryRunOperation` and reports
/// nothing; a request that would have been REFUSED is refused with its own reason instead,
/// and `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_whether_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let dry_run = vec![("DryRun".to_string(), "true".to_string())];
    let (status, body) = describe(&router, &dry_run).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "secondaryInterfaceId").is_empty(),
        "a dry run reports no interface, got {body}"
    );

    // A dry run whose filter name is undefined is refused for that reason instead.
    let mut undefined_filter = filters(&[("mac-address", &["02:00:00:00:00:01"])]);
    undefined_filter.extend(dry_run);
    let (status, body) = describe(&router, &undefined_filter).await;
    assert_eq!(
        status, 400,
        "a dry run with an undefined filter: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a dry run with an undefined filter: expected InvalidParameterValue, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded.ids,
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}
