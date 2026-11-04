// SPDX-License-Identifier: CC0-1.0

//! A legal hold's `DateRange`: the window of creation INSTANTS it selects
//! stored recovery points by, driven as real HTTP through the router.
//!
//! The rule under test: a `RecoveryPointSelection` `DateRange` selects a stored
//! recovery point whose creation instant lies between `FromDate`, "the
//! beginning date, inclusive", and `ToDate`, "the end date, inclusive",
//! compared as instants at the precision the bounds carry — "accurate to
//! milliseconds" (`smithy_query` shape `DateRange`). So a point created one
//! millisecond outside a bound is NOT held, while one created exactly ON a
//! bound is, and the second a bound falls in is not selected wholesale.
//!
//! The table's rows differ in how far a stored point's creation instant sits
//! from a window bound — a millisecond either side of each bound, exactly on
//! each bound, inside, and well outside both ends — which is what the rule
//! generalises over. The window itself is run twice, once opening on a whole
//! second and once a quarter of a second into one, so the sub-second part of a
//! bound is part of the comparison rather than something the rows depend on.
//!
//! Every instant is placed by the suites' own recovery-point seam
//! (`recovery_point_deletion::set_point_creation`) relative to the bound the
//! case sends, so no assertion here depends on what the clock read when the
//! fixture ran. The request harness and the hold helpers are the ones the
//! sibling suites expose, so there is ONE wire path for the service's suites.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::legal_hold_coverage::{arns_of, held_points, hold_over};
use super::recovery_point_deletion::{router_with_engine, set_point_creation, start_point};
use super::vault_and_backup_job::{create_vault, epoch_seconds, ACCOUNT, REGION};

/// The vault every case stores its recovery points in.
const VAULT: &str = "hold-window";

/// How long the window the cases are read against stays open.
const WINDOW: Duration = Duration::from_secs(600);

/// How far back the window opens. Far enough that every instant a row places —
/// including the ones past `ToDate` — is in the past, since "future DateTime
/// values are not permitted" for a selection's bounds (`smithy_query` shape
/// `DateRange`), and a hold is placed on recovery points that already exist.
const WINDOW_OPENS_AGO: Duration = Duration::from_secs(3 * 3_600);

/// The sub-second parts a window's bounds are stated with: a bound on a whole
/// second, and one a quarter of a second in. Both are exact in the fractional
/// epoch seconds the protocol serialises a timestamp as (a dyadic fraction at
/// this magnitude), so a row placing a point exactly ON a bound compares equal;
/// a fraction added here has to be dyadic too, or the two equality rows stop
/// being exact.
const BOUND_FRACTIONS: [Duration; 2] = [Duration::ZERO, Duration::from_millis(250)];

/// The window's start instant for a run whose bounds carry `fraction`: a whole
/// second [`WINDOW_OPENS_AGO`] before now, plus that fraction.
fn window_opens_at(fraction: Duration) -> SystemTime {
    let whole_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(whole_seconds) - WINDOW_OPENS_AGO + fraction
}

/// An instant `offset` milliseconds from `base`, either side of it.
fn offset_from(base: SystemTime, offset_millis: i64) -> SystemTime {
    let magnitude = Duration::from_millis(offset_millis.unsigned_abs());
    if offset_millis < 0 {
        base.checked_sub(magnitude)
            .expect("an instant that far before the window")
    } else {
        base.checked_add(magnitude)
            .expect("an instant that far after the window")
    }
}

/// A hold's window selects exactly the stored recovery points whose creation
/// instant it contains, both bounds inclusive and to the millisecond.
#[tokio::test]
async fn a_window_covers_the_points_whose_creation_instant_it_contains() {
    // (case, milliseconds from `FromDate` the point was created, whether the
    // window of 600_000 ms covers it)
    let rows: [(&str, i64, bool); 9] = [
        ("an hour before the window opens", -3_600_000, false),
        ("one millisecond before FromDate", -1, false),
        ("exactly FromDate, the inclusive beginning", 0, true),
        ("one millisecond after FromDate", 1, true),
        ("halfway through the window", 300_000, true),
        ("one millisecond before ToDate", 599_999, true),
        ("exactly ToDate, the inclusive end", 600_000, true),
        ("one millisecond after ToDate", 600_001, false),
        ("ten minutes after the window closes", 1_200_000, false),
    ];

    for fraction in BOUND_FRACTIONS {
        let bounds = format!("bounds {} ms into a second", fraction.as_millis());
        let (router, engine) = router_with_engine();
        let (status, created) = create_vault(&router, VAULT, "{}").await;
        assert_eq!(
            status, 200,
            "{bounds}: expected CreateBackupVault 200, got {status} {created:?}"
        );

        let opens_at = window_opens_at(fraction);
        let closes_at = opens_at
            .checked_add(WINDOW)
            .expect("an instant a window's length after it opens");

        // One stored recovery point per row, each of a protected resource of its
        // own so the rows cannot be confused for one another, placed at the
        // row's distance from the window's opening instant.
        let mut placed: Vec<(&str, String, bool)> = Vec::new();
        for (index, (case, offset_millis, expected)) in rows.iter().enumerate() {
            let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/window-{index}");
            let arn = start_point(&router, VAULT, &resource, None).await;
            set_point_creation(&engine, &arn, offset_from(opens_at, *offset_millis));
            placed.push((case, arn, *expected));
        }

        let id = hold_over(
            &router,
            &bounds,
            json!({"DateRange": {
                "FromDate": epoch_seconds(opens_at),
                "ToDate": epoch_seconds(closes_at),
            }}),
        )
        .await;
        let (status, page) = held_points(&router, &id, "").await;
        assert_eq!(
            status, 200,
            "{bounds}: expected ListRecoveryPointsByLegalHold 200, got {status} {page:?}"
        );
        let covered = arns_of(&page);

        for (case, arn, expected) in &placed {
            assert_eq!(
                covered.contains(arn),
                *expected,
                "{bounds}, case {case:?}: expected the window to cover it: {expected}, \
                 got the covered set {covered:?}"
            );
        }
        let expected_set: BTreeSet<String> = placed
            .iter()
            .filter(|(_, _, expected)| *expected)
            .map(|(_, arn, _)| arn.clone())
            .collect();
        assert_eq!(
            covered, expected_set,
            "{bounds}: the window covers nothing beyond the points inside it, got {page:?}"
        );
    }
}

/// A window the stored points all sit inside, and one they all sit outside,
/// read through the same path: the window is what narrows the hold, so a
/// selection naming a window that contains everything holds everything, and one
/// that contains nothing holds nothing.
#[tokio::test]
async fn a_window_that_contains_everything_or_nothing_holds_just_that() {
    let (router, engine) = router_with_engine();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );

    let opens_at = window_opens_at(Duration::ZERO);
    let mut stored: BTreeSet<String> = BTreeSet::new();
    for (index, offset_millis) in [0_i64, 1, 250, 599_999, 600_000].iter().enumerate() {
        let resource = format!("arn:aws:s3:::window-wide-{index}");
        let arn = start_point(&router, VAULT, &resource, None).await;
        set_point_creation(&engine, &arn, offset_from(opens_at, *offset_millis));
        stored.insert(arn);
    }

    // (case, the window's bounds, the points it covers)
    let cases: [(&str, SystemTime, SystemTime, BTreeSet<String>); 2] = [
        (
            "a window around every stored point",
            offset_from(opens_at, -60_000),
            offset_from(opens_at, 660_000),
            stored.clone(),
        ),
        (
            "a window closed before any of them",
            offset_from(opens_at, -600_000),
            offset_from(opens_at, -1),
            BTreeSet::new(),
        ),
    ];

    for (case, from, to, expected) in cases {
        let id = hold_over(
            &router,
            case,
            json!({"DateRange": {
                "FromDate": epoch_seconds(from),
                "ToDate": epoch_seconds(to),
            }}),
        )
        .await;
        let (status, page) = held_points(&router, &id, "").await;
        assert_eq!(
            (status, arns_of(&page)),
            (200, expected),
            "case {case:?}: expected the window's points, got {status} {page:?}"
        );
    }
}
