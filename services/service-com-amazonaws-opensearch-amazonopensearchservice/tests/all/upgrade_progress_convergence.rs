// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that a SERVICE-MANAGED engine version upgrade progresses to its
//! terminal state on the service's own clock, within a bounded budget, with no call
//! from the caller to advance it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`],
//! the published compatibility table from [`super::compatible_versions`], the
//! `UpgradeDomain` / `GetUpgradeHistory` calls from [`super::upgrade_history`] and the
//! `GetUpgradeStatus` call from [`super::upgrade_status`]; this suite holds no second
//! copy of any of them.
//!
//! The rule under test: an engine version upgrade is work the SERVICE performs, so the
//! only thing that carries it forward is the clock — a caller observes it, and within
//! the bounded span the service owes the run every observation of it agrees on the same
//! terminal outcome: `GetUpgradeStatus` reports the run's LAST step terminal under the
//! name that describes the run, `GetUpgradeHistory`'s newest entry carries that same
//! terminal status with every step the run performed, the domain stops processing and
//! reports no upgrade in flight, and the domain descriptor reports the engine version
//! the run was asked for. A domain that reports itself idle stays idle, so two reads
//! taken one after the other agree — which is what lets a client wait for the domain
//! before it polls the run at all.
//!
//! So the assertions span SETS of inputs rather than one call: a table of hops taken
//! from the compatibility table the service itself publishes, differing along what the
//! rule generalises over — how far the target sits from the source (the next published
//! version, and the furthest version that source reaches) and which engine line the hop
//! is on (inside the OpenSearch line, inside the Elasticsearch line, and the crossing
//! from the Elasticsearch line into the one that succeeds it) — each hop driven both by
//! a client that polls throughout and by one that makes no call at all while the run is
//! being performed, so the terminal state is shown to be the clock's work and not the
//! polling's.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    UPGRADE_STATUS_IN_PROGRESS, UPGRADE_STEP_PRE_UPGRADE_CHECK, UPGRADE_STEP_UPGRADE,
};

use super::compatible_versions::{
    compatible, entries as published_entries, rank, target_versions,
};
use super::domain_lifecycle::{string_member, Answer, Emulator, SETTLE_WAIT};
use super::upgrade_history::{entries as history_entries, expected_name, history, upgrade};
use super::upgrade_status::{reported, status};

/// The values of `UpgradeStatus` a run that has finished its work reports — the enum
/// admits `IN_PROGRESS | SUCCEEDED | SUCCEEDED_WITH_ISSUES | FAILED`
/// (`smithy_query(query="shape",shape="UpgradeStatus")`), of which these two are the
/// terminal outcomes of a run that performed what it was asked for.
const TERMINAL_STEP_STATUSES: [&str; 2] = ["SUCCEEDED", "SUCCEEDED_WITH_ISSUES"];

/// How often a wait below asks again.
///
/// Far under the span the service owes any clock move, so what a case measures is when
/// the SERVICE converges rather than when the next poll happens to land.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The budget every wait below is given: the whole progression must be observable to
/// its terminal state inside it.
///
/// A client gives its own wait a bound and treats exceeding it as a failure, so a
/// progression that needs longer than this is a hang rather than a slow success. It is
/// a generous multiple of the span the service owes a clock move, so a case that fails
/// it has not merely been unlucky with scheduling.
const BUDGET: Duration = Duration::from_secs(10);

/// One hop a case drives: the version the domain is created on, the version the upgrade
/// moves it to, and what makes the row different from the others.
struct Hop {
    case: &'static str,
    source: String,
    target: String,
}

/// The `DomainStatus` of an accepted domain read, with the case named on failure.
fn descriptor(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"].clone()
}

/// A boolean member of a descriptor, with the case named on failure.
fn flag(status: &Value, member: &str, case: &str) -> bool {
    status[member]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case}: expected a boolean {member}, got {status:?}"))
}

/// The hops every case below is asserted over, read from the compatibility table the
/// SERVICE publishes (`GetCompatibleVersions`) rather than written out here, so the rows
/// are pairs this service admits rather than a hand-written list that can drift from it.
///
/// The rows differ along what the rule generalises over: how far the target sits from
/// the source (the next published version, and the furthest version that source
/// reaches) and which engine line the hop is on (inside the OpenSearch line, inside the
/// Elasticsearch line, and the crossing into the line that succeeds it).
async fn hops(emulator: &Emulator) -> Vec<Hop> {
    let case = "published compatibility table";
    let answer = compatible(emulator, None).await;
    let table: Vec<(String, Vec<String>)> = published_entries(&answer, case)
        .iter()
        .map(|entry| {
            (
                string_member(entry, "SourceVersion", case),
                target_versions(entry, case),
            )
        })
        .collect();

    // The oldest published source of a line that has anywhere to go, compared as a
    // PARSED version rather than taken from the order the table reported: the oldest
    // version of a line is the one with the most targets to pick a near and a far hop
    // from.
    let oldest_source = |prefix: &str| -> (String, Vec<String>) {
        table
            .iter()
            .filter(|(source, targets)| source.starts_with(prefix) && !targets.is_empty())
            .min_by_key(|(source, _)| rank(source))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a published {prefix} source with targets, got {table:?}")
            })
    };

    let (open_search, open_search_targets) = oldest_source("OpenSearch_");
    assert!(
        open_search_targets.len() >= 2,
        "case {case}: expected the oldest OpenSearch source to reach more than one \
         version, so a near and a far hop differ, got {open_search_targets:?}"
    );
    let (elasticsearch, elasticsearch_targets) = oldest_source("Elasticsearch_");

    // Crossing engine lines is the last published version of a line's privilege, so the
    // crossing row is whichever Elasticsearch source reaches the OpenSearch line at all.
    let (crossing, crossing_targets) = table
        .iter()
        .filter(|(source, targets)| {
            source.starts_with("Elasticsearch_")
                && targets.iter().any(|target| target.starts_with("OpenSearch_"))
        })
        .min_by_key(|(source, _)| rank(source))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case}: expected one Elasticsearch source to reach the OpenSearch line, got {table:?}")
        });

    vec![
        Hop {
            case: "the next published version on the OpenSearch line",
            source: open_search.clone(),
            target: nearest(&open_search_targets, "OpenSearch_", case),
        },
        Hop {
            case: "the furthest version the OpenSearch source reaches",
            source: open_search,
            target: furthest(&open_search_targets, case),
        },
        Hop {
            case: "the next published version on the Elasticsearch line",
            source: elasticsearch,
            target: nearest(&elasticsearch_targets, "Elasticsearch_", case),
        },
        Hop {
            case: "the crossing into the engine line that succeeds it",
            source: crossing,
            target: nearest(&crossing_targets, "OpenSearch_", case),
        },
    ]
}

/// The nearest target of a set that is on `prefix`'s engine line: the lowest-ranked
/// one, compared as a PARSED version, so the row stays the intended one whatever order
/// the published table reports its targets in.
fn nearest(targets: &[String], prefix: &str, case: &str) -> String {
    targets
        .iter()
        .filter(|target| target.starts_with(prefix))
        .min_by_key(|target| rank(target))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case}: expected a published {prefix} target, got {targets:?}")
        })
}

/// The furthest target of a set: the highest-ranked one, on whichever line it is.
fn furthest(targets: &[String], case: &str) -> String {
    targets
        .iter()
        .max_by_key(|target| rank(target))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a published target, got {targets:?}"))
}

/// Create a domain on `source` and wait until it reports itself idle, answering the
/// descriptor the last read saw.
///
/// The transient state is observable: the answer `CreateDomain` itself returns reports
/// the domain processing, which is why a client waits at all.
async fn idle_domain(emulator: &Emulator, name: &str, source: &str, case: &str) -> Value {
    let created = descriptor(
        &emulator
            .create(json!({ "DomainName": name, "EngineVersion": source }))
            .await,
        case,
    );
    assert!(
        flag(&created, "Processing", case),
        "case {case}: expected a domain just created to report itself processing, got {created:?}"
    );
    assert_eq!(
        string_member(&created, "EngineVersion", case),
        source,
        "case {case}: expected the domain on the version it was created with, got {created:?}"
    );

    let started = Instant::now();
    loop {
        let status = descriptor(&emulator.describe(name).await, case);
        if !flag(&status, "Processing", case) && !flag(&status, "UpgradeProcessing", case) {
            // Idleness is a settled state, not a flicker: the very next read agrees, so
            // a client that needs two consecutive idle observations gets them.
            let again = descriptor(&emulator.describe(name).await, case);
            assert!(
                !flag(&again, "Processing", case) && !flag(&again, "UpgradeProcessing", case),
                "case {case}: expected an idle domain to stay idle on the next read, got {again:?}"
            );
            return again;
        }
        assert!(
            started.elapsed() < BUDGET,
            "case {case}: expected the domain to stop processing within {BUDGET:?}, still \
             {status:?} after {:?}",
            started.elapsed()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What polling a run to its terminal state observed.
struct Converged {
    step: String,
    step_status: String,
    upgrade_name: String,
}

/// Poll `GetUpgradeStatus` — and NOTHING else — until the run it reports is terminal,
/// failing the case if it has not got there inside [`BUDGET`].
async fn polled_to_terminal(emulator: &Emulator, name: &str, case: &str) -> Converged {
    let started = Instant::now();
    loop {
        let (step, step_status, upgrade_name) = reported(&status(emulator, name).await, case);
        if TERMINAL_STEP_STATUSES.contains(&step_status.as_str()) {
            return Converged {
                step,
                step_status,
                upgrade_name,
            };
        }
        assert_eq!(
            step_status, UPGRADE_STATUS_IN_PROGRESS,
            "case {case}: a run that has not finished reports its step in progress, got \
             {step_status:?} on {step:?}"
        );
        assert!(
            started.elapsed() < BUDGET,
            "case {case}: expected the run to reach a terminal step within {BUDGET:?}, still \
             {step:?} / {step_status:?} after {:?}",
            started.elapsed()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `GetUpgradeHistory`'s newest entry for a domain.
async fn newest_entry(emulator: &Emulator, name: &str, case: &str) -> Value {
    let answer = history(emulator, name, "").await;
    let entries = history_entries(&answer, case);
    entries
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected the run in the history, got {entries:?}"))
}

/// Assert that the run the history's newest entry describes finished in `step_status`,
/// having performed at least one step and ended on the upgrade itself.
fn assert_terminal_entry(entry: &Value, upgrade_name: &str, step_status: &str, case: &str) {
    assert_eq!(
        string_member(entry, "UpgradeName", case),
        upgrade_name,
        "case {case}: expected the history to name the run the status poll named, got {entry:?}"
    );
    assert_eq!(
        string_member(entry, "UpgradeStatus", case),
        step_status,
        "case {case}: expected the history to carry the terminal status the poll reported, \
         got {entry:?}"
    );
    let steps = entry["StepsList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a StepsList, got {entry:?}"));
    assert!(
        !steps.is_empty(),
        "case {case}: expected the steps the run performed, got {entry:?}"
    );
    for step in steps {
        let reported_status = string_member(step, "UpgradeStepStatus", case);
        assert!(
            TERMINAL_STEP_STATUSES.contains(&reported_status.as_str()),
            "case {case}: expected every step of a finished run terminal, got {step:?}"
        );
    }
    let last = steps
        .last()
        .unwrap_or_else(|| panic!("case {case}: the StepsList is not empty"));
    assert_eq!(
        string_member(last, "UpgradeStep", case),
        UPGRADE_STEP_UPGRADE,
        "case {case}: expected the run to end on the upgrade itself, got {last:?}"
    );
}

#[tokio::test]
async fn an_upgrade_polled_through_the_wire_converges_to_its_terminal_state() {
    let emulator = Emulator::new();
    for (index, hop) in hops(&emulator).await.into_iter().enumerate() {
        let case = format!("{} ({} -> {})", hop.case, hop.source, hop.target);
        let name = format!("upgrade-polled-{index}");
        let idle = idle_domain(&emulator, &name, &hop.source, &case).await;
        assert!(
            !flag(&idle, "UpgradeProcessing", &case),
            "case {case}: expected no upgrade in flight on a domain nothing has upgraded, \
             got {idle:?}"
        );

        let started = Instant::now();
        let answer = upgrade(&emulator, &name, &hop.target, false).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the upgrade accepted with HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !string_member(&answer.body, "UpgradeId", &case).is_empty(),
            "case {case}: expected an UpgradeId for the run started, got {:?}",
            answer.body
        );
        assert_eq!(
            string_member(&answer.body, "DomainName", &case),
            name,
            "case {case}: expected the domain upgraded named, got {:?}",
            answer.body
        );
        assert_eq!(
            string_member(&answer.body, "TargetVersion", &case),
            hop.target,
            "case {case}: expected the requested TargetVersion echoed, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["PerformCheckOnly"].as_bool(),
            Some(false),
            "case {case}: expected a run that performs the upgrade, got {:?}",
            answer.body
        );
        assert!(
            answer.body["ChangeProgressDetails"].is_object(),
            "case {case}: expected the change the upgrade started reported, got {:?}",
            answer.body
        );

        // The run is still being performed the instant it is started: a client acting
        // straight after the call sees the transient state, which is what it then waits
        // out.
        let (step, step_status, upgrade_name) =
            reported(&status(&emulator, &name).await, &case);
        assert_eq!(
            (step.as_str(), step_status.as_str()),
            (UPGRADE_STEP_PRE_UPGRADE_CHECK, UPGRADE_STATUS_IN_PROGRESS),
            "case {case}: expected a just-started run on its first step, got {step:?} / \
             {step_status:?}"
        );
        assert_eq!(
            upgrade_name,
            expected_name(&hop.source, &hop.target, false),
            "case {case}: expected the run described by the versions it goes between, got \
             {upgrade_name:?}"
        );

        // Polling the status is the ONLY call made from here: nothing else is asked to
        // carry the run forward.
        let converged = polled_to_terminal(&emulator, &name, &case).await;
        assert_eq!(
            converged.step, UPGRADE_STEP_UPGRADE,
            "case {case}: expected the run to finish on the upgrade step, got {:?}",
            converged.step
        );
        assert_eq!(
            converged.upgrade_name, upgrade_name,
            "case {case}: expected the terminal poll to name the same run, got {:?}",
            converged.upgrade_name
        );

        let entry = newest_entry(&emulator, &name, &case).await;
        assert_terminal_entry(&entry, &upgrade_name, &converged.step_status, &case);

        let landed = descriptor(&emulator.describe(&name).await, &case);
        assert_eq!(
            string_member(&landed, "EngineVersion", &case),
            hop.target,
            "case {case}: expected the domain on the version the upgrade named, got {landed:?}"
        );
        assert!(
            !flag(&landed, "UpgradeProcessing", &case),
            "case {case}: expected no upgrade in flight once the run is terminal, got {landed:?}"
        );
        assert!(
            !flag(&landed, "Processing", &case),
            "case {case}: expected the domain idle once the run is terminal, got {landed:?}"
        );
        assert!(
            started.elapsed() < BUDGET,
            "case {case}: expected the whole progression observable within {BUDGET:?}, took {:?}",
            started.elapsed()
        );
    }
}

#[tokio::test]
async fn an_upgrade_reaches_the_same_terminal_state_with_no_poll_at_all() {
    let emulator = Emulator::new();
    for (index, hop) in hops(&emulator).await.into_iter().enumerate() {
        let case = format!("unpolled {} ({} -> {})", hop.case, hop.source, hop.target);
        let name = format!("upgrade-unpolled-{index}");
        idle_domain(&emulator, &name, &hop.source, &case).await;

        let answer = upgrade(&emulator, &name, &hop.target, false).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the upgrade accepted with HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );

        // Not one call while the run is being performed: whatever carries it to its
        // terminal state is the service's own clock.
        tokio::time::sleep(SETTLE_WAIT).await;

        let (step, step_status, upgrade_name) = reported(&status(&emulator, &name).await, &case);
        assert_eq!(
            step, UPGRADE_STEP_UPGRADE,
            "case {case}: expected the unpolled run to have finished on the upgrade step, \
             got {step:?} / {step_status:?}"
        );
        assert!(
            TERMINAL_STEP_STATUSES.contains(&step_status.as_str()),
            "case {case}: expected the unpolled run terminal, got {step_status:?}"
        );
        assert_eq!(
            upgrade_name,
            expected_name(&hop.source, &hop.target, false),
            "case {case}: expected the run described by the versions it goes between, got \
             {upgrade_name:?}"
        );

        let answer = history(&emulator, &name, "").await;
        let entries = history_entries(&answer, &case);
        assert_eq!(
            entries.len(),
            1,
            "case {case}: expected the one run performed in the history, got {entries:?}"
        );
        let entry = entries
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the history is not empty"));
        assert_terminal_entry(&entry, &upgrade_name, &step_status, &case);

        let landed = descriptor(&emulator.describe(&name).await, &case);
        assert_eq!(
            string_member(&landed, "EngineVersion", &case),
            hop.target,
            "case {case}: expected the domain on the version the upgrade named, got {landed:?}"
        );
        assert!(
            !flag(&landed, "Processing", &case) && !flag(&landed, "UpgradeProcessing", &case),
            "case {case}: expected the domain idle once the run has finished, got {landed:?}"
        );
    }
}
