// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's EC2 SERIAL CONSOLE access setting —
//! `EnableSerialConsoleAccess`, `DisableSerialConsoleAccess` and
//! `GetSerialConsoleAccessStatus` — driven over the wire the service actually speaks:
//! `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the setting is ONE persisted account-level boolean: an
//! account that has never written it reports `false` ("By default, access to the EC2
//! serial console is disabled for your account"), an enable puts it at `true` and a disable
//! at `false`, each writer reporting the value now in effect, and the get REPORTS whatever
//! the last writer left — every read alike, with `ManagedBy` `account`, because no EC2
//! operation can put the setting under a declarative policy.
//!
//! So the tables below span that class rather than one recorded request: both values of the
//! boolean, every writer that can reach each value, sequences of writes in different orders
//! (including a write of the value already in effect), repeated reads of each value, and the
//! `DryRun` refusal each of the three operations answers.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::error_code;
use super::wire::{child_text, document, param, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// `account` — the member of the model's own `ManagedBy` catalog
/// (`smithy_query(query="shape", shape="ManagedBy")`: `account | declarative-policy`) that
/// names the account itself. Written out here rather than read off the handler's own
/// constant, so the expectation is the MODELLED word and not whatever the implementation
/// happens to carry; [`the_managed_by_word_is_the_models_own_account_member`] pins it
/// against the generated catalog, so a word the model drops cannot survive here.
const MANAGED_BY_ACCOUNT: &str = "account";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call to `action` with `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The text of a top-level member of a response, or a panic naming the case. The three
/// responses here carry only scalars at the top level (`serialConsoleAccessEnabled`,
/// `managedBy`), so the member is a child of the response element itself.
fn top_level(case: &str, body: &str, name: &str) -> String {
    let parsed = document(case, body);
    child_text(parsed.root_element(), name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in the response body {body}"))
}

/// The boolean a response reports as `serialConsoleAccessEnabled`, or a panic naming the
/// case when it is not a boolean at all.
fn reported_flag(case: &str, body: &str) -> bool {
    let text = top_level(case, body, "serialConsoleAccessEnabled");
    text.parse::<bool>().unwrap_or_else(|_| {
        panic!(
            "case {case:?}: expected serialConsoleAccessEnabled to be a boolean, got \
             {text:?} in body {body}"
        )
    })
}

/// What `GetSerialConsoleAccessStatus` reports: the flag, with the `ManagedBy` beside it.
async fn get(router: &MultiServiceRouter, case: &str) -> (bool, String) {
    let (status, body) = call(router, "GetSerialConsoleAccessStatus", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetSerialConsoleAccessStatus, got {status} \
         with body {body}"
    );
    (
        reported_flag(case, &body),
        top_level(case, &body, "managedBy"),
    )
}

/// The flag the get reports, asserted to be `expected`, together with the `ManagedBy` that
/// every value of this emulator carries: `account`.
async fn assert_flag(router: &MultiServiceRouter, case: &str, expected: bool) {
    let (enabled, managed_by) = get(router, case).await;
    assert_eq!(
        enabled, expected,
        "case {case:?}: expected the setting to be {expected}, got {enabled}"
    );
    assert_eq!(
        managed_by, MANAGED_BY_ACCOUNT,
        "case {case:?}: expected the setting to be managed by {MANAGED_BY_ACCOUNT:?}, got \
         {managed_by:?}"
    );
}

/// Put the setting at `enabled` through the operation that reaches it: an enable for
/// `true`, a disable for `false`. Answers the value that writer reported.
async fn put(router: &MultiServiceRouter, case: &str, enabled: bool) -> bool {
    let action = if enabled {
        "EnableSerialConsoleAccess"
    } else {
        "DisableSerialConsoleAccess"
    };
    let (status, body) = call(router, action, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    reported_flag(case, &body)
}

// ---------------------------------------------------------------------------
// The entity that manages the setting
// ---------------------------------------------------------------------------

/// The `ManagedBy` word the get reports is a member of the model's own `ManagedBy` catalog
/// — the generated `ec2::ops::types::MANAGED_BY_VALUES`, which is where the two words come
/// from — and it is the one that names the account itself, because the other member names
/// access a declarative policy manages that "can't be modified by the account" and no
/// operation of this API can put the setting into it.
#[tokio::test]
async fn the_managed_by_word_is_the_models_own_account_member() {
    let case = "the ManagedBy catalog";
    let modelled = ec2::ops::types::MANAGED_BY_VALUES;
    assert!(
        modelled.contains(&MANAGED_BY_ACCOUNT),
        "case {case:?}: expected the word under test to be a modelled ManagedBy member, got \
         {MANAGED_BY_ACCOUNT:?} against {modelled:?}"
    );

    let router = emulator();
    let (_, managed_by) = get(&router, case).await;
    assert!(
        modelled.contains(&managed_by.as_str()),
        "case {case:?}: expected a modelled ManagedBy member, got {managed_by:?} against \
         {modelled:?}"
    );
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// An account that has never written the setting reports it OFF: "By default, access to the
/// EC2 serial console is disabled for your account".
#[tokio::test]
async fn a_fresh_account_reports_the_setting_disabled() {
    let router = emulator();
    assert_flag(&router, "a fresh account", false).await;
}

/// The rule: a writer puts the setting at the value it stands for, reports that value back,
/// and the get reads the same value out afterwards.
///
/// The rows are both values of the boolean, each reached through the operation that can
/// reach it, on a fresh account — so each row differs along the value the rule generalises
/// over.
#[tokio::test]
async fn a_write_puts_the_setting_where_it_stands_and_the_get_reads_it_back() {
    for enabled in [true, false] {
        let case = format!("writing {enabled}");
        let router = emulator();
        assert_flag(&router, &case, false).await;
        let reported = put(&router, &case, enabled).await;
        assert_eq!(
            reported, enabled,
            "case {case:?}: the writer must report the value now in effect, expected \
             {enabled}, got {reported}"
        );
        assert_flag(&router, &case, enabled).await;
    }
}

/// The setting PERSISTS and every write replaces it: a sequence of writes leaves it at the
/// value its last write stands for, whatever it held before — including a write of the value
/// already in effect, which is the same call again and changes nothing.
///
/// The rows are the sequence's steps, applied in order to one account; the expectation after
/// each step is the step's own value, so the sequence can be reordered or extended without
/// restating it.
#[tokio::test]
async fn the_setting_holds_what_the_last_write_left() {
    let router = emulator();
    // (case, the value the step writes)
    let steps = [
        ("disable before anything enabled it", false),
        ("enable from off", true),
        ("enable again", true),
        ("enable a third time", true),
        ("disable from on", false),
        ("disable again", false),
        ("enable once more", true),
    ];

    for (case, enabled) in steps {
        let reported = put(&router, case, enabled).await;
        assert_eq!(
            reported, enabled,
            "case {case:?}: the writer must report the value now in effect, expected \
             {enabled}, got {reported}"
        );
        assert_flag(&router, case, enabled).await;
    }
}

/// `GetSerialConsoleAccessStatus` is a REPORT, so reading cannot change what a later caller
/// sees: a run of gets with no writer between them all report the same value.
///
/// The rows are both values the setting can hold, so the invariant is checked from each one
/// rather than only from the default.
#[tokio::test]
async fn reading_the_setting_never_changes_it() {
    /// Gets per value — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    for enabled in [true, false] {
        let case = format!("repeated reads of {enabled}");
        let router = emulator();
        put(&router, &case, enabled).await;
        for read in 1..=READS {
            let case = format!("{case}, read {read}");
            assert_flag(&router, &case, enabled).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The documented refusal
// ---------------------------------------------------------------------------

/// A `DryRun` request "would have succeeded", so each of the three operations answers
/// `DryRunOperation` at HTTP 412 and writes nothing: "If you have the required permissions,
/// the error response is DryRunOperation" (`DryRun`). Never a 501, which would say the
/// operation is unimplemented.
///
/// The rows are the three operations, run against a setting that is NOT the default, so
/// "nothing was written" is a claim about a value a writer put there.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation_and_writes_nothing() {
    let router = emulator();
    // The value the refusals run against, so a dry run that wrote anything would show.
    let seeded = true;
    put(&router, "the refusal table's starting value", seeded).await;

    // (case, the action, its form fields, the HTTP status, the EC2 error code)
    let cases: [(&str, &str, Vec<Param>, u16, &str); 3] = [
        (
            "a dry run of an enable that would have succeeded",
            "EnableSerialConsoleAccess",
            vec![param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a disable that would have succeeded",
            "DisableSerialConsoleAccess",
            vec![param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a get that would have succeeded",
            "GetSerialConsoleAccessStatus",
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
        // the same value this one did.
        assert_flag(&router, case, seeded).await;
    }
}

/// A `DryRun` that is explicitly off is not a dry run: the request runs normally.
///
/// The rows are the writers and the reader, so the flag's off value is exercised on every
/// operation that reads it.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    let router = emulator();
    // (case, the action, its form fields, the value in effect afterwards)
    let cases: [(&str, &str, Vec<Param>, bool); 3] = [
        (
            "an enable with DryRun=false",
            "EnableSerialConsoleAccess",
            vec![param("DryRun", "false")],
            true,
        ),
        (
            "a get with DryRun=false",
            "GetSerialConsoleAccessStatus",
            vec![param("DryRun", "false")],
            true,
        ),
        (
            "a disable with DryRun=false",
            "DisableSerialConsoleAccess",
            vec![param("DryRun", "false")],
            false,
        ),
    ];

    for (case, action, params, expected) in cases {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
        );
        let reported = reported_flag(case, &body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {action} to report {expected}, got {reported}"
        );
        assert_flag(&router, case, expected).await;
    }
}
