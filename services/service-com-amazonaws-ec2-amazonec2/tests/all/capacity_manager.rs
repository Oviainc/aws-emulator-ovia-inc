// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's EC2 Capacity Manager settings —
//! `EnableCapacityManager` and the `GetCapacityManagerAttributes` that reports them —
//! driven over the wire the service speaks (`ec2Query`, through [`super::ec2_query`]).
//!
//! The rules under test are:
//!
//! 1. REPORTING: `GetCapacityManagerAttributes` answers the account's settings and writes
//!    nothing — Capacity Manager is `disabled` until the account enables it, and one read
//!    never changes what a later read sees;
//! 2. ROUND TRIP: the enable SETS the status and the `OrganizationsAccess` the request
//!    asked for (a request that says nothing asks for the documented default, `false`),
//!    answering the settings read back out of the store, and a later report agrees;
//! 3. INGESTION: the enable "starts data ingestion", which "may take several hours to
//!    complete", so the ingestion is `initial-ingestion-in-progress` to a caller acting
//!    straight after the enable and `ingestion-complete` one
//!    [`ec2::state::TRANSITION_DELAY`] later whether or not anybody calls in between —
//!    and a repeat enable does not send a caught-up ingestion back to the start;
//! 4. ACCOUNT SCOPE: the settings are a fact of the ACCOUNT — "enables EC2 Capacity
//!    Manager for your account" names no Region — so an enable asked for in one Region is
//!    reported in every other;
//! 5. a `DryRun` request performs nothing and is answered 412 `DryRunOperation`;
//! 6. a `ClientToken` is held to the bound every client token of this crate is held to
//!    ([`MAX_CLIENT_TOKEN_CHARS`]), and a refused one stores nothing.
//!
//! Each table row is a distinct input along what its rule generalises over — what the
//! request says about `OrganizationsAccess`, which sequence of enables ran, which Region
//! the call was signed for, which operation carries the `DryRun`, and how long the token
//! is — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::state::{MAX_CLIENT_TOKEN_CHARS, TRANSITION_DELAY};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};
use super::wire::{param, Param};

/// Regions spanning the partitions and the naming shapes the Region field takes — a
/// three-part name, a four-part one, and a non-commercial partition — so no assertion
/// below is a property of one Region string.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// Long enough past [`TRANSITION_DELAY`] that the ingestion an enable started is certainly
/// complete, with a margin so a slow machine fails nothing.
fn past_the_ingestion() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The settings `GetCapacityManagerAttributes` reports in `region`: the status, the
/// Organizations access flag, the ingestion status (`None` when the response carries none
/// at all), the data export count, and the whole body so a failing case can name it.
struct Reported {
    status: Option<String>,
    organizations_access: Option<String>,
    ingestion_status: Option<String>,
    data_export_count: Option<String>,
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
        data_export_count: element(&body, "dataExportCount"),
        body,
    }
}

/// One `EnableCapacityManager` call in `region`, with whatever extra parameters the case
/// needs.
async fn enable(router: &MultiServiceRouter, region: &str, extra: &[Param]) -> (u16, String) {
    call(router, region, "EnableCapacityManager", extra).await
}

/// RULE 1: a fresh account reports Capacity Manager off in every Region, with no ingestion
/// and no datapoints, and asking twice reports the same thing — the read is
/// reporting-only.
#[tokio::test]
async fn a_fresh_account_reports_capacity_manager_disabled_in_every_region() {
    let router = emulator();
    for region in REGIONS {
        let case = format!("a fresh account in {region}");
        let first = report(&router, &case, region).await;
        assert_eq!(
            first.status.as_deref(),
            Some("disabled"),
            "case {case:?}: expected Capacity Manager disabled, got {}",
            first.body
        );
        assert_eq!(
            first.organizations_access.as_deref(),
            Some("false"),
            "case {case:?}: expected no Organizations access, got {}",
            first.body
        );
        // No enable has started an ingestion, so there is no ingestion to report a status
        // or a message for, and no datapoint to bound.
        for absent in [
            "ingestionStatus",
            "ingestionStatusMessage",
            "earliestDatapointTimestamp",
            "latestDatapointTimestamp",
        ] {
            assert!(
                element(&first.body, absent).is_none(),
                "case {case:?}: expected no {absent} member, got {}",
                first.body
            );
        }
        assert_eq!(
            first.data_export_count.as_deref(),
            Some("0"),
            "case {case:?}: expected no data export configurations, got {}",
            first.body
        );

        let second = report(&router, &case, region).await;
        assert_eq!(
            second.status, first.status,
            "case {case:?}: expected a read to leave the status at {:?}, got {:?}",
            first.status, second.status
        );
        assert_eq!(
            second.ingestion_status, first.ingestion_status,
            "case {case:?}: expected a read to start no ingestion, got {:?}",
            second.ingestion_status
        );
    }
}

/// RULE 2: whatever the request says about `OrganizationsAccess`, the enable answers the
/// status `enabled` with that flag, and a later report agrees — the write is stored, not
/// echoed.
#[tokio::test]
async fn the_enable_records_the_organizations_access_the_request_asked_for() {
    // `(case, what the request says about OrganizationsAccess, the flag it must record)`.
    let rows: [(&str, Option<&str>, &str); 3] = [
        ("the request says nothing", None, "false"),
        ("the request asks against Organizations access", Some("false"), "false"),
        ("the request asks for Organizations access", Some("true"), "true"),
    ];

    for (case, requested, expected) in rows {
        for region in REGIONS {
            let router = emulator();
            let case = format!("{case} in {region}");
            let extra: Vec<Param> = requested
                .map(|value| vec![param("OrganizationsAccess", value)])
                .unwrap_or_default();
            let (status, body) = enable(&router, region, &extra).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from EnableCapacityManager, \
                 got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "capacityManagerStatus").as_deref(),
                Some("enabled"),
                "case {case:?}: expected the status enabled, got {body}"
            );
            assert_eq!(
                element(&body, "organizationsAccess").as_deref(),
                Some(expected),
                "case {case:?}: expected organizationsAccess {expected:?}, got {body}"
            );

            // ROUND TRIP: the reader sees what the enable left behind.
            let reported = report(&router, &case, region).await;
            assert_eq!(
                reported.status.as_deref(),
                Some("enabled"),
                "case {case:?}: expected the stored status enabled, got {}",
                reported.body
            );
            assert_eq!(
                reported.organizations_access.as_deref(),
                Some(expected),
                "case {case:?}: expected the stored organizationsAccess {expected:?}, got {}",
                reported.body
            );
        }
    }
}

/// RULE 2 over SEQUENCES of enables: the account ends where its LAST enable put it, each
/// enable answers that, and a repeat of the same enable changes nothing.
#[tokio::test]
async fn the_settings_follow_the_sequence_of_enables_and_read_back() {
    // Each row is a sequence of enables: `Some(..)` is what that call says about
    // `OrganizationsAccess`, `None` a call that says nothing at all.
    let sequences: [(&str, &[Option<bool>]); 5] = [
        ("one silent enable", &[None]),
        ("one enable asking for Organizations access", &[Some(true)]),
        ("the same enable twice", &[Some(true), Some(true)]),
        (
            "an enable that takes Organizations access back",
            &[Some(true), Some(false)],
        ),
        (
            "a silent enable then one asking for Organizations access",
            &[None, Some(true)],
        ),
    ];

    for (case, sequence) in sequences {
        let region = REGIONS[1];
        let router = emulator();
        for (step, requested) in sequence.iter().enumerate() {
            let expected = if requested.unwrap_or(false) {
                "true"
            } else {
                "false"
            };
            let extra: Vec<Param> = requested
                .map(|value| vec![param("OrganizationsAccess", if value { "true" } else { "false" })])
                .unwrap_or_default();
            let (status, body) = enable(&router, region, &extra).await;
            assert_eq!(
                status, 200,
                "case {case:?} step {step}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "capacityManagerStatus").as_deref(),
                Some("enabled"),
                "case {case:?} step {step}: expected the status enabled, got {body}"
            );
            assert_eq!(
                element(&body, "organizationsAccess").as_deref(),
                Some(expected),
                "case {case:?} step {step}: expected organizationsAccess {expected:?}, \
                 got {body}"
            );
            let reported = report(&router, case, region).await;
            assert_eq!(
                reported.organizations_access.as_deref(),
                Some(expected),
                "case {case:?} step {step}: expected the stored organizationsAccess \
                 {expected:?}, got {}",
                reported.body
            );
        }
    }
}

/// RULE 3: the ingestion the enable starts is reported in progress to a caller acting
/// straight after it, and complete one [`TRANSITION_DELAY`] later — whether or not anybody
/// asked in between, and whether or not another enable ran.
#[tokio::test]
async fn the_enable_starts_an_ingestion_the_clock_completes() {
    // `(case, whether anybody polls the ingestion before the span has passed, whether a
    // second enable runs before it)`.
    let rows: [(&str, bool, bool); 4] = [
        ("nobody asks while the ingestion runs", false, false),
        ("the ingestion is polled while it runs", true, false),
        ("a second enable runs while the ingestion runs", false, true),
        (
            "the ingestion is polled and re-enabled while it runs",
            true,
            true,
        ),
    ];

    for (case, polls, re_enables) in rows {
        let region = REGIONS[2];
        let router = emulator();
        let (status, body) = enable(&router, region, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from EnableCapacityManager, \
             got {status} with body {body}"
        );

        if polls {
            let reported = report(&router, case, region).await;
            assert_eq!(
                reported.ingestion_status.as_deref(),
                Some("initial-ingestion-in-progress"),
                "case {case:?}: expected the ingestion still in progress straight after \
                 the enable, got {}",
                reported.body
            );
        }
        if re_enables {
            let (status, body) = enable(&router, region, &[param("OrganizationsAccess", "true")]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the second EnableCapacityManager, \
                 got {status} with body {body}"
            );
        }

        tokio::time::sleep(past_the_ingestion()).await;
        let reported = report(&router, case, region).await;
        assert_eq!(
            reported.ingestion_status.as_deref(),
            Some("ingestion-complete"),
            "case {case:?}: expected the ingestion complete once the span had passed, got {}",
            reported.body
        );
        // The completion is not a one-off reading: the status stays complete, and the
        // enable's own settings are untouched by it.
        let again = report(&router, case, region).await;
        assert_eq!(
            again.ingestion_status, reported.ingestion_status,
            "case {case:?}: expected the ingestion status to stay {:?}, got {:?}",
            reported.ingestion_status, again.ingestion_status
        );
        assert_eq!(
            again.status.as_deref(),
            Some("enabled"),
            "case {case:?}: expected Capacity Manager still enabled, got {}",
            again.body
        );
    }
}

/// RULE 3, the other way round: a repeat enable AFTER the ingestion has completed leaves it
/// complete — only the enable that turns the setting on starts an ingestion.
#[tokio::test]
async fn a_repeat_enable_does_not_restart_a_completed_ingestion() {
    let region = REGIONS[0];
    let router = emulator();
    let (status, body) = enable(&router, region, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the first enable\": expected HTTP 200, got {status} with body {body}"
    );
    tokio::time::sleep(past_the_ingestion()).await;

    for (step, requested) in ["true", "false"].into_iter().enumerate() {
        let case = format!("re-enable with OrganizationsAccess={requested}");
        let (status, body) = enable(&router, region, &[param("OrganizationsAccess", requested)]).await;
        assert_eq!(
            status, 200,
            "case {case:?} step {step}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = report(&router, &case, region).await;
        assert_eq!(
            reported.ingestion_status.as_deref(),
            Some("ingestion-complete"),
            "case {case:?} step {step}: expected the completed ingestion to stay complete, \
             got {}",
            reported.body
        );
        assert_eq!(
            reported.organizations_access.as_deref(),
            Some(requested),
            "case {case:?} step {step}: expected organizationsAccess {requested:?}, got {}",
            reported.body
        );
    }
}

/// RULE 4: the settings belong to the account, not to the Region the request is signed
/// for, so an enable asked for in one Region is reported in every other.
#[tokio::test]
async fn the_settings_are_a_fact_of_the_account_not_of_one_region() {
    for asked_in in REGIONS {
        let router = emulator();
        let (status, body) = enable(&router, asked_in, &[param("OrganizationsAccess", "true")]).await;
        assert_eq!(
            status, 200,
            "case \"enable in {asked_in}\": expected HTTP 200, got {status} with body {body}"
        );
        for region in REGIONS {
            let case = format!("enabled in {asked_in}, asking in {region}");
            let reported = report(&router, &case, region).await;
            assert_eq!(
                reported.status.as_deref(),
                Some("enabled"),
                "case {case:?}: expected the status enabled, got {}",
                reported.body
            );
            assert_eq!(
                reported.organizations_access.as_deref(),
                Some("true"),
                "case {case:?}: expected organizationsAccess true, got {}",
                reported.body
            );
        }
    }
}

/// RULE 5: `DryRun` is answered 412 `DryRunOperation` on both operations and leaves the
/// settings exactly where they were, whichever state the account was already in.
#[tokio::test]
async fn dry_run_reports_and_changes_nothing() {
    /// One dry-run row: whether Capacity Manager was already enabled, the operation the
    /// `DryRun` names, and whether that call also asks for Organizations access.
    struct Row {
        case: &'static str,
        enabled_before: bool,
        action: &'static str,
        asks_for_organizations_access: bool,
    }
    let rows = [
        Row {
            case: "enable, Capacity Manager off",
            enabled_before: false,
            action: "EnableCapacityManager",
            asks_for_organizations_access: false,
        },
        Row {
            case: "enable asking for Organizations access, Capacity Manager off",
            enabled_before: false,
            action: "EnableCapacityManager",
            asks_for_organizations_access: true,
        },
        Row {
            case: "enable, Capacity Manager on",
            enabled_before: true,
            action: "EnableCapacityManager",
            asks_for_organizations_access: false,
        },
        Row {
            case: "enable asking for Organizations access, Capacity Manager on",
            enabled_before: true,
            action: "EnableCapacityManager",
            asks_for_organizations_access: true,
        },
        Row {
            case: "get, Capacity Manager off",
            enabled_before: false,
            action: "GetCapacityManagerAttributes",
            asks_for_organizations_access: false,
        },
        Row {
            case: "get, Capacity Manager on",
            enabled_before: true,
            action: "GetCapacityManagerAttributes",
            asks_for_organizations_access: false,
        },
    ];
    let region = REGIONS[3];

    for Row {
        case,
        enabled_before,
        action,
        asks_for_organizations_access,
    } in rows
    {
        let router = emulator();
        if enabled_before {
            let (status, body) = enable(&router, region, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 enabling Capacity Manager, \
                 got {status} with body {body}"
            );
        }
        let expected_status = if enabled_before { "enabled" } else { "disabled" };

        let mut params = vec![param("DryRun", "true")];
        if asks_for_organizations_access {
            params.push(param("OrganizationsAccess", "true"));
        }
        let (status, body) = call(&router, region, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun {action}, \
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
            Some("false"),
            "case {case:?}: expected a DryRun to leave Organizations access off, got {}",
            reported.body
        );
    }

    // `DryRun=false` is an ordinary enable.
    let router = emulator();
    let (status, body) = enable(&router, region, &[param("DryRun", "false")]).await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    let reported = report(&router, "DryRun=false", region).await;
    assert_eq!(
        reported.status.as_deref(),
        Some("enabled"),
        "case \"DryRun=false\": expected the status enabled, got {}",
        reported.body
    );
}

/// RULE 6: a `ClientToken` is accepted up to [`MAX_CLIENT_TOKEN_CHARS`] and refused one
/// character past it, with nothing stored for the refused request; and a repeat of the same
/// token leaves the account exactly where the first call did.
#[tokio::test]
async fn a_client_token_is_bounded_and_a_repeat_of_one_changes_nothing() {
    let region = REGIONS[0];

    // One character past the published bound: refused, and the account stays off.
    let router = emulator();
    let too_long = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let (status, body) = enable(&router, region, &[param("ClientToken", &too_long)]).await;
    assert_eq!(
        status, 400,
        "case \"a token one character past the bound\": expected HTTP 400, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"a token one character past the bound\": expected InvalidParameterValue, \
         got {body}"
    );
    let reported = report(&router, "a token one character past the bound", region).await;
    assert_eq!(
        reported.status.as_deref(),
        Some("disabled"),
        "case \"a token one character past the bound\": expected a refused enable to store \
         nothing, got {}",
        reported.body
    );

    // Tokens AT and below the bound are accepted, and repeating one is idempotent.
    for length in [1, MAX_CLIENT_TOKEN_CHARS / 2, MAX_CLIENT_TOKEN_CHARS] {
        let router = emulator();
        let token = "t".repeat(length);
        let case = format!("a {length}-character token");
        for step in 0..2 {
            let (status, body) = enable(
                &router,
                region,
                &[
                    param("ClientToken", &token),
                    param("OrganizationsAccess", "true"),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} step {step}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "capacityManagerStatus").as_deref(),
                Some("enabled"),
                "case {case:?} step {step}: expected the status enabled, got {body}"
            );
            assert_eq!(
                element(&body, "organizationsAccess").as_deref(),
                Some("true"),
                "case {case:?} step {step}: expected organizationsAccess true, got {body}"
            );
        }
        let reported = report(&router, &case, region).await;
        assert_eq!(
            reported.status.as_deref(),
            Some("enabled"),
            "case {case:?}: expected the stored status enabled, got {}",
            reported.body
        );
        assert_eq!(
            reported.organizations_access.as_deref(),
            Some("true"),
            "case {case:?}: expected the stored organizationsAccess true, got {}",
            reported.body
        );
    }
}
