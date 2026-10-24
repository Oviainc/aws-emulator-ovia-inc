// SPDX-License-Identifier: CC0-1.0

//! Cover for the bounds every `insert_*` writer of [`ec2::state`] is held to, rather than
//! for one operation's wire behaviour.
//!
//! The rule under test is that NOTHING in this emulator reclaims a stored record, so a
//! store that mints a new id on every call is bounded two ways and both are enforced by the
//! WRITER rather than by a caller remembering to ask: a write past the store's count
//! ceiling ([`MAX_STORED_RECORDS_PER_STORE`]) stores nothing and answers no id, and every
//! record that is stored costs at least [`STORED_RECORD_OVERHEAD_BYTES`] against the byte
//! budget, so repeated
//! writes advance the budget whatever they carry. The rows are the stores whose writers
//! carry no published quota of their own — a VPC endpoint, an endpoint service, an IPAM pool
//! and a secondary subnet — plus the Dedicated Host store, whose own ceiling the writer
//! enforces beside the batch gate `AllocateHosts` asks first.
//!
//! The last suite pins the other half of the budget: a record that MOVES between two stores
//! gives back exactly what its create charged and is charged for its new home, so a
//! create/bin loop cannot walk the budget away in either direction. The BYOIP suite states
//! the same bounds for a store keyed by a CALLER-SUPPLIED range rather than by a minted id.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{Host, IpamPool, ServiceConfiguration, VpcEndpoint};
use ec2::state::{
    BYOIP_CIDR_STATE_PROVISIONED, ByoipCidrRecord, ComAmazonawsEc2Amazonec2State as Ec2State,
    MAX_STORED_DEDICATED_HOSTS, MAX_STORED_RECORDS_PER_STORE, SECONDARY_NETWORK_TYPE_RDMA,
    SECONDARY_SUBNET_STATE_CREATE_COMPLETE, STORED_RECORD_OVERHEAD_BYTES, SecondarySubnetSpec,
    volume_recycle_bin_entry_payload_bytes,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::element;
use super::volume_lifecycle::{REGION, p, zone_name};

/// One store whose writer mints a new id on every call: how to write a record carrying no
/// caller-chosen bytes, and how many records the store holds.
struct Store {
    /// The resource the row is about, named in every assertion.
    case: &'static str,
    /// Write one record, answering the id it was given or `None` when the store refused it.
    insert: fn(&mut Ec2State) -> Option<String>,
    /// How many records the store holds right now.
    len: fn(&Ec2State) -> usize,
}

/// The stores this suite holds to [`MAX_STORED_RECORDS_PER_STORE`]. Each row's record
/// carries no tags, no description and no associations, so what it costs against the byte
/// budget is the writer's fixed overhead alone.
fn stores() -> Vec<Store> {
    vec![
        Store {
            case: "a VPC endpoint",
            insert: |store| store.insert_vpc_endpoint(VpcEndpoint::default()),
            len: |store| store.vpc_endpoints.len(),
        },
        Store {
            case: "a VPC endpoint service",
            insert: |store| store.insert_vpc_endpoint_service(ServiceConfiguration::default()),
            len: |store| store.vpc_endpoint_services.len(),
        },
        Store {
            case: "an IPAM pool",
            insert: |store| store.insert_ipam_pool(IpamPool::default()),
            len: |store| store.ipam_pools.len(),
        },
        Store {
            case: "a secondary subnet",
            insert: |store| {
                store.insert_secondary_subnet(SecondarySubnetSpec {
                    secondary_network_id: String::new(),
                    secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
                    availability_zone: String::new(),
                    availability_zone_id: String::new(),
                    region: REGION.to_string(),
                    ipv4_cidr_blocks: Vec::new(),
                    state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
                    state_reason: None,
                    tags: Vec::new(),
                })
            },
            len: |store| store.secondary_subnets.len(),
        },
        Store {
            // The Dedicated Host store keeps its own published-style ceiling, which the
            // writer enforces for itself: a caller that forgets `AllocateHosts`'s batch gate
            // cannot get past it either.
            case: "a Dedicated Host",
            insert: |store| {
                let host_id = store.next_id("h");
                store.insert_dedicated_host(Host {
                    host_id: Some(host_id),
                    ..Host::default()
                })
            },
            len: |store| store.dedicated_hosts.len(),
        },
    ]
}

/// The ceiling each row's store is held to: its own where it publishes one, the shared
/// [`MAX_STORED_RECORDS_PER_STORE`] otherwise.
fn ceiling(case: &str) -> usize {
    if case == "a Dedicated Host" {
        MAX_STORED_DEDICATED_HOSTS
    } else {
        MAX_STORED_RECORDS_PER_STORE
    }
}

/// A record stored with no caller-chosen bytes is NOT free against the byte budget: every
/// write advances it by at least the writer's fixed overhead, so repeated writes cannot grow
/// a store for nothing.
#[test]
fn a_record_carrying_nothing_the_caller_chose_still_costs_the_budget() {
    for store in stores() {
        let case = store.case;
        let (_router, engine) = super::ec2_query::emulator();
        let state = engine.state::<Ec2State>();
        let mut guard = state.lock();

        let before = guard.stored_bytes();
        let id = (store.insert)(&mut guard)
            .unwrap_or_else(|| panic!("case {case:?}: a fresh store has room for one record"));
        assert!(
            !id.is_empty(),
            "case {case:?}: the writer answers the id it minted, got {id:?}"
        );
        let charged = guard.stored_bytes().saturating_sub(before);
        assert!(
            charged >= STORED_RECORD_OVERHEAD_BYTES,
            "case {case:?}: expected at least {STORED_RECORD_OVERHEAD_BYTES} bytes charged \
             for a record carrying nothing the caller chose, got {charged}"
        );
    }
}

/// A write past the store's count ceiling stores NOTHING: no id is answered, the store holds
/// what it held, and the byte budget is where it was.
#[test]
fn a_write_past_the_ceiling_stores_nothing() {
    for store in stores() {
        let case = store.case;
        let ceiling = ceiling(case);
        let (_router, engine) = super::ec2_query::emulator();
        let state = engine.state::<Ec2State>();
        let mut guard = state.lock();

        while (store.len)(&guard) < ceiling {
            let held = (store.len)(&guard);
            assert!(
                (store.insert)(&mut guard).is_some(),
                "case {case:?}: the store held {held} of {ceiling}, so the write must be \
                 admitted"
            );
        }
        let held = (store.len)(&guard);
        let budget = guard.stored_bytes();

        let refused = (store.insert)(&mut guard);
        assert!(
            refused.is_none(),
            "case {case:?}: a write past {ceiling} records must answer no id, got {refused:?}"
        );
        assert_eq!(
            (store.len)(&guard),
            held,
            "case {case:?}: a refused write stores nothing, so the store must still hold \
             {held} records"
        );
        assert_eq!(
            guard.stored_bytes(),
            budget,
            "case {case:?}: a refused write charges nothing, so the budget must still read \
             {budget}"
        );
    }
}

/// The BYOIP store is keyed by a CALLER-SUPPLIED range rather than by a minted id, so its
/// bounds are stated over that key: a range that REPLACES the one already held under its key
/// adds no record and is charged once, while a fresh range past the ceiling is refused and
/// stores nothing.
#[test]
fn the_byoip_store_is_bounded_and_a_replaced_range_is_charged_once() {
    let case = "a BYOIP range";
    let (_router, engine) = super::ec2_query::emulator();
    let state = engine.state::<Ec2State>();
    let mut guard = state.lock();

    let before = guard.stored_bytes();
    let record = ByoipCidrRecord::in_state("192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED);
    assert!(
        guard.insert_byoip_cidr(record.clone()),
        "case {case:?}: a fresh store has room for one range"
    );
    let charged = guard.stored_bytes().saturating_sub(before);
    assert!(
        charged >= STORED_RECORD_OVERHEAD_BYTES,
        "case {case:?}: expected at least {STORED_RECORD_OVERHEAD_BYTES} bytes charged for a \
         stored range, got {charged}"
    );

    assert!(
        guard.insert_byoip_cidr(record.clone()),
        "case {case:?}: a range replacing the one held under its key is stored"
    );
    assert_eq!(
        guard.byoip_cidrs.len(),
        1,
        "case {case:?}: a replacing write adds no record"
    );
    assert_eq!(
        guard.stored_bytes(),
        before + charged,
        "case {case:?}: a replacing write gives the replaced record's charge back, so the \
         budget must still read {}",
        before + charged
    );

    // Distinct ranges up to the ceiling, then one past it.
    for index in 1..MAX_STORED_RECORDS_PER_STORE {
        let cidr = format!("10.{}.{}.0/24", index / 256, index % 256);
        assert!(
            guard.insert_byoip_cidr(ByoipCidrRecord::in_state(
                &cidr,
                BYOIP_CIDR_STATE_PROVISIONED
            )),
            "case {case:?}: the store held {index} of {MAX_STORED_RECORDS_PER_STORE}, so \
             {cidr} must be admitted"
        );
    }
    let held = guard.byoip_cidrs.len();
    let budget = guard.stored_bytes();
    assert!(
        !guard.insert_byoip_cidr(ByoipCidrRecord::in_state(
            "198.51.100.0/24",
            BYOIP_CIDR_STATE_PROVISIONED
        )),
        "case {case:?}: a fresh range past {MAX_STORED_RECORDS_PER_STORE} must be refused"
    );
    assert_eq!(
        guard.byoip_cidrs.len(),
        held,
        "case {case:?}: a refused write stores nothing, so the store must still hold {held} \
         ranges"
    );
    assert_eq!(
        guard.stored_bytes(),
        budget,
        "case {case:?}: a refused write charges nothing, so the budget must still read \
         {budget}"
    );
}

/// Moving a volume into the Recycle Bin gives the byte budget back exactly what `CreateVolume`
/// charged for it and charges the bin's own entry, so a create/bin loop walks the budget
/// neither up nor down by the volume's caller-chosen bytes.
///
/// The rows differ in how many caller-chosen bytes the volume carries — none, a tag, and a
/// tag plus a client token — so the released charge is read off the volume rather than
/// assumed.
#[tokio::test]
async fn binning_a_volume_gives_back_its_charge_and_charges_the_bin_entry() {
    // `(case, the CreateVolume parameters beyond the zone and the size)`.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        ("a volume carrying nothing the caller chose", Vec::new()),
        (
            "a volume carrying a tag",
            vec![
                p("TagSpecification.1.ResourceType", "volume"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "binned"),
            ],
        ),
        (
            "a volume carrying a tag and a client token",
            vec![
                p("TagSpecification.1.ResourceType", "volume"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "binned-with-a-token"),
                p("ClientToken", "a-client-token-of-some-length"),
            ],
        ),
    ];

    for (case, extra) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let budget = || engine.state::<Ec2State>().lock().stored_bytes();

        let before = budget();
        let mut params = vec![p("AvailabilityZone", zone_name(0)), p("Size", "8")];
        params.extend(extra);
        let (status, body) = super::ec2_query::call(&router, REGION, "CreateVolume", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}"
        );
        let volume_id =
            element(&body, "volumeId").expect("CreateVolume reports the volume it minted");
        let charged = budget().saturating_sub(before);

        // A retention period of the Recycle Bin rule that covered the volume — a fact of
        // that rule, so the caller gives it.
        let binned = engine
            .state::<Ec2State>()
            .lock()
            .move_volume_to_recycle_bin(
                &volume_id,
                SystemTime::now(),
                Duration::from_secs(60 * 60 * 24),
            )
            .unwrap_or_else(|| {
                panic!("case {case:?}: the volume {volume_id:?} was just created, so it can be binned")
            });

        // What the bin's entry costs is the ONE definition of that charge, so the row reads
        // it off the entry the move answered rather than restating it.
        let expected = before + volume_recycle_bin_entry_payload_bytes(&binned);
        let after = budget();
        assert_eq!(
            after, expected,
            "case {case:?}: the move must give back the {charged} bytes the create charged \
             and charge the bin's entry alone, expected {expected}, got {after}"
        );
    }
}
