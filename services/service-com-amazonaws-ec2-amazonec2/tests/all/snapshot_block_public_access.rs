// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's *block public access for snapshots* setting —
//! `EnableSnapshotBlockPublicAccess`, `DisableSnapshotBlockPublicAccess` and
//! `GetSnapshotBlockPublicAccessState` — driven over the wire the service actually speaks:
//! `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the setting is ONE account-level value holding one member
//! of the modelled `SnapshotBlockPublicAccessState` catalog: an account that has never
//! enabled it is `unblocked`, an enable puts it into the blocking mode its request names
//! (and only the catalog's two blocking modes are accepted — "`unblocked` is not a valid
//! value for **EnableSnapshotBlockPublicAccess**"), a disable puts it back to `unblocked`,
//! and the get REPORTS whatever the last writer left, every read alike, with `ManagedBy`
//! `account` because no EC2 operation can put the setting under a declarative policy.
//!
//! So the tables below span that class rather than one recorded request: every member of
//! the model's own catalog, every writer that can reach each member, sequences of writes
//! in different orders, repeated reads of each state, and the refusals the members' own
//! documentation states.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::error_code;
use super::wire::{child_text, document, param, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// `unblocked` — the state of an account that has never enabled the setting, and the one
/// `DisableSnapshotBlockPublicAccess` returns it to.
const UNBLOCKED: &str = ec2::state::SNAPSHOT_BLOCK_PUBLIC_ACCESS_UNBLOCKED;

/// `block-all-sharing`.
const BLOCK_ALL: &str = ec2::state::SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_ALL_SHARING;

/// `block-new-sharing`.
const BLOCK_NEW: &str = ec2::state::SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING;

/// Every state the setting can be in. Pinned against the model's own
/// `SnapshotBlockPublicAccessState` catalog by [`the_state_catalog_is_the_models_own`], so
/// a member the model gains cannot be left untested here.
const STATES: [&str; 3] = [BLOCK_ALL, BLOCK_NEW, UNBLOCKED];

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call to `action` with `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The text of a top-level member of a response, or a panic naming the case. The three
/// responses here carry only scalars at the top level (`state`, `managedBy`), so the
/// member is a child of the response element itself.
fn top_level(case: &str, body: &str, name: &str) -> String {
    let parsed = document(case, body);
    child_text(parsed.root_element(), name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in the response body {body}"))
}

/// The state `GetSnapshotBlockPublicAccessState` reports, with the `ManagedBy` it reports
/// beside it.
async fn get(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let (status, body) = call(router, "GetSnapshotBlockPublicAccessState", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetSnapshotBlockPublicAccessState, \
         got {status} with body {body}"
    );
    (
        top_level(case, &body, "state"),
        top_level(case, &body, "managedBy"),
    )
}

/// The state the get reports, asserted to be `expected`, together with the `ManagedBy`
/// that every state of this emulator carries: `account`.
async fn assert_state(router: &MultiServiceRouter, case: &str, expected: &str) {
    let (state, managed_by) = get(router, case).await;
    assert_eq!(
        state, expected,
        "case {case:?}: expected the setting to be {expected:?}, got {state:?}"
    );
    assert_eq!(
        managed_by,
        ec2::state::MANAGED_BY_ACCOUNT,
        "case {case:?}: expected the setting to be managed by {:?}, got {managed_by:?}",
        ec2::state::MANAGED_BY_ACCOUNT
    );
}

/// `EnableSnapshotBlockPublicAccess` asking for `mode`, required to succeed; answers the
/// state it reports as now in effect.
async fn enable(router: &MultiServiceRouter, case: &str, mode: &str) -> String {
    let (status, body) = call(
        router,
        "EnableSnapshotBlockPublicAccess",
        &[param("State", mode)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 enabling {mode}, got {status} with body {body}"
    );
    top_level(case, &body, "state")
}

/// `DisableSnapshotBlockPublicAccess`, required to succeed; answers the state it reports
/// as now in effect.
async fn disable(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = call(router, "DisableSnapshotBlockPublicAccess", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 disabling the setting, got {status} with \
         body {body}"
    );
    top_level(case, &body, "state")
}

/// Put the setting into `state` through the operation that reaches it: an enable for a
/// blocking mode, a disable for `unblocked`. Answers the state that writer reported.
async fn put(router: &MultiServiceRouter, case: &str, state: &str) -> String {
    if state == UNBLOCKED {
        disable(router, case).await
    } else {
        enable(router, case, state).await
    }
}

// ---------------------------------------------------------------------------
// The catalog of states
// ---------------------------------------------------------------------------

/// The state words this suite and the handlers use are the model's own
/// `SnapshotBlockPublicAccessState` members — no more and no fewer — and the modes an
/// enable accepts are exactly that catalog minus `unblocked`, which its own documentation
/// excludes.
#[tokio::test]
async fn the_state_catalog_is_the_models_own() {
    let mut used = STATES.to_vec();
    used.sort_unstable();
    let mut modelled = ec2::ops::types::SNAPSHOT_BLOCK_PUBLIC_ACCESS_STATE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        used, modelled,
        "case \"SnapshotBlockPublicAccessState catalog\": the states under test must be \
         the model's own members, got {used:?} against {modelled:?}"
    );

    let mut modes = ec2::state::SNAPSHOT_BLOCK_PUBLIC_ACCESS_MODES.to_vec();
    modes.sort_unstable();
    let mut blocking: Vec<&str> = modelled
        .iter()
        .copied()
        .filter(|state| *state != UNBLOCKED)
        .collect();
    blocking.sort_unstable();
    assert_eq!(
        modes, blocking,
        "case \"EnableSnapshotBlockPublicAccess modes\": the modes an enable accepts must \
         be the catalog without {UNBLOCKED:?}, got {modes:?} against {blocking:?}"
    );
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// An account that has never written the setting reports `unblocked`, managed by the
/// account: "Public sharing is not blocked. Users can publicly share snapshots".
#[tokio::test]
async fn a_fresh_account_reports_the_setting_unblocked() {
    let router = emulator();
    assert_state(&router, "a fresh account", UNBLOCKED).await;
}

/// The rule: a writer puts the setting into the state it names, reports that state back,
/// and the get reads the same state out afterwards.
///
/// The rows are every member of the catalog, each reached through the operation that can
/// reach it, on a fresh account — so each row differs along the state the rule generalises
/// over.
#[tokio::test]
async fn a_write_puts_the_setting_into_the_state_it_names_and_the_get_reads_it_back() {
    for state in STATES {
        let case = format!("writing {state}");
        let router = emulator();
        assert_state(&router, &case, UNBLOCKED).await;
        let reported = put(&router, &case, state).await;
        assert_eq!(
            reported, state,
            "case {case:?}: the writer must report the state now in effect, expected \
             {state:?}, got {reported:?}"
        );
        assert_state(&router, &case, state).await;
    }
}

/// The setting PERSISTS and every write replaces it: a sequence of writes leaves it at
/// the state its last write named, whatever it held before — including a write that asks
/// for the state already in effect, which is the same call again.
///
/// The rows are the sequence's steps, applied in order to one account; the expectation
/// after each step is the step's own state, so the sequence can be reordered or extended
/// without restating it.
#[tokio::test]
async fn the_setting_holds_what_the_last_write_named() {
    let router = emulator();
    // (case, the state the step writes)
    let steps = [
        ("block new sharing first", BLOCK_NEW),
        ("then widen to all sharing", BLOCK_ALL),
        ("the same mode again", BLOCK_ALL),
        ("back to unblocked", UNBLOCKED),
        ("unblocked again", UNBLOCKED),
        ("block all sharing from unblocked", BLOCK_ALL),
        ("narrow to new sharing", BLOCK_NEW),
    ];

    for (case, state) in steps {
        let reported = put(&router, case, state).await;
        assert_eq!(
            reported, state,
            "case {case:?}: the writer must report the state now in effect, expected \
             {state:?}, got {reported:?}"
        );
        assert_state(&router, case, state).await;
    }
}

/// `GetSnapshotBlockPublicAccessState` is a REPORT, so reading cannot change what a later
/// caller sees: a run of gets with no writer between them all report the same state.
///
/// The rows are every state the setting can be in, so the invariant is checked from each
/// one rather than only from the default.
#[tokio::test]
async fn reading_the_setting_never_changes_it() {
    /// Gets per state — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    for state in STATES {
        let case = format!("repeated reads of {state}");
        let router = emulator();
        put(&router, &case, state).await;
        for read in 1..=READS {
            let case = format!("{case}, read {read}");
            assert_state(&router, &case, state).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The documented refusals
// ---------------------------------------------------------------------------

/// Every refusal these three operations make, each with the EC2 error code and HTTP status
/// it answers — and none of them writes anything.
///
/// The rows differ along what makes the request bad: an enable asking for `unblocked`
/// ("`unblocked` is not a valid value for **EnableSnapshotBlockPublicAccess**") — a word
/// the enum DOES hold, so the handler is what refuses it, with EC2's
/// `InvalidParameterValue` — an enable asking for a word the catalog does not hold, for
/// the empty string, and for a catalog member in the wrong case, all three refused by the
/// modelled enum constraint before the handler runs and so carrying the boundary's
/// `ValidationError`, plus a dry run of each of the three operations, which "would have
/// succeeded" and so is refused with `DryRunOperation`. Every one of them is a 400 or a
/// 412 — never a 501, which would say the operation is unimplemented.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    // The state the refusals run against, so "nothing was written" is a claim about a
    // state that is not the default either.
    let seeded = BLOCK_NEW;
    put(&router, "the refusal table's starting state", seeded).await;

    // (case, the action, its form fields, the HTTP status, the EC2 error code)
    let cases: [(&str, &str, Vec<Param>, u16, &str); 7] = [
        (
            "an enable asking for unblocked",
            "EnableSnapshotBlockPublicAccess",
            vec![param("State", UNBLOCKED)],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "an enable asking for a word the catalog does not hold",
            "EnableSnapshotBlockPublicAccess",
            vec![param("State", "block-some-sharing")],
            400,
            "ValidationError",
        ),
        (
            "an enable asking for the empty string",
            "EnableSnapshotBlockPublicAccess",
            vec![param("State", "")],
            400,
            "ValidationError",
        ),
        (
            "an enable asking for a catalog member in the wrong case",
            "EnableSnapshotBlockPublicAccess",
            vec![param("State", "Block-All-Sharing")],
            400,
            "ValidationError",
        ),
        (
            "a dry run of an enable that would have succeeded",
            "EnableSnapshotBlockPublicAccess",
            vec![param("State", BLOCK_ALL), param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a disable that would have succeeded",
            "DisableSnapshotBlockPublicAccess",
            vec![param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a get that would have succeeded",
            "GetSnapshotBlockPublicAccessState",
            vec![param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, action, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {action}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {action}, got \
             {code:?} with body {body}"
        );
        // Each refusal leaves the setting exactly where it was: the next row starts from
        // the same state this one did.
        assert_state(&router, case, seeded).await;
    }
}

/// A `DryRun` that is explicitly off is not a dry run: the request runs normally.
///
/// The rows are the writers and the reader, so the flag's off value is exercised on every
/// operation that reads it.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    let router = emulator();
    // (case, the action, its form fields, the state in effect afterwards)
    let cases: [(&str, &str, Vec<Param>, &str); 3] = [
        (
            "an enable with DryRun=false",
            "EnableSnapshotBlockPublicAccess",
            vec![param("State", BLOCK_ALL), param("DryRun", "false")],
            BLOCK_ALL,
        ),
        (
            "a get with DryRun=false",
            "GetSnapshotBlockPublicAccessState",
            vec![param("DryRun", "false")],
            BLOCK_ALL,
        ),
        (
            "a disable with DryRun=false",
            "DisableSnapshotBlockPublicAccess",
            vec![param("DryRun", "false")],
            UNBLOCKED,
        ),
    ];

    for (case, action, params, expected) in cases {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
        );
        let state = top_level(case, &body, "state");
        assert_eq!(
            state, expected,
            "case {case:?}: expected {action} to report {expected:?}, got {state:?}"
        );
        assert_state(&router, case, expected).await;
    }
}
