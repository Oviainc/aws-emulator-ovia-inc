// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RestoreVolumeFromRecycleBin`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A RESTORE GIVES BACK THE VOLUME THE RECYCLE BIN HELD, not a volume that resembles
//!    it: every member the bin kept — the id, the ARN, the account, the create time, the
//!    size and performance members, the zone, the tags, the state — reads back through
//!    `DescribeVolumes` exactly as it read before the volume was recycled, and the call
//!    answers `return=true`;
//! 2. a restore is a MOVE between the two stores, so a volume is in exactly one of them:
//!    while it is in the bin `DescribeVolumes` refuses its id, and once it is restored a
//!    second restore of the same id is refused;
//! 3. a volume id the bin holds no entry for is refused with `InvalidVolumeID.Malformed`
//!    (HTTP 400) whatever the reason — the recorded service answers that one code even for
//!    a well-formed long-form id, so this operation does not distinguish "no such entry"
//!    from "not a volume id" — and nothing is written;
//! 4. `DryRun` answers `DryRunOperation` and performs nothing: the volume is still in the
//!    bin, with the retention period it had;
//! 5. the end of a retention period is the CLOCK's: once the exit instant has passed the
//!    volume is permanently deleted, whether or not anybody called, so it can no longer be
//!    restored and the bytes it was charged go back to the store.
//!
//! Each table row is a distinct input along what the rules generalise over — the volume's
//! type, its size, the performance members that type takes, which zone of the Region it is
//! in, whether it carries tags and whether it is encrypted, and which way an id can fail to
//! name an entry — so no row is the specification.
//!
//! Nothing in this service's implemented surface puts a volume INTO the Recycle Bin: a
//! deleted resource is retained only when a Recycle Bin retention rule covers it, retention
//! rules are the `rbin` service's resources rather than EC2's, and `DeleteVolume` is still a
//! scaffold stub. These suites therefore retain a volume the way that operation will, by
//! calling the state primitive the bin is written through
//! ([`Ec2State::move_volume_to_recycle_bin`]) — the same seam the other suites in this
//! crate seed a store through.

use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_VOLUME_ID_MALFORMED,
    INVALID_VOLUME_ID_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed, unescaped};
use super::volume_lifecycle::{REGION, is_volume_id, p, past_the_transition, zone_id, zone_name};

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// A retention period long enough that nothing in a test run reaches its end. Stands for
/// the `RetentionPeriod` of the Recycle Bin rule that retained the volume, which is the
/// `rbin` service's value rather than one this crate owns.
const LONG_RETENTION: Duration = Duration::from_secs(3_600);

/// One row of the restore table: the case, and the `CreateVolume` members that mint the
/// volume the Recycle Bin will hold.
type VolumeRow = (&'static str, Vec<(String, String)>);

/// One row of the refusal table: the case, the `VolumeId` the request names, and the code
/// the service refuses it with.
type RefusalRow = (&'static str, String, &'static str);

/// One call of `action` signed for the Region the volume suites use.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Mint one volume from `params` and answer its id.
async fn create_volume(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = invoke(router, "CreateVolume", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}"
    );
    let volume_id = element(&body, "volumeId").unwrap_or_default();
    assert!(
        is_volume_id(&volume_id),
        "case {case:?}: expected a minted volume id, got {volume_id:?}"
    );
    volume_id
}

/// `DescribeVolumes` for one volume id: the HTTP status and the body.
async fn describe(router: &MultiServiceRouter, volume_id: &str) -> (u16, String) {
    invoke(router, "DescribeVolumes", &indexed("VolumeId", &[volume_id])).await
}

/// The whole `<volumeSet>` `DescribeVolumes` reports for `volume_id` — every member of the
/// volume as the service renders it, so comparing two of these compares the WHOLE volume
/// rather than the handful of members an assertion remembered to name.
async fn described_volume(router: &MultiServiceRouter, case: &str, volume_id: &str) -> String {
    let (status, body) = describe(router, volume_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVolumes, got {status} with body {body}"
    );
    element(&body, "volumeSet").unwrap_or_else(|| {
        panic!("case {case:?}: expected DescribeVolumes to report a volumeSet, got {body}")
    })
}

/// Retain `volume_id` in the Recycle Bin for `retention`, the way a `DeleteVolume` covered
/// by a retention rule will — through the state primitive the bin is written through, which
/// `ListVolumesInRecycleBin`'s own suite seeds the bin with too.
fn recycle(engine: &engine::InMemoryEngine, case: &str, volume_id: &str, retention: Duration) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    assert!(
        guard
            .move_volume_to_recycle_bin(volume_id, SystemTime::now(), retention)
            .is_some(),
        "case {case:?}: expected {volume_id} to be a stored volume the bin could take"
    );
}

/// Whether the Recycle Bin holds `volume_id` right now.
fn bin_holds(engine: &engine::InMemoryEngine, volume_id: &str) -> bool {
    engine
        .state::<Ec2State>()
        .lock()
        .volumes_in_recycle_bin
        .contains_key(volume_id)
}

/// How much of `volume_id`'s retention period is left, or `None` when the bin does not
/// hold it at all.
fn retention_left(engine: &engine::InMemoryEngine, volume_id: &str) -> Option<Duration> {
    let now = Instant::now();
    engine
        .state::<Ec2State>()
        .lock()
        .volumes_in_recycle_bin
        .get(volume_id)
        .map(|record| record.retention_left(now))
}

/// The caller-chosen bytes the store is holding right now.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// The `TagSpecification` members that tag a created volume.
fn tag_specification(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut members = vec![p("TagSpecification.1.ResourceType", "volume")];
    for (index, (key, value)) in tags.iter().enumerate() {
        members.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
        members.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    members
}

/// Six volumes that differ along everything the restore rule generalises over: the volume
/// type, the size, the IOPS and throughput that type takes, the zone designator, the tags
/// and the encryption members. The types and their bounds come from the service's own
/// catalogs, whose source is `CreateVolumeRequest$Size` / `$Iops` / `$Throughput`.
fn volume_rows() -> Vec<VolumeRow> {
    vec![
        (
            "a gp2 volume at its smallest size",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("VolumeType", "gp2"),
                p("Size", "1"),
            ],
        ),
        (
            "a gp3 volume with IOPS and throughput set",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("VolumeType", "gp3"),
                p("Size", "40"),
                p("Iops", "4000"),
                p("Throughput", "250"),
            ],
        ),
        (
            "an io2 volume, the type that must provision IOPS",
            vec![
                p("AvailabilityZone", zone_name(1)),
                p("VolumeType", "io2"),
                p("Size", "4"),
                p("Iops", "100"),
                p("MultiAttachEnabled", "true"),
            ],
        ),
        (
            "an st1 volume named by its zone's id",
            vec![
                p("AvailabilityZoneId", zone_id(1)),
                p("VolumeType", "st1"),
                p("Size", "125"),
            ],
        ),
        (
            "a standard volume carrying tags",
            {
                let mut members = vec![
                    p("AvailabilityZone", zone_name(0)),
                    p("VolumeType", "standard"),
                    p("Size", "1024"),
                ];
                members.extend(tag_specification(&[("Name", "retained"), ("Team", "A")]));
                members
            },
        ),
        (
            "an encrypted gp3 volume with a KMS key",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("VolumeType", "gp3"),
                p("Size", "8"),
                p("Encrypted", "true"),
                p("KmsKeyId", "alias/ExampleAlias"),
            ],
        ),
    ]
}

/// RULES 1 and 2, over the whole table: a volume the Recycle Bin holds is refused by
/// `DescribeVolumes`, a restore answers `return=true` and gives back the SAME volume —
/// byte for byte the `volumeSet` the describe reported before it was recycled — and a
/// second restore of the same id is refused because the bin no longer holds it.
#[tokio::test]
async fn restoring_a_recycled_volume_gives_back_the_volume_the_bin_held() {
    let (router, engine) = super::ec2_query::emulator();

    // Mint every row first, then wait out the `creating` -> `available` span once, so each
    // row's "before" reading is of a settled volume and the comparison afterwards is not a
    // comparison of two clocks.
    let mut rows = Vec::new();
    for (case, params) in volume_rows() {
        let volume_id = create_volume(&router, case, &params).await;
        rows.push((case, volume_id));
    }
    past_the_transition().await;

    for (case, volume_id) in &rows {
        let case: &str = case;
        let before = described_volume(&router, case, volume_id).await;
        assert!(
            before.contains("<status>available</status>"),
            "case {case:?}: expected a settled volume before it is recycled, got {before}"
        );

        recycle(&engine, case, volume_id, LONG_RETENTION);
        assert!(
            bin_holds(&engine, volume_id),
            "case {case:?}: expected the Recycle Bin to hold {volume_id}"
        );
        // RULE 2: the two stores partition the volumes, so while the bin holds it the
        // describe cannot see it — the same refusal as for a volume that never existed.
        let (status, body) = describe(&router, volume_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeVolumes for a recycled volume, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_VOLUME_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_VOLUME_ID_NOT_FOUND}, got {body}"
        );

        let (status, body) = invoke(
            &router,
            "RestoreVolumeFromRecycleBin",
            &[p("VolumeId", volume_id.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from RestoreVolumeFromRecycleBin, \
             got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected return=true, got {body}"
        );

        // RULE 1: the volume is back, and it is the one the bin held.
        let after = described_volume(&router, case, volume_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the restored volume to read back exactly as it did \
             before it was recycled, got {after} instead of {before}"
        );
        assert!(
            !bin_holds(&engine, volume_id),
            "case {case:?}: expected the Recycle Bin to have given {volume_id} up"
        );

        // RULE 2: the bin holds it no more, so restoring it again is a refusal rather than
        // a second success.
        let (status, body) = invoke(
            &router,
            "RestoreVolumeFromRecycleBin",
            &[p("VolumeId", volume_id.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a second restore, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_VOLUME_ID_MALFORMED,
            "case {case:?}: expected {INVALID_VOLUME_ID_MALFORMED}, got {body}"
        );
    }
}

/// RULE 3: every way a `VolumeId` can fail to name an entry of the bin — a value that is
/// not a volume id at all, in each of the ways an id can be misshapen; a well-formed id of
/// either published form naming nothing; and a volume the account still holds — is refused
/// with the one code this operation answers for all of them.
#[tokio::test]
async fn a_volume_id_the_recycle_bin_holds_no_entry_for_is_refused_as_malformed() {
    let (router, engine) = super::ec2_query::emulator();

    // A volume the account still has: it is not in the bin, so it is not restorable.
    let live = create_volume(
        &router,
        "a volume the account still holds",
        &[
            p("AvailabilityZone", zone_name(0)),
            p("VolumeType", "gp3"),
            p("Size", "1"),
        ],
    )
    .await;
    // A volume the bin holds, so the table also covers the one id that WOULD be restored.
    let retained = create_volume(
        &router,
        "a volume the bin holds",
        &[
            p("AvailabilityZone", zone_name(0)),
            p("VolumeType", "gp3"),
            p("Size", "1"),
        ],
    )
    .await;
    recycle(&engine, "a volume the bin holds", &retained, LONG_RETENTION);

    let rows: Vec<RefusalRow> = vec![
        // A volume id is `vol-` followed by 8 or 17 lowercase hex digits
        // (`volume_id_is_well_formed`), so each of these is not one.
        (
            "a suffix that is not hexadecimal",
            "vol-notavolume".to_string(),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a suffix of neither length",
            "vol-0123456789".to_string(),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "one digit past the long form",
            format!("vol-{}", "0".repeat(18)),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "the long form in upper case",
            "vol-0123456789ABCDEF0".to_string(),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "an id of another resource kind",
            format!("snap-{}", "0".repeat(17)),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "no prefix at all",
            "0123456789abcdef0".to_string(),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        // Well formed, both published id forms, naming nothing this store holds. The
        // recorded service answers the SAME code for these as for a misshapen id: the
        // operation resolves against the bin and reports one failure for every id it
        // cannot resolve there.
        (
            "a well-formed long id naming nothing",
            format!("vol-{}", "0".repeat(17)),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a well-formed short id naming nothing",
            "vol-000000ff".to_string(),
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a volume the account still holds",
            live.clone(),
            INVALID_VOLUME_ID_MALFORMED,
        ),
    ];

    for (case, volume_id, expected) in rows {
        let (status, body) = invoke(
            &router,
            "RestoreVolumeFromRecycleBin",
            &[p("VolumeId", volume_id.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        // The message is built from the request: it names the value the caller sent, so the
        // request can be fixed from the message alone — the wording is
        // `crate::state::volume_id_not_in_recycle_bin`'s, and the id is the only part of it
        // that varies from row to row.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            format!("The volume ID '{volume_id}' is malformed"),
            "case {case:?}: expected a message naming {volume_id:?}, got {message:?}"
        );
        // Nothing was written: the live volume is still the account's, and the retained one
        // is still in the bin.
        assert!(
            bin_holds(&engine, &retained),
            "case {case:?}: a refused restore must leave the bin as it was"
        );
        let (status, _) = describe(&router, &live).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused restore must leave the account's volumes alone"
        );
    }
}

/// RULE 4: an authorized dry run answers `DryRunOperation` and performs nothing — the
/// volume is still in the bin with the retention period it had, and a real call after it
/// still restores.
#[tokio::test]
async fn a_dry_run_leaves_the_volume_in_the_recycle_bin() {
    let (router, engine) = super::ec2_query::emulator();

    let case = "a dry run of a restorable volume";
    let volume_id = create_volume(
        &router,
        case,
        &[
            p("AvailabilityZone", zone_name(0)),
            p("VolumeType", "gp3"),
            p("Size", "2"),
        ],
    )
    .await;
    recycle(&engine, case, &volume_id, LONG_RETENTION);
    let before = retention_left(&engine, &volume_id)
        .unwrap_or_else(|| panic!("case {case:?}: expected the bin to hold {volume_id}"));

    for attempt in 1..=2 {
        let (status, body) = invoke(
            &router,
            "RestoreVolumeFromRecycleBin",
            &[p("VolumeId", volume_id.as_str()), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "attempt {attempt}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "attempt {attempt}: expected DryRunOperation, got {body}"
        );
        let left = retention_left(&engine, &volume_id).unwrap_or_else(|| {
            panic!("attempt {attempt}: expected the bin to still hold {volume_id}")
        });
        // Still essentially the whole retention period: a dry run neither restores the
        // volume nor shortens what is left of its retention, which only the clock does.
        assert!(
            left <= before && left + Duration::from_secs(60) > LONG_RETENTION,
            "attempt {attempt}: expected the retention period to run down with the clock \
             only, got {left:?} against {before:?}"
        );
        let (status, body) = describe(&router, &volume_id).await;
        assert_eq!(
            status, 400,
            "attempt {attempt}: a dry run must not give the volume back, got {status} / {body}"
        );
    }

    // The dry runs changed nothing, so the real call still has a volume to restore.
    let (status, body) = invoke(
        &router,
        "RestoreVolumeFromRecycleBin",
        &[p("VolumeId", volume_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 after the dry runs, got {status} with body {body}"
    );
    let (status, body) = describe(&router, &volume_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the restored volume to be the account's again, \
         got {status} with body {body}"
    );
}

/// RULE 5: the end of a retention period belongs to the clock. A volume whose exit instant
/// has passed is permanently deleted without anybody calling, so neither reader reports it,
/// it cannot be restored, it is not one of the account's volumes, and the bytes it was
/// charged are back.
#[tokio::test]
async fn a_retention_period_that_has_run_out_deletes_the_volume_permanently() {
    let (router, engine) = super::ec2_query::emulator();

    let case = "a retention period that runs out";
    let empty = stored_bytes(&engine);
    let mut params = vec![
        p("AvailabilityZone", zone_name(0)),
        p("VolumeType", "gp3"),
        p("Size", "3"),
        p("ClientToken", "the-token-that-minted-it"),
    ];
    params.extend(tag_specification(&[("Name", "expiring")]));
    let volume_id = create_volume(&router, case, &params).await;
    assert!(
        stored_bytes(&engine) > empty,
        "case {case:?}: expected the volume's tags and token to be charged, \
         got {} against {empty}",
        stored_bytes(&engine)
    );

    // Short, so the test waits out a span rather than a span of zero: the retention period
    // is the rule's value, and nothing but the clock ends it.
    let retention = Duration::from_millis(100);
    recycle(&engine, case, &volume_id, retention);
    tokio::time::sleep(retention + Duration::from_millis(150)).await;

    // The report of the bin is the first caller after the exit instant, so it is what
    // materialises the deletion the clock owed: the volume is gone from the bin without
    // any caller having asked for its removal.
    let (status, body) = invoke(&router, "ListVolumesInRecycleBin", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListVolumesInRecycleBin, \
         got {status} with body {body}"
    );
    assert!(
        elements(&body, "volumeId").is_empty(),
        "case {case:?}: expected the bin to report no volume past its exit instant, got {body}"
    );
    assert!(
        !bin_holds(&engine, &volume_id),
        "case {case:?}: expected the bin to have given {volume_id} up at its exit instant"
    );

    let (status, body) = invoke(
        &router,
        "RestoreVolumeFromRecycleBin",
        &[p("VolumeId", volume_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a volume whose retention period ended, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_VOLUME_ID_MALFORMED,
        "case {case:?}: expected {INVALID_VOLUME_ID_MALFORMED}, got {body}"
    );
    let (status, body) = describe(&router, &volume_id).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a permanently deleted volume is not the account's either, \
         got {status} with body {body}"
    );
    assert_eq!(
        stored_bytes(&engine),
        empty,
        "case {case:?}: expected the permanently deleted volume's bytes to go back to the \
         store's budget"
    );
}

/// A retention period whose end is not a representable instant REFUSES the move instead of
/// unwinding part way through it: the volume is still the account's, the bin holds nothing,
/// and the byte budget is unchanged — every fallible step of the move is taken before its
/// first mutation.
///
/// Driven through the state primitive, because the operation that will carry a rule's
/// retention period onto this path (`DeleteVolume`) is still a scaffold.
#[tokio::test]
async fn a_retention_period_with_no_representable_end_refuses_the_move() {
    let (router, engine) = super::ec2_query::emulator();

    let case = "a retention period with no representable end";
    let volume_id = create_volume(
        &router,
        case,
        &[
            p("AvailabilityZone", zone_name(0)),
            p("VolumeType", "gp3"),
            p("Size", "1"),
        ],
    )
    .await;
    let charged = stored_bytes(&engine);

    // Two periods past the end of both clocks, so neither row is the boundary itself.
    for retention in [Duration::MAX, Duration::MAX - Duration::from_secs(1)] {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        assert!(
            guard
                .move_volume_to_recycle_bin(&volume_id, SystemTime::now(), retention)
                .is_none(),
            "case {case:?}: expected a retention of {retention:?} to refuse the move"
        );
    }

    assert!(
        !bin_holds(&engine, &volume_id),
        "case {case:?}: a refused move must store no entry, yet the bin holds {volume_id}"
    );
    let (status, body) = describe(&router, &volume_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a refused move must leave the volume the account's, \
         got {status} with body {body}"
    );
    assert_eq!(
        stored_bytes(&engine),
        charged,
        "case {case:?}: a refused move must leave the byte budget as it was"
    );
}
