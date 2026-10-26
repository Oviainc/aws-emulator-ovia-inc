// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's *block public access for AMIs* setting —
//! `EnableImageBlockPublicAccess`, `DisableImageBlockPublicAccess` and
//! `GetImageBlockPublicAccessState` — driven over the wire the service actually speaks
//! (`ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML — README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. REPORTING: the get answers the setting of the Region the request is signed for, beside
//!    `ManagedBy` `account` because no EC2 operation can put the setting under a declarative
//!    policy, and it writes nothing — a Region nothing has enabled the setting in reads
//!    `unblocked`, and a run of reads all report the same state;
//! 2. ROUND TRIP: a writer puts the setting into the state it names and answers that state,
//!    the get reads the same state back out afterwards, and both writers are idempotent — an
//!    enable is the one way into the `block-new-sharing` mode and a disable the one way back
//!    to `unblocked`;
//! 3. REGION SCOPE: the setting is a fact of the account "in the specified Amazon Web
//!    Services Region", so writing it in one Region leaves every other Region's where it
//!    was;
//! 4. REFUSALS: a `DryRun` of each of the three operations performs nothing and is answered
//!    412 `DryRunOperation`, and an enable that names no mode this API accepts is refused
//!    with the EC2 code for what is wrong with it — never with a 501, which would say the
//!    operation is unimplemented.
//!
//! Each row below differs along what its rule generalises over — which Region, which
//! sequence of writes, how many reads follow it, which operation carries the flag, and what
//! the enable names — so no row is the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};

/// Regions spanning the partitions and the naming shapes a Region name takes — a three-part
/// name, a four-part one, and a non-commercial partition — so no assertion below is a
/// property of one Region string.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// `unblocked` — "Your AMIs in the specified Region can be publicly shared": where a Region
/// the account has never enabled the setting in sits, and the state a disable returns it to.
const UNBLOCKED: &str = ec2::state::IMAGE_BLOCK_PUBLIC_ACCESS_UNBLOCKED;

/// `block-new-sharing` — the one mode an enable puts the setting into.
const BLOCK_NEW: &str = ec2::state::IMAGE_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING;

/// One row of a table that drives an operation with the form fields it names: the case's
/// name, the `Action`, those fields, and the state expected afterwards.
///
/// A named struct rather than a tuple because the fields a row carries are a list of
/// `(name, value)` pairs, which is one type too many for a readable tuple.
struct Row {
    /// What this row is, named in every assertion it drives.
    case: &'static str,
    /// The ec2Query `Action` the row calls.
    action: &'static str,
    /// The form fields the row sends beside that action.
    params: Vec<(String, String)>,
    /// The state the setting is expected to read afterwards.
    expected: &'static str,
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The state `GetImageBlockPublicAccessState` reports in `region`, with the `ManagedBy` it
/// reports beside it.
async fn get(router: &MultiServiceRouter, case: &str, region: &str) -> (String, String) {
    let (status, body) = call(router, region, "GetImageBlockPublicAccessState", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetImageBlockPublicAccessState in {region}, \
         got {status} with body {body}"
    );
    let state = element(&body, "imageBlockPublicAccessState").unwrap_or_else(|| {
        panic!("case {case:?}: expected an imageBlockPublicAccessState member, got {body}")
    });
    let managed_by = element(&body, "managedBy")
        .unwrap_or_else(|| panic!("case {case:?}: expected a managedBy member, got {body}"));
    (state, managed_by)
}

/// The state the get reports in `region`, asserted to be `expected`, together with the
/// `ManagedBy` every state this emulator holds carries: `account`.
async fn assert_state(router: &MultiServiceRouter, case: &str, region: &str, expected: &str) {
    let (state, managed_by) = get(router, case, region).await;
    assert_eq!(
        state, expected,
        "case {case:?}: expected the setting in {region} to read {expected:?}, got {state:?}"
    );
    assert_eq!(
        managed_by,
        ec2::state::MANAGED_BY_ACCOUNT,
        "case {case:?}: expected the setting to be managed by {:?}, got {managed_by:?}",
        ec2::state::MANAGED_BY_ACCOUNT
    );
}

/// `EnableImageBlockPublicAccess` in `region`, required to succeed; answers the state it
/// reports as now in effect.
async fn enable(router: &MultiServiceRouter, case: &str, region: &str) -> String {
    let (status, body) = call(
        router,
        region,
        "EnableImageBlockPublicAccess",
        &[p("ImageBlockPublicAccessState", BLOCK_NEW)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 enabling the setting in {region}, \
         got {status} with body {body}"
    );
    element(&body, "imageBlockPublicAccessState").unwrap_or_else(|| {
        panic!("case {case:?}: expected an imageBlockPublicAccessState member, got {body}")
    })
}

/// `DisableImageBlockPublicAccess` in `region`, required to succeed; answers the state it
/// reports as now in effect.
async fn disable(router: &MultiServiceRouter, case: &str, region: &str) -> String {
    let (status, body) = call(router, region, "DisableImageBlockPublicAccess", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 disabling the setting in {region}, \
         got {status} with body {body}"
    );
    element(&body, "imageBlockPublicAccessState").unwrap_or_else(|| {
        panic!("case {case:?}: expected an imageBlockPublicAccessState member, got {body}")
    })
}

// ---------------------------------------------------------------------------
// The catalog of states
// ---------------------------------------------------------------------------

/// The state words this suite and the handlers use are the MODEL's own: the one word a
/// disable can report is the whole `ImageBlockPublicAccessDisabledState` catalog, the one
/// mode an enable accepts is the whole `ImageBlockPublicAccessEnabledState` catalog, and
/// together they are the two values the get's own documentation lists.
#[tokio::test]
async fn the_state_words_are_the_models_own() {
    assert_eq!(
        ec2::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_DISABLED_STATE_VALUES,
        [UNBLOCKED],
        "case \"ImageBlockPublicAccessDisabledState catalog\": expected the disabled state to \
         be the model's own member, got {:?}",
        ec2::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_DISABLED_STATE_VALUES
    );
    assert_eq!(
        ec2::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_ENABLED_STATE_VALUES,
        [BLOCK_NEW],
        "case \"ImageBlockPublicAccessEnabledState catalog\": expected the enabled state to be \
         the model's own member, got {:?}",
        ec2::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_ENABLED_STATE_VALUES
    );
    assert_eq!(
        ec2::state::IMAGE_BLOCK_PUBLIC_ACCESS_MODES,
        ec2::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_ENABLED_STATE_VALUES,
        "case \"EnableImageBlockPublicAccess modes\": the modes an enable accepts must be the \
         model's own enabled-state catalog, got {:?}",
        ec2::state::IMAGE_BLOCK_PUBLIC_ACCESS_MODES
    );
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// RULE 1: an account that has never written the setting reads `unblocked` in every Region,
/// managed by the account, and a read leaves it there.
///
/// The rows are the Regions of [`REGIONS`], so the default is not read off one Region name.
#[tokio::test]
async fn a_fresh_account_reports_the_setting_unblocked_in_every_region() {
    let router = emulator();
    for region in REGIONS {
        let case = format!("a fresh account in {region}");
        assert_state(&router, &case, region, UNBLOCKED).await;
        assert_state(&router, &case, region, UNBLOCKED).await;
    }
}

/// RULE 1: the get is a REPORT, so reading cannot change what a later caller sees — a run of
/// gets with no writer between them all report the same state.
///
/// The rows are the two states the setting can be in, so the invariant is checked from each
/// one rather than only from the default.
#[tokio::test]
async fn reading_the_setting_never_changes_it() {
    /// Gets per state — enough to catch a read that consumes or advances something.
    const READS: usize = 4;
    let region = REGIONS[1];

    for state in [UNBLOCKED, BLOCK_NEW] {
        let case = format!("repeated reads of {state}");
        let router = emulator();
        if state == BLOCK_NEW {
            enable(&router, &case, region).await;
        }
        for read in 1..=READS {
            let case = format!("{case}, read {read}");
            assert_state(&router, &case, region, state).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// RULE 2: a writer puts the setting into the state it names, reports that state back, and
/// the get reads the same state out afterwards — including a read made straight after the
/// write, because the mode an enable asks for is the mode the next read reports.
///
/// The rows are the Regions of [`REGIONS`], so the round trip is not a property of one.
#[tokio::test]
async fn an_enable_puts_the_setting_into_the_mode_it_names_and_the_get_reads_it_back() {
    for region in REGIONS {
        let case = format!("enabling in {region}");
        let router = emulator();
        assert_state(&router, &case, region, UNBLOCKED).await;

        let reported = enable(&router, &case, region).await;
        assert_eq!(
            reported, BLOCK_NEW,
            "case {case:?}: the enable must report the state now in effect, expected \
             {BLOCK_NEW:?}, got {reported:?}"
        );
        assert_state(&router, &case, region, BLOCK_NEW).await;
    }
}

/// RULE 2: the setting PERSISTS and every write replaces it: a sequence of writes leaves it
/// at the state its last write named, whatever it held before — including a write that asks
/// for the state already in effect, which is the same call again.
///
/// The rows are the sequence's steps, applied in order to one Region; the expectation after
/// each step is the step's own state, so the sequence can be reordered or extended without
/// restating it.
#[tokio::test]
async fn the_setting_holds_what_the_last_write_left() {
    let region = REGIONS[0];
    let router = emulator();
    // (case, whether the step enables, the state in effect afterwards)
    let steps = [
        ("block new sharing first", true, BLOCK_NEW),
        ("the same enable again", true, BLOCK_NEW),
        ("back to unblocked", false, UNBLOCKED),
        ("unblocked again", false, UNBLOCKED),
        ("block new sharing from unblocked", true, BLOCK_NEW),
    ];

    for (case, enables, expected) in steps {
        let reported = if enables {
            enable(&router, case, region).await
        } else {
            disable(&router, case, region).await
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: the writer must report the state it puts in effect, expected \
             {expected:?}, got {reported:?}"
        );
        assert_state(&router, case, region, expected).await;
    }
}

/// RULE 2: a disable is the one way back to `unblocked`, and it is idempotent — a Region it
/// was never enabled in is already there.
///
/// The rows differ in what the Region held when the disable was made: the mode an enable put
/// it in, and nothing at all.
#[tokio::test]
async fn a_disable_returns_the_setting_to_unblocked_from_wherever_it_was() {
    // (case, whether the Region was enabled first)
    let rows = [
        ("disabling a setting in effect", true),
        ("disabling a Region it was never enabled in", false),
    ];

    for (case, enabled_first) in rows {
        let region = REGIONS[2];
        let router = emulator();
        if enabled_first {
            enable(&router, case, region).await;
            assert_state(&router, case, region, BLOCK_NEW).await;
        }

        let reported = disable(&router, case, region).await;
        assert_eq!(
            reported, UNBLOCKED,
            "case {case:?}: expected the disable to report {UNBLOCKED:?}, got {reported:?}"
        );
        assert_state(&router, case, region, UNBLOCKED).await;
    }
}

// ---------------------------------------------------------------------------
// The Region the setting belongs to
// ---------------------------------------------------------------------------

/// RULE 3: the setting belongs to the Region the request was signed for, so one Region's
/// writes leave every other Region's setting alone.
#[tokio::test]
async fn the_setting_is_scoped_to_the_region_it_was_written_in() {
    let router = emulator();
    // Enable in two of the four Regions and leave the others alone.
    for region in [REGIONS[0], REGIONS[3]] {
        enable(&router, "enabling two Regions", region).await;
    }
    for region in REGIONS {
        let expected = if region == REGIONS[0] || region == REGIONS[3] {
            BLOCK_NEW
        } else {
            UNBLOCKED
        };
        let case = format!("two Regions enabled, asking in {region}");
        assert_state(&router, &case, region, expected).await;
    }

    // Disabling one of them leaves the other enabled.
    disable(&router, "disabling one of the two", REGIONS[0]).await;
    for region in REGIONS {
        let expected = if region == REGIONS[3] {
            BLOCK_NEW
        } else {
            UNBLOCKED
        };
        let case = format!("one Region disabled again, asking in {region}");
        assert_state(&router, &case, region, expected).await;
    }
}

/// The store holds the setting for [`ec2::state::MAX_IMAGE_BLOCK_PUBLIC_ACCESS_REGIONS`]
/// Regions: the enable that would hold one more is refused with nothing stored, a Region
/// already stored costs nothing and is still accepted, and a disable gives the slot back.
#[tokio::test]
async fn the_number_of_regions_the_setting_is_held_for_is_bounded() {
    let router = emulator();
    // Synthetic Region names of the shape a credential scope accepts, one per slot.
    let held: Vec<String> = (0..ec2::state::MAX_IMAGE_BLOCK_PUBLIC_ACCESS_REGIONS)
        .map(|nth| format!("ap-slot{nth}-1"))
        .collect();
    for region in &held {
        enable(&router, "filling the store", region).await;
    }

    // A Region the store already holds costs nothing, so re-enabling it still succeeds.
    enable(&router, "re-enabling a Region the store holds", &held[0]).await;

    // One Region past the ceiling is refused, and nothing is stored for it.
    let past = "ap-slot9999-1";
    let case = "one Region past the ceiling";
    let (status, body) = call(
        &router,
        past,
        "EnableImageBlockPublicAccess",
        &[p("ImageBlockPublicAccessState", BLOCK_NEW)],
    )
    .await;
    assert_eq!(
        status, 503,
        "case {case:?}: expected HTTP 503, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "RequestLimitExceeded",
        "case {case:?}: expected error code \"RequestLimitExceeded\", got {code:?} with \
         body {body}"
    );
    assert_state(&router, case, past, UNBLOCKED).await;

    // A disable gives the slot back, so the Region that was refused now fits.
    let case = "the freed slot admits the Region";
    disable(&router, case, &held[0]).await;
    enable(&router, case, past).await;
    assert_state(&router, case, past, BLOCK_NEW).await;
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// RULE 4: `DryRun` is answered 412 `DryRunOperation` on all three operations and leaves the
/// setting exactly where it was, whichever state it already held.
///
/// The rows are every operation crossed with both states the setting can be in, so the flag
/// is exercised on the reader and on each writer from each starting point.
#[tokio::test]
async fn a_dry_run_reports_and_changes_nothing() {
    let region = REGIONS[1];
    // One row per operation and per state the setting can be in; `expected` is the state the
    // refused call must leave in effect, which is the state it was in before.
    let rows = [
        Row {
            case: "an enable while unblocked",
            action: "EnableImageBlockPublicAccess",
            params: vec![p("ImageBlockPublicAccessState", BLOCK_NEW)],
            expected: UNBLOCKED,
        },
        Row {
            case: "an enable while already blocking",
            action: "EnableImageBlockPublicAccess",
            params: vec![p("ImageBlockPublicAccessState", BLOCK_NEW)],
            expected: BLOCK_NEW,
        },
        Row {
            case: "a disable while unblocked",
            action: "DisableImageBlockPublicAccess",
            params: Vec::new(),
            expected: UNBLOCKED,
        },
        Row {
            case: "a disable while blocking",
            action: "DisableImageBlockPublicAccess",
            params: Vec::new(),
            expected: BLOCK_NEW,
        },
        Row {
            case: "a get while unblocked",
            action: "GetImageBlockPublicAccessState",
            params: Vec::new(),
            expected: UNBLOCKED,
        },
        Row {
            case: "a get while blocking",
            action: "GetImageBlockPublicAccessState",
            params: Vec::new(),
            expected: BLOCK_NEW,
        },
    ];

    for row in rows {
        let Row {
            case,
            action,
            mut params,
            expected: before,
        } = row;
        let router = emulator();
        if before == BLOCK_NEW {
            enable(&router, case, region).await;
        }

        params.push(p("DryRun", "true"));
        let (status, body) = call(&router, region, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun {action}, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DryRunOperation",
            "case {case:?}: expected error code \"DryRunOperation\" from {action}, got \
             {code:?} with body {body}"
        );
        // The refusal leaves the setting exactly where it was.
        assert_state(&router, case, region, before).await;
    }
}

/// RULE 4: a `DryRun` that is explicitly off is not a dry run — the request runs normally.
///
/// The rows are the two writers and the reader, so the flag's off value is exercised on every
/// operation that reads it.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    let region = REGIONS[2];
    let router = emulator();
    // One row per operation, each carrying the flag explicitly off.
    let rows = [
        Row {
            case: "an enable with DryRun=false",
            action: "EnableImageBlockPublicAccess",
            params: vec![
                p("ImageBlockPublicAccessState", BLOCK_NEW),
                p("DryRun", "false"),
            ],
            expected: BLOCK_NEW,
        },
        Row {
            case: "a get with DryRun=false",
            action: "GetImageBlockPublicAccessState",
            params: vec![p("DryRun", "false")],
            expected: BLOCK_NEW,
        },
        Row {
            case: "a disable with DryRun=false",
            action: "DisableImageBlockPublicAccess",
            params: vec![p("DryRun", "false")],
            expected: UNBLOCKED,
        },
    ];

    for Row {
        case,
        action,
        params,
        expected,
    } in rows
    {
        let (status, body) = call(&router, region, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
        );
        assert_state(&router, case, region, expected).await;
    }
}

/// RULE 4: an enable that names no mode this API accepts is refused with the EC2 code for
/// what is wrong with it, and writes nothing.
///
/// The rows differ along what makes the request bad: a request carrying no
/// `ImageBlockPublicAccessState` at all, which is a parameter the model makes `required` and
/// the BOUNDARY refuses before this handler runs; the empty string; a word the catalog does
/// not hold; a word of the SNAPSHOT setting's catalog, which this one does not share; and
/// the catalog's own member in the wrong case. None of them is a 501, which would say the
/// operation is unimplemented.
#[tokio::test]
async fn an_enable_that_names_no_accepted_mode_is_refused() {
    let region = REGIONS[3];
    let router = emulator();

    /// One refused enable: the case's name, the form fields it sends, and the HTTP status and
    /// EC2 error code it must be answered with.
    struct Refusal {
        /// What this row is, named in every assertion it drives.
        case: &'static str,
        /// The form fields the enable sends.
        params: Vec<(String, String)>,
        /// The HTTP status the refusal carries.
        status: u16,
        /// The EC2 error code the refusal carries.
        code: &'static str,
    }

    let rows = [
        Refusal {
            case: "an enable naming no state at all",
            params: Vec::new(),
            status: 400,
            code: "MalformedInput",
        },
        Refusal {
            case: "an enable naming the empty string",
            params: vec![p("ImageBlockPublicAccessState", "")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "an enable naming a word the catalog does not hold",
            params: vec![p("ImageBlockPublicAccessState", "block-some-sharing")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "an enable naming the snapshot setting's block-all-sharing",
            params: vec![p("ImageBlockPublicAccessState", "block-all-sharing")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "an enable naming the catalog member in the wrong case",
            params: vec![p("ImageBlockPublicAccessState", "Block-New-Sharing")],
            status: 400,
            code: "ValidationError",
        },
    ];

    for row in rows {
        let (status, body) = call(
            &router,
            region,
            "EnableImageBlockPublicAccess",
            &row.params,
        )
        .await;
        let case = row.case;
        let expected_status = row.status;
        let expected_code = row.code;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from \
             EnableImageBlockPublicAccess, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} with \
             body {body}"
        );
        // Every refusal leaves the setting exactly where it was.
        assert_state(&router, case, region, UNBLOCKED).await;
    }
}
