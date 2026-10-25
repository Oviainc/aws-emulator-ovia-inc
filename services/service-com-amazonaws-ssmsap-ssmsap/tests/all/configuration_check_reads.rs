// SPDX-License-Identifier: CC0-1.0

//! What a READ of a configuration check operation may and may not do.
//!
//! `tests/all/configuration_checks.rs` pins what `StartConfigurationChecks` files and
//! that each record reads back; this suite pins the rule that belongs to the READER,
//! `GetConfigurationCheckOperation`, and drives the same harness over the same wire.
//!
//! RULE (a report changes nothing): reading a configuration check operation reports the
//! record and leaves it exactly as the next caller would have found it — every read
//! reports a record consistent with the status it carries, repeated reads of a RUNNING
//! record report an IDENTICAL record and do not end it (a second start of that check is
//! still refused as a conflict), and once the clock has ended it the terminal record,
//! including the `EndTime` it ended at, does not move with the read that observes it.
//!
//! The rows span the model's whole `ConfigurationCheckType` catalogue and both phases of
//! a record's life (running, ended), not one recorded call. The assertions about the
//! transient phase are made only while the clock still owes the transition
//! (`TRANSITION_SPAN` from the start call), so a loaded machine cannot turn them into a
//! spurious failure; the consistency assertions hold in either phase and are always made.

use std::time::Instant;

use serde_json::json;
use service_com_amazonaws_ssmsap_ssmsap::ops::types::CONFIGURATION_CHECK_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{configuration_check_entry, TRANSITION_SPAN};

use crate::application_lifecycle::Emulator;
use crate::configuration_checks::{field, read_record, settle, started, string_field};

/// How many times a row reads the same record. More than two, so a drift that only
/// shows after the first read is still caught.
const READS_PER_PHASE: usize = 3;

/// Start `check` against a freshly registered application and return its `OperationId`
/// together with the instant the start was made (which is what the clock owes the
/// transition from).
async fn start_one(emulator: &Emulator, application_id: &str, check: &str) -> (String, Instant) {
    emulator.with_application(application_id).await;
    let reply = emulator
        .start_checks(json!({
            "ApplicationId": application_id,
            "ConfigurationCheckIds": [check],
        }))
        .await;
    let started_at = Instant::now();
    assert_eq!(
        reply.status, 200,
        "case {check:?}: expected 200 from StartConfigurationChecks, got {reply:?}"
    );
    let operations = started(&reply);
    assert_eq!(
        operations.len(),
        1,
        "case {check:?}: expected exactly one started check, got {operations:?}"
    );
    (string_field(&operations[0], "Id"), started_at)
}

/// What a record must carry for the status it reports, whichever phase a read caught it
/// in: a running check has nothing to explain, no end and nothing to count yet, while an
/// ended one reports when it ended and the tally of the rule results its rules produced.
/// Either way it names the check it performs with the catalogue's own prose.
fn assert_consistent_with_its_status(case: &str, check: &str, record: &serde_json::Value) {
    match string_field(record, "Status").as_str() {
        "INPROGRESS" => {
            assert!(
                field(record, "EndTime").is_none() && field(record, "StatusMessage").is_none(),
                "case {case:?}: a running check has no EndTime and nothing to explain, got \
                 {record:?}"
            );
            assert!(
                field(record, "RuleStatusCounts").is_none(),
                "case {case:?}: a running check has produced no rule result to count, got \
                 {record:?}"
            );
        }
        "SUCCESS" => {
            assert!(
                field(record, "EndTime").is_some(),
                "case {case:?}: an ended check reports an EndTime, got {record:?}"
            );
            assert!(
                field(record, "StatusMessage").is_none(),
                "case {case:?}: a check whose rules ran has nothing to explain, got {record:?}"
            );
            assert!(
                field(record, "RuleStatusCounts").is_some(),
                "case {case:?}: an ended check reports the tally of its rule results, got \
                 {record:?}"
            );
        }
        other => panic!(
            "case {case:?}: a read reported the unmodelled status {other:?}, got {record:?}"
        ),
    }
    let catalogued = configuration_check_entry(check)
        .unwrap_or_else(|| panic!("case {case:?}: the service catalogues {check:?}"));
    assert_eq!(
        (
            string_field(record, "ConfigurationCheckName"),
            string_field(record, "ConfigurationCheckDescription")
        ),
        (
            catalogued.name.to_string(),
            catalogued.description.to_string()
        ),
        "case {case:?}: a record names the check it performs with the catalogue's prose, got \
         {record:?}"
    );
}

/// RULE: reading a running check reports it without ending it, and every read reports
/// the same record — so a read never consumes the run it observes.
#[tokio::test]
async fn reading_a_running_check_reports_it_without_ending_it() {
    // One row per check the model defines: the rule is the reader's, not one check's.
    for check in CONFIGURATION_CHECK_TYPE_VALUES {
        let emulator = Emulator::new();
        let application_id = format!("HDB-READ-{check}");
        let (id, started_at) = start_one(&emulator, &application_id, check).await;

        let mut records = Vec::with_capacity(READS_PER_PHASE);
        for attempt in 0..READS_PER_PHASE {
            let reply = emulator.get_check(&id).await;
            assert_eq!(
                reply.status, 200,
                "case {check:?} read {attempt}: expected 200, got {reply:?}"
            );
            let record = read_record(&reply);
            assert_consistent_with_its_status(&format!("{check} read {attempt}"), check, &record);
            records.push(record);
        }

        // A second start of the same check is refused while the first one runs, so it
        // reports whether the reads above left that run in place.
        let again = emulator
            .start_checks(json!({
                "ApplicationId": application_id,
                "ConfigurationCheckIds": [check],
            }))
            .await;

        // The transient assertions hold only while the clock still owes the transition:
        // past that instant the record was due to end whether or not anybody read it.
        if started_at.elapsed() >= TRANSITION_SPAN {
            continue;
        }
        for (attempt, record) in records.iter().enumerate().skip(1) {
            assert_eq!(
                record, &records[0],
                "case {check:?} read {attempt}: every read of a running check must report the \
                 same record, got {record:?}"
            );
        }
        assert_eq!(
            string_field(&records[0], "Status"),
            "INPROGRESS",
            "case {check:?}: a read must not end a running check, got {:?}",
            records[0]
        );
        assert_eq!(
            again.status, 409,
            "case {check:?}: the check read {READS_PER_PHASE} times must still be running, got \
             {again:?}"
        );
    }
}

/// RULE, the other phase: once the clock has ended a check, the terminal record is the
/// same on every read — the status, the reason and the `EndTime` it ended at do not move
/// with the read that observes them.
#[tokio::test]
async fn an_ended_check_reports_the_same_terminal_record_on_every_read() {
    for check in CONFIGURATION_CHECK_TYPE_VALUES {
        let emulator = Emulator::new();
        let application_id = format!("HDB-ENDED-{check}");
        let (id, _) = start_one(&emulator, &application_id, check).await;

        settle().await;

        let first = read_record(&emulator.get_check(&id).await);
        assert_eq!(
            string_field(&first, "Status"),
            "SUCCESS",
            "case {check:?}: expected the terminal status once the clock owes it, got {first:?}"
        );
        assert_consistent_with_its_status(check, check, &first);

        for attempt in 1..READS_PER_PHASE {
            // Enough time passes between the reads that an `EndTime` stamped with the
            // time of the read, rather than with the instant the check ended, would
            // differ from the one the first read reported.
            settle().await;
            let later = read_record(&emulator.get_check(&id).await);
            assert_eq!(
                later, first,
                "case {check:?} read {attempt}: a terminal record must not move with the read \
                 that observes it, got {later:?}"
            );
        }
    }
}
