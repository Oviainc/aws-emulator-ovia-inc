// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisableCapacityManager` — what turning the account's EC2 Capacity
//! Manager OFF records, and what the report that answers those settings
//! (`GetCapacityManagerAttributes`) says afterwards — driven over the wire the service
//! speaks (`ec2Query`, through [`super::ec2_query`]).
//!
//! [`super::capacity_manager`] covers the enable and the report; this suite pins the
//! disable:
//!
//! 1. ROUND TRIP: the disable SETS the account's status to `disabled` and its
//!    `OrganizationsAccess` to `false` — "indicates whether Organizations access is
//!    enabled. This will be false after disabling Capacity Manager" — answers those
//!    settings read back out of the store, and a later report agrees, whatever history the
//!    account had behind it;
//! 2. NOTHING TO DISABLE: the disable "stops data ingestion and removes access to capacity
//!    analytics", which an account that has Capacity Manager off has neither of, so a
//!    disable of one is refused `CapacityManager.Disabled` — an account that never enabled it
//!    and one a disable already turned off alike — and the account stays exactly where it
//!    was;
//! 3. INGESTION: the disable "stops data ingestion", so the ingestion an earlier enable
//!    started is gone — no `IngestionStatus` is reported at all — and an enable that comes
//!    after one starts a FRESH ingestion, in progress straight away and complete one
//!    [`TRANSITION_DELAY`] later, whether or not the stopped one had already caught up;
//! 4. ACCOUNT SCOPE: the settings are a fact of the ACCOUNT — "disables EC2 Capacity
//!    Manager for your account" names no Region — so a disable asked for in one Region is
//!    reported in every other;
//! 5. a `DryRun` disable performs nothing and is answered 412 `DryRunOperation`;
//! 6. a `ClientToken` is held to the bound every client token of this crate is held to
//!    ([`MAX_CLIENT_TOKEN_CHARS`]), a refused disable changes nothing, and repeating one
//!    token answers the disable it made rather than the refusal a second disable would get —
//!    "unique, case-sensitive identifier that you provide to ensure the idempotency of the
//!    request".
//!
//! Each table row is a distinct input along what its rule generalises over — which
//! sequence of enables and disables the account has behind it, whether the ingestion that
//! was running had had its span, which Region the call was signed for, and how long the
//! token is — so no row is the specification.

use std::time::Duration;

use ec2::state::{MAX_CLIENT_TOKEN_CHARS, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};
use super::wire::{param, Param};

/// Regions spanning the partitions and the naming shapes a Region name takes, so no
/// assertion below is a property of one Region string.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// Long enough past [`TRANSITION_DELAY`] that an ingestion an enable started is certainly
/// complete, with a margin so a slow machine fails nothing.
fn past_the_ingestion() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One step of an account's history: what was called, and what it said about
/// `OrganizationsAccess`.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// `EnableCapacityManager`, with what the request says about `OrganizationsAccess`
    /// (`None` is a request that says nothing).
    Enable(Option<bool>),
    /// `DisableCapacityManager`.
    Disable,
    /// Nobody calls, but [`TRANSITION_DELAY`] passes — so an ingestion that was running
    /// catches up.
    Wait,
}

/// The code EC2 answers a disable of an account that has Capacity Manager off with.
const DISABLED_ERROR: &str = "CapacityManager.Disabled";

/// Whether the account `history` leaves EC2 Capacity Manager ON: the last enable or disable
/// in it decides, and a history with neither leaves the account where a fresh one sits — off.
fn leaves_the_account_on(history: &[Step]) -> bool {
    history
        .iter()
        .rev()
        .find_map(|step| match step {
            Step::Enable(_) => Some(true),
            Step::Disable => Some(false),
            Step::Wait => None,
        })
        .unwrap_or(false)
}

/// Run `history` in `region`, asserting each call was answered the way its own rule says.
async fn run(router: &MultiServiceRouter, case: &str, region: &str, history: &[Step]) {
    // What the account's status is as the history runs, so each step asserts exactly ONE
    // outcome: RULE 2 says a disable is served while Capacity Manager is on and refused once
    // it is off, and the steps before this one decide which of the two it is.
    let mut account_is_on = false;
    for (index, step) in history.iter().enumerate() {
        match step {
            Step::Enable(organizations_access) => {
                let extra: Vec<Param> = organizations_access
                    .map(|value| vec![param("OrganizationsAccess", if value { "true" } else { "false" })])
                    .unwrap_or_default();
                let (status, body) = call(router, region, "EnableCapacityManager", &extra).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} step {index} ({step:?}): expected HTTP 200 from \
                     EnableCapacityManager, got {status} with body {body}"
                );
                account_is_on = true;
            }
            Step::Disable => {
                let (status, body) = call(router, region, "DisableCapacityManager", &[]).await;
                if account_is_on {
                    assert_eq!(
                        status, 200,
                        "case {case:?} step {index} ({step:?}): expected HTTP 200 from \
                         DisableCapacityManager, got {status} with body {body}"
                    );
                    account_is_on = false;
                } else {
                    assert_eq!(
                        status, 400,
                        "case {case:?} step {index} ({step:?}): expected HTTP 400 disabling \
                         an account that is already off, got {status} with body {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        DISABLED_ERROR,
                        "case {case:?} step {index} ({step:?}): expected \
                         {DISABLED_ERROR}, got {body}"
                    );
                }
            }
            Step::Wait => tokio::time::sleep(past_the_ingestion()).await,
        }
    }
    assert_eq!(
        account_is_on,
        leaves_the_account_on(history),
        "case {case:?}: the history's own reading of where it leaves the account must agree \
         with what its calls were answered"
    );
}

/// The settings `GetCapacityManagerAttributes` reports in `region`.
struct Reported {
    status: Option<String>,
    organizations_access: Option<String>,
    ingestion_status: Option<String>,
    body: String,
}

async fn report(router: &MultiServiceRouter, case: &str, region: &str) -> Reported {
    let (status, body) = call(router, region, "GetCapacityManagerAttributes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCapacityManagerAttributes in {region}, \
         got {status} with body {body}"
    );
    Reported {
        status: element(&body, "capacityManagerStatus"),
        organizations_access: element(&body, "organizationsAccess"),
        ingestion_status: element(&body, "ingestionStatus"),
        body,
    }
}

/// Assert `body` is a disable result reporting the account off with no Organizations
/// access.
fn assert_disabled_result(case: &str, body: &str) {
    assert_eq!(
        element(body, "capacityManagerStatus").as_deref(),
        Some("disabled"),
        "case {case:?}: expected the status disabled, got {body}"
    );
    assert_eq!(
        element(body, "organizationsAccess").as_deref(),
        Some("false"),
        "case {case:?}: expected organizationsAccess false, got {body}"
    );
}

/// RULE 1, RULE 2 and RULE 3: whatever the account's history, a disable of an account that is
/// ON answers `disabled` with no Organizations access and one of an account that is already
/// off is refused `CapacityManager.Disabled`; either way a later report reads the account off,
/// with no ingestion at all.
#[tokio::test]
async fn the_disable_turns_the_account_off_whatever_its_history() {
    // Each row is a history the disable runs after: histories differing in whether an
    // enable ever ran, what it asked for about Organizations access, how many ran, and
    // whether the ingestion one started had had its span.
    let rows: [(&str, &[Step]); 6] = [
        ("an account that never enabled Capacity Manager", &[]),
        ("a silent enable", &[Step::Enable(None)]),
        (
            "an enable asking for Organizations access",
            &[Step::Enable(Some(true))],
        ),
        (
            "an enable whose ingestion has caught up",
            &[Step::Enable(Some(true)), Step::Wait],
        ),
        (
            "two enables, the last asking for Organizations access",
            &[Step::Enable(Some(false)), Step::Enable(Some(true))],
        ),
        (
            "an account disabled and enabled again",
            &[Step::Enable(Some(true)), Step::Disable, Step::Enable(None)],
        ),
    ];

    for (case, history) in rows {
        let region = REGIONS[0];
        let router = emulator();
        run(&router, case, region, history).await;

        let (status, body) = call(&router, region, "DisableCapacityManager", &[]).await;
        if leaves_the_account_on(history) {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DisableCapacityManager, \
                 got {status} with body {body}"
            );
            assert_disabled_result(case, &body);
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 disabling an account that is already off, \
                 got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                DISABLED_ERROR,
                "case {case:?}: expected {DISABLED_ERROR}, got {body}"
            );
        }

        // ROUND TRIP: the reader sees what the disable left behind, not what it answered.
        let reported = report(&router, case, region).await;
        assert_eq!(
            reported.status.as_deref(),
            Some("disabled"),
            "case {case:?}: expected the stored status disabled, got {}",
            reported.body
        );
        assert_eq!(
            reported.organizations_access.as_deref(),
            Some("false"),
            "case {case:?}: expected the stored organizationsAccess false, got {}",
            reported.body
        );
        // The disable stopped the ingestion, so there is none to report a status for.
        assert_eq!(
            reported.ingestion_status, None,
            "case {case:?}: expected no ingestionStatus once the ingestion was stopped, \
             got {:?} in {}",
            reported.ingestion_status, reported.body
        );
    }
}

/// RULE 2: only the FIRST disable of an account that is on is served; every disable after it
/// is refused `CapacityManager.Disabled`, and the account stays exactly where the first one
/// put it.
#[tokio::test]
async fn repeating_the_disable_is_refused_and_changes_nothing() {
    // Rows differ in how many disables run and what ran before them.
    let rows: [(&str, &[Step], usize); 4] = [
        ("three disables on a fresh account", &[], 3),
        ("two disables after a silent enable", &[Step::Enable(None)], 2),
        (
            "four disables after an enable asking for Organizations access",
            &[Step::Enable(Some(true))],
            4,
        ),
        (
            "two disables after an enable whose ingestion caught up",
            &[Step::Enable(None), Step::Wait],
            2,
        ),
    ];

    for (case, history, repeats) in rows {
        let region = REGIONS[1];
        let router = emulator();
        run(&router, case, region, history).await;
        // Only a disable made while the account is ON has anything to stop, so at most the
        // first of the repeats is served.
        let mut account_is_on = leaves_the_account_on(history);

        for repeat in 0..repeats {
            let (status, body) = call(&router, region, "DisableCapacityManager", &[]).await;
            if account_is_on {
                assert_eq!(
                    status, 200,
                    "case {case:?} repeat {repeat}: expected HTTP 200, \
                     got {status} with body {body}"
                );
                assert_disabled_result(&format!("{case} repeat {repeat}"), &body);
                account_is_on = false;
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?} repeat {repeat}: expected HTTP 400 for a disable of an \
                     account that is already off, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    DISABLED_ERROR,
                    "case {case:?} repeat {repeat}: expected {DISABLED_ERROR}, got {body}"
                );
            }
            let reported = report(&router, case, region).await;
            assert_eq!(
                reported.status.as_deref(),
                Some("disabled"),
                "case {case:?} repeat {repeat}: expected the account still disabled, got {}",
                reported.body
            );
            assert_eq!(
                reported.ingestion_status, None,
                "case {case:?} repeat {repeat}: expected no ingestionStatus, got {:?}",
                reported.ingestion_status
            );
        }
    }
}

/// RULE 3: the enable AFTER a disable starts a fresh ingestion — in progress straight
/// after it and complete one [`TRANSITION_DELAY`] later — whether or not the ingestion the
/// disable stopped had already caught up.
#[tokio::test]
async fn an_enable_after_a_disable_starts_a_fresh_ingestion() {
    // Rows differ in how far the ingestion the disable stopped had got.
    let rows: [(&str, &[Step]); 3] = [
        (
            "disabled while the first ingestion was still running",
            &[Step::Enable(None), Step::Disable],
        ),
        (
            "disabled after the first ingestion had caught up",
            &[Step::Enable(None), Step::Wait, Step::Disable],
        ),
        (
            "disabled twice after the first ingestion had caught up",
            &[
                Step::Enable(Some(true)),
                Step::Wait,
                Step::Disable,
                Step::Disable,
            ],
        ),
    ];

    for (case, history) in rows {
        let region = REGIONS[2];
        let router = emulator();
        run(&router, case, region, history).await;

        // Nothing is ingesting while the account is off.
        let reported = report(&router, case, region).await;
        assert_eq!(
            reported.ingestion_status, None,
            "case {case:?}: expected no ingestionStatus while the account is off, got {:?}",
            reported.ingestion_status
        );

        let (status, body) = call(&router, region, "EnableCapacityManager", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 re-enabling Capacity Manager, \
             got {status} with body {body}"
        );
        let reported = report(&router, case, region).await;
        assert_eq!(
            reported.ingestion_status.as_deref(),
            Some("initial-ingestion-in-progress"),
            "case {case:?}: expected a fresh ingestion in progress straight after the \
             re-enable, got {}",
            reported.body
        );

        tokio::time::sleep(past_the_ingestion()).await;
        let reported = report(&router, case, region).await;
        assert_eq!(
            reported.ingestion_status.as_deref(),
            Some("ingestion-complete"),
            "case {case:?}: expected the fresh ingestion complete once the span had passed, \
             got {}",
            reported.body
        );
        assert_eq!(
            reported.status.as_deref(),
            Some("enabled"),
            "case {case:?}: expected Capacity Manager enabled again, got {}",
            reported.body
        );
    }
}

/// RULE 4: the settings belong to the account, not to the Region the request is signed
/// for, so a disable asked for in one Region is reported in every other — including one
/// enabled from a third.
#[tokio::test]
async fn the_disable_is_a_fact_of_the_account_not_of_one_region() {
    for enabled_in in REGIONS {
        for disabled_in in REGIONS {
            let router = emulator();
            let case = format!("enabled in {enabled_in}, disabled in {disabled_in}");
            run(&router, &case, enabled_in, &[Step::Enable(Some(true))]).await;
            let (status, body) = call(&router, disabled_in, "DisableCapacityManager", &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DisableCapacityManager, \
                 got {status} with body {body}"
            );
            assert_disabled_result(&case, &body);

            for region in REGIONS {
                let case = format!("{case}, asking in {region}");
                let reported = report(&router, &case, region).await;
                assert_eq!(
                    reported.status.as_deref(),
                    Some("disabled"),
                    "case {case:?}: expected the status disabled, got {}",
                    reported.body
                );
                assert_eq!(
                    reported.organizations_access.as_deref(),
                    Some("false"),
                    "case {case:?}: expected organizationsAccess false, got {}",
                    reported.body
                );
            }
        }
    }
}

/// RULE 5: a `DryRun` disable is answered 412 `DryRunOperation` and leaves the settings
/// exactly where they were, whichever state the account was already in.
#[tokio::test]
async fn a_dry_run_disable_changes_nothing() {
    /// One dry-run row: the history behind the dry run, and the settings it must leave.
    struct Row {
        case: &'static str,
        history: &'static [Step],
        expected_status: &'static str,
        expected_organizations_access: &'static str,
    }
    let rows = [
        Row {
            case: "a fresh account",
            history: &[],
            expected_status: "disabled",
            expected_organizations_access: "false",
        },
        Row {
            case: "a silently enabled account",
            history: &[Step::Enable(None)],
            expected_status: "enabled",
            expected_organizations_access: "false",
        },
        Row {
            case: "an account with Organizations access",
            history: &[Step::Enable(Some(true))],
            expected_status: "enabled",
            expected_organizations_access: "true",
        },
        Row {
            case: "an account already disabled",
            history: &[Step::Enable(Some(true)), Step::Disable],
            expected_status: "disabled",
            expected_organizations_access: "false",
        },
    ];
    let region = REGIONS[3];

    for Row {
        case,
        history,
        expected_status,
        expected_organizations_access,
    } in rows
    {
        let router = emulator();
        run(&router, case, region, history).await;

        let (status, body) = call(
            &router,
            region,
            "DisableCapacityManager",
            &[param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun DisableCapacityManager, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );

        let reported = report(&router, case, region).await;
        assert_eq!(
            reported.status.as_deref(),
            Some(expected_status),
            "case {case:?}: expected a DryRun to leave the status at {expected_status:?}, \
             got {}",
            reported.body
        );
        assert_eq!(
            reported.organizations_access.as_deref(),
            Some(expected_organizations_access),
            "case {case:?}: expected a DryRun to leave organizationsAccess at \
             {expected_organizations_access:?}, got {}",
            reported.body
        );
    }

    // `DryRun=false` is an ordinary disable.
    let router = emulator();
    run(&router, "DryRun=false", region, &[Step::Enable(Some(true))]).await;
    let (status, body) = call(
        &router,
        region,
        "DisableCapacityManager",
        &[param("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    assert_disabled_result("DryRun=false", &body);
}

/// RULE 6: a `ClientToken` is accepted up to [`MAX_CLIENT_TOKEN_CHARS`] and refused one
/// character past it, with the refused disable changing nothing; and repeating one token
/// leaves the account exactly where the first call did.
#[tokio::test]
async fn a_client_token_is_bounded_and_a_repeat_of_one_changes_nothing() {
    let region = REGIONS[0];

    // One character past the published bound: refused, and the enabled account stays on.
    let router = emulator();
    let case = "a token one character past the bound";
    run(&router, case, region, &[Step::Enable(Some(true))]).await;
    let too_long = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let (status, body) = call(
        &router,
        region,
        "DisableCapacityManager",
        &[param("ClientToken", &too_long)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );
    let reported = report(&router, case, region).await;
    assert_eq!(
        reported.status.as_deref(),
        Some("enabled"),
        "case {case:?}: expected a refused disable to change nothing, got {}",
        reported.body
    );
    assert_eq!(
        reported.organizations_access.as_deref(),
        Some("true"),
        "case {case:?}: expected a refused disable to leave organizationsAccess true, got {}",
        reported.body
    );

    // Tokens AT and below the bound are accepted, and repeating one is idempotent.
    for length in [1, MAX_CLIENT_TOKEN_CHARS / 2, MAX_CLIENT_TOKEN_CHARS] {
        let router = emulator();
        let case = format!("a {length}-character token");
        let token = "t".repeat(length);
        run(&router, &case, region, &[Step::Enable(Some(true))]).await;
        for step in 0..2 {
            let (status, body) = call(
                &router,
                region,
                "DisableCapacityManager",
                &[param("ClientToken", &token)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} step {step}: expected HTTP 200, got {status} with body {body}"
            );
            assert_disabled_result(&format!("{case} step {step}"), &body);
        }
        let reported = report(&router, &case, region).await;
        assert_eq!(
            reported.status.as_deref(),
            Some("disabled"),
            "case {case:?}: expected the stored status disabled, got {}",
            reported.body
        );
    }
}
