// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeCapacityBlocks`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeCapacityBlocks`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! Two rules are under test:
//!
//! 1. The operation REPORTS THE STORE: the set it answers with is the account's Capacity
//!    Blocks narrowed by the `CapacityBlockIds` and `Filters` the request names and cut
//!    into `MaxResults`-sized pages.
//! 2. A block's `State` is READ OFF ITS RESERVED WINDOW: `scheduled` before its
//!    `StartDate`, `active` from it, `expired` from its `EndDate` — unless a state no clock
//!    drives is stored on it, which wins. A read never changes it.
//!
//! Every table row is a distinct input along what those rules generalise over — where the
//! clock stands in the window, which filter, how many values, how many filters, which ids,
//! which page size — so no single recorded block is the specification.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::Tag;
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{iso8601_utc, CapacityBlockSpec, CAPACITY_BLOCK_PINNED_STATES};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The store holds Capacity Blocks per account
/// rather than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One `DescribeCapacityBlocks` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeCapacityBlocks", params).await
}

/// The Capacity Block ids a successful response reports, in the order it reported them.
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
    elements(&body, "capacityBlockId")
}

/// A tag, as the seeded blocks carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// `now` with the sub-second part dropped, so the instants the rows below are written
/// against render identically wherever they are compared: in the response body, in a filter
/// value, and in this suite's expectations.
fn whole_second(at: SystemTime) -> SystemTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    UNIX_EPOCH + Duration::from_secs(since_epoch.as_secs())
}

/// `base` shifted by `hours`, which may be negative.
fn hours_from(base: SystemTime, hours: i64) -> SystemTime {
    let span = Duration::from_secs(hours.unsigned_abs() * 3_600);
    if hours < 0 {
        base - span
    } else {
        base + span
    }
}

/// What one seeded Capacity Block is created with, plus the state the rules above say it
/// must be reported in.
struct BlockSpec {
    /// "The EC2 UltraServer type of the Capacity Block" (`CapacityBlock$UltraserverType`),
    /// or `None` for a block of plain instances, which carries no UltraServer type.
    ultraserver_type: Option<&'static str>,
    availability_zone: &'static str,
    availability_zone_id: &'static str,
    capacity_reservation_ids: Vec<&'static str>,
    /// When the reserved window opens, closes and when the block was created, in hours from
    /// the moment the suite seeded the store.
    start_hours: i64,
    end_hours: i64,
    create_hours: i64,
    /// A state no clock drives, from `CapacityBlockResourceState`.
    pinned_state: Option<&'static str>,
    tags: Vec<Tag>,
    /// The `CapacityBlockResourceState` the block must be reported in.
    expected_state: &'static str,
}

/// The blocks every table below is written against.
///
/// The rows differ along everything the two rules generalise over: where the clock stands
/// in the window (before it, inside it, after it), whether a state is pinned and which one,
/// the zone, the UltraServer type (including a block that has none), the number of Capacity
/// Reservations, and the tags. Between them the rows cover every member of
/// `CapacityBlockResourceState`
/// (`smithy_query(query="shape",shape="CapacityBlockResourceState")`): the three the window
/// drives, and the five [`CAPACITY_BLOCK_PINNED_STATES`] holds.
fn seeded_specs() -> Vec<BlockSpec> {
    vec![
        // 0: window entirely ahead of now.
        BlockSpec {
            ultraserver_type: Some("u-p6e-gb200x72"),
            availability_zone: "us-east-1a",
            availability_zone_id: "use1-az1",
            capacity_reservation_ids: vec!["cr-0000000000000000a"],
            start_hours: 24,
            end_hours: 48,
            create_hours: -72,
            pinned_state: None,
            tags: vec![tag("Name", "train"), tag("Owner", "TeamA")],
            expected_state: "scheduled",
        },
        // 1: window open around now, and a block of plain instances (no UltraServer type).
        BlockSpec {
            ultraserver_type: None,
            availability_zone: "us-east-1b",
            availability_zone_id: "use1-az2",
            capacity_reservation_ids: vec!["cr-0000000000000000b", "cr-0000000000000000c"],
            start_hours: -1,
            end_hours: 1,
            create_hours: -48,
            pinned_state: None,
            tags: vec![tag("Name", "infer")],
            expected_state: "active",
        },
        // 2: window entirely behind now.
        BlockSpec {
            ultraserver_type: Some("u-p6e-gb200x72"),
            availability_zone: "us-east-1a",
            availability_zone_id: "use1-az1",
            capacity_reservation_ids: vec!["cr-0000000000000000d"],
            start_hours: -48,
            end_hours: -24,
            create_hours: -96,
            pinned_state: None,
            tags: vec![tag("Owner", "TeamB")],
            expected_state: "expired",
        },
        // 3: an open window, but the block was cancelled — the pinned state wins.
        BlockSpec {
            ultraserver_type: Some("u-p5en-gb200x36"),
            availability_zone: "us-east-1c",
            availability_zone_id: "use1-az3",
            capacity_reservation_ids: Vec::new(),
            start_hours: -2,
            end_hours: 2,
            create_hours: -24,
            pinned_state: Some("cancelled"),
            tags: vec![tag("Name", "train")],
            expected_state: "cancelled",
        },
        // 4: purchased but not paid for yet.
        BlockSpec {
            ultraserver_type: None,
            availability_zone: "us-east-1b",
            availability_zone_id: "use1-az2",
            capacity_reservation_ids: vec!["cr-0000000000000000e"],
            start_hours: 25,
            end_hours: 49,
            create_hours: -2,
            pinned_state: Some("payment-pending"),
            tags: Vec::new(),
            expected_state: "payment-pending",
        },
        // 5: the payment failed.
        BlockSpec {
            ultraserver_type: Some("u-p5en-gb200x36"),
            availability_zone: "us-east-1c",
            availability_zone_id: "use1-az3",
            capacity_reservation_ids: vec!["cr-0000000000000000f"],
            start_hours: 48,
            end_hours: 72,
            create_hours: -3,
            pinned_state: Some("payment-failed"),
            tags: vec![tag("Owner", "TeamA")],
            expected_state: "payment-failed",
        },
        // 6: an open window AWS could not deliver the capacity for.
        BlockSpec {
            ultraserver_type: None,
            availability_zone: "us-east-1a",
            availability_zone_id: "use1-az1",
            capacity_reservation_ids: Vec::new(),
            start_hours: -3,
            end_hours: 3,
            create_hours: -5,
            pinned_state: Some("unavailable"),
            tags: vec![tag("Name", "infer"), tag("Owner", "TeamB")],
            expected_state: "unavailable",
        },
        // 7: a closed window whose block failed rather than expiring.
        BlockSpec {
            ultraserver_type: Some("u-p6e-gb200x72"),
            availability_zone: "us-east-1b",
            availability_zone_id: "use1-az2",
            capacity_reservation_ids: vec!["cr-00000000000000010"],
            start_hours: -47,
            end_hours: -23,
            create_hours: -120,
            pinned_state: Some("failed"),
            tags: Vec::new(),
            expected_state: "failed",
        },
    ]
}

/// What a seeded store holds, for the tables to name rows by.
struct Seeded {
    /// Every Capacity Block id, in the order `DescribeCapacityBlocks` reports them (the
    /// store's key order).
    ids: Vec<String>,
    /// The instant the store was seeded, which every row's window is relative to.
    seeded_at: SystemTime,
    /// The rows, in the same order as `ids`.
    specs: Vec<BlockSpec>,
}

impl Seeded {
    /// The `StartDate` of row `index`, as the wire renders it.
    fn start_date(&self, index: usize) -> String {
        iso8601_utc(hours_from(self.seeded_at, self.specs[index].start_hours))
    }

    /// The `EndDate` of row `index`, as the wire renders it.
    fn end_date(&self, index: usize) -> String {
        iso8601_utc(hours_from(self.seeded_at, self.specs[index].end_hours))
    }

    /// The `CreateDate` of row `index`, as the wire renders it.
    fn create_date(&self, index: usize) -> String {
        iso8601_utc(hours_from(self.seeded_at, self.specs[index].create_hours))
    }
}

/// Write the seeded blocks into the engine's store, the way a purchasing operation would,
/// and report the ids they were given.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let seeded_at = whole_second(SystemTime::now());
    let specs = seeded_specs();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for spec in &specs {
        ids.push(guard.insert_capacity_block(CapacityBlockSpec {
            ultraserver_type: spec.ultraserver_type.map(str::to_string),
            availability_zone: spec.availability_zone.to_string(),
            availability_zone_id: spec.availability_zone_id.to_string(),
            capacity_reservation_ids: spec
                .capacity_reservation_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect(),
            start_date: hours_from(seeded_at, spec.start_hours),
            end_date: hours_from(seeded_at, spec.end_hours),
            create_date: hours_from(seeded_at, spec.create_hours),
            pinned_state: spec.pinned_state,
            tags: spec.tags.clone(),
        }));
    }
    drop(guard);
    Seeded {
        ids,
        seeded_at,
        specs,
    }
}

/// The ids at `indices` of a seeded store, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.ids[*index].clone())
        .collect()
}

/// One `CapacityBlockId.N` list.
fn id_params(ids: &[String]) -> Vec<(String, String)> {
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    indexed("CapacityBlockId", &borrowed)
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Nothing in the EC2 API gives a fresh account a Capacity Block, so a fresh account has
/// none — and asking is a READ: two calls answer the same and the store is untouched.
#[tokio::test]
async fn a_fresh_account_reports_no_capacity_blocks_and_describing_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let first = described_ids(&router, "fresh account", &[]).await;
    assert!(
        first.is_empty(),
        "fresh account: expected no Capacity Blocks, got {first:?}"
    );
    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert!(
        engine.state::<Ec2State>().lock().capacity_blocks.is_empty(),
        "describing Capacity Blocks must not write to the store"
    );
}

/// Every block the store holds is reported, in store order, with every member it carries —
/// and a block written after a call shows up in the next one, so the answer is computed from
/// the store rather than fixed.
#[tokio::test]
async fn every_stored_block_is_reported_with_its_members_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all blocks", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all blocks: expected every stored block in store order, got {reported:?}"
    );

    // One row read back member by member: every member the store holds round-trips.
    for index in 0..seeded.ids.len() {
        let case = format!("row {index}");
        let params = id_params(&ids_at(&seeded, &[index]));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let spec = &seeded.specs[index];
        let expected_members = [
            ("capacityBlockId", seeded.ids[index].clone()),
            ("availabilityZone", spec.availability_zone.to_string()),
            ("availabilityZoneId", spec.availability_zone_id.to_string()),
            ("startDate", seeded.start_date(index)),
            ("endDate", seeded.end_date(index)),
            ("createDate", seeded.create_date(index)),
            ("state", spec.expected_state.to_string()),
        ];
        for (member, expected) in expected_members {
            let reported = element(&body, member).map(|text| unescaped(&text));
            assert_eq!(
                reported.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{member}>{expected}</{member}>, got {body}"
            );
        }
        assert_eq!(
            element(&body, "ultraserverType").as_deref(),
            spec.ultraserver_type,
            "case {case:?}: expected ultraserverType {:?}, got {body}",
            spec.ultraserver_type
        );
        for reservation_id in &spec.capacity_reservation_ids {
            assert!(
                body.contains(&format!("<item>{reservation_id}</item>")),
                "case {case:?}: expected the Capacity Reservation {reservation_id} to be \
                 reported, got {body}"
            );
        }
        for carried in &spec.tags {
            let key = carried.key.clone().unwrap_or_default();
            let value = carried.value.clone().unwrap_or_default();
            assert!(
                body.contains(&format!("<key>{key}</key>"))
                    && body.contains(&format!("<value>{value}</value>")),
                "case {case:?}: expected the tag {key}={value} to be reported, got {body}"
            );
        }
    }

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_capacity_block(CapacityBlockSpec {
            ultraserver_type: Some("u-p6e-gb200x72".to_string()),
            availability_zone: "us-east-1d".to_string(),
            availability_zone_id: "use1-az4".to_string(),
            capacity_reservation_ids: vec!["cr-00000000000000011".to_string()],
            start_date: hours_from(seeded.seeded_at, 1),
            end_date: hours_from(seeded.seeded_at, 2),
            create_date: seeded.seeded_at,
            pinned_state: None,
            tags: vec![tag("Name", "late")],
        });
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The state is read off the reserved window
// ---------------------------------------------------------------------------

/// Every seeded block is reported in the state its window — or its pinned state — puts it
/// in, and reading it again reports the same thing.
#[tokio::test]
async fn the_state_of_a_block_is_the_state_its_window_puts_it_in() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (index, spec) in seeded.specs.iter().enumerate() {
        let case = format!(
            "row {index}: window {}h..{}h, pinned {:?}",
            spec.start_hours, spec.end_hours, spec.pinned_state
        );
        let params = id_params(&ids_at(&seeded, &[index]));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(spec.expected_state),
            "case {case:?}: expected state {:?}, got {body}",
            spec.expected_state
        );
    }

    // The rows between them cover every member of the modelled enum, so the suite cannot
    // silently stop exercising one of them.
    let reported: Vec<String> = described_ids(&router, "every state", &[])
        .await
        .iter()
        .zip(seeded.specs.iter())
        .map(|(_, spec)| spec.expected_state.to_string())
        .collect();
    for state in ["scheduled", "active", "expired"]
        .iter()
        .chain(CAPACITY_BLOCK_PINNED_STATES.iter())
    {
        assert!(
            reported.iter().any(|seen| seen == state),
            "the seeded rows must cover the state {state:?}, they report {reported:?}"
        );
    }
}

/// A block whose window has not opened is `scheduled`, one inside its window is `active`,
/// one past it is `expired` — whatever the window's width, and whichever block it is.
#[tokio::test]
async fn the_window_decides_scheduled_active_and_expired() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded_at = whole_second(SystemTime::now());

    // (case, start hours, end hours, expected state): windows of several widths and
    // positions, including one that opened a moment ago and one that closed a moment ago.
    let cases: [(&str, i64, i64, &str); 7] = [
        ("opens in an hour", 1, 2, "scheduled"),
        ("opens next week", 168, 192, "scheduled"),
        ("open, one-hour window", -1, 1, "active"),
        ("open, 24-hour window", -12, 12, "active"),
        ("open, opened an hour ago", -1, 167, "active"),
        ("closed an hour ago", -25, -1, "expired"),
        ("closed last week", -336, -168, "expired"),
    ];
    let mut expected: Vec<(String, &str)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for (case, start_hours, end_hours, state) in cases {
            let id = guard.insert_capacity_block(CapacityBlockSpec {
                ultraserver_type: Some("u-p6e-gb200x72".to_string()),
                availability_zone: "us-east-1a".to_string(),
                availability_zone_id: "use1-az1".to_string(),
                capacity_reservation_ids: vec!["cr-0000000000000000a".to_string()],
                start_date: hours_from(seeded_at, start_hours),
                end_date: hours_from(seeded_at, end_hours),
                create_date: hours_from(seeded_at, -336),
                pinned_state: None,
                tags: vec![tag("Case", case)],
            });
            expected.push((id, state));
        }
    }

    for (id, state) in expected {
        let (status, body) = describe(&router, &id_params(std::slice::from_ref(&id))).await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(state),
            "case {id:?}: expected state {state:?}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// CapacityBlockIds
// ---------------------------------------------------------------------------

/// `CapacityBlockIds` narrows the answer to the blocks it names, in store order whatever
/// order they were named in; an id the account holds no block for is refused rather than
/// reported as an absent row.
#[tokio::test]
async fn capacity_block_ids_narrow_the_answer_and_an_unknown_id_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: one id, two, the whole store, and the same two named in reverse.
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("one id", vec![3]),
        ("two ids", vec![1, 6]),
        ("every id", (0..seeded.ids.len()).collect()),
        ("two ids, reversed", vec![6, 1]),
    ];
    for (case, indices) in cases {
        let named = ids_at(&seeded, &indices);
        let reported = described_ids(&router, case, &id_params(&named)).await;
        let mut expected = named.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} in store order, got {reported:?}"
        );
    }

    // An id of the right shape that names nothing, one of another resource's family, and
    // free text: each is `InvalidCapacityBlockId.NotFound`, not an empty answer.
    for unknown in [
        "cb-00000000000000fff",
        "cb-0000ffff",
        "vol-00000000000000001",
        "not-an-id",
    ] {
        let params = id_params(&[unknown.to_string()]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidCapacityBlockId.NotFound",
            "case {unknown:?}: expected InvalidCapacityBlockId.NotFound, got {body}"
        );
    }

    // An unknown id is refused even when it is named beside blocks that do exist.
    let mut mixed = ids_at(&seeded, &[0]);
    mixed.push("cb-00000000000000fff".to_string());
    let (status, body) = describe(&router, &id_params(&mixed)).await;
    assert_eq!(
        status, 400,
        "a known id beside an unknown one: expected HTTP 400, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// several filters are AND'd, and the comparison is exact, case-sensitive and understands
/// EC2's `*` wildcard.
#[tokio::test]
async fn each_documented_filter_selects_the_blocks_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let all: Vec<usize> = (0..seeded.ids.len()).collect();

    // One row per filter name `DescribeCapacityBlocksRequest$Filters` documents (plus the
    // variable-key `tag:<key>` form), each asserted on a block the issue never cited.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("availability-zone", vec!["us-east-1a".into()], vec![0, 2, 6]),
        // Two values for one filter: joined with OR.
        (
            "availability-zone",
            vec!["us-east-1a".into(), "us-east-1c".into()],
            vec![0, 2, 3, 5, 6],
        ),
        // A zone no block sits in: an empty set, not an error.
        ("availability-zone", vec!["us-west-2a".into()], vec![]),
        // EC2's own wildcard.
        ("availability-zone", vec!["us-east-1*".into()], all.clone()),
        (
            "capacity-block-id",
            vec![seeded.ids[4].clone()],
            vec![4],
        ),
        (
            "ultraserver-type",
            vec!["u-p6e-gb200x72".into()],
            vec![0, 2, 7],
        ),
        (
            "ultraserver-type",
            vec!["u-p5en-gb200x36".into()],
            vec![3, 5],
        ),
        ("state", vec!["scheduled".into()], vec![0]),
        ("state", vec!["active".into()], vec![1]),
        ("state", vec!["expired".into()], vec![2]),
        ("state", vec!["cancelled".into(), "failed".into()], vec![3, 7]),
        ("state", vec!["payment-pending".into()], vec![4]),
        ("state", vec!["payment-failed".into()], vec![5]),
        ("state", vec!["unavailable".into()], vec![6]),
        ("state", vec!["payment-*".into()], vec![4, 5]),
        // Case-sensitive: a state spelled differently selects nothing.
        ("state", vec!["ACTIVE".into()], vec![]),
        ("start-date", vec![seeded.start_date(2)], vec![2]),
        ("end-date", vec![seeded.end_date(5)], vec![5]),
        ("create-date", vec![seeded.create_date(6)], vec![6]),
        ("tag:Name", vec!["train".into()], vec![0, 3]),
        ("tag:Name", vec!["infer".into()], vec![1, 6]),
        ("tag:Owner", vec!["TeamA".into()], vec![0, 5]),
        ("tag:Owner", vec!["TeamB".into()], vec![2, 6]),
        ("tag-key", vec!["Owner".into()], vec![0, 2, 5, 6]),
        (
            "tag-key",
            vec!["Name".into(), "Owner".into()],
            vec![0, 1, 2, 3, 5, 6],
        ),
        // A filter carrying no value at all matches nothing.
        ("state", vec![], vec![]),
    ];
    for (name, values, indices) in cases {
        let case = format!("{name} {values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let params = filters(&[(name, &borrowed)]);
        let reported = described_ids(&router, &case, &params).await;
        let expected = ids_at(&seeded, &indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Several filters are AND'd: only the blocks that satisfy every one of them.
    let anded = filters(&[
        ("availability-zone", &["us-east-1a"]),
        ("state", &["expired"]),
    ]);
    let reported = described_ids(&router, "two filters", &anded).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[2]),
        "two filters: expected only the expired us-east-1a block, got {reported:?}"
    );
}

/// A filter name this operation does not document is refused, rather than read as selecting
/// nothing.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a plural misspelling, a member that exists but is not a documented filter name,
    // the tag prefix with no key, a name differing only in case, and an empty name.
    for name in [
        "capacity-block-ids",
        "availability-zone-id",
        "tag:",
        "State",
        "",
    ] {
        let params = filters(&[(name, &["anything"])]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        let message = element(&body, "Message").map(|text| unescaped(&text));
        assert_eq!(
            message.as_deref(),
            Some(format!("The filter '{name}' is invalid").as_str()),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The bounds a request's own size is held to
// ---------------------------------------------------------------------------

/// The `Filter.N.Name` / `Filter.N.Value.M` form fields for a filter list of any size.
fn sized_filter_params(entries: &[(&str, Vec<String>)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (name, values)) in entries.iter().enumerate() {
        params.push((format!("Filter.{}.Name", index + 1), (*name).to_string()));
        for (position, value) in values.iter().enumerate() {
            params.push((
                format!("Filter.{}.Value.{}", index + 1, position + 1),
                value.clone(),
            ));
        }
    }
    params
}

/// The model bounds neither the number of ids, nor the number of filters, nor the number or
/// the size of a filter's values, so the handler is where each is bounded: a request at a
/// bound is answered, one past it is refused with `InvalidParameterValue` rather than being
/// served at whatever cost it asks for.
#[tokio::test]
async fn a_request_is_held_to_the_bounds_its_own_size_is_capped_at() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The caps this handler imposes, read off `crate::state`: ids per request, filters per
    // request, values per filter, bytes per value.
    let max_ids = ec2::state::MAX_RESOURCE_IDS;
    let max_filters = ec2::state::MAX_FILTERS;
    let max_values = ec2::state::MAX_FILTER_VALUES;
    let max_value_bytes = ec2::state::MAX_FILTER_VALUE_BYTES;

    // (case, params, expected status). Each bound is exercised AT the cap and one past it;
    // at the cap the request is a perfectly ordinary call.
    let at_cap_ids: Vec<String> = (0..max_ids)
        .map(|_| seeded.ids[0].clone())
        .collect();
    let past_cap_ids: Vec<String> = (0..max_ids + 1)
        .map(|_| seeded.ids[0].clone())
        .collect();
    let at_cap_filters: Vec<(&str, Vec<String>)> = (0..max_filters)
        .map(|_| ("state", vec!["active".to_string()]))
        .collect();
    let past_cap_filters: Vec<(&str, Vec<String>)> = (0..max_filters + 1)
        .map(|_| ("state", vec!["active".to_string()]))
        .collect();
    let value_of = |bytes: usize| "a".repeat(bytes);
    /// One row: the case's name, the form fields it sends, and the status it must get.
    struct BoundCase {
        case: String,
        params: Vec<(String, String)>,
        expected_status: u16,
    }
    let row = |case: String, params: Vec<(String, String)>, expected_status: u16| BoundCase {
        case,
        params,
        expected_status,
    };
    let cases: Vec<BoundCase> = vec![
        row(format!("{max_ids} ids"), id_params(&at_cap_ids), 200),
        row(
            format!("{} ids", max_ids + 1),
            id_params(&past_cap_ids),
            400,
        ),
        row(
            format!("{max_filters} filters"),
            sized_filter_params(&at_cap_filters),
            200,
        ),
        row(
            format!("{} filters", max_filters + 1),
            sized_filter_params(&past_cap_filters),
            400,
        ),
        row(
            format!("{max_values} values"),
            sized_filter_params(&[(
                "state",
                (0..max_values).map(|_| "active".to_string()).collect(),
            )]),
            200,
        ),
        row(
            format!("{} values", max_values + 1),
            sized_filter_params(&[(
                "state",
                (0..max_values + 1).map(|_| "active".to_string()).collect(),
            )]),
            400,
        ),
        row(
            format!("a {max_value_bytes}-byte value"),
            sized_filter_params(&[("state", vec![value_of(max_value_bytes)])]),
            200,
        ),
        row(
            format!("a {}-byte value", max_value_bytes + 1),
            sized_filter_params(&[("state", vec![value_of(max_value_bytes + 1)])]),
            400,
        ),
    ];
    for BoundCase {
        case,
        params,
        expected_status,
    } in cases
    {
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
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `MaxResults` bounds the page and the token resumes from where it ended, so walking the
/// pages reports every block exactly once in store order; a request with no `MaxResults` is
/// answered in one page.
#[tokio::test]
async fn max_results_pages_the_store_and_the_token_resumes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeCapacityBlocksMaxResults` is modelled `range 1..1000`, so the rows are its
    // lower bound, sizes that do and do not divide the store, the store's own size and the
    // published upper bound.
    for page_size in [1, 2, 3, 8, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut params = vec![("MaxResults".to_string(), page_size.to_string())];
        let mut walked: Vec<String> = Vec::new();
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "capacityBlockId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults blocks, got {page:?}"
            );
            walked.extend(page);
            match element(&body, "nextToken") {
                None => break,
                Some(token) => {
                    params = vec![
                        ("MaxResults".to_string(), page_size.to_string()),
                        ("NextToken".to_string(), token),
                    ];
                }
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: the walk must report every block exactly once in store order, \
             got {walked:?}"
        );
    }

    let unpaged = described_ids(&router, "no MaxResults", &[]).await;
    assert_eq!(
        unpaged, seeded.ids,
        "no MaxResults: expected every block in one answer, got {unpaged:?}"
    );
}

/// A page is a cut of the FILTERED set, not of the whole store.
#[tokio::test]
async fn a_page_is_a_cut_of_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[("availability-zone", &["us-east-1a"])]);
    params.push(("MaxResults".to_string(), "2".to_string()));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 200,
        "filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "capacityBlockId"),
        ids_at(&seeded, &[0, 2]),
        "filtered first page: expected the first two us-east-1a blocks, got {body}"
    );
    let token = element(&body, "nextToken").expect("a third us-east-1a block follows the page");

    let mut next = filters(&[("availability-zone", &["us-east-1a"])]);
    next.push(("MaxResults".to_string(), "2".to_string()));
    next.push(("NextToken".to_string(), token));
    let reported = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[6]),
        "filtered second page: expected the remaining us-east-1a block, got {reported:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 1..1000`) and one past each.
    let cases = [(0, 400), (1, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let params = vec![("MaxResults".to_string(), max_results.to_string())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, that id hex-encoded, and a token
    // carrying this service's marker with a checksum that does not match.
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0]));
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
        forged_marker,
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

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call. A request that would have been refused is
/// refused with THAT reason instead.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "capacityBlockId").is_empty(),
        "DryRun=true: nothing may be described, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded.ids,
        "DryRun=false: expected an ordinary answer, got {reported:?}"
    );

    // An unknown id beats the flag: the request would NOT have succeeded.
    let mut params = id_params(&["cb-00000000000000fff".to_string()]);
    params.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        error_code(&body),
        "InvalidCapacityBlockId.NotFound",
        "DryRun with an unknown id: expected the refusal that applies, got {status} {body}"
    );
}
