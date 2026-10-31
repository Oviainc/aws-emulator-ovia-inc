// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's *managed resource visibility* setting —
//! `GetManagedResourceVisibility` and `ModifyManagedResourceVisibility` — driven over the
//! wire the service actually speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=…`, answered with XML — README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rules under test are:
//!
//! 1. REPORTING: the get answers the word the account's setting holds, inside the
//!    `<visibility>` settings structure the model binds, and it writes nothing — an account
//!    that has never modified the setting reads `visible`, and a run of reads all report the
//!    same word;
//! 2. ROUND TRIP: the modify puts the setting into the word it names and answers that word,
//!    the get reads the same word back out afterwards, and the modify is idempotent — asking
//!    for the word already in effect is the same call again;
//! 3. ACCOUNT SCOPE: "visibility settings are account-wide"
//!    (`ModifyManagedResourceVisibility`), so a modify made in one Region is what a get made
//!    in every other Region reports;
//! 4. REFUSALS: a `DryRun` of either operation performs nothing and is answered 412
//!    `DryRunOperation`, and a modify that names no value the setting's catalog holds is
//!    refused with the code for what is wrong with it — never with a 501, which would say the
//!    operation is unimplemented.
//!
//! Each row below differs along what its rule generalises over — which word, which Region,
//! which sequence of writes, how many reads follow one, and what the modify names — so no row
//! is the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code};
use super::wire::{child, child_text, document};

/// Regions spanning the partitions and the naming shapes a Region name takes — a three-part
/// name, a four-part one, and a non-commercial partition — so no assertion below is a
/// property of one Region string. The setting is account-wide, so these are what RULE 3 reads
/// it from rather than Regions it is held per.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// `visible` — managed resources are included in Describe responses by default, which is
/// where an account that has never modified the setting sits.
const VISIBLE: &str = ec2::state::MANAGED_RESOURCE_VISIBILITY_VISIBLE;

/// `hidden` — managed resources are not included in Describe responses by default.
const HIDDEN: &str = ec2::state::MANAGED_RESOURCE_VISIBILITY_HIDDEN;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The `defaultVisibility` of the `<visibility>` settings structure a response carries.
///
/// Read through the XML tree rather than by substring, so the word is taken from INSIDE the
/// settings structure the model binds (`Visibility`, bound as `<visibility>`) rather than
/// from anywhere in the body.
fn reported_visibility(case: &str, body: &str) -> String {
    let parsed = document(case, body);
    let settings = child(parsed.root_element(), "visibility")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <visibility> member, got {body}"));
    child_text(settings, "defaultVisibility").unwrap_or_else(|| {
        panic!("case {case:?}: expected a <defaultVisibility> inside <visibility>, got {body}")
    })
}

/// `GetManagedResourceVisibility` asked in `region`, required to succeed; answers the word it
/// reports.
async fn get(router: &MultiServiceRouter, case: &str, region: &str) -> String {
    let (status, body) = call(router, region, "GetManagedResourceVisibility", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetManagedResourceVisibility in {region}, \
         got {status} with body {body}"
    );
    reported_visibility(case, &body)
}

/// The word the get reports in `region`, asserted to be `expected`.
async fn assert_visibility(router: &MultiServiceRouter, case: &str, region: &str, expected: &str) {
    let reported = get(router, case, region).await;
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the setting asked for in {region} to read {expected:?}, \
         got {reported:?}"
    );
}

/// `ModifyManagedResourceVisibility` in `region`, required to succeed; answers the word it
/// reports as now in effect.
async fn modify(router: &MultiServiceRouter, case: &str, region: &str, word: &str) -> String {
    let (status, body) = call(
        router,
        region,
        "ModifyManagedResourceVisibility",
        &[p("DefaultVisibility", word)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 modifying the setting to {word:?} in {region}, \
         got {status} with body {body}"
    );
    reported_visibility(case, &body)
}

// ---------------------------------------------------------------------------
// The catalog of words
// ---------------------------------------------------------------------------

/// The words this suite and the handlers use are the MODEL's own: the two members of the
/// `ManagedResourceDefaultVisibility` catalog, which is also the pair
/// `ModifyManagedResourceVisibility` documents as its valid values.
#[tokio::test]
async fn the_visibility_words_are_the_models_own() {
    let catalog = ec2::ops::types::MANAGED_RESOURCE_DEFAULT_VISIBILITY_VALUES;
    assert_eq!(
        catalog,
        [HIDDEN, VISIBLE],
        "case \"ManagedResourceDefaultVisibility catalog\": expected the catalog's own two \
         members, got {catalog:?}"
    );
    for word in [VISIBLE, HIDDEN] {
        assert!(
            catalog.contains(&word),
            "case {word:?}: the word this crate names must be a catalog member, got \
             {catalog:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// RULE 1: an account that has never modified the setting reports `visible`, whichever Region
/// the get is asked in, and the read leaves it there.
///
/// The rows are the Regions of [`REGIONS`], so the default is not read off one Region name.
#[tokio::test]
async fn a_fresh_account_reports_managed_resources_visible() {
    let router = emulator();
    for region in REGIONS {
        let case = format!("a fresh account asked in {region}");
        assert_visibility(&router, &case, region, VISIBLE).await;
        assert_visibility(&router, &case, region, VISIBLE).await;
    }
}

/// RULE 1: the get is a REPORT, so reading cannot change what a later caller sees — a run of
/// gets with no modify between them all report the same word.
///
/// The rows are the two words the setting can hold, so the invariant is checked from each one
/// rather than only from the default.
#[tokio::test]
async fn reading_the_setting_never_changes_it() {
    /// Gets per word — enough to catch a read that consumes or advances something.
    const READS: usize = 4;
    let region = REGIONS[1];

    for word in [VISIBLE, HIDDEN] {
        let case = format!("repeated reads of {word}");
        let router = emulator();
        if word != VISIBLE {
            modify(&router, &case, region, word).await;
        }
        for read in 1..=READS {
            let case = format!("{case}, read {read}");
            assert_visibility(&router, &case, region, word).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// RULE 2: the modify puts the setting into the word it names, reports that word back, and
/// the get reads the same word out afterwards — including a read made straight after the
/// write, because the setting has no clock-owed move.
///
/// The rows are every word of the catalog crossed with the Regions of [`REGIONS`], so neither
/// the round trip nor the word is a property of one Region.
#[tokio::test]
async fn a_modify_puts_the_setting_into_the_word_it_names_and_the_get_reads_it_back() {
    for word in ec2::ops::types::MANAGED_RESOURCE_DEFAULT_VISIBILITY_VALUES {
        for region in REGIONS {
            let case = format!("modifying to {word} in {region}");
            let router = emulator();
            assert_visibility(&router, &case, region, VISIBLE).await;

            let reported = modify(&router, &case, region, word).await;
            assert_eq!(
                &reported, word,
                "case {case:?}: the modify must report the setting now in effect, expected \
                 {word:?}, got {reported:?}"
            );
            assert_visibility(&router, &case, region, word).await;
        }
    }
}

/// RULE 2: the setting PERSISTS and every modify replaces it: a sequence of writes leaves it
/// at the word its last write named, whatever it held before — including a write that asks
/// for the word already in effect, which is the same call again.
///
/// The rows are the sequence's steps, applied in order; the expectation after each step is
/// the step's own word, so the sequence can be reordered or extended without restating it.
#[tokio::test]
async fn the_setting_holds_what_the_last_modify_left() {
    let region = REGIONS[0];
    let router = emulator();
    // (case, the word the step asks for) — the word asked for is also the word expected.
    let steps = [
        ("hidden first", HIDDEN),
        ("hidden again", HIDDEN),
        ("back to visible", VISIBLE),
        ("visible again", VISIBLE),
        ("hidden from visible", HIDDEN),
    ];

    for (case, word) in steps {
        let reported = modify(&router, case, region, word).await;
        assert_eq!(
            reported, word,
            "case {case:?}: the modify must report the word it puts in effect, expected \
             {word:?}, got {reported:?}"
        );
        assert_visibility(&router, case, region, word).await;
    }
}

// ---------------------------------------------------------------------------
// The account the setting belongs to
// ---------------------------------------------------------------------------

/// RULE 3: the setting is ACCOUNT-WIDE, so a modify made in one Region is the word a get
/// reports in every Region — it is not held per Region.
///
/// The rows are the Regions the modify is made in, each checked by a get in every Region of
/// [`REGIONS`], so the rule is not read off one pair.
#[tokio::test]
async fn the_setting_is_account_wide_rather_than_a_fact_of_a_region() {
    for written_in in REGIONS {
        let router = emulator();
        let case = format!("modified in {written_in}");
        modify(&router, &case, written_in, HIDDEN).await;
        for asked_in in REGIONS {
            let case = format!("{case}, asked in {asked_in}");
            assert_visibility(&router, &case, asked_in, HIDDEN).await;
        }

        // And the same for the way back, so the rule is not a property of one word.
        let case = format!("{case}, then back to visible");
        modify(&router, &case, written_in, VISIBLE).await;
        for asked_in in REGIONS {
            let case = format!("{case}, asked in {asked_in}");
            assert_visibility(&router, &case, asked_in, VISIBLE).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// One driven call: the case's name, the `Action`, the form fields it sends, and the word the
/// setting must hold afterwards.
struct Row {
    /// What this row is, named in every assertion it drives.
    case: &'static str,
    /// The ec2Query `Action` the row calls.
    action: &'static str,
    /// The form fields the row sends beside that action.
    params: Vec<(String, String)>,
    /// The word the setting is expected to read afterwards.
    expected: &'static str,
}

/// RULE 4: `DryRun` is answered 412 `DryRunOperation` on both operations and leaves the
/// setting exactly where it was, whichever word it already held.
///
/// The rows are each operation crossed with the words the setting can hold, so the flag is
/// exercised on the reader and on the writer from each starting point — including a modify
/// that would have changed the setting and one that would have left it as it was.
#[tokio::test]
async fn a_dry_run_reports_and_changes_nothing() {
    let region = REGIONS[1];
    let rows = [
        Row {
            case: "a modify to hidden while visible",
            action: "ModifyManagedResourceVisibility",
            params: vec![p("DefaultVisibility", HIDDEN)],
            expected: VISIBLE,
        },
        Row {
            case: "a modify to visible while hidden",
            action: "ModifyManagedResourceVisibility",
            params: vec![p("DefaultVisibility", VISIBLE)],
            expected: HIDDEN,
        },
        Row {
            case: "a modify to the word already in effect",
            action: "ModifyManagedResourceVisibility",
            params: vec![p("DefaultVisibility", HIDDEN)],
            expected: HIDDEN,
        },
        Row {
            case: "a get while visible",
            action: "GetManagedResourceVisibility",
            params: Vec::new(),
            expected: VISIBLE,
        },
        Row {
            case: "a get while hidden",
            action: "GetManagedResourceVisibility",
            params: Vec::new(),
            expected: HIDDEN,
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
        if before != VISIBLE {
            modify(&router, case, region, before).await;
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
        assert_visibility(&router, case, region, before).await;
    }
}

/// RULE 4: a `DryRun` that is explicitly off is not a dry run — the request runs normally.
///
/// The rows are the writer and the reader, so the flag's off value is exercised on both
/// operations that read it.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    let region = REGIONS[2];
    let router = emulator();
    let rows = [
        Row {
            case: "a modify with DryRun=false",
            action: "ModifyManagedResourceVisibility",
            params: vec![p("DefaultVisibility", HIDDEN), p("DryRun", "false")],
            expected: HIDDEN,
        },
        Row {
            case: "a get with DryRun=false",
            action: "GetManagedResourceVisibility",
            params: vec![p("DryRun", "false")],
            expected: HIDDEN,
        },
        Row {
            case: "a modify back with DryRun=false",
            action: "ModifyManagedResourceVisibility",
            params: vec![p("DefaultVisibility", VISIBLE), p("DryRun", "false")],
            expected: VISIBLE,
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
        assert_visibility(&router, case, region, expected).await;
    }
}

/// RULE 4: a modify that names no value the setting's catalog holds is refused, and writes
/// nothing.
///
/// The rows differ along what makes the request bad: a request carrying no
/// `DefaultVisibility` at all, which the model makes `required` and the BOUNDARY refuses
/// before the handler runs; the empty string; a word the catalog does not hold; a word of the
/// SNAPSHOT block-public-access catalog, which this setting does not share; and a catalog
/// member in the wrong case. None of them is a 501, which would say the operation is
/// unimplemented.
#[tokio::test]
async fn a_modify_that_names_no_catalog_member_is_refused() {
    let region = REGIONS[3];
    let router = emulator();

    /// One refused modify: the case's name, the form fields it sends, and the HTTP status and
    /// EC2 error code it must be answered with.
    struct Refusal {
        /// What this row is, named in every assertion it drives.
        case: &'static str,
        /// The form fields the modify sends.
        params: Vec<(String, String)>,
        /// The HTTP status the refusal carries.
        status: u16,
        /// The EC2 error code the refusal carries.
        code: &'static str,
    }

    let rows = [
        Refusal {
            case: "a modify naming no visibility at all",
            params: Vec::new(),
            status: 400,
            code: "MalformedInput",
        },
        Refusal {
            case: "a modify naming the empty string",
            params: vec![p("DefaultVisibility", "")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "a modify naming a word the catalog does not hold",
            params: vec![p("DefaultVisibility", "invisible")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "a modify naming the snapshot setting's block-all-sharing",
            params: vec![p("DefaultVisibility", "block-all-sharing")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "a modify naming a catalog member in the wrong case",
            params: vec![p("DefaultVisibility", "Hidden")],
            status: 400,
            code: "ValidationError",
        },
    ];

    for row in rows {
        let (status, body) = call(
            &router,
            region,
            "ModifyManagedResourceVisibility",
            &row.params,
        )
        .await;
        let case = row.case;
        let expected_status = row.status;
        let expected_code = row.code;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from \
             ModifyManagedResourceVisibility, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} with \
             body {body}"
        );
        // Every refusal leaves the setting exactly where it was.
        assert_visibility(&router, case, region, VISIBLE).await;
    }
}
