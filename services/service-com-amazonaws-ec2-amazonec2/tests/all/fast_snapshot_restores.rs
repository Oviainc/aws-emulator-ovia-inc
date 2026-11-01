// SPDX-License-Identifier: CC0-1.0

//! What `DisableFastSnapshotRestores` answers for each (snapshot, Availability Zone) pair a
//! request names, and the values it refuses — end to end over the wire the service actually
//! speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DisableFastSnapshotRestores`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * A fast snapshot restore is a property of a (snapshot, zone) PAIR, so every pair the
//!   request names is answered on its own: the pair's new state in `Successful`, or that
//!   pair's error under its snapshot id in `Unsuccessful`. A snapshot the call cannot
//!   resolve therefore never refuses the snapshots beside it — the whole call still answers
//!   HTTP 200, with the per-pair channel carrying the cause. Every pair of this suite names
//!   a snapshot the account does NOT hold (which is why `CreateVolume` refuses the same id
//!   too), so every pair lands in `Unsuccessful` and `Successful` comes back EMPTY; what a
//!   pair naming a STORED snapshot answers is covered by
//!   [`super::fast_snapshot_restore_lifecycle`].
//! * ONE classification of an unresolvable snapshot id, whichever channel carries it:
//!   `InvalidSnapshotID.Malformed` for a value that is not a snapshot id EC2 could have
//!   ISSUED — including the all-zero placeholder, which the recorded
//!   `DisableFastSnapshotRestores` call this suite comes from was answered that way — and
//!   `InvalidSnapshot.NotFound` for one that is. The table below asserts the per-pair code
//!   and `CreateVolume`'s request-level code for the SAME id agree.
//! * The answer reports each pair by the identifier the request named its zones with:
//!   `availabilityZone` for a request that used `AvailabilityZone.N`, `availabilityZoneId`
//!   for one that used `AvailabilityZoneId.N` — "either AvailabilityZone or
//!   AvailabilityZoneId must be specified in the request, but not both".
//! * A request the service cannot read at all is refused outright, with the EC2 code for
//!   what is wrong with it, and never with a 501.
//!
//! So the tables span those classes rather than the one recorded request: every id form
//! (both published lengths, the null id, a non-hexadecimal suffix, another resource's
//! prefix, upper case, the bare prefix), both zone forms, one and several zones, one and
//! several snapshots, repeats of each, and every documented refusal.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::availability_zones_in_region;

use super::ec2_query::{element, error_code, indexed};
use super::wire::{Param, child, child_text, document, param, set_items};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// EC2's code for a snapshot id the account holds no snapshot for.
const NOT_FOUND: &str = ec2::state::INVALID_SNAPSHOT_ID_NOT_FOUND;

/// EC2's code for a value that is not a snapshot id EC2 could have issued.
const MALFORMED: &str = ec2::state::INVALID_SNAPSHOT_ID_MALFORMED;

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The `zone_name` of zone `index` of the signed Region.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// The `zone_id` of zone `index` of the signed Region.
fn zone_id(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_id.clone()
}

/// Which of the two identifiers a request names its Availability Zones by — the choice the
/// operation's own documentation forces ("either AvailabilityZone or AvailabilityZoneId
/// must be specified in the request, but not both") and the one the answer mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    /// `AvailabilityZone.N`, carrying zone NAMES.
    Name,
    /// `AvailabilityZoneId.N`, carrying zone IDS.
    Id,
}

impl By {
    /// The ec2Query form-field prefix this form flattens its list into.
    fn field(self) -> &'static str {
        match self {
            // ec2Query spells the list by its member's `xmlName`, not by the member name.
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
}

/// One error a response reported for one (snapshot, zone) pair.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PairError {
    /// The `snapshotId` of the `unsuccessful` item this error sits under.
    snapshot_id: String,
    /// The pair's `availabilityZone`, when the response reported one.
    availability_zone: Option<String>,
    /// The pair's `availabilityZoneId`, when the response reported one.
    availability_zone_id: Option<String>,
    /// The error `code`.
    code: String,
    /// The error `message`.
    message: String,
}

/// One `DisableFastSnapshotRestores` call naming `snapshot_ids` and the zones at `zones`
/// (indices into the signed Region's zone list) in the `by` form, with `extra` form fields.
async fn disable(
    router: &MultiServiceRouter,
    snapshot_ids: &[&str],
    by: By,
    zones: &[usize],
    extra: &[Param],
) -> (u16, String) {
    let mut params = indexed("SourceSnapshotId", snapshot_ids);
    let zone_values: Vec<String> = zones.iter().map(|index| by.zone(*index)).collect();
    let zone_refs: Vec<&str> = zone_values.iter().map(String::as_str).collect();
    params.extend(indexed(by.field(), &zone_refs));
    params.extend_from_slice(extra);
    super::ec2_query::call(router, REGION, "DisableFastSnapshotRestores", &params).await
}

/// The per-pair errors a response carries, in document order, read through the document so
/// a per-pair `<code>` is never confused with an error envelope's `<Code>`.
fn pair_errors(case: &str, body: &str) -> Vec<PairError> {
    let doc = document(case, body);
    let root = doc.root_element();
    let mut errors = Vec::new();
    for item in set_items(root, "unsuccessful") {
        let snapshot_id = child_text(item, "snapshotId").unwrap_or_default();
        for pair in set_items(item, "fastSnapshotRestoreStateErrorSet") {
            let error = child(pair, "error");
            errors.push(PairError {
                snapshot_id: snapshot_id.clone(),
                availability_zone: child_text(pair, "availabilityZone"),
                availability_zone_id: child_text(pair, "availabilityZoneId"),
                code: error
                    .and_then(|error| child_text(error, "code"))
                    .unwrap_or_default(),
                message: error
                    .and_then(|error| child_text(error, "message"))
                    .unwrap_or_default(),
            });
        }
    }
    errors
}

/// Assert that a response reports the `successful` set EMPTY rather than omitting it: the
/// call did run, and disabled nothing.
fn assert_no_successes(case: &str, body: &str) {
    let doc = document(case, body);
    let root = doc.root_element();
    assert!(
        child(root, "successful").is_some(),
        "case {case:?}: the response must carry a successful set, got body {body}"
    );
    let successes = set_items(root, "successful");
    assert!(
        successes.is_empty(),
        "case {case:?}: expected no successful pair, got {count} in body {body}",
        count = successes.len()
    );
}

/// The per-pair errors of a call required to answer HTTP 200 with no successes.
async fn errors_of(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_ids: &[&str],
    by: By,
    zones: &[usize],
) -> Vec<PairError> {
    let (status, body) = disable(router, snapshot_ids, by, zones, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a snapshot the call cannot resolve is a per-pair failure, so the \
         call answers HTTP 200; got {status} with body {body}"
    );
    assert_no_successes(case, &body);
    pair_errors(case, &body)
}

// ---------------------------------------------------------------------------
// The per-pair channel
// ---------------------------------------------------------------------------

/// Every snapshot id form, with the EC2 code the service answers it with.
///
/// The forms come from the published snapshot id shape — `snap-` and 8 or 17 lowercase
/// hexadecimal digits, of which not all are `0`
/// (`ec2::state::snapshot_id_is_well_formed`) — so a row is either an id EC2 could have
/// issued, which the account simply holds no snapshot for, or a value that is not one.
/// None of these is the id the issue cited, except the all-zero row that IS it: the row
/// that pins the null id's own classification.
fn id_forms() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        // (case, the id, the code it is answered with)
        ("the long 17-digit form", "snap-0123456789abcdef0", NOT_FOUND),
        ("the short 8-digit form", "snap-0a1b2c3d", NOT_FOUND),
        (
            "a long form whose leading digit is above zero",
            "snap-1234567890abcdef0",
            NOT_FOUND,
        ),
        ("every digit zero", "snap-00000000000000000", MALFORMED),
        ("every digit zero, short form", "snap-00000000", MALFORMED),
        ("a suffix that is not hexadecimal", "snap-not-hex-at-all", MALFORMED),
        ("an upper-case digit", "snap-0123456789ABCDEF0", MALFORMED),
        ("a suffix of neither length", "snap-0123456789", MALFORMED),
        ("the prefix alone", "snap-", MALFORMED),
        ("another resource's prefix", "vol-0123456789abcdef0", MALFORMED),
        ("no prefix at all", "not-a-snapshot", MALFORMED),
    ]
}

/// RULE: a snapshot id the call cannot resolve is reported in `Unsuccessful` under that id,
/// with one error per requested zone — never as a refusal of the whole call — and the code
/// is `InvalidSnapshotID.Malformed` for a value that is not a snapshot id EC2 could have
/// issued and `InvalidSnapshot.NotFound` for one that is.
///
/// The rows are every id form, so the classification is asserted over the class rather than
/// on the one recorded id.
#[tokio::test]
async fn an_unresolvable_snapshot_is_reported_per_pair_with_its_own_code() {
    let router = emulator();
    for (case, snapshot_id, expected_code) in id_forms() {
        let errors = errors_of(&router, case, &[snapshot_id], By::Name, &[0]).await;
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected one error for one snapshot in one zone, got {errors:?}"
        );
        let error = &errors[0];
        assert_eq!(
            error.snapshot_id, snapshot_id,
            "case {case:?}: the item must be reported under the id the request named, \
             got {error:?}"
        );
        assert_eq!(
            error.code, expected_code,
            "case {case:?}: expected code {expected_code:?}, got {error:?}"
        );
        assert!(
            error.message.contains(snapshot_id),
            "case {case:?}: the message must name the value the request gave, got {error:?}"
        );
    }
}

/// RULE: the per-pair code for a snapshot id is the SAME code the service answers that id
/// with where it refuses a whole request for it — one classification per cause, whichever
/// channel carries it.
///
/// `CreateVolume` is the other channel: it refuses a `SnapshotId` outright, because this
/// emulator holds no snapshot either. The rows are the same id forms as above.
#[tokio::test]
async fn the_per_pair_code_is_the_code_a_request_level_refusal_uses() {
    let router = emulator();
    for (case, snapshot_id, _) in id_forms() {
        let errors = errors_of(&router, case, &[snapshot_id], By::Name, &[0]).await;
        let per_pair = errors
            .first()
            .map(|error| error.code.clone())
            .unwrap_or_default();

        let params = vec![
            param("AvailabilityZone", &zone_name(0)),
            param("SnapshotId", snapshot_id),
        ];
        let (status, body) = super::ec2_query::call(&router, REGION, "CreateVolume", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateVolume must refuse an unresolvable SnapshotId with HTTP \
             400, got {status} with body {body}"
        );
        let request_level = error_code(&body);
        assert_eq!(
            per_pair, request_level,
            "case {case:?}: the per-pair code and the request-level code for one id must \
             agree, got {per_pair:?} against {request_level:?}"
        );
    }
}

/// One row of the pair-counting table: the case, the snapshot ids the request names, the
/// zone indices it names, the distinct ids expected back in order, and how many distinct
/// zones each of them must carry.
type PairCountCase = (
    &'static str,
    Vec<&'static str>,
    Vec<usize>,
    Vec<&'static str>,
    usize,
);

/// RULE: every (snapshot, zone) pair the request names is answered, once each — one
/// `Unsuccessful` item per distinct snapshot id in request order, carrying one error per
/// distinct zone.
///
/// The rows differ along the counts the rule generalises over: one snapshot and one zone,
/// several snapshots, several zones, both at once, and repeats of each, which collapse
/// because one pair gets one answer however many times it is named.
#[tokio::test]
async fn every_named_pair_is_answered_once() {
    let router = emulator();
    let long = "snap-0123456789abcdef0";
    let short = "snap-0a1b2c3d";
    let null = "snap-00000000000000000";

    let cases: Vec<PairCountCase> = vec![
        ("one snapshot, one zone", vec![long], vec![0], vec![long], 1),
        (
            "three snapshots, one zone",
            vec![long, short, null],
            vec![0],
            vec![long, short, null],
            1,
        ),
        (
            "one snapshot, three zones",
            vec![long],
            vec![0, 1, 2],
            vec![long],
            3,
        ),
        (
            "two snapshots, two zones",
            vec![short, null],
            vec![1, 2],
            vec![short, null],
            2,
        ),
        (
            "a repeated snapshot id",
            vec![long, long, long],
            vec![0],
            vec![long],
            1,
        ),
        (
            "a repeated zone",
            vec![long],
            vec![2, 2, 0, 2],
            vec![long],
            2,
        ),
        (
            "repeats of both",
            vec![null, long, null],
            vec![1, 1],
            vec![null, long],
            1,
        ),
    ];

    for (case, ids, zones, expected_ids, expected_zones) in cases {
        let errors = errors_of(&router, case, &ids, By::Name, &zones).await;
        assert_eq!(
            errors.len(),
            expected_ids.len() * expected_zones,
            "case {case:?}: expected {expected} errors ({ids} ids x {zones} zones), \
             got {errors:?}",
            expected = expected_ids.len() * expected_zones,
            ids = expected_ids.len(),
            zones = expected_zones
        );

        // One item per distinct id, in request order.
        let mut reported_ids: Vec<String> = Vec::new();
        for error in &errors {
            if reported_ids.last() != Some(&error.snapshot_id) {
                reported_ids.push(error.snapshot_id.clone());
            }
        }
        assert_eq!(
            reported_ids, expected_ids,
            "case {case:?}: expected one item per distinct id in request order, got \
             {reported_ids:?}"
        );

        // One error per distinct zone, under every id.
        for id in &expected_ids {
            let mut zones_for_id: Vec<Option<String>> = errors
                .iter()
                .filter(|error| error.snapshot_id == *id)
                .map(|error| error.availability_zone.clone())
                .collect();
            let reported = zones_for_id.len();
            zones_for_id.sort_unstable();
            zones_for_id.dedup();
            assert_eq!(
                zones_for_id.len(),
                reported,
                "case {case:?}: id {id:?} must carry each zone once, got {errors:?}"
            );
            assert_eq!(
                reported, expected_zones,
                "case {case:?}: id {id:?} must carry {expected_zones} zones, got {errors:?}"
            );
        }
    }
}

/// RULE: the answer reports each pair by the identifier the REQUEST named its zones with —
/// `availabilityZone` for a request that named zone names, `availabilityZoneId` for one
/// that named zone ids — and reports no second identifier the request never used.
///
/// The rows are both forms, over several zones, so neither form is read off the other.
#[tokio::test]
async fn each_pair_is_reported_by_the_zone_identifier_the_request_used() {
    let router = emulator();
    let snapshot = "snap-0123456789abcdef0";
    for by in [By::Name, By::Id] {
        let case = format!("named by {by:?}");
        let zones = [0, 1];
        let errors = errors_of(&router, &case, &[snapshot], by, &zones).await;
        let expected: Vec<String> = zones.iter().map(|index| by.zone(*index)).collect();

        let (reported, absent): (Vec<Option<String>>, Vec<Option<String>>) = match by {
            By::Name => (
                errors.iter().map(|e| e.availability_zone.clone()).collect(),
                errors
                    .iter()
                    .map(|e| e.availability_zone_id.clone())
                    .collect(),
            ),
            By::Id => (
                errors
                    .iter()
                    .map(|e| e.availability_zone_id.clone())
                    .collect(),
                errors.iter().map(|e| e.availability_zone.clone()).collect(),
            ),
        };
        let reported: Vec<String> = reported.into_iter().flatten().collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the zones the request named, got {errors:?}"
        );
        assert!(
            absent.iter().all(Option::is_none),
            "case {case:?}: the other zone identifier must not be reported, got {errors:?}"
        );
    }
}

/// RULE: the call reports and never changes what a later caller sees — it disabled nothing,
/// so repeating it answers identically.
///
/// The rows are repeated calls, each asserted against the first one's answer.
#[tokio::test]
async fn repeating_the_call_answers_identically() {
    /// Calls per case — enough to catch an answer that drifts with a counter.
    const CALLS: usize = 4;

    let router = emulator();
    let ids = ["snap-0123456789abcdef0", "snap-00000000000000000"];
    let first = errors_of(&router, "the first call", &ids, By::Name, &[0, 1]).await;
    for call in 2..=CALLS {
        let case = format!("call {call}");
        let again = errors_of(&router, &case, &ids, By::Name, &[0, 1]).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected the same answer as the first call, {first:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// RULE: a request the service cannot read is refused outright with the EC2 code for what
/// is wrong with it — never with a 501, which would say the operation is unimplemented, and
/// never through the per-pair channel, which is about the pairs rather than the request.
///
/// The rows differ along what makes the request unreadable: no snapshot named, neither zone
/// form named, both named, a zone name and a zone id the Region does not have, and a
/// `DryRun` of a request that would otherwise have run.
#[tokio::test]
async fn an_unreadable_request_is_refused_with_the_ec2_code_for_it() {
    let router = emulator();
    let snapshot = param("SourceSnapshotId.1", "snap-0123456789abcdef0");

    // (case, the form fields, the HTTP status, the EC2 error code)
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            // `SourceSnapshotIds` is `@required`, so the MODEL boundary refuses a request
            // that names none before the handler runs; the handler's own
            // `MissingParameter` is what an empty list would get, which ec2Query cannot
            // express (an empty list is an omitted member).
            "no snapshot named",
            vec![param("AvailabilityZone.1", &zone_name(0))],
            400,
            "MalformedInput",
        ),
        (
            "neither zone form named",
            vec![snapshot.clone()],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "both zone forms named",
            vec![
                snapshot.clone(),
                param("AvailabilityZone.1", &zone_name(0)),
                param("AvailabilityZoneId.1", &zone_id(0)),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a zone name the Region does not have",
            vec![
                snapshot.clone(),
                param("AvailabilityZone.1", "us-east-1-nowhere"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a zone id the Region does not have",
            vec![
                snapshot.clone(),
                param("AvailabilityZoneId.1", "use9-az99"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a zone of another Region",
            vec![
                snapshot.clone(),
                param("AvailabilityZone.1", "us-west-2a"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "one good zone and one the Region does not have",
            vec![
                snapshot.clone(),
                param("AvailabilityZone.1", &zone_name(0)),
                param("AvailabilityZone.2", "us-east-1-nowhere"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a dry run that would otherwise have run",
            vec![
                snapshot.clone(),
                param("AvailabilityZone.1", &zone_name(0)),
                param("DryRun", "true"),
            ],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, params, expected_status, expected_code) in cases {
        let (status, body) =
            super::ec2_query::call(&router, REGION, "DisableFastSnapshotRestores", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} with \
             body {body}"
        );
    }
}

/// RULE: a `DryRun` that is explicitly off is not a dry run — the request runs normally and
/// reports its pairs.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    let router = emulator();
    let case = "DryRun=false";
    let (status, body) = disable(
        &router,
        &["snap-0123456789abcdef0"],
        By::Name,
        &[0],
        &[param("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_no_successes(case, &body);
    let errors = pair_errors(case, &body);
    assert_eq!(
        errors.len(),
        1,
        "case {case:?}: expected one per-pair error, got {errors:?}"
    );
}

/// RULE: a caller-chosen list is bounded — a request naming more ids than this emulator
/// answers for is refused rather than answered with a response of unbounded size.
///
/// The rows are the bound itself (accepted) and one past it (refused), for both lists the
/// request carries.
#[tokio::test]
async fn an_over_long_list_is_refused_at_the_bound() {
    let router = emulator();
    let maximum = ec2::state::MAX_RESOURCE_IDS;

    // At the bound, the request is answered: one item per distinct id.
    let at_bound: Vec<String> = (0..maximum)
        .map(|index| format!("snap-{index:017x}"))
        .collect();
    let at_bound_refs: Vec<&str> = at_bound.iter().map(String::as_str).collect();
    let errors = errors_of(&router, "ids at the bound", &at_bound_refs, By::Name, &[0]).await;
    assert_eq!(
        errors.len(),
        maximum,
        "case \"ids at the bound\": expected {maximum} errors, got {count}",
        count = errors.len()
    );

    // One past it, and the whole call is refused.
    let past_bound: Vec<String> = (0..=maximum)
        .map(|index| format!("snap-{index:017x}"))
        .collect();
    let past_bound_refs: Vec<&str> = past_bound.iter().map(String::as_str).collect();
    let (status, body) = disable(&router, &past_bound_refs, By::Name, &[0], &[]).await;
    assert_eq!(
        status, 400,
        "case \"ids past the bound\": expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::INVALID_PARAMETER_VALUE,
        "case \"ids past the bound\": expected {:?}, got {code:?} with body {body}",
        ec2::state::INVALID_PARAMETER_VALUE
    );

    // The same bound on the zone list, which is read the same way — asserted with repeats
    // of a VALID zone name, so the refusal can only come from the bound (the dedup that
    // collapses the repeats happens after it) rather than from the zone-validity check, and
    // with the message naming the member it is about.
    let valid_zone = zone_name(0);
    let zone_repeats = |count: usize| -> Vec<Param> {
        let zones: Vec<&str> = (0..count).map(|_| valid_zone.as_str()).collect();
        let mut params = indexed("SourceSnapshotId", &["snap-0123456789abcdef0"]);
        params.extend(indexed("AvailabilityZone", &zones));
        params
    };

    // At the bound, the repeats collapse to the one zone and the request is answered.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DisableFastSnapshotRestores",
        &zone_repeats(maximum),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"zones at the bound\": expected HTTP 200, got {status} with body {body}"
    );
    let errors = pair_errors("zones at the bound", &body);
    assert_eq!(
        errors.len(),
        1,
        "case \"zones at the bound\": the repeats must collapse to one pair, got {errors:?}"
    );

    // One past it, and the whole call is refused before the repeats are collapsed.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DisableFastSnapshotRestores",
        &zone_repeats(maximum + 1),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"zones past the bound\": expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::INVALID_PARAMETER_VALUE,
        "case \"zones past the bound\": expected {:?}, got {code:?} with body {body}",
        ec2::state::INVALID_PARAMETER_VALUE
    );
    let message = element(&body, "Message").unwrap_or_default();
    assert!(
        message.contains("AvailabilityZones"),
        "case \"zones past the bound\": the refusal must name the member it is about, \
         got {message:?}"
    );
}
