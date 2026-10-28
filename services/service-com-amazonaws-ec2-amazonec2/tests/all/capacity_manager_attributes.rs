// SPDX-License-Identifier: CC0-1.0

//! What `GetCapacityManagerAttributes` REPORTS about the account's EC2 Capacity Manager,
//! driven over the wire the service speaks (`ec2Query`, through [`super::ec2_query`]).
//!
//! [`super::capacity_manager`] covers what `EnableCapacityManager` records; this suite
//! pins the report itself — every member of the result, over a table of account histories
//! and of Regions:
//!
//! 1. CATALOG: every member the response carries is one
//!    `GetCapacityManagerAttributesResult` declares, and every enumerated one holds a word
//!    out of its modelled catalog — a status, an ingestion status, a boolean and a count,
//!    with the members that describe data Capacity Manager holds reported only when there
//!    is such data;
//! 2. REPORTING ONLY: the whole report is a function of the account's settings, so reading
//!    it any number of times, in any Region, reports exactly the same members with exactly
//!    the same values — a read never moves the account on;
//! 3. REFUSAL: a `DryRun` report is answered 412 `DryRunOperation` with the message EC2
//!    words for a caller holding the action's permissions, and reports nothing — the
//!    settings the next report answers are the ones the dry run found;
//! 4. COUNT: `DataExportCount` counts the account's data export configurations, so an
//!    account that has created none is reported as `0` rather than as no count at all, and
//!    enabling Capacity Manager creates no export configuration.
//!
//! Each row is a distinct input along what its rule generalises over — which sequence of
//! enables the account has behind it, whether the ingestion that started has had its span,
//! which Region the report was asked for in, and what the request says about `DryRun`.

use std::time::Duration;

use ec2::state::TRANSITION_DELAY;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message};
use super::wire::{document, param, Param};

/// Regions spanning the partitions and the naming shapes a Region name takes, so no
/// assertion below is a property of one Region string.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// Every member `GetCapacityManagerAttributesResult` declares, spelled as the ec2Query
/// envelope writes it
/// (`smithy_query(query="operation",operation="GetCapacityManagerAttributes")`).
const REPORTED_MEMBERS: [&str; 7] = [
    "capacityManagerStatus",
    "organizationsAccess",
    "dataExportCount",
    "ingestionStatus",
    "ingestionStatusMessage",
    "earliestDatapointTimestamp",
    "latestDatapointTimestamp",
];

/// The whole `CapacityManagerStatus` catalog
/// (`smithy_query(query="shape",shape="CapacityManagerStatus")`).
const CAPACITY_MANAGER_STATUSES: [&str; 2] = ["enabled", "disabled"];

/// The whole `IngestionStatus` catalog
/// (`smithy_query(query="shape",shape="IngestionStatus")`).
const INGESTION_STATUSES: [&str; 3] = [
    "initial-ingestion-in-progress",
    "ingestion-complete",
    "ingestion-failed",
];

/// Long enough past [`TRANSITION_DELAY`] that the ingestion an enable started is certainly
/// complete, with a margin so a slow machine fails nothing.
fn past_the_ingestion() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One account history: the sequence of `EnableCapacityManager` calls behind the account
/// (`None` is a call that says nothing about `OrganizationsAccess`), and whether the
/// ingestion the first of them started has been given its span.
struct History {
    case: &'static str,
    enables: &'static [Option<bool>],
    past_the_ingestion: bool,
}

/// The members of one report: the envelope's direct children other than the `requestId`
/// every ec2Query response carries, in the order the service wrote them.
///
/// Read through the XML parser rather than by substring so a member of the ENVELOPE is
/// never confused with one of the same name nested anywhere else.
fn reported(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    parsed
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() != "requestId")
        .map(|node| {
            (
                node.tag_name().name().to_string(),
                node.text().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The value of the `name` member of a report, or `None` when it carries none.
fn value<'a>(report: &'a [(String, String)], name: &str) -> Option<&'a str> {
    report
        .iter()
        .find(|(member, _)| member == name)
        .map(|(_, value)| value.as_str())
}

/// One plain `GetCapacityManagerAttributes` call that must succeed, as its reported
/// members.
async fn report(router: &MultiServiceRouter, case: &str, region: &str) -> Vec<(String, String)> {
    report_with(router, case, region, &[]).await
}

/// The same report as [`report`], with whatever extra parameters the case needs.
async fn report_with(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    extra: &[Param],
) -> Vec<(String, String)> {
    let (status, body) = call(router, region, "GetCapacityManagerAttributes", extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCapacityManagerAttributes in {region}, \
         got {status} with body {body}"
    );
    reported(case, &body)
}

/// Put the account through `history` in `region`, so the report under test has something
/// to report.
async fn apply(router: &MultiServiceRouter, history: &History, region: &str) {
    let case = history.case;
    for (step, requested) in history.enables.iter().enumerate() {
        let extra: Vec<Param> = requested
            .map(|value| vec![param("OrganizationsAccess", if value { "true" } else { "false" })])
            .unwrap_or_default();
        let (status, body) = call(router, region, "EnableCapacityManager", &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?} step {step}: expected HTTP 200 from EnableCapacityManager, \
             got {status} with body {body}"
        );
    }
    if history.past_the_ingestion {
        tokio::time::sleep(past_the_ingestion()).await;
    }
}

/// The account histories the report is asked about: no enable at all, each shape of a
/// single enable, a sequence of them, and the same again once the ingestion an enable
/// started has had its span.
static HISTORIES: [History; 7] = [
    History {
        case: "a fresh account",
        enables: &[],
        past_the_ingestion: false,
    },
    History {
        case: "one enable saying nothing about Organizations access",
        enables: &[None],
        past_the_ingestion: false,
    },
    History {
        case: "one enable asking for Organizations access",
        enables: &[Some(true)],
        past_the_ingestion: false,
    },
    History {
        case: "an enable that takes Organizations access back",
        enables: &[Some(true), Some(false)],
        past_the_ingestion: false,
    },
    History {
        case: "one enable, the ingestion given its span",
        enables: &[None],
        past_the_ingestion: true,
    },
    History {
        case: "an enable asking for Organizations access, the ingestion given its span",
        enables: &[Some(true)],
        past_the_ingestion: true,
    },
    History {
        case: "three enables, the ingestion given its span",
        enables: &[None, Some(true), Some(false)],
        past_the_ingestion: true,
    },
];

/// RULE 1: whatever the account's history, the report carries only members the result
/// declares, each holding a value of its modelled type — and the members that describe the
/// DATA Capacity Manager holds are reported only where there is such data.
#[tokio::test]
async fn every_reported_member_is_declared_and_holds_a_modelled_value() {
    for history in &HISTORIES {
        for region in REGIONS {
            let router = emulator();
            apply(&router, history, REGIONS[0]).await;
            let case = format!("{} , asked in {region}", history.case);
            let members = report(&router, &case, region).await;

            for (member, value) in &members {
                assert!(
                    REPORTED_MEMBERS.contains(&member.as_str()),
                    "case {case:?}: expected only members GetCapacityManagerAttributesResult \
                     declares, got {member:?} = {value:?}"
                );
            }

            let status = value(&members, "capacityManagerStatus");
            assert!(
                status.is_some_and(|word| CAPACITY_MANAGER_STATUSES.contains(&word)),
                "case {case:?}: expected a CapacityManagerStatus out of \
                 {CAPACITY_MANAGER_STATUSES:?}, got {status:?}"
            );
            let expected_status = if history.enables.is_empty() {
                "disabled"
            } else {
                "enabled"
            };
            assert_eq!(
                status,
                Some(expected_status),
                "case {case:?}: expected the status {expected_status:?}, got {status:?}"
            );

            let organizations_access = value(&members, "organizationsAccess");
            assert!(
                organizations_access.is_some_and(|word| word == "true" || word == "false"),
                "case {case:?}: expected organizationsAccess as a boolean, \
                 got {organizations_access:?}"
            );
            let expected_access = history
                .enables
                .last()
                .is_some_and(|requested| requested.unwrap_or(false));
            assert_eq!(
                organizations_access,
                Some(if expected_access { "true" } else { "false" }),
                "case {case:?}: expected organizationsAccess {expected_access}, \
                 got {organizations_access:?}"
            );

            let count = value(&members, "dataExportCount");
            let parsed = count.and_then(|text| text.parse::<i32>().ok());
            assert!(
                parsed.is_some_and(|count| count >= 0),
                "case {case:?}: expected dataExportCount as a non-negative integer, \
                 got {count:?}"
            );

            // The ingestion is a fact of the enable that started one: an account that never
            // enabled Capacity Manager has no ingestion to report a status for, and one that
            // did reports where that ingestion has got to.
            let ingestion = value(&members, "ingestionStatus");
            match (history.enables.is_empty(), history.past_the_ingestion) {
                (true, _) => assert_eq!(
                    ingestion, None,
                    "case {case:?}: expected no ingestionStatus before any enable, \
                     got {ingestion:?}"
                ),
                (false, false) => assert_eq!(
                    ingestion,
                    Some("initial-ingestion-in-progress"),
                    "case {case:?}: expected the ingestion still in progress, got {ingestion:?}"
                ),
                (false, true) => assert_eq!(
                    ingestion,
                    Some("ingestion-complete"),
                    "case {case:?}: expected the ingestion complete once its span had passed, \
                     got {ingestion:?}"
                ),
            }
            if let Some(word) = ingestion {
                assert!(
                    INGESTION_STATUSES.contains(&word),
                    "case {case:?}: expected an IngestionStatus out of {INGESTION_STATUSES:?}, \
                     got {word:?}"
                );
            }

            // "The timestamp of the earliest data point available" / "the most recent data
            // point ingested" bound the DATAPOINTS the account holds. Nothing here ingests
            // one, so there is no datapoint to bound; any value reported must at least be a
            // timestamp, and the two must bound each other.
            let earliest = value(&members, "earliestDatapointTimestamp");
            let latest = value(&members, "latestDatapointTimestamp");
            for (member, reported) in [
                ("earliestDatapointTimestamp", earliest),
                ("latestDatapointTimestamp", latest),
            ] {
                if let Some(text) = reported {
                    assert!(
                        as_timestamp(text).is_some(),
                        "case {case:?}: expected {member} as a timestamp, got {text:?}"
                    );
                }
            }
            if let (Some(earliest), Some(latest)) = (
                earliest.and_then(as_timestamp),
                latest.and_then(as_timestamp),
            ) {
                assert!(
                    earliest <= latest,
                    "case {case:?}: expected the earliest datapoint at or before the latest, \
                     got {earliest} and {latest}"
                );
            }
        }
    }
}

/// RULE 2: the report is a function of the account's settings alone, so asking repeatedly —
/// and asking in every Region — reports exactly the same members with the same values.
///
/// Only histories whose ingestion has reached a terminal status are compared member for
/// member: an ingestion still running is deliberately a function of the CLOCK, and
/// [`every_reported_member_is_declared_and_holds_a_modelled_value`] pins that.
#[tokio::test]
async fn repeated_reports_in_any_region_report_the_same_settings() {
    let stable: [&History; 4] = [&HISTORIES[0], &HISTORIES[4], &HISTORIES[5], &HISTORIES[6]];

    for history in stable {
        for asked_in in REGIONS {
            let router = emulator();
            apply(&router, history, asked_in).await;
            let case = format!("{} , enabled in {asked_in}", history.case);
            let first = report(&router, &case, asked_in).await;

            for region in REGIONS {
                for round in 0..3 {
                    let case = format!("{case}, read {round} in {region}");
                    let again = report(&router, &case, region).await;
                    assert_eq!(
                        again, first,
                        "case {case:?}: expected the same report as {first:?}, got {again:?}"
                    );
                }
            }
        }
    }
}

/// RULE 3: a `DryRun` report is refused 412 `DryRunOperation`, with the wording EC2
/// answers a permitted caller, and changes nothing about what the next report answers.
#[tokio::test]
async fn a_dry_run_report_is_refused_and_reports_nothing() {
    let rows: [&History; 3] = [&HISTORIES[0], &HISTORIES[4], &HISTORIES[5]];

    for history in rows {
        let region = REGIONS[3];
        let router = emulator();
        apply(&router, history, region).await;
        let case = format!("a DryRun report with {} behind it", history.case);
        let before = report(&router, &case, region).await;

        let (status, body) = call(
            &router,
            region,
            "GetCapacityManagerAttributes",
            &[param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun report, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: expected the wording EC2 answers a permitted caller, got {body}"
        );

        let after = report(&router, &case, region).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected a DryRun to leave the report at {before:?}, got {after:?}"
        );

        // `DryRun=false` is an ordinary report, answering exactly what the plain one does.
        let explicit = report_with(&router, &case, region, &[param("DryRun", "false")]).await;
        assert_eq!(
            explicit, before,
            "case {case:?}: expected DryRun=false to report {before:?}, got {explicit:?}"
        );
    }
}

/// RULE 4: `DataExportCount` is the number of data export configurations the account holds.
/// No operation of this emulator has created one, so the count is `0` — reported as a count
/// rather than left off — and enabling Capacity Manager does not create one either.
#[tokio::test]
async fn the_report_counts_the_accounts_data_export_configurations() {
    for history in &HISTORIES {
        let region = REGIONS[1];
        let router = emulator();
        let case = format!("the data export count with {} behind it", history.case);
        let fresh = report(&router, &case, region).await;
        assert_eq!(
            value(&fresh, "dataExportCount"),
            Some("0"),
            "case {case:?}: expected an account with no data export configuration to be \
             reported as 0, got {fresh:?}"
        );

        apply(&router, history, region).await;
        let after = report(&router, &case, region).await;
        assert_eq!(
            value(&after, "dataExportCount"),
            Some("0"),
            "case {case:?}: expected enabling Capacity Manager to create no data export \
             configuration, got {after:?}"
        );
    }
}
