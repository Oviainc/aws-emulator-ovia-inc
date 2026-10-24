// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `ErrorCode` `BatchGetFindings` reports for an identifier
//! it cannot resolve, and for the identifier each entry reports itself with.
//!
//! The rule under test: a `failedFindings` entry carries the classification for the
//! reason that identifier could not be resolved — a part of the identifier whose value is
//! not one the service takes for that kind of identifier is reported as that part being
//! invalid (`INVALID_SCAN_NAME` against the modelled `ScanName` shape,
//! `INVALID_FINDING_ID` against the `Uuid` form every identifier this service issues),
//! an identifier that repeats inside the batch is `DUPLICATE_IDENTIFIER`, and an
//! identifier whose parts are all well formed but which names no stored finding is
//! `ITEM_DOES_NOT_EXIST` — while the entry reports the identifier inside each reported
//! member's own modelled shape.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), reusing
//! the harness of `crate::scan_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::types::{
    Finding, ERROR_CODE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scan_lifecycle::{batch_get_findings, harness, start_scan, store_of, Answer};

/// The scan the table rows that name an existing scan refer to.
const KNOWN_SCAN: &str = "known-scan";

/// A `findingId` in the modelled `Uuid` form
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, from
/// `smithy_query(query="shape",shape="Uuid")`) that no stored finding carries: an
/// identifier built from it is well formed and names nothing.
const ABSENT_UUID: &str = "1f0e4d2c-3b4a-5968-8776-a5b4c3d2e1f0";

/// The `@length` bounds of the modelled `ScanName` shape, in characters
/// (`smithy_query(query="shape",shape="ScanName")`: length 1..140).
const SCAN_NAME_MIN: usize = 1;
const SCAN_NAME_MAX: usize = 140;

/// True when `value` satisfies the modelled `ScanName` shape: the `@length` above and the
/// `@pattern` `^[a-zA-Z0-9-_$:.]*$`. `BatchGetFindingsError$scanName` targets that shape,
/// so no answer may report a value this rejects.
fn is_modelled_scan_name(value: &str) -> bool {
    let length = value.chars().count();
    (SCAN_NAME_MIN..=SCAN_NAME_MAX).contains(&length)
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '$' | ':' | '.'))
}

/// The `failedFindings` of an answer.
fn failed_findings(answer: &Answer) -> Vec<Value> {
    answer
        .get("failedFindings")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `findings` ids of an answer, in the order it listed them.
fn resolved_ids(answer: &Answer) -> Vec<String> {
    answer
        .get("findings")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|finding| finding["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `BatchGetFindings` over `identifiers`, asserting the call itself succeeded: an
/// identifier the service cannot resolve is reported per entry, never as a failure of the
/// whole call.
async fn classify(router: &MultiServiceRouter, identifiers: Value) -> Answer {
    let answer = batch_get_findings(router, identifiers).await;
    assert_eq!(
        answer.status, 200,
        "BatchGetFindings: an unresolvable identifier is reported per entry, got {} {}",
        answer.status, answer.json
    );
    answer
}

/// Assert the invariants every reported entry carries, whatever its row: a modelled
/// `ErrorCode`, a message, the `findingId` verbatim (the member is an unconstrained
/// `String`) and a `scanName` inside the constrained shape that member targets.
fn assert_entry_invariants(case: &str, entry: &Value, requested_finding_id: &str) {
    let code = entry["errorCode"].as_str().unwrap_or_default();
    assert!(
        ERROR_CODE_VALUES.contains(&code),
        "case {case:?}: errorCode must be one the model enumerates ({ERROR_CODE_VALUES:?}), \
         got {entry}"
    );
    assert!(
        !entry["message"].as_str().unwrap_or_default().is_empty(),
        "case {case:?}: message is a required member, got {entry}"
    );
    assert_eq!(
        entry["findingId"].as_str(),
        Some(requested_finding_id),
        "case {case:?}: the entry must report the findingId it is about, got {entry}"
    );
    let reported = entry["scanName"].as_str().unwrap_or_default();
    assert!(
        is_modelled_scan_name(reported),
        "case {case:?}: scanName targets the constrained ScanName shape, got {reported:?} \
         in {entry}"
    );
}

#[tokio::test]
async fn classifies_each_unresolvable_identifier_by_the_reason_it_could_not_be_resolved() {
    let (_engine, router) = harness();
    start_scan(&router, KNOWN_SCAN).await;

    let at_max_length = "b".repeat(SCAN_NAME_MAX);
    let past_max_length = "a".repeat(SCAN_NAME_MAX + 1);

    // Rows: one per class of identifier that resolves to nothing, differing in which part
    // the service cannot take as one of its identifiers, in how that part is malformed,
    // and in whether a well-formed part names anything. The last column is the `scanName`
    // the entry must report for a member whose target shape is the constrained `ScanName`.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a well-formed finding id no stored finding carries",
            KNOWN_SCAN.to_string(),
            ABSENT_UUID.to_string(),
            "ITEM_DOES_NOT_EXIST",
            KNOWN_SCAN.to_string(),
        ),
        (
            "a valid scan name this account never created",
            "never-created-scan".to_string(),
            ABSENT_UUID.to_string(),
            "ITEM_DOES_NOT_EXIST",
            "never-created-scan".to_string(),
        ),
        (
            "a scan name using every character class the pattern admits",
            "Scan-9_$:.".to_string(),
            ABSENT_UUID.to_string(),
            "ITEM_DOES_NOT_EXIST",
            "Scan-9_$:.".to_string(),
        ),
        (
            "a scan name at the modelled maximum length",
            at_max_length.clone(),
            ABSENT_UUID.to_string(),
            "ITEM_DOES_NOT_EXIST",
            at_max_length.clone(),
        ),
        (
            "a finding id that is not an identifier at all",
            KNOWN_SCAN.to_string(),
            "not-a-finding-identifier".to_string(),
            "INVALID_FINDING_ID",
            KNOWN_SCAN.to_string(),
        ),
        (
            "an empty finding id",
            KNOWN_SCAN.to_string(),
            String::new(),
            "INVALID_FINDING_ID",
            KNOWN_SCAN.to_string(),
        ),
        (
            "a finding id whose hex digits are uppercase",
            KNOWN_SCAN.to_string(),
            ABSENT_UUID.to_uppercase(),
            "INVALID_FINDING_ID",
            KNOWN_SCAN.to_string(),
        ),
        (
            "a finding id missing a group",
            KNOWN_SCAN.to_string(),
            "1f0e4d2c-3b4a-5968-8776".to_string(),
            "INVALID_FINDING_ID",
            KNOWN_SCAN.to_string(),
        ),
        (
            "a finding id with a group of the wrong width",
            KNOWN_SCAN.to_string(),
            "1f0e4d2-3b4a-5968-8776-a5b4c3d2e1f0a".to_string(),
            "INVALID_FINDING_ID",
            KNOWN_SCAN.to_string(),
        ),
        (
            "a finding id with a non-hex digit",
            KNOWN_SCAN.to_string(),
            "1f0e4d2g-3b4a-5968-8776-a5b4c3d2e1f0".to_string(),
            "INVALID_FINDING_ID",
            KNOWN_SCAN.to_string(),
        ),
        (
            "a scan name sent as a finding id",
            KNOWN_SCAN.to_string(),
            KNOWN_SCAN.to_string(),
            "INVALID_FINDING_ID",
            KNOWN_SCAN.to_string(),
        ),
        (
            "a scan name holding a space",
            "bad name with spaces".to_string(),
            ABSENT_UUID.to_string(),
            "INVALID_SCAN_NAME",
            "bad_name_with_spaces".to_string(),
        ),
        (
            "a scan name holding a path separator",
            "scans/known-scan".to_string(),
            ABSENT_UUID.to_string(),
            "INVALID_SCAN_NAME",
            "scans_known-scan".to_string(),
        ),
        (
            "a scan name one character past the modelled maximum length",
            past_max_length.clone(),
            ABSENT_UUID.to_string(),
            "INVALID_SCAN_NAME",
            "a".repeat(SCAN_NAME_MAX),
        ),
        (
            "a scan name below the modelled minimum length",
            String::new(),
            ABSENT_UUID.to_string(),
            "INVALID_SCAN_NAME",
            "_".to_string(),
        ),
        (
            "a scan name outside ASCII",
            "scan-\u{fc}".to_string(),
            ABSENT_UUID.to_string(),
            "INVALID_SCAN_NAME",
            "scan-_".to_string(),
        ),
        (
            // The scan name names the scan a finding id is resolved within, so a request
            // that has not named a scan has not named a finding either.
            "neither part is one of this service's identifiers",
            "bad name".to_string(),
            "garbage".to_string(),
            "INVALID_SCAN_NAME",
            "bad_name".to_string(),
        ),
    ];

    for (case, scan_name, finding_id, expected_code, expected_scan_name) in &cases {
        let identifiers = json!([{ "scanName": scan_name, "findingId": finding_id }]);
        let answer = classify(&router, identifiers.clone()).await;
        assert!(
            resolved_ids(&answer).is_empty(),
            "case {case:?}: no finding is stored, so nothing may resolve, got {}",
            answer.json
        );
        let failed = failed_findings(&answer);
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: one entry per unresolvable identifier, got {}",
            answer.json
        );
        assert_entry_invariants(case, &failed[0], finding_id);
        assert_eq!(
            failed[0]["errorCode"].as_str(),
            Some(*expected_code),
            "case {case:?}: expected {expected_code}, got {}",
            failed[0]
        );
        assert_eq!(
            failed[0]["scanName"].as_str(),
            Some(expected_scan_name.as_str()),
            "case {case:?}: expected scanName {expected_scan_name:?}, got {}",
            failed[0]
        );

        // Classifying an identifier reports on the store; it must not change what the
        // next caller sees, so the same request answers the same way.
        let again = classify(&router, identifiers).await;
        assert_eq!(
            failed_findings(&again),
            failed,
            "case {case:?}: a second identical call must answer identically, got {}",
            again.json
        );
    }
}

#[tokio::test]
async fn every_reported_scan_name_lies_inside_the_modelled_shape() {
    let (_engine, router) = harness();
    start_scan(&router, KNOWN_SCAN).await;

    // Rows: every ASCII code point as a one-character scan name — so each side of the
    // pattern's character class is covered, not only the characters the issue cited —
    // plus each published `@length` bound and one past it on both ends.
    let mut names: Vec<String> = (0u8..=127).map(|byte| (byte as char).to_string()).collect();
    for length in [
        0,
        SCAN_NAME_MIN,
        SCAN_NAME_MIN + 1,
        SCAN_NAME_MAX - 1,
        SCAN_NAME_MAX,
        SCAN_NAME_MAX + 1,
    ] {
        names.push("a".repeat(length));
    }
    // One identifier may appear only once in a batch, or the repeat is reported as a
    // duplicate instead of being classified: the one-character names and the length rows
    // overlap on "a", so the sweep keeps the first occurrence of each name.
    let mut seen = std::collections::HashSet::new();
    names.retain(|name| seen.insert(name.clone()));

    // The modelled `FindingIdentifiers` list holds at most 25 entries, so the sweep is
    // sent a batch at a time.
    for chunk in names.chunks(25) {
        let identifiers: Vec<Value> = chunk
            .iter()
            .map(|name| json!({ "scanName": name, "findingId": ABSENT_UUID }))
            .collect();
        let answer = classify(&router, Value::Array(identifiers)).await;
        let failed = failed_findings(&answer);
        assert_eq!(
            failed.len(),
            chunk.len(),
            "case chunk of {}: one entry per identifier, got {}",
            chunk.len(),
            answer.json
        );
        for (name, entry) in chunk.iter().zip(&failed) {
            let case = format!("scanName {name:?}");
            assert_entry_invariants(&case, entry, ABSENT_UUID);
            if is_modelled_scan_name(name) {
                assert_eq!(
                    entry["errorCode"].as_str(),
                    Some("ITEM_DOES_NOT_EXIST"),
                    "case {case}: a valid scan name naming no scan does not exist, got {entry}"
                );
                assert_eq!(
                    entry["scanName"].as_str(),
                    Some(name.as_str()),
                    "case {case}: a valid scan name is reported verbatim, got {entry}"
                );
            } else {
                assert_eq!(
                    entry["errorCode"].as_str(),
                    Some("INVALID_SCAN_NAME"),
                    "case {case}: a value outside the ScanName shape is not a scan name, \
                     got {entry}"
                );
            }
        }
    }
}

#[tokio::test]
async fn reports_one_entry_per_identifier_in_request_order() {
    let (engine, router) = harness();
    start_scan(&router, KNOWN_SCAN).await;

    // A finding as a run's analysis would have stored it, carrying an id in the form this
    // service issues. Seeded directly: this emulator performs no analysis (see
    // `state::SCAN_NOT_ANALYSED_MESSAGE`), so no request path writes one.
    let stored_id = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";
    {
        let store = store_of(&engine);
        store.lock().findings.insert(
            (KNOWN_SCAN.to_string(), stored_id.to_string()),
            Finding {
                id: Some(stored_id.to_string()),
                status: Some("Open".to_string()),
                ..Finding::default()
            },
        );
    }

    // One batch mixing every outcome, including a repeat of a resolvable identifier and a
    // repeat of an unresolvable one: the first occurrence is answered on its own merits
    // and only the repeat is reported as a duplicate.
    let answer = classify(
        &router,
        json!([
            { "scanName": KNOWN_SCAN, "findingId": stored_id },
            { "scanName": KNOWN_SCAN, "findingId": ABSENT_UUID },
            { "scanName": KNOWN_SCAN, "findingId": stored_id },
            { "scanName": KNOWN_SCAN, "findingId": ABSENT_UUID },
            { "scanName": "bad name", "findingId": ABSENT_UUID },
        ]),
    )
    .await;

    assert_eq!(
        resolved_ids(&answer),
        vec![stored_id.to_string()],
        "the stored finding resolves exactly once, got {}",
        answer.json
    );
    let codes: Vec<String> = failed_findings(&answer)
        .iter()
        .map(|entry| entry["errorCode"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        codes,
        vec![
            "ITEM_DOES_NOT_EXIST",
            "DUPLICATE_IDENTIFIER",
            "DUPLICATE_IDENTIFIER",
            "INVALID_SCAN_NAME",
        ],
        "every unresolvable identifier is reported once, in request order, got {}",
        answer.json
    );
}

#[tokio::test]
async fn a_stored_finding_resolves_whatever_form_its_id_takes() {
    let (engine, router) = harness();
    start_scan(&router, KNOWN_SCAN).await;

    // The ids a run's analysis wrote are what `GetFindings` hands the caller, so each one
    // it reports must resolve back through `BatchGetFindings` — the format checks judge
    // only an identifier that resolved to nothing.
    let stored: &[&str] = &["0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9", "f-1"];
    {
        let store = store_of(&engine);
        let mut guard = store.lock();
        for id in stored {
            guard.findings.insert(
                (KNOWN_SCAN.to_string(), (*id).to_string()),
                Finding {
                    id: Some((*id).to_string()),
                    status: Some("Open".to_string()),
                    ..Finding::default()
                },
            );
        }
    }

    let identifiers: Vec<Value> = stored
        .iter()
        .map(|id| json!({ "scanName": KNOWN_SCAN, "findingId": id }))
        .collect();
    let answer = classify(&router, Value::Array(identifiers)).await;
    let mut resolved = resolved_ids(&answer);
    resolved.sort();
    let mut expected: Vec<String> = stored.iter().map(|id| (*id).to_string()).collect();
    expected.sort();
    assert_eq!(
        resolved, expected,
        "case stored ids {stored:?}: every stored identifier must resolve, got {}",
        answer.json
    );
    assert!(
        failed_findings(&answer).is_empty(),
        "case stored ids {stored:?}: a resolvable identifier is not an error, got {}",
        answer.json
    );
}
