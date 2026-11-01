// SPDX-License-Identifier: CC0-1.0

//! The fast snapshot restore of a (snapshot, Availability Zone) pair as a RESOURCE of the
//! account: what `EnableFastSnapshotRestores` stores for each pair a request names, the
//! lifecycle the clock carries that pair through, what `DescribeFastSnapshotRestores`
//! reports, narrows, pages and refuses, and what `DisableFastSnapshotRestores` leaves
//! behind — end to end over the wire the service actually speaks, `ec2Query`, through the
//! crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. AN ENABLE STORES ONE RECORD PER (SNAPSHOT, ZONE) PAIR, resolved against the snapshots
//!    the account holds, and `DescribeFastSnapshotRestores` reports exactly the pairs that
//!    store holds — so an account that has enabled none reports an EMPTY set, whether or not
//!    it holds snapshots.
//! 2. THE CLOCK, not a caller, carries a pair through `FastSnapshotRestoreStateCode`:
//!    `enabling` -> `optimizing` -> `enabled` after an enable and `disabling` -> `disabled`
//!    after a disable, each phase reporting the timestamp it was entered at and the state
//!    transition reason the member's own catalog gives for it. Reading a pair never moves
//!    it.
//! 3. A SNAPSHOT THE ACCOUNT DOES NOT HOLD IS A PER-PAIR FAILURE, carrying that id's own
//!    cause, so the snapshots named beside it are still enabled and nothing is stored for
//!    the one that failed.
//! 4. THE DESCRIBE NARROWS BY THE FILTERS IT DOCUMENTS and refuses a name it does not, and
//!    pages by `MaxResults`/`NextToken` in one stable order.
//! 5. A DISABLE REPORTS THE STATE IT LEFT THE PAIR IN, and a pair nothing enabled is already
//!    `disabled`; `DryRun` reports whether a call would have worked and changes nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — how many
//! snapshots, how many zones, which of the two zone identifiers, which filter, which page
//! size, which lifecycle phase, which unresolvable id form — so no row is the specification.
//! The snapshots the rows enable restores for are made through `CreateSnapshots`, this
//! crate's writer of the snapshot store, and every pair is read back over the wire through
//! `DescribeFastSnapshotRestores`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{
    FAST_SNAPSHOT_RESTORE_REASON_LIFECYCLE, FAST_SNAPSHOT_RESTORE_REASON_USER_INITIATED,
    FAST_SNAPSHOT_RESTORE_STATE_DISABLED, FAST_SNAPSHOT_RESTORE_STATE_DISABLING,
    FAST_SNAPSHOT_RESTORE_STATE_ENABLED, FAST_SNAPSHOT_RESTORE_STATE_ENABLING,
    FAST_SNAPSHOT_RESTORE_STATE_OPTIMIZING, INVALID_PAGINATION_TOKEN, INVALID_PARAMETER_VALUE,
    INVALID_SNAPSHOT_ID_MALFORMED, INVALID_SNAPSHOT_ID_NOT_FOUND, availability_zones_in_region,
};

use super::ec2_query::{as_timestamp, element, elements, error_code, error_message, filters};
use super::fpga_image_tasks_and_volume_io::attach_in_store;
use super::volume_lifecycle::past_the_transition;
use super::wire::{Param, child, child_text, document, param, set_items};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<Param>;

/// The Region every call here is signed for: the one the default network is seeded for, and
/// the Region whose zones the pairs are enabled in.
const REGION: &str = "us-east-1";

/// The AMI a seeding launch names. A launch resolves no image, and this suite is about the
/// SNAPSHOTS, so an id the account sees no image for is enough: the launch still gives the
/// instance the one root volume every launch creates, which the seeding excludes from the
/// snapshot set.
const ANY_AMI: &str = "ami-0123456789abcdef0";

/// The device a seeded source volume is attached at: a snapshot set covers the volumes the
/// instance holds an attachment of, and one data volume is all a seed needs.
const DATA_DEVICE: &str = "/dev/sdf";

/// The emulator the tests drive, with the engine beside it so a seed can reach the store.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `action` call signed for the Region this suite works in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The `zone_name` of zone `index` of the signed Region.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// The `zone_id` of zone `index` of the signed Region.
fn zone_id(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_id.clone()
}

/// The account every pair of this emulator is enabled by.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// Which of the two identifiers a request names its Availability Zones by — the choice the
/// operation's own documentation forces ("either AvailabilityZone or AvailabilityZoneId must
/// be specified in the request, but not both") and the one a per-pair answer mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    /// `AvailabilityZone.N`, carrying zone NAMES.
    Name,
    /// `AvailabilityZoneId.N`, carrying zone IDS.
    Id,
}

impl By {
    /// The ec2Query form-field prefix this form flattens its list into. ec2Query spells the
    /// list by its member's `xmlName`, not by the member name.
    fn field(self) -> &'static str {
        match self {
            Self::Name => "AvailabilityZone",
            Self::Id => "AvailabilityZoneId",
        }
    }

    /// Zone `index` of the signed Region, in this form.
    fn zone(self, index: usize) -> String {
        match self {
            Self::Name => zone_name(index),
            Self::Id => zone_id(index),
        }
    }

    /// The member a per-pair answer reports the zone in, for a request of this form.
    fn reported_member(self) -> &'static str {
        match self {
            Self::Name => "availabilityZone",
            Self::Id => "availabilityZoneId",
        }
    }

    /// The member such an answer must NOT report, the zone's other identifier.
    fn absent_member(self) -> &'static str {
        match self {
            Self::Name => "availabilityZoneId",
            Self::Id => "availabilityZone",
        }
    }
}

/// Launch one instance and answer its id.
async fn launch(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = op(
        router,
        "RunInstances",
        &[
            param("ImageId", ANY_AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: launching an instance: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reports the instance it minted"))
}

/// Store one snapshot through `CreateSnapshots` — a writer of this crate's snapshot store —
/// and answer its id.
///
/// The seed gets its OWN instance so the set is exactly one snapshot: an instance holds the
/// root volume its launch created, and excluding that boot volume leaves only the volume
/// this seed attached.
async fn seed_snapshot(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
) -> String {
    let instance_id = launch(router, case).await;
    let (status, body) = op(
        router,
        "CreateVolume",
        &[
            param("AvailabilityZone", &zone_name(0)),
            param("Size", "8"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the seed volume: expected HTTP 200, got {status} with body {body}"
    );
    let volume_id = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports the volume it minted"));
    attach_in_store(engine, &volume_id, &instance_id, DATA_DEVICE);

    let (status, body) = op(
        router,
        "CreateSnapshots",
        &[
            param("InstanceSpecification.InstanceId", &instance_id),
            param("InstanceSpecification.ExcludeBootVolume", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: seeding a snapshot: expected HTTP 200, got {status} with body {body}"
    );
    let minted = elements(&body, "snapshotId");
    assert_eq!(
        minted.len(),
        1,
        "case {case:?}: seeding makes exactly one snapshot, got {minted:?}"
    );
    minted[0].clone()
}

/// Store `count` snapshots, in the order their ids are answered.
async fn seed_snapshots(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    count: usize,
) -> Vec<String> {
    let mut snapshots = Vec::with_capacity(count);
    for _ in 0..count {
        snapshots.push(seed_snapshot(router, engine, case).await);
    }
    snapshots
}

/// The `SourceSnapshotId.N` and zone form fields naming the pairs `snapshot_ids` x `zones`
/// (indices into the signed Region's zone list), with `extra` beside them.
fn pair_params(snapshot_ids: &[&str], by: By, zones: &[usize], extra: &[Param]) -> Params {
    let mut params = Vec::new();
    for (index, snapshot_id) in snapshot_ids.iter().enumerate() {
        params.push(param(
            &format!("SourceSnapshotId.{}", index + 1),
            snapshot_id,
        ));
    }
    for (index, zone) in zones.iter().enumerate() {
        params.push(param(
            &format!("{}.{}", by.field(), index + 1),
            &by.zone(*zone),
        ));
    }
    params.extend(extra.iter().cloned());
    params
}

/// One `EnableFastSnapshotRestores` call naming those pairs.
async fn enable(
    router: &MultiServiceRouter,
    snapshot_ids: &[&str],
    by: By,
    zones: &[usize],
    extra: &[Param],
) -> (u16, String) {
    op(
        router,
        "EnableFastSnapshotRestores",
        &pair_params(snapshot_ids, by, zones, extra),
    )
    .await
}

/// One `DisableFastSnapshotRestores` call naming those pairs.
async fn disable(
    router: &MultiServiceRouter,
    snapshot_ids: &[&str],
    by: By,
    zones: &[usize],
    extra: &[Param],
) -> (u16, String) {
    op(
        router,
        "DisableFastSnapshotRestores",
        &pair_params(snapshot_ids, by, zones, extra),
    )
    .await
}

/// One `DescribeFastSnapshotRestores` call.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    op(router, "DescribeFastSnapshotRestores", params).await
}

/// One item of a response set, as the text the response carried for each of its direct
/// members — the shape every fast snapshot restore item has, all of its members being
/// scalars.
type Item = BTreeMap<String, String>;

/// Every item of the `set` a response carries, in document order, read through the document
/// so a member of an item is never confused with one of the same name elsewhere.
fn items(case: &str, body: &str, set: &str) -> Vec<Item> {
    let doc = document(case, body);
    let root = doc.root_element();
    assert!(
        child(root, set).is_some(),
        "case {case:?}: the response must carry a {set} element, got body {body}"
    );
    set_items(root, set)
        .into_iter()
        .map(|item| {
            item.children()
                .filter(roxmltree::Node::is_element)
                .map(|member| {
                    (
                        member.tag_name().name().to_string(),
                        member.text().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

/// The pairs a `DescribeFastSnapshotRestores` response reports, in the order it reported
/// them, having first pinned that the call answered HTTP 200.
fn reported(case: &str, status: u16, body: &str) -> Vec<Item> {
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeFastSnapshotRestores must answer HTTP 200, got {status} with \
         body {body}"
    );
    items(case, body, "fastSnapshotRestoreSet")
}

/// Every pair the account holds, as `<snapshotId>|<availabilityZone>` keys — the identity of
/// a pair, which is what a narrowing or a page is asserted in terms of.
fn keys(pairs: &[Item]) -> Vec<String> {
    pairs
        .iter()
        .map(|pair| {
            format!(
                "{snapshot}|{zone}",
                snapshot = member(pair, "snapshotId"),
                zone = member(pair, "availabilityZone")
            )
        })
        .collect()
}

/// The text of `name` on `item`, or the empty string when the response reported none.
fn member(item: &Item, name: &str) -> String {
    item.get(name).cloned().unwrap_or_default()
}

/// The one pair the account holds for `snapshot_id` in zone `zone`, read back over the wire.
async fn pair_of(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
    zone: &str,
) -> Option<Item> {
    let (status, body) = describe(
        router,
        &filters(&[
            ("snapshot-id", &[snapshot_id]),
            ("availability-zone", &[zone]),
        ]),
    )
    .await;
    let pairs = reported(case, status, &body);
    assert!(
        pairs.len() <= 1,
        "case {case:?}: one snapshot has at most one fast snapshot restore per zone, got {pairs:?}"
    );
    pairs.into_iter().next()
}

// ---------------------------------------------------------------------------
// RULE 1: the describe reports the pairs the store holds, and only those
// ---------------------------------------------------------------------------

/// RULE: `DescribeFastSnapshotRestores` reports the fast snapshot restores the ACCOUNT
/// holds, so an account that has enabled none reports an EMPTY set — present, and carrying
/// no item — whether or not it holds snapshots fast snapshot restores could be enabled for.
#[tokio::test]
async fn an_account_with_no_fast_snapshot_restores_reports_an_empty_set() {
    let (router, engine) = emulator();
    for case in ["no snapshots at all", "a snapshot with no restore enabled"] {
        let (status, body) = describe(&router, &[]).await;
        let pairs = reported(case, status, &body);
        assert!(
            pairs.is_empty(),
            "case {case:?}: expected no fast snapshot restore, got {pairs:?} in body {body}"
        );
        assert_eq!(
            super::ec2_query::member(&body, "nextToken"),
            None,
            "case {case:?}: a page nothing follows carries no nextToken, got body {body}"
        );
        // The second round asks the same question of an account that DOES hold a snapshot,
        // so an empty answer can only come from the fast snapshot restore store.
        seed_snapshot(&router, &engine, case).await;
    }
}

/// One row of the enable table: how many snapshots, which zones of the Region, and which of
/// the two zone identifiers the request names them by.
struct EnableRow {
    case: &'static str,
    snapshots: usize,
    zones: &'static [usize],
    by: By,
}

/// RULE: an enable stores one fast snapshot restore per (snapshot, zone) pair it names, each
/// reported by the zone identifier the request used, and the describe reports exactly those
/// pairs with BOTH of the zone's identifiers.
///
/// The rows differ in how many snapshots and zones one request crosses and in which
/// identifier it names the zones by, so neither dimension is read off the other.
#[tokio::test]
async fn an_enable_stores_one_fast_snapshot_restore_per_pair() {
    let rows = [
        EnableRow {
            case: "one snapshot, one zone, by zone name",
            snapshots: 1,
            zones: &[0],
            by: By::Name,
        },
        EnableRow {
            case: "one snapshot, every zone of the Region, by zone name",
            snapshots: 1,
            zones: &[0, 1, 2],
            by: By::Name,
        },
        EnableRow {
            case: "two snapshots, one zone, by zone id",
            snapshots: 2,
            zones: &[1],
            by: By::Id,
        },
        EnableRow {
            case: "two snapshots, two zones, by zone id",
            snapshots: 2,
            zones: &[0, 2],
            by: By::Id,
        },
    ];
    for row in rows {
        let EnableRow {
            case,
            snapshots,
            zones,
            by,
        } = row;
        let (router, engine) = emulator();
        let seeded = seed_snapshots(&router, &engine, case, snapshots).await;
        let named: Vec<&str> = seeded.iter().map(String::as_str).collect();

        let (status, body) = enable(&router, &named, by, zones, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let successful = items(case, &body, "successful");
        assert_eq!(
            successful.len(),
            snapshots * zones.len(),
            "case {case:?}: expected one item per pair, got {successful:?}"
        );
        assert!(
            items(case, &body, "unsuccessful").is_empty(),
            "case {case:?}: every pair names a stored snapshot, so none is unsuccessful: {body}"
        );
        for item in &successful {
            assert_eq!(
                member(item, "state"),
                FAST_SNAPSHOT_RESTORE_STATE_ENABLING,
                "case {case:?}: an enable answers the state it put the pair in, got {item:?}"
            );
            assert_eq!(
                member(item, "stateTransitionReason"),
                FAST_SNAPSHOT_RESTORE_REASON_USER_INITIATED,
                "case {case:?}: the reason of a state a caller asked for, got {item:?}"
            );
            assert_eq!(
                member(item, "ownerId"),
                account(),
                "case {case:?}: the account that enabled the pair, got {item:?}"
            );
            assert!(
                as_timestamp(&member(item, "enablingTime")).is_some(),
                "case {case:?}: a pair in enabling reports when it entered it, got {item:?}"
            );
            assert!(
                !item.contains_key("optimizingTime") && !item.contains_key("enabledTime"),
                "case {case:?}: a phase the pair has not entered reports no time, got {item:?}"
            );
            assert!(
                !member(item, by.reported_member()).is_empty(),
                "case {case:?}: the zone is reported by the identifier the request used, \
                 got {item:?}"
            );
            assert!(
                !item.contains_key(by.absent_member()),
                "case {case:?}: the zone's other identifier is not reported, got {item:?}"
            );
        }

        // The pairs the enable reported are the pairs the describe reports, each carrying
        // both of its zone's identifiers.
        let (status, body) = describe(&router, &[]).await;
        let described = reported(case, status, &body);
        let mut expected: Vec<String> = Vec::new();
        for snapshot_id in &seeded {
            for zone in zones {
                expected.push(format!(
                    "{snapshot_id}|{zone_name}",
                    zone_name = zone_name(*zone)
                ));
            }
        }
        expected.sort();
        let mut actual = keys(&described);
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the pairs the enable stored, got {described:?}"
        );
        for item in &described {
            let reported_zone = member(item, "availabilityZone");
            let reported_zone_id = member(item, "availabilityZoneId");
            let index = zones
                .iter()
                .find(|zone| zone_name(**zone) == reported_zone)
                .copied()
                .unwrap_or_else(|| panic!("case {case:?}: unexpected zone in {item:?}"));
            assert_eq!(
                reported_zone_id,
                zone_id(index),
                "case {case:?}: the describe reports the zone's id beside its name, got {item:?}"
            );
        }
    }
}

/// RULE: a pair is identified by the snapshot AND the zone, so naming either of them twice
/// in one request answers that pair once — and naming the zone by its id reaches the same
/// pair an enable by zone name stored.
#[tokio::test]
async fn one_pair_is_one_record_however_it_is_named() {
    let case = "repeats and both zone identifiers";
    let (router, engine) = emulator();
    let snapshot = seed_snapshot(&router, &engine, case).await;

    let (status, body) = enable(
        &router,
        &[snapshot.as_str(), snapshot.as_str()],
        By::Name,
        &[0, 0],
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let successful = items(case, &body, "successful");
    assert_eq!(
        successful.len(),
        1,
        "case {case:?}: one pair gets one answer however often it is named, got {successful:?}"
    );

    // The same pair, named by the zone's OTHER identifier: it is the pair already stored, so
    // the account still holds exactly one.
    let (status, body) = enable(&router, &[snapshot.as_str()], By::Id, &[0], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let first_enabling = member(&successful[0], "enablingTime");
    let again = items(case, &body, "successful");
    assert_eq!(
        again.len(),
        1,
        "case {case:?}: expected one item, got {again:?}"
    );
    assert_eq!(
        member(&again[0], "enablingTime"),
        first_enabling,
        "case {case:?}: re-enabling a pair already enabling reports the enable already \
         running, got {again:?}"
    );
    let (status, body) = describe(&router, &[]).await;
    let described = reported(case, status, &body);
    assert_eq!(
        described.len(),
        1,
        "case {case:?}: the account holds one pair, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the clock carries a pair through the state catalog
// ---------------------------------------------------------------------------

/// RULE: a pair an enable started is `enabling`, which the clock carries to `optimizing` and
/// then to `enabled`; each phase reports the time it was entered at, the reason the member's
/// catalog gives for it, and no time for a phase not yet reached. Reading a pair does not
/// move it.
///
/// The rows are the three phases of one pair, each read after the span the service owes the
/// move — the phase is what varies, the pair is the same one throughout.
#[tokio::test]
async fn the_clock_carries_an_enabled_pair_to_enabled() {
    let case = "one pair through its whole enable";
    let (router, engine) = emulator();
    let snapshot = seed_snapshot(&router, &engine, case).await;
    let (status, body) = enable(&router, &[snapshot.as_str()], By::Name, &[0], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );

    /// One expected phase: the state, the reason, and the timestamp members that must be
    /// reported by the time it is reached.
    struct Phase {
        case: &'static str,
        state: &'static str,
        reason: &'static str,
        times: &'static [&'static str],
    }
    let phases = [
        Phase {
            case: "straight after the enable",
            state: FAST_SNAPSHOT_RESTORE_STATE_ENABLING,
            reason: FAST_SNAPSHOT_RESTORE_REASON_USER_INITIATED,
            times: &["enablingTime"],
        },
        Phase {
            case: "one span after the enable",
            state: FAST_SNAPSHOT_RESTORE_STATE_OPTIMIZING,
            reason: FAST_SNAPSHOT_RESTORE_REASON_LIFECYCLE,
            times: &["enablingTime", "optimizingTime"],
        },
        Phase {
            case: "two spans after the enable",
            state: FAST_SNAPSHOT_RESTORE_STATE_ENABLED,
            reason: FAST_SNAPSHOT_RESTORE_REASON_LIFECYCLE,
            times: &["enablingTime", "optimizingTime", "enabledTime"],
        },
    ];
    let every_time = ["enablingTime", "optimizingTime", "enabledTime"];
    for phase in phases {
        let case = phase.case;
        let pair = pair_of(&router, case, &snapshot, &zone_name(0))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the pair the enable stored is reported"));
        assert_eq!(
            member(&pair, "state"),
            phase.state,
            "case {case:?}: expected state {state:?}, got {pair:?}",
            state = phase.state
        );
        assert_eq!(
            member(&pair, "stateTransitionReason"),
            phase.reason,
            "case {case:?}: expected reason {reason:?}, got {pair:?}",
            reason = phase.reason
        );
        let mut reached: Vec<f64> = Vec::new();
        for name in every_time {
            let entered = phase.times.contains(&name);
            match pair.get(name) {
                Some(text) if entered => {
                    let at = as_timestamp(text).unwrap_or_else(|| {
                        panic!("case {case:?}: {name} must be a timestamp, got {text:?}")
                    });
                    reached.push(at);
                }
                reported => assert!(
                    !entered && reported.is_none(),
                    "case {case:?}: {name} must be reported only once the phase is entered, \
                     got {pair:?}"
                ),
            }
        }
        assert!(
            reached.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case:?}: the phases must be reported in the order they happened, got \
             {reached:?}"
        );

        // A read does not move the pair: asking again at the same phase answers the same
        // thing.
        let again = pair_of(&router, case, &snapshot, &zone_name(0))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the pair is still reported"));
        assert_eq!(
            again, pair,
            "case {case:?}: reading a pair must not change what a later caller sees"
        );
        past_the_transition().await;
    }
}

// ---------------------------------------------------------------------------
// RULE 3: a snapshot the account does not hold is a per-pair failure
// ---------------------------------------------------------------------------

/// The snapshot id forms an enable cannot resolve, and the code each is answered with: a
/// value that is not a snapshot id EC2 could have ISSUED is malformed — including the
/// all-zero placeholder — and one that is but names no snapshot of the account is not found.
///
/// Both published id lengths, the all-zero placeholder, a non-hexadecimal suffix, another
/// resource's prefix and the bare prefix, so the classification is asserted over the class.
fn unresolvable_ids() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("the long id form", "snap-0123456789abcdef0", INVALID_SNAPSHOT_ID_NOT_FOUND),
        ("the short id form", "snap-1a2b3c4d", INVALID_SNAPSHOT_ID_NOT_FOUND),
        ("the null id", "snap-00000000000000000", INVALID_SNAPSHOT_ID_MALFORMED),
        ("a non-hexadecimal suffix", "snap-zzzzzzzzzzzzzzzzz", INVALID_SNAPSHOT_ID_MALFORMED),
        ("another resource's prefix", "vol-0123456789abcdef0", INVALID_SNAPSHOT_ID_MALFORMED),
        ("the bare prefix", "snap-", INVALID_SNAPSHOT_ID_MALFORMED),
    ]
}

/// RULE: a snapshot id an enable cannot resolve is reported in `Unsuccessful` under that id,
/// with one error per requested zone carrying that id's own cause — never as a refusal of
/// the whole call — and nothing is stored for it.
#[tokio::test]
async fn a_snapshot_the_account_does_not_hold_fails_per_pair() {
    let (router, _engine) = emulator();
    for (case, snapshot_id, expected_code) in unresolvable_ids() {
        let (status, body) = enable(&router, &[snapshot_id], By::Name, &[0, 1], &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an unresolvable snapshot is a per-pair failure, so the call \
             answers HTTP 200; got {status} with body {body}"
        );
        assert!(
            items(case, &body, "successful").is_empty(),
            "case {case:?}: nothing was enabled, so no pair is successful: {body}"
        );
        let unsuccessful = items(case, &body, "unsuccessful");
        assert_eq!(
            unsuccessful.len(),
            1,
            "case {case:?}: one item per named snapshot, got {unsuccessful:?}"
        );
        assert_eq!(
            member(&unsuccessful[0], "snapshotId"),
            snapshot_id,
            "case {case:?}: the item is reported under the id the request named, got \
             {unsuccessful:?}"
        );
        let doc = document(case, &body);
        let errors: Vec<(String, String)> = set_items(doc.root_element(), "unsuccessful")
            .into_iter()
            .flat_map(|item| set_items(item, "fastSnapshotRestoreStateErrorSet"))
            .filter_map(|pair| child(pair, "error"))
            .map(|error| {
                (
                    child_text(error, "code").unwrap_or_default(),
                    child_text(error, "message").unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            errors.len(),
            2,
            "case {case:?}: the snapshot's cause is reported for each requested zone, got \
             {errors:?}"
        );
        for (code, message) in &errors {
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected code {expected_code:?}, got {errors:?}"
            );
            assert!(
                message.contains(snapshot_id),
                "case {case:?}: the message must name the value the request gave, got \
                 {errors:?}"
            );
        }
        let (status, body) = describe(&router, &[]).await;
        let described = reported(case, status, &body);
        assert!(
            described.is_empty(),
            "case {case:?}: a pair that failed stores nothing, got {described:?}"
        );
    }
}

/// RULE: each pair is answered on its own, so a snapshot the call cannot resolve never stops
/// the snapshots named beside it from being enabled.
#[tokio::test]
async fn one_unresolvable_snapshot_does_not_refuse_the_others() {
    let case = "a stored snapshot beside an unknown one";
    let (router, engine) = emulator();
    let stored = seed_snapshot(&router, &engine, case).await;
    let unknown = "snap-0123456789abcdef0";

    let (status, body) = enable(
        &router,
        &[stored.as_str(), unknown],
        By::Name,
        &[0],
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let successful = items(case, &body, "successful");
    assert_eq!(
        successful.len(),
        1,
        "case {case:?}: the stored snapshot's pair is enabled, got {successful:?}"
    );
    assert_eq!(
        member(&successful[0], "snapshotId"),
        stored,
        "case {case:?}: the enabled pair is the stored snapshot's, got {successful:?}"
    );
    let unsuccessful = items(case, &body, "unsuccessful");
    assert_eq!(
        unsuccessful.len(),
        1,
        "case {case:?}: the unknown snapshot is reported on its own, got {unsuccessful:?}"
    );
    assert_eq!(
        member(&unsuccessful[0], "snapshotId"),
        unknown,
        "case {case:?}: under the id the request named, got {unsuccessful:?}"
    );
    let (status, body) = describe(&router, &[]).await;
    let described = reported(case, status, &body);
    assert_eq!(
        keys(&described),
        vec![format!("{stored}|{zone}", zone = zone_name(0))],
        "case {case:?}: only the pair that succeeded is stored, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: the describe narrows and pages
// ---------------------------------------------------------------------------

/// Two snapshots with fast snapshot restores in two zones each, all four `enabled`, and then
/// the first pair disabled all the way to `disabled` so the set spans two states. Answers the
/// router and the two snapshot ids in the order they were seeded.
///
/// Both states are TERMINAL ones, reached before the first assertion runs, so no row depends
/// on being read inside a transition's own span.
async fn seed_four_pairs(case: &str) -> (MultiServiceRouter, Vec<String>) {
    let (router, engine) = emulator();
    let seeded = seed_snapshots(&router, &engine, case, 2).await;
    let named: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let (status, body) = enable(&router, &named, By::Name, &[0, 1], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: seeding four pairs: expected HTTP 200, got {status} with body {body}"
    );
    // Two spans takes every pair to `enabled`, the terminal state of an enable.
    past_the_transition().await;
    past_the_transition().await;
    let (status, body) = disable(&router, &named[..1], By::Name, &[0], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: disabling one pair: expected HTTP 200, got {status} with body {body}"
    );
    // One more span takes that pair to `disabled`, the terminal state of a disable.
    past_the_transition().await;
    (router, seeded)
}

/// RULE: `Filters` narrows the report to the pairs that match every filter, each filter's
/// values joined with an OR and matched with EC2's wildcards, over the attributes the member
/// documents — and a filter naming an attribute no pair matches selects nothing rather than
/// everything.
///
/// The rows are every documented filter name, one filter and two together, a wildcard value
/// and values nothing matches, so the narrowing is asserted over the class of filters rather
/// than on one of them.
#[tokio::test]
async fn the_describe_narrows_by_every_documented_filter() {
    let case = "four pairs, one of them disabled";
    let (router, seeded) = seed_four_pairs(case).await;
    let (first, second) = (seeded[0].clone(), seeded[1].clone());
    let pair = |snapshot: &str, zone: usize| format!("{snapshot}|{name}", name = zone_name(zone));

    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        (
            "snapshot-id names one snapshot",
            filters(&[("snapshot-id", &[first.as_str()])]),
            vec![pair(&first, 0), pair(&first, 1)],
        ),
        (
            "snapshot-id names both snapshots",
            filters(&[("snapshot-id", &[first.as_str(), second.as_str()])]),
            vec![
                pair(&first, 0),
                pair(&first, 1),
                pair(&second, 0),
                pair(&second, 1),
            ],
        ),
        (
            "availability-zone names one zone",
            filters(&[("availability-zone", &[&zone_name(1)])]),
            vec![pair(&first, 1), pair(&second, 1)],
        ),
        (
            "availability-zone-id names the same zone by its id",
            filters(&[("availability-zone-id", &[&zone_id(1)])]),
            vec![pair(&first, 1), pair(&second, 1)],
        ),
        (
            "owner-id names the account that enabled them",
            filters(&[("owner-id", &[&account()])]),
            vec![
                pair(&first, 0),
                pair(&first, 1),
                pair(&second, 0),
                pair(&second, 1),
            ],
        ),
        (
            "state names the disabled pair's state",
            filters(&[("state", &[FAST_SNAPSHOT_RESTORE_STATE_DISABLED])]),
            vec![pair(&first, 0)],
        ),
        (
            "state names the enabled ones",
            filters(&[("state", &[FAST_SNAPSHOT_RESTORE_STATE_ENABLED])]),
            vec![pair(&first, 1), pair(&second, 0), pair(&second, 1)],
        ),
        (
            "a wildcard value matches the state it spans",
            filters(&[("state", &["disabl*"])]),
            vec![pair(&first, 0)],
        ),
        (
            "two values of one filter are joined with an OR",
            filters(&[(
                "state",
                &[
                    FAST_SNAPSHOT_RESTORE_STATE_DISABLED,
                    FAST_SNAPSHOT_RESTORE_STATE_ENABLED,
                ],
            )]),
            vec![
                pair(&first, 0),
                pair(&first, 1),
                pair(&second, 0),
                pair(&second, 1),
            ],
        ),
        (
            "a state no pair is in matches nothing",
            filters(&[("state", &[FAST_SNAPSHOT_RESTORE_STATE_OPTIMIZING])]),
            Vec::new(),
        ),
        (
            "two filters are joined with an AND",
            filters(&[
                ("snapshot-id", &[second.as_str()]),
                ("availability-zone", &[&zone_name(0)]),
            ]),
            vec![pair(&second, 0)],
        ),
        (
            "a snapshot nothing enabled matches nothing",
            filters(&[("snapshot-id", &["snap-0123456789abcdef0"])]),
            Vec::new(),
        ),
        (
            "another account matches nothing",
            filters(&[("owner-id", &["123456789012"])]),
            Vec::new(),
        ),
        (
            "a zone of the Region nothing was enabled in matches nothing",
            filters(&[("availability-zone", &[&zone_name(2)])]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        let described = reported(case, status, &body);
        let mut actual = keys(&described);
        actual.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the pairs the filter selects, got {described:?}"
        );
    }
}

/// RULE: a `Filters` entry naming a filter the operation does not document is refused, with
/// the EC2 code for a value the service cannot use and a message naming the filter — never
/// read as "no results".
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let (router, _engine) = emulator();
    for case in ["snapshot", "tag:owner", "SNAPSHOT-ID", "state "] {
        let (status, body) = describe(&router, &filters(&[(case, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(case),
            "case {case:?}: the refusal must name the filter it is about, got {message:?}"
        );
    }
}

/// RULE: the report is paged by `MaxResults` in one stable order, each page carrying a
/// `NextToken` only while a further pair follows it, and a walk of the pages sees every pair
/// exactly once in that same order.
///
/// The rows are page sizes across the class: one, a size that does not divide the set, the
/// whole set, the `0` the modelled range admits (which asks for no page size at all) and no
/// `MaxResults` at all.
#[tokio::test]
async fn the_describe_pages_by_max_results() {
    let case = "six pairs";
    let (router, engine) = emulator();
    let seeded = seed_snapshots(&router, &engine, case, 2).await;
    let named: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let (status, body) = enable(&router, &named, By::Name, &[0, 1, 2], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = describe(&router, &[]).await;
    let whole = keys(&reported(case, status, &body));
    assert_eq!(
        whole.len(),
        6,
        "case {case:?}: two snapshots in three zones is six pairs, got {whole:?}"
    );
    assert_eq!(
        whole.iter().collect::<BTreeSet<_>>().len(),
        whole.len(),
        "case {case:?}: every pair is reported once, got {whole:?}"
    );

    for page_size in ["1", "4", "6", "0", "200"] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", page_size)];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let (status, body) = describe(&router, &params).await;
            let described = reported(&case, status, &body);
            let limit: usize = page_size.parse().unwrap_or_default();
            if limit > 0 {
                assert!(
                    described.len() <= limit,
                    "case {case:?}: a page carries at most {limit} pairs, got {described:?}"
                );
            }
            walked.extend(keys(&described));
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the walk must end, got {walked:?}"
            );
            match super::ec2_query::member(&body, "nextToken") {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: a paged walk sees every pair once, in the order the unpaged \
             report gives"
        );
    }
}

/// RULE: a `NextToken` this service did not issue is refused rather than read as a position,
/// so a bad token can never be answered with a page.
#[tokio::test]
async fn a_token_the_service_did_not_issue_is_refused() {
    let (router, _engine) = emulator();
    for case in ["not-a-token", "", "snap-0123456789abcdef0"] {
        let (status, body) = describe(&router, &[param("NextToken", case)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected {INVALID_PAGINATION_TOKEN}, got body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: a disable reports the state it left the pair in
// ---------------------------------------------------------------------------

/// RULE: a disable puts an enabled pair in `disabling`, from which the clock owes it
/// `disabled`; the pair keeps the phases it already walked, the describe reports it in the
/// state the disable left it in, and an enable afterwards starts the lifecycle again.
#[tokio::test]
async fn a_disable_carries_an_enabled_pair_to_disabled() {
    let case = "one enabled pair";
    let (router, engine) = emulator();
    let snapshot = seed_snapshot(&router, &engine, case).await;
    let (status, body) = enable(&router, &[snapshot.as_str()], By::Name, &[0], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    past_the_transition().await;
    past_the_transition().await;

    let (status, body) = disable(&router, &[snapshot.as_str()], By::Name, &[0], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let successful = items(case, &body, "successful");
    assert_eq!(
        successful.len(),
        1,
        "case {case:?}: one pair was named, got {successful:?}"
    );
    assert!(
        items(case, &body, "unsuccessful").is_empty(),
        "case {case:?}: the pair resolved, so none is unsuccessful: {body}"
    );
    assert_eq!(
        member(&successful[0], "state"),
        FAST_SNAPSHOT_RESTORE_STATE_DISABLING,
        "case {case:?}: a disable answers the state it put the pair in, got {successful:?}"
    );
    assert_eq!(
        member(&successful[0], "stateTransitionReason"),
        FAST_SNAPSHOT_RESTORE_REASON_USER_INITIATED,
        "case {case:?}: the reason of a state a caller asked for, got {successful:?}"
    );

    // The pair is still reported, in the state the disable left it in, and it keeps the
    // phases it walked on the way.
    let disabling = pair_of(&router, case, &snapshot, &zone_name(0))
        .await
        .unwrap_or_else(|| panic!("case {case:?}: a disabled pair is still reported"));
    assert_eq!(
        member(&disabling, "state"),
        FAST_SNAPSHOT_RESTORE_STATE_DISABLING,
        "case {case:?}: expected the disabling state, got {disabling:?}"
    );
    for name in ["enablingTime", "optimizingTime", "enabledTime", "disablingTime"] {
        assert!(
            as_timestamp(&member(&disabling, name)).is_some(),
            "case {case:?}: {name} must be reported for a pair that walked it, got {disabling:?}"
        );
    }
    assert!(
        !disabling.contains_key("disabledTime"),
        "case {case:?}: a pair still disabling has not reached disabled, got {disabling:?}"
    );

    past_the_transition().await;
    let disabled = pair_of(&router, case, &snapshot, &zone_name(0))
        .await
        .unwrap_or_else(|| panic!("case {case:?}: a disabled pair is still reported"));
    assert_eq!(
        member(&disabled, "state"),
        FAST_SNAPSHOT_RESTORE_STATE_DISABLED,
        "case {case:?}: the clock owes a disabling pair the disabled state, got {disabled:?}"
    );
    assert_eq!(
        member(&disabled, "stateTransitionReason"),
        FAST_SNAPSHOT_RESTORE_REASON_LIFECYCLE,
        "case {case:?}: the reason of a state the lifecycle reached, got {disabled:?}"
    );
    assert!(
        as_timestamp(&member(&disabled, "disabledTime")).is_some(),
        "case {case:?}: a disabled pair reports when it entered that state, got {disabled:?}"
    );

    // Enabling the pair again starts the lifecycle over: it is `enabling`, with none of the
    // disable's own timestamps.
    let (status, body) = enable(&router, &[snapshot.as_str()], By::Name, &[0], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let again = items(case, &body, "successful");
    assert_eq!(
        again.len(),
        1,
        "case {case:?}: one pair was named, got {again:?}"
    );
    assert_eq!(
        member(&again[0], "state"),
        FAST_SNAPSHOT_RESTORE_STATE_ENABLING,
        "case {case:?}: enabling a disabled pair starts its lifecycle again, got {again:?}"
    );
    for name in ["disablingTime", "disabledTime", "optimizingTime", "enabledTime"] {
        assert!(
            !again[0].contains_key(name),
            "case {case:?}: a pair enabling again reports no {name}, got {again:?}"
        );
    }
}

/// RULE: a pair the account never enabled is already in the state a disable asks for, so the
/// disable reports it `disabled` — with no lifecycle at all, nothing having run — and stores
/// nothing for it.
#[tokio::test]
async fn disabling_a_pair_nothing_enabled_reports_it_disabled() {
    let case = "a stored snapshot with no fast snapshot restore";
    let (router, engine) = emulator();
    let snapshot = seed_snapshot(&router, &engine, case).await;

    let (status, body) = disable(&router, &[snapshot.as_str()], By::Id, &[1], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let successful = items(case, &body, "successful");
    assert_eq!(
        successful.len(),
        1,
        "case {case:?}: one pair was named, got {successful:?}"
    );
    assert_eq!(
        member(&successful[0], "state"),
        FAST_SNAPSHOT_RESTORE_STATE_DISABLED,
        "case {case:?}: a pair nothing enabled is already disabled, got {successful:?}"
    );
    assert_eq!(
        member(&successful[0], "availabilityZoneId"),
        zone_id(1),
        "case {case:?}: the zone is reported by the identifier the request used, got \
         {successful:?}"
    );
    for name in [
        "enablingTime",
        "optimizingTime",
        "enabledTime",
        "disablingTime",
        "disabledTime",
        "ownerId",
    ] {
        assert!(
            !successful[0].contains_key(name),
            "case {case:?}: nothing ran for this pair, so it reports no {name}, got \
             {successful:?}"
        );
    }
    let (status, body) = describe(&router, &[]).await;
    let described = reported(case, status, &body);
    assert!(
        described.is_empty(),
        "case {case:?}: a pair nothing enabled is no pair of the account, got {described:?}"
    );
}

/// RULE: `DryRun` reports whether the call would have worked and performs it NOT AT ALL — so
/// the store a later describe reads is exactly what it was before the call.
///
/// The rows are the three operations of this surface, each asked with the flag set and then
/// with it clear, so the refusal is asserted for the class rather than for one of them.
#[tokio::test]
async fn a_dry_run_changes_nothing() {
    let case = "an enabled pair";
    let (router, engine) = emulator();
    let snapshot = seed_snapshot(&router, &engine, case).await;
    let (status, body) = enable(&router, &[snapshot.as_str()], By::Name, &[0], &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    // Let the enable's own lifecycle finish before the probes, so the state the rows compare
    // is the TERMINAL `enabled` one: a clock-owed move happening between two of them would
    // otherwise read as a change a `DryRun` made.
    past_the_transition().await;
    past_the_transition().await;
    let before = pair_of(&router, case, &snapshot, &zone_name(0)).await;
    assert_eq!(
        before.as_ref().map(|pair| member(pair, "state")),
        Some(FAST_SNAPSHOT_RESTORE_STATE_ENABLED.to_string()),
        "case {case:?}: the probes start from a pair in its terminal state, got {before:?}"
    );

    let dry_run = [param("DryRun", "true")];
    let probes: Vec<(&str, Params)> = vec![
        (
            "EnableFastSnapshotRestores",
            pair_params(&[snapshot.as_str()], By::Name, &[1], &dry_run),
        ),
        (
            "DisableFastSnapshotRestores",
            pair_params(&[snapshot.as_str()], By::Name, &[0], &dry_run),
        ),
        ("DescribeFastSnapshotRestores", dry_run.to_vec()),
    ];
    for (action, params) in probes {
        let (status, body) = op(&router, action, &params).await;
        assert_eq!(
            status, 412,
            "case {action:?}: a permitted DryRun is answered HTTP 412, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {action:?}: expected the DryRun code, got body {body}"
        );
        let after = pair_of(&router, action, &snapshot, &zone_name(0)).await;
        assert_eq!(
            after.as_ref().map(|pair| member(pair, "state")),
            before.as_ref().map(|pair| member(pair, "state")),
            "case {action:?}: a DryRun must leave the pair's state as it was"
        );
        let (status, body) = describe(&router, &[]).await;
        let described = reported(action, status, &body);
        assert_eq!(
            keys(&described),
            vec![format!("{snapshot}|{zone}", zone = zone_name(0))],
            "case {action:?}: a DryRun stores and removes nothing, got {described:?}"
        );
    }
}
