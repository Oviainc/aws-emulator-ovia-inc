// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVolumesModifications`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeVolumesModifications`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"), through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the operation REPORTS THE STORE: the set it answers with is the most recent
//!    modification of each EBS volume that has one, narrowed by `VolumeIds` and the
//!    documented filters and cut into `MaxResults`-sized pages, and every member of a
//!    stored modification round-trips;
//! 2. a volume that RESOLVES but carries NO modification is refused with
//!    `InvalidVolumeModification.NotFound`, never reported as an empty answer — while a
//!    request that names no volume at all reports however many modifications the account
//!    has, including none;
//! 3. a volume id that resolves to no volume is refused before its modification is looked
//!    for: a malformed id as malformed, a well-formed unknown one as not found;
//! 4. the CLOCK, not a caller, moves a modification `modifying` -> `optimizing` ->
//!    `completed`, and describing is a read: two calls answer the same.
//!
//! Each table row is a distinct input along what the rules generalise over — the volume
//! type, the size, which attribute the modification changes, which documented filter, how
//! many filter values, which page size — so no row is the specification.

use std::time::{Duration, Instant, SystemTime};

use ec2::ops::types::VolumeModification;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_VOLUME_ID_MALFORMED,
    INVALID_VOLUME_ID_NOT_FOUND, INVALID_VOLUME_MODIFICATION_NOT_FOUND, TRANSITION_DELAY,
    VOLUME_MODIFICATION_PROGRESS_COMPLETE, VOLUME_MODIFICATION_STATE_COMPLETED,
    VOLUME_MODIFICATION_STATE_MODIFYING, VOLUME_MODIFICATION_STATE_OPTIMIZING,
    VolumeModificationRecord, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};
use super::volume_lifecycle::{REGION, p, zone_name};

/// A margin on top of a clock-owed span, so a move that has fallen due has certainly
/// fallen due by the time the assertion runs.
const MARGIN: Duration = Duration::from_millis(200);

/// One row of the filter table: `(case, the `Filters` entries, the volume ids the report
/// must name)`.
type FilterRow = (&'static str, Vec<(&'static str, &'static [&'static str])>, Vec<String>);

/// One modification fixture: the volume it is made on, and the change it asks for.
///
/// The `original_*` values are whatever `CreateVolume` gave the volume — the fixture
/// creates it with exactly them — so a reported `original` member can be checked against
/// the volume itself rather than against a literal.
#[derive(Debug, Clone)]
struct Fixture {
    /// What the row is exercising, named in every assertion it makes.
    case: &'static str,
    /// The volume's type before the modification, which it is created as.
    volume_type: &'static str,
    /// Its size, in GiB, before the modification.
    size: i32,
    /// Its provisioned IOPS, for a type that provisions them.
    iops: Option<i32>,
    /// Its throughput, in MiB/s, for the one type that carries one.
    throughput: Option<i32>,
    /// Its Multi-Attach setting, for a type that supports it.
    multi_attach: Option<bool>,
    /// The size the modification targets.
    target_size: i32,
    /// The volume type it targets.
    target_volume_type: &'static str,
    /// The IOPS it targets, for a target type that provisions them.
    target_iops: Option<i32>,
    /// The throughput it targets, for a target type that carries one.
    target_throughput: Option<i32>,
    /// The Multi-Attach setting it targets, for a target type that supports it.
    target_multi_attach: Option<bool>,
    /// The wall-clock second the modification started on. Fixed per fixture, so the
    /// reported `StartTime` is the one that was stored rather than whatever the clock said
    /// while the test ran.
    start_time_secs: u64,
}

impl Fixture {
    /// This fixture as the modelled shape a writer stores: the volume it belongs to, its
    /// original values, its target values and its start time. The state, the progress and
    /// the end time are left to the record, which derives them from the clock.
    fn view(&self, volume_id: &str) -> VolumeModification {
        VolumeModification {
            volume_id: Some(volume_id.to_string()),
            original_size: Some(self.size),
            original_volume_type: Some(self.volume_type.to_string()),
            original_iops: self.iops,
            original_throughput: self.throughput,
            original_multi_attach_enabled: self.multi_attach,
            target_size: Some(self.target_size),
            target_volume_type: Some(self.target_volume_type.to_string()),
            target_iops: self.target_iops,
            target_throughput: self.target_throughput,
            target_multi_attach_enabled: self.target_multi_attach,
            start_time: Some(system_time_to_date_time(
                SystemTime::UNIX_EPOCH + Duration::from_secs(self.start_time_secs),
            )),
            ..VolumeModification::default()
        }
    }
}

/// The modifications the round-trip table asserts on: one row per kind of change
/// `ModifyVolume` documents — a size increase, a type change, an IOPS change, a
/// throughput change and a Multi-Attach change — across the volume types that admit each.
fn round_trip_fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            case: "size only, gp3",
            volume_type: "gp3",
            size: 100,
            iops: Some(3_000),
            throughput: Some(125),
            multi_attach: None,
            target_size: 200,
            target_volume_type: "gp3",
            target_iops: Some(3_000),
            target_throughput: Some(125),
            target_multi_attach: None,
            start_time_secs: 1_700_000_000,
        },
        Fixture {
            case: "type change gp2 -> gp3",
            volume_type: "gp2",
            size: 1,
            iops: None,
            throughput: None,
            multi_attach: None,
            target_size: 1,
            target_volume_type: "gp3",
            target_iops: Some(3_000),
            target_throughput: Some(125),
            target_multi_attach: None,
            start_time_secs: 1_700_000_100,
        },
        Fixture {
            case: "iops change, io1",
            volume_type: "io1",
            size: 4,
            iops: Some(100),
            throughput: None,
            multi_attach: Some(false),
            target_size: 4,
            target_volume_type: "io1",
            target_iops: Some(64_000),
            target_throughput: None,
            target_multi_attach: Some(false),
            start_time_secs: 1_700_000_200,
        },
        Fixture {
            case: "multi-attach enabled, io2",
            volume_type: "io2",
            size: 4,
            iops: Some(100),
            throughput: None,
            multi_attach: Some(false),
            target_size: 8,
            target_volume_type: "io2",
            target_iops: Some(100),
            target_throughput: None,
            target_multi_attach: Some(true),
            start_time_secs: 1_700_000_300,
        },
        Fixture {
            case: "throughput change at the top of the gp3 range",
            volume_type: "gp3",
            size: 1,
            iops: Some(80_000),
            throughput: Some(2_000),
            multi_attach: None,
            target_size: 65_536,
            target_volume_type: "gp3",
            target_iops: Some(80_000),
            target_throughput: Some(125),
            target_multi_attach: None,
            start_time_secs: 1_700_000_400,
        },
        Fixture {
            case: "type change st1 -> sc1 at the bottom of both ranges",
            volume_type: "st1",
            size: 125,
            iops: None,
            throughput: None,
            multi_attach: None,
            target_size: 500,
            target_volume_type: "sc1",
            target_iops: None,
            target_throughput: None,
            target_multi_attach: None,
            start_time_secs: 1_700_000_500,
        },
    ]
}

/// One `DescribeVolumesModifications` call carrying `params`, as its status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVolumesModifications", params).await
}

/// Create the volume `fixture` is a modification of, and answer the id it was minted with.
async fn create_volume(router: &MultiServiceRouter, fixture: &Fixture) -> String {
    let mut params = vec![
        p("AvailabilityZone", zone_name(0)),
        p("VolumeType", fixture.volume_type),
        p("Size", fixture.size.to_string()),
    ];
    if let Some(iops) = fixture.iops {
        params.push(p("Iops", iops.to_string()));
    }
    if let Some(throughput) = fixture.throughput {
        params.push(p("Throughput", throughput.to_string()));
    }
    if let Some(multi_attach) = fixture.multi_attach {
        params.push(p("MultiAttachEnabled", multi_attach.to_string()));
    }
    let (status, body) = call(router, REGION, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}",
        fixture.case
    );
    element(&body, "volumeId").unwrap_or_else(|| {
        panic!(
            "case {:?}: CreateVolume answered no volumeId: {body}",
            fixture.case
        )
    })
}

/// Store `fixture` as the volume's most recent modification, the way `ModifyVolume` would,
/// and answer the instant it started on.
fn seed_modification(
    engine: &engine::InMemoryEngine,
    volume_id: &str,
    fixture: &Fixture,
) -> Instant {
    let store = engine.state::<Ec2State>();
    let record = VolumeModificationRecord::starting_now(fixture.view(volume_id));
    let started_at = record.started_at;
    store
        .lock()
        .volume_modifications
        .insert(volume_id.to_string(), record);
    started_at
}

/// Create a volume for every fixture and seed its modification, answering the volume ids in
/// fixture order.
async fn seed_all(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    fixtures: &[Fixture],
) -> Vec<String> {
    let mut volume_ids = Vec::new();
    for fixture in fixtures {
        let volume_id = create_volume(router, fixture).await;
        seed_modification(engine, &volume_id, fixture);
        volume_ids.push(volume_id);
    }
    volume_ids
}

/// The volume ids a successful report names, in the order it reported them.
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
    elements(&body, "volumeId")
}

/// Sleep until `span` has passed since `started_at`, however long the assertions before it
/// took, so a clock-owed move is observed on the service's own schedule rather than on the
/// test's.
async fn sleep_until(started_at: Instant, span: Duration) {
    let elapsed = started_at.elapsed();
    if let Some(remaining) = span.checked_sub(elapsed) {
        tokio::time::sleep(remaining).await;
    }
}

// ---------------------------------------------------------------------------
// RULE 2: no modification is a refusal, not an empty answer
// ---------------------------------------------------------------------------

/// A volume nothing has modified has no modification to describe: naming it is refused
/// with `InvalidVolumeModification.NotFound`, whatever kind of volume it is — while a
/// request that names no volume reports the account's modifications, of which a fresh
/// account has none.
#[tokio::test]
async fn a_volume_with_no_modification_is_refused_rather_than_reported_empty() {
    let (router, _engine) = super::ec2_query::emulator();

    // An account-wide report is a successful empty answer: there is no volume named for a
    // missing modification to be missing FOR.
    for read in 1..=2 {
        let (status, body) = describe(&router, &[]).await;
        assert_eq!(
            status, 200,
            "read {read}: expected HTTP 200 for an account with no modification, \
             got {status} with body {body}"
        );
        assert!(
            elements(&body, "volumeId").is_empty(),
            "read {read}: expected no modification reported, got {body}"
        );
        assert!(
            element(&body, "nextToken").is_none(),
            "read {read}: nothing follows an empty report, so no token is due, got {body}"
        );
    }

    // Every row is a freshly created volume of a different type and size — the size at the
    // bottom of that type's published range, and a type from each family — so the refusal
    // is not a property of one volume shape.
    for fixture in round_trip_fixtures() {
        let volume_id = create_volume(&router, &fixture).await;
        let (status, body) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for a volume with no modification, \
             got {status} with body {body}",
            fixture.case
        );
        assert_eq!(
            error_code(&body),
            INVALID_VOLUME_MODIFICATION_NOT_FOUND,
            "case {:?}: expected {INVALID_VOLUME_MODIFICATION_NOT_FOUND}, got body {body}",
            fixture.case
        );
        // The refusal is built from the request: it names the volume that was asked about,
        // so the caller can tell which of the ids it named has no modification.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(&volume_id),
            "case {:?}: the refusal names the volume asked about, got message {message:?}",
            fixture.case
        );
        // The volume itself is still there and still reported: the refusal is about the
        // modification, not about the volume.
        let (volume_status, volume_body) =
            call(&router, REGION, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
        assert_eq!(
            volume_status, 200,
            "case {:?}: the volume itself still resolves, got {volume_status} / {volume_body}",
            fixture.case
        );
    }
}

/// A request naming several volumes is refused as soon as ONE of them carries no
/// modification: an answer reporting the others would claim the named volume had none to
/// report, which is a different statement.
#[tokio::test]
async fn one_unmodified_volume_refuses_a_request_that_names_several() {
    let (router, engine) = super::ec2_query::emulator();
    let fixtures = round_trip_fixtures();
    let modified = seed_all(&router, &engine, &fixtures[..2]).await;
    let unmodified = create_volume(&router, &fixtures[2]).await;

    let mut named: Vec<&str> = modified.iter().map(String::as_str).collect();
    named.push(unmodified.as_str());
    let (status, body) = describe(&router, &indexed("VolumeId", &named)).await;
    assert_eq!(
        status, 400,
        "case \"two modified and one not\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_VOLUME_MODIFICATION_NOT_FOUND,
        "case \"two modified and one not\": expected \
         {INVALID_VOLUME_MODIFICATION_NOT_FOUND}, got body {body}"
    );

    // Without the unmodified volume the same request succeeds, so the refusal above is
    // caused by that volume and nothing else.
    assert_eq!(
        described_ids(
            &router,
            "the modified volumes only",
            &indexed("VolumeId", &modified.iter().map(String::as_str).collect::<Vec<_>>())
        )
        .await,
        modified,
        "case \"the modified volumes only\": expected both modifications reported"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: an id that resolves to no volume is refused first
// ---------------------------------------------------------------------------

/// A volume id that cannot be resolved is answered on the id, before any modification is
/// looked for: something that is not a volume id at all is malformed, a well-formed id the
/// account holds no volume for does not exist.
#[tokio::test]
async fn an_unresolvable_volume_id_is_refused_before_its_modification() {
    let (router, _engine) = super::ec2_query::emulator();

    // Every row is a different way of being unresolvable: the short and the long id form
    // for the unknown case, and for the malformed case a wrong prefix, a wrong length,
    // non-hex digits, upper-case hex and no prefix at all.
    for (case, volume_id, expected) in [
        ("unknown, long form", "vol-0123456789abcdef0", INVALID_VOLUME_ID_NOT_FOUND),
        ("unknown, short form", "vol-0123abcd", INVALID_VOLUME_ID_NOT_FOUND),
        ("another resource's id", "i-0123456789abcdef0", INVALID_VOLUME_ID_MALFORMED),
        ("one hex digit short", "vol-0123abc", INVALID_VOLUME_ID_MALFORMED),
        ("not hexadecimal", "vol-zzzzzzzz", INVALID_VOLUME_ID_MALFORMED),
        ("upper-case hex", "vol-0123ABCD", INVALID_VOLUME_ID_MALFORMED),
        ("no prefix", "0123456789abcdef0", INVALID_VOLUME_ID_MALFORMED),
        ("empty", "", INVALID_VOLUME_ID_MALFORMED),
    ] {
        let (status, body) = describe(&router, &indexed("VolumeId", &[volume_id])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: every member of a stored modification round-trips
// ---------------------------------------------------------------------------

/// What the store holds is what the describe reports: for every kind of change, each
/// original and target member comes back as it was stored, the `original` members agree
/// with the volume `DescribeVolumes` reports, and a member the modification does not carry
/// is reported not at all rather than as a zero.
#[tokio::test]
async fn a_stored_modification_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();

    for fixture in round_trip_fixtures() {
        let case = fixture.case;
        let volume_id = create_volume(&router, &fixture).await;
        seed_modification(&engine, &volume_id, &fixture);

        let (status, body) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "volumeId").as_deref(),
            Some(volume_id.as_str()),
            "case {case:?}: expected the modification of {volume_id}, got {body}"
        );
        for (member, expected) in [
            ("originalSize", Some(fixture.size.to_string())),
            (
                "originalVolumeType",
                Some(fixture.volume_type.to_string()),
            ),
            ("originalIops", fixture.iops.map(|iops| iops.to_string())),
            (
                "originalThroughput",
                fixture.throughput.map(|value| value.to_string()),
            ),
            (
                "originalMultiAttachEnabled",
                fixture.multi_attach.map(|value| value.to_string()),
            ),
            ("targetSize", Some(fixture.target_size.to_string())),
            (
                "targetVolumeType",
                Some(fixture.target_volume_type.to_string()),
            ),
            (
                "targetIops",
                fixture.target_iops.map(|iops| iops.to_string()),
            ),
            (
                "targetThroughput",
                fixture.target_throughput.map(|value| value.to_string()),
            ),
            (
                "targetMultiAttachEnabled",
                fixture.target_multi_attach.map(|value| value.to_string()),
            ),
        ] {
            assert_eq!(
                element(&body, member),
                expected,
                "case {case:?}: member {member} of the report, body {body}"
            );
        }
        assert!(
            element(&body, "startTime").is_some(),
            "case {case:?}: a modification reports the time it started, got {body}"
        );
        assert!(
            element(&body, "statusMessage").is_none(),
            "case {case:?}: nothing set a status message, so none is reported, got {body}"
        );

        // The `original` members describe the volume itself, not a literal: the volume
        // `DescribeVolumes` reports carries exactly them.
        let (_, volume) =
            call(&router, REGION, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
        for (modification_member, volume_member) in [
            ("originalSize", "size"),
            ("originalVolumeType", "volumeType"),
            ("originalIops", "iops"),
            ("originalThroughput", "throughput"),
        ] {
            assert_eq!(
                element(&body, modification_member),
                element(&volume, volume_member),
                "case {case:?}: {modification_member} must be the volume's own \
                 {volume_member}, volume body {volume}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: the clock finishes a modification, and the describe is a read
// ---------------------------------------------------------------------------

/// A modification starts `modifying` and the CLOCK moves it to `optimizing` and then to
/// `completed`: no caller does it, every read of the same instant answers the same, and the
/// end time appears only once the modification has finished.
#[tokio::test]
async fn the_clock_moves_a_modification_through_optimizing_to_completed() {
    let (router, engine) = super::ec2_query::emulator();
    let fixture = round_trip_fixtures()
        .into_iter()
        .next()
        .expect("at least one fixture");
    let volume_id = create_volume(&router, &fixture).await;
    let started_at = seed_modification(&engine, &volume_id, &fixture);

    // Straight after the call that starts it, the modification is still `modifying`: the
    // span the clock owes is never zero, so a caller that reads immediately sees the
    // transient state.
    let (status, body) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
    assert_eq!(
        status, 200,
        "case \"straight after the start\": expected HTTP 200, got {status} / {body}"
    );
    assert_eq!(
        element(&body, "modificationState").as_deref(),
        Some(VOLUME_MODIFICATION_STATE_MODIFYING),
        "case \"straight after the start\": expected {VOLUME_MODIFICATION_STATE_MODIFYING}, \
         got {body}"
    );
    assert!(
        element(&body, "endTime").is_none(),
        "case \"straight after the start\": an unfinished modification has no end time, \
         got {body}"
    );
    let progress: i64 = element(&body, "progress")
        .unwrap_or_default()
        .parse()
        .unwrap_or(-1);
    assert!(
        (0..VOLUME_MODIFICATION_PROGRESS_COMPLETE).contains(&progress),
        "case \"straight after the start\": expected progress in \
         0..{VOLUME_MODIFICATION_PROGRESS_COMPLETE}, got {progress} in {body}"
    );

    // One span later it is `optimizing` — the move fell due without anybody asking for it.
    sleep_until(started_at, TRANSITION_DELAY + MARGIN).await;
    let (status, body) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
    assert_eq!(
        status, 200,
        "case \"one span later\": expected HTTP 200, got {status} / {body}"
    );
    assert_eq!(
        element(&body, "modificationState").as_deref(),
        Some(VOLUME_MODIFICATION_STATE_OPTIMIZING),
        "case \"one span later\": expected {VOLUME_MODIFICATION_STATE_OPTIMIZING}, got {body}"
    );
    assert!(
        element(&body, "endTime").is_none(),
        "case \"one span later\": an optimizing modification has not finished, got {body}"
    );

    // One span after that it is `completed`, at full progress and with the end time it
    // finished on — and reading it twice answers the same, a read being a read.
    sleep_until(started_at, TRANSITION_DELAY * 2 + MARGIN).await;
    let mut reports = Vec::new();
    for read in 1..=2 {
        let (status, body) = describe(&router, &indexed("VolumeId", &[&volume_id])).await;
        assert_eq!(
            status, 200,
            "read {read}: expected HTTP 200, got {status} / {body}"
        );
        assert_eq!(
            element(&body, "modificationState").as_deref(),
            Some(VOLUME_MODIFICATION_STATE_COMPLETED),
            "read {read}: expected {VOLUME_MODIFICATION_STATE_COMPLETED}, got {body}"
        );
        assert_eq!(
            element(&body, "progress").as_deref(),
            Some(VOLUME_MODIFICATION_PROGRESS_COMPLETE.to_string().as_str()),
            "read {read}: a completed modification is {VOLUME_MODIFICATION_PROGRESS_COMPLETE} \
             percent complete, got {body}"
        );
        reports.push((
            element(&body, "endTime"),
            element(&body, "startTime"),
            element(&body, "targetSize"),
        ));
    }
    assert!(
        reports[0].0.is_some(),
        "case \"completed\": a finished modification reports the time it finished"
    );
    assert_eq!(
        reports[0], reports[1],
        "case \"completed\": two reads of a finished modification answer the same"
    );
}

// ---------------------------------------------------------------------------
// RULE 1: narrowing by id and by every documented filter
// ---------------------------------------------------------------------------

/// The modifications the filter table is run against: three volumes whose original and
/// target values differ along every attribute a documented filter selects on.
fn filter_fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            case: "gp2 grown and converted to gp3",
            volume_type: "gp2",
            size: 100,
            iops: None,
            throughput: None,
            multi_attach: None,
            target_size: 200,
            target_volume_type: "gp3",
            target_iops: Some(3_000),
            target_throughput: Some(125),
            target_multi_attach: None,
            start_time_secs: 1_700_000_000,
        },
        Fixture {
            case: "io1 given more IOPS and Multi-Attach",
            volume_type: "io1",
            size: 4,
            iops: Some(100),
            throughput: None,
            multi_attach: Some(false),
            target_size: 8,
            target_volume_type: "io1",
            target_iops: Some(200),
            target_throughput: None,
            target_multi_attach: Some(true),
            start_time_secs: 1_700_000_100,
        },
        Fixture {
            case: "st1 grown",
            volume_type: "st1",
            size: 125,
            iops: None,
            throughput: None,
            multi_attach: None,
            target_size: 500,
            target_volume_type: "st1",
            target_iops: None,
            target_throughput: None,
            target_multi_attach: None,
            start_time_secs: 1_700_000_200,
        },
    ]
}

/// The report is the store narrowed by `VolumeIds` and by each documented filter, with
/// multiple values on one filter joined with OR and multiple filters with AND; an
/// undocumented filter name is refused rather than answered as "nothing matched".
#[tokio::test]
async fn the_report_is_narrowed_by_id_and_by_every_documented_filter() {
    let (router, engine) = super::ec2_query::emulator();
    let fixtures = filter_fixtures();
    let volume_ids = seed_all(&router, &engine, &fixtures).await;
    let started_at = Instant::now();
    // Past both clock-owed moves, so every seeded modification sits in the terminal
    // `completed` state and the `modification-state` rows below are about the filter rather
    // than about the clock.
    sleep_until(started_at, TRANSITION_DELAY * 2 + MARGIN).await;

    let all: Vec<String> = volume_ids.clone();
    let id = |index: usize| volume_ids[index].clone();

    // No narrowing: every stored modification, in volume-id order.
    let mut in_id_order = all.clone();
    in_id_order.sort();
    assert_eq!(
        described_ids(&router, "no narrowing", &[]).await,
        in_id_order,
        "case \"no narrowing\": expected every stored modification in volume-id order"
    );

    // `VolumeIds` narrows to the ids named, however many are named.
    for count in 1..=all.len() {
        let wanted: Vec<&str> = all.iter().take(count).map(String::as_str).collect();
        let case = format!("{count} of {} ids", all.len());
        let mut expected: Vec<String> = wanted.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            described_ids(&router, &case, &indexed("VolumeId", &wanted)).await,
            expected,
            "case {case:?}: expected exactly the modifications of the ids named"
        );
    }

    // One row per documented filter name, plus a row for the OR within one filter, a row
    // for the AND across two, a wildcard row and a row for a filter carrying no value at
    // all.
    let filter_rows: Vec<FilterRow> = vec![
        (
            "modification-state, the state they are all in",
            vec![("modification-state", &["completed"][..])],
            in_id_order.clone(),
        ),
        (
            "modification-state, a state none is in",
            vec![("modification-state", &["modifying"][..])],
            Vec::new(),
        ),
        (
            "modification-state, by wildcard",
            vec![("modification-state", &["comple*"][..])],
            in_id_order.clone(),
        ),
        (
            "original-iops",
            vec![("original-iops", &["100"][..])],
            vec![id(1)],
        ),
        (
            "original-iops, a value no modification carries",
            vec![("original-iops", &["3000"][..])],
            Vec::new(),
        ),
        (
            "original-size",
            vec![("original-size", &["125"][..])],
            vec![id(2)],
        ),
        (
            "original-volume-type",
            vec![("original-volume-type", &["io1"][..])],
            vec![id(1)],
        ),
        (
            "original-volume-type, two values joined with OR",
            vec![("original-volume-type", &["gp2", "st1"][..])],
            vec![id(0), id(2)],
        ),
        (
            "originalMultiAttachEnabled, which only an io1 volume carries",
            vec![("originalMultiAttachEnabled", &["false"][..])],
            vec![id(1)],
        ),
        (
            "target-iops",
            vec![("target-iops", &["200"][..])],
            vec![id(1)],
        ),
        (
            "target-size",
            vec![("target-size", &["200"][..])],
            vec![id(0)],
        ),
        (
            "target-volume-type",
            vec![("target-volume-type", &["gp3"][..])],
            vec![id(0)],
        ),
        (
            "targetMultiAttachEnabled",
            vec![("targetMultiAttachEnabled", &["true"][..])],
            vec![id(1)],
        ),
        (
            "volume-id, by wildcard",
            vec![("volume-id", &["vol-*"][..])],
            in_id_order.clone(),
        ),
        (
            "two filters joined with AND",
            vec![
                ("original-volume-type", &["io1"][..]),
                ("target-size", &["8"][..]),
            ],
            vec![id(1)],
        ),
        (
            "two filters joined with AND that no modification satisfies together",
            vec![
                ("original-volume-type", &["io1"][..]),
                ("target-size", &["200"][..]),
            ],
            Vec::new(),
        ),
        (
            "a filter carrying no value at all",
            vec![("target-size", &[][..])],
            Vec::new(),
        ),
    ];
    for (case, entries, mut expected) in filter_rows {
        expected.sort();
        let params = filters(&entries);
        assert_eq!(
            described_ids(&router, case, &params).await,
            expected,
            "case {case:?}: expected exactly the modifications the filter selects"
        );
    }

    // `volume-id` selects on the id itself, which is minted at run time rather than being
    // a literal of the table above.
    assert_eq!(
        described_ids(
            &router,
            "volume-id",
            &filters(&[("volume-id", &[volume_ids[2].as_str()])])
        )
        .await,
        vec![volume_ids[2].clone()],
        "case \"volume-id\": the id of one modification selects exactly that one"
    );

    // `start-time` selects on the time the modification started, matched against the value
    // the report itself carries for it.
    let (_, body) = describe(&router, &indexed("VolumeId", &[volume_ids[1].as_str()])).await;
    let start_time = element(&body, "startTime")
        .unwrap_or_else(|| panic!("a modification reports its start time: {body}"));
    assert_eq!(
        described_ids(
            &router,
            "start-time",
            &filters(&[("start-time", &[start_time.as_str()])])
        )
        .await,
        vec![volume_ids[1].clone()],
        "case \"start-time\": the reported start time selects that modification"
    );

    // A filter name this operation does not document is refused, so an unsupported filter
    // can never read as "nothing matched".
    for name in [
        "modification_state",
        "Modification-State",
        "original-throughput",
        "originalmultiattachenabled",
        "tag:Name",
        "tag-key",
        "",
    ] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an undocumented filter, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: paging
// ---------------------------------------------------------------------------

/// The report is cut into `MaxResults`-sized pages a `NextToken` walks exactly once, a
/// token this service did not mint is refused, and a page size outside what the operation
/// documents is refused.
#[tokio::test]
async fn the_report_is_cut_into_pages() {
    let (router, engine) = super::ec2_query::emulator();
    let fixtures = filter_fixtures();
    let mut expected = seed_all(&router, &engine, &fixtures).await;
    expected.sort();

    for page_size in [1usize, 2, expected.len(), expected.len() + 1] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next_token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "volumeId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults modifications, got {}",
                page.len()
            );
            walked.extend(page);
            next_token = element(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: paging walks every modification exactly once, in id order"
        );
    }

    // A token this service did not issue is refused rather than read as a cursor.
    for token in ["not-a-token", "00", ""] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got body {body}"
        );
    }

    // "The maximum number of results (up to a limit of 500)": a page size of zero, a
    // negative one, and one past that documented ceiling are refused; the ceiling itself is
    // accepted.
    for (case, max_results, expected_status) in [
        ("zero", "0", 400),
        ("negative", "-1", 400),
        ("the documented ceiling", "500", 200),
        ("past the documented ceiling", "501", 400),
    ] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults={max_results}, \
             got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got body {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` answers whether the request WOULD have succeeded: a request that would have
/// been served is refused with `DryRunOperation`, and one that would have been refused is
/// refused with its own reason instead.
#[tokio::test]
async fn dry_run_reports_whether_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    let fixtures = filter_fixtures();
    let modified = seed_all(&router, &engine, &fixtures[..1]).await;
    let unmodified = create_volume(&router, &fixtures[1]).await;

    for (case, params) in [
        ("the whole account", Vec::new()),
        (
            "a volume that has a modification",
            indexed("VolumeId", &[modified[0].as_str()]),
        ),
    ] {
        let mut with_dry_run = params.clone();
        with_dry_run.push(p("DryRun", "true"));
        let (status, body) = describe(&router, &with_dry_run).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 for a DryRun that would have succeeded, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got body {body}"
        );
    }

    for (case, mut params, expected) in [
        (
            "a volume with no modification",
            indexed("VolumeId", &[unmodified.as_str()]),
            INVALID_VOLUME_MODIFICATION_NOT_FOUND,
        ),
        (
            "an unknown volume",
            indexed("VolumeId", &["vol-0123456789abcdef0"]),
            INVALID_VOLUME_ID_NOT_FOUND,
        ),
        (
            "a malformed volume id",
            indexed("VolumeId", &["banana"]),
            INVALID_VOLUME_ID_MALFORMED,
        ),
    ] {
        params.push(p("DryRun", "true"));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: a DryRun that would have been refused answers {expected}, \
             got body {body}"
        );
    }
}
