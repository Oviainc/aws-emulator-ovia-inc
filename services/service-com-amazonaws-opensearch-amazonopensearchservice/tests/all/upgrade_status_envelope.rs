// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that `GetUpgradeStatus` answers the WHOLE modelled response.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`], the `UpgradeDomain` call
//! from [`super::upgrade_history`], and the `GetUpgradeStatus` call, the progress table
//! and its seeding from [`super::upgrade_status`]; this suite adds no second copy of any
//! of them.
//!
//! The rule under test: every member of the response — the step the reported run is on,
//! that step's status and the run's description — is REPORTED on every answer, carrying
//! the value the domain's upgrade record yields or an explicit null where that record
//! leaves it unset, and the answer carries no member the model does not declare. That is
//! what lets a client tell "no upgrade on record" (the member, reported as null) from
//! "the member was not reported at all", and what lets one that checks the body against
//! the modelled output shape read it at all. The sibling suites assert WHICH step and
//! status are reported; this one asserts that the three members are always there.
//!
//! So the assertions span SETS of inputs rather than one call: the twelve-row progress
//! table of [`super::upgrade_status::PROGRESS`] (progress through the span × kind of
//! run), four runs driven live through the wire (an upgrade and an eligibility check,
//! each read in flight and once the clock has made it due), and four domains with no run
//! on record at all (differing in engine line, version and whether the domain has
//! finished provisioning), each polled repeatedly.

use serde_json::{json, Map, Value};

use super::domain_lifecycle::{Answer, Emulator, SETTLE_WAIT};
use super::upgrade_history::{settled_domain, upgrade};
use super::upgrade_status::{seed_run, status, PROGRESS};

/// The members `GetUpgradeStatusResponse` declares, as
/// `smithy_query(query="operation", operation="GetUpgradeStatus")` reports them:
/// `UpgradeStep` (the `UpgradeStep` enum), `StepStatus` (the `UpgradeStatus` enum) and
/// `UpgradeName` (a string). Every answer carries all three and nothing else.
const ENVELOPE: [&str; 3] = ["UpgradeStep", "StepStatus", "UpgradeName"];

/// How many times a case polls the status, so an answer is asserted to be what a client
/// sees however often it asks.
const POLLS: usize = 3;

/// The engine version the seeded rows create their domain on — the one
/// [`seed_run`] records its runs as starting from.
const SEEDED_SOURCE: &str = "OpenSearch_1.0";

/// The members of an accepted read, as the JSON OBJECT the answer carries — so a member
/// reported as null is told apart from one not reported at all.
fn envelope(answer: &Answer, case: &str) -> Map<String, Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a JSON object body, got {:?}", answer.body))
}

/// Assert the answer reports every modelled member and nothing besides, each one either
/// a string or an explicit null — a null exactly where `reports_a_run` says the domain
/// has no run to report on.
fn assert_whole_envelope(answer: &Answer, case: &str, reports_a_run: bool) {
    let members = envelope(answer, case);
    let reported: Vec<&str> = members.keys().map(String::as_str).collect();
    let mut sorted = reported.clone();
    sorted.sort_unstable();
    let mut expected = ENVELOPE;
    expected.sort_unstable();
    assert_eq!(
        sorted,
        expected.to_vec(),
        "case {case}: expected the members {expected:?} and no others, got {reported:?}"
    );
    for member in ENVELOPE {
        let Some(value) = members.get(member) else {
            panic!("case {case}: expected {member} reported, got {members:?}");
        };
        match value {
            Value::String(text) => assert!(
                reports_a_run,
                "case {case}: expected {member} reported as null, got {text:?}"
            ),
            Value::Null => assert!(
                !reports_a_run,
                "case {case}: expected {member} to carry what the run reports, got null"
            ),
            other => panic!("case {case}: expected {member} as a string or a null, got {other:?}"),
        }
    }
}

/// Create a domain and answer the case name, WITHOUT waiting for its provisioning span:
/// a suite that creates many domains waits once for all of them rather than once each.
async fn create(emulator: &Emulator, name: &str, engine_version: &str) {
    let created = emulator
        .create(json!({ "DomainName": name, "EngineVersion": engine_version }))
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}, {engine_version:?}): expected HTTP 200, got {} with {:?}",
        created.status, created.body
    );
}

#[tokio::test]
async fn every_answer_reports_each_modelled_member() {
    let emulator = Emulator::new();

    // One domain per row, so the run each row seeds is that domain's latest, and one
    // wait for the provisioning span of all of them.
    let names: Vec<String> = PROGRESS
        .iter()
        .map(|row| {
            format!(
                "envelope-{}-{}",
                row.elapsed_per_mille,
                u8::from(row.check_only)
            )
        })
        .collect();
    for name in &names {
        create(&emulator, name, SEEDED_SOURCE).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    // Whatever step a run has reached, and whichever kind of run it is, the same three
    // members are reported — here each carrying what the run reports.
    for (row, name) in PROGRESS.iter().zip(&names) {
        let case = format!(
            "{}/1000 through {}",
            row.elapsed_per_mille,
            if row.check_only {
                "a check"
            } else {
                "an upgrade"
            }
        );
        seed_run(&emulator, name, row);
        for poll in 0..POLLS {
            let case = format!("{case}, poll {poll}");
            assert_whole_envelope(&status(&emulator, name).await, &case, true);
        }
    }

    // And runs driven live through the wire: an upgrade and an eligibility check, each
    // read while it is in flight and again once the clock has made it due.
    for (index, (source, target, check_only)) in [
        ("OpenSearch_2.11", "OpenSearch_2.17", false),
        ("OpenSearch_1.3", "OpenSearch_2.19", true),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("envelope-live-{index}");
        settled_domain(&emulator, &name, source).await;
        let started = upgrade(&emulator, &name, target, check_only).await;
        assert_eq!(
            started.status, 200,
            "case live {source} -> {target}: expected UpgradeDomain to answer HTTP 200, \
             got {} with {:?}",
            started.status, started.body
        );
        assert_whole_envelope(
            &status(&emulator, &name).await,
            &format!("live {source} -> {target} in flight"),
            true,
        );
        tokio::time::sleep(SETTLE_WAIT).await;
        assert_whole_envelope(
            &status(&emulator, &name).await,
            &format!("live {source} -> {target} landed"),
            true,
        );
    }
}

#[tokio::test]
async fn a_member_with_nothing_to_report_is_a_null_and_not_an_omission() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: the engine line and version
    // the domain runs, and whether its provisioning span has fallen due — none of which
    // is a run, so every one of them has nothing to report and reports a null for each
    // member rather than leaving it out.
    let quiet = [
        ("envelope-null-opensearch", "OpenSearch_2.11", true),
        ("envelope-null-elasticsearch", "Elasticsearch_7.10", true),
        ("envelope-null-latest", "OpenSearch_2.19", true),
        ("envelope-null-creating", "OpenSearch_1.3", false),
    ];
    for (name, engine_version, settle) in quiet {
        if settle {
            settled_domain(&emulator, name, engine_version).await;
        } else {
            create(&emulator, name, engine_version).await;
        }
        for poll in 0..POLLS {
            let case = format!("{name} on {engine_version}, poll {poll}");
            let members = envelope(&status(&emulator, name).await, &case);
            for member in ENVELOPE {
                let reported = members.get(member);
                assert_eq!(
                    reported,
                    Some(&Value::Null),
                    "case {case}: expected {member} reported as null, got {reported:?} \
                     in {members:?}"
                );
            }
            assert_whole_envelope(&status(&emulator, name).await, &case, false);
        }
    }
}
