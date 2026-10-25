// SPDX-License-Identifier: CC0-1.0

//! What the two configuration-check RESULT listings report, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary.
//!
//! RULE under test (both readers follow it): a result listing resolves the record its
//! required id names IN THE REGION the request was signed for and reports that record's
//! children — `ListSubCheckResults` the sub-check results the configuration check run
//! produced, `ListSubCheckRuleResults` the rule results of one sub-check result — in
//! their own id order, at most `MaxResults` per page, carrying a `NextToken` exactly when
//! further children remain; a run that produced no child reports an empty list, and an id
//! that resolves to NO record is refused with the only client error either operation
//! models, the 400 `ValidationException`, naming the id.
//!
//! The rows span what the rule generalises over: child count (none, one, several, exactly
//! a full page, one past it), requested page size (absent, the smallest, a middling one,
//! the largest the model allows), the stored order (children are stored in DESCENDING id
//! order, so a listing that reports them ascending cannot be reporting them in the order
//! it found them), the modelled `RuleResultStatus` catalogue, and which members a child
//! carries. The refusal rows span the forms the shared id pattern admits, the id of
//! another resource of this service, and the same id at another regional endpoint.
//!
//! One thing here does NOT go through the wire: the sub-check results a run produced are
//! PLACED in the store. A run of this emulator produces exactly the results of the rules
//! its check's catalogue entry names (`state::CONFIGURATION_CHECK_CATALOGUE`), which is a
//! fixed handful per check, so the paging rows above — a full page of children, one past
//! it — could not exist otherwise. What the readers report for a run as this emulator
//! actually performs it is pinned over the wire alone, in
//! [`a_run_reports_the_results_its_rules_produced`], and
//! `tests/all/configuration_check_rules.rs` pins what those rules decide.

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    CONFIGURATION_CHECK_TYPE_VALUES, RULE_RESULT_STATUS_VALUES,
};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    configuration_check_entry, no_configuration_check_operation_message,
    no_sub_check_result_message, StoredRuleResult, StoredSubCheckResult, MAX_PAGE_SIZE,
};

use crate::application_lifecycle::{Emulator, Reply, REGION};
use crate::configuration_checks::{read_record, settle, started, string_field};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The paths the two readers are bound to
/// (`smithy_query(query="operation", operation="ListSubCheckResults,ListSubCheckRuleResults")`).
const SUB_CHECK_RESULTS: &str = "/list-sub-check-results";
const RULE_RESULTS: &str = "/list-sub-check-rule-results";

/// Another Region to drive the same id at: the service is regional, so a result produced
/// at one endpoint must not resolve through another.
const ELSEWHERE: &str = "eu-west-1";

/// How many pages a walk will follow before it gives up. A reader that handed out a token
/// that does not advance must fail the case rather than loop forever.
const PAGE_LIMIT: usize = 200;

impl Emulator {
    pub(crate) async fn list_sub_check_results(&self, body: Value) -> Reply {
        self.send(SUB_CHECK_RESULTS, body).await
    }

    async fn list_rule_results(&self, body: Value) -> Reply {
        self.send(RULE_RESULTS, body).await
    }

    /// Start ONE check against a registered application and return its `OperationId`.
    pub(crate) async fn one_check(&self, application_id: &str, check: &str) -> String {
        let reply = self
            .start_checks(json!({
                "ApplicationId": application_id,
                "ConfigurationCheckIds": [check],
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "expected 200 from StartConfigurationChecks, got {reply:?}"
        );
        let operations = started(&reply);
        assert_eq!(
            operations.len(),
            1,
            "expected exactly one started check, got {operations:?}"
        );
        string_field(&operations[0], "Id")
    }
}

/// A `SubCheckResultId` / `RuleResultId` of the form the model's shared pattern admits
/// (`^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$`), built from `family`
/// and `n` so a row knows its children's ids in advance — and, the last group being
/// zero-padded hex, knows their ASCENDING order is ascending `n`.
fn id(family: u32, n: u32) -> String {
    format!("{family:08x}-0000-4000-8000-{n:012x}")
}

/// `count` rule results of one sub-check, spanning the modelled `RuleResultStatus`
/// catalogue and the members a result may carry: a description on some, a message on
/// others, metadata of none, one and two entries. `family` distinguishes one sub-check's
/// results from another's.
///
/// Built in DESCENDING id order, so a listing that reports them in ascending id order is
/// ordering them rather than echoing the order it found them in.
fn rule_results(family: u32, count: u32) -> Vec<StoredRuleResult> {
    (0..count)
        .rev()
        .map(|n| StoredRuleResult {
            id: id(family, n),
            description: (n % 2 == 0).then(|| format!("what rule {n} validates")),
            // The status catalogue is read off the model's own enum, so a status added
            // to it is exercised here without this file changing.
            status: Some(
                RULE_RESULT_STATUS_VALUES[n as usize % RULE_RESULT_STATUS_VALUES.len()].to_string(),
            ),
            message: (n % 3 == 0).then(|| format!("what rule {n} found")),
            metadata: (0..n % 3)
                .map(|entry| (format!("key-{entry}"), format!("value-{n}-{entry}")))
                .collect(),
        })
        .collect()
}

/// `count` sub-check results of one run, each holding `rules_each` rule results, spanning
/// the members a sub-check result may carry (a name, a description, references of none,
/// one and two entries) and stored in DESCENDING id order for the reason
/// [`rule_results`] gives. `family` distinguishes one run's results from another's.
pub(crate) fn sub_check_results(
    family: u32,
    count: u32,
    rules_each: u32,
) -> Vec<StoredSubCheckResult> {
    (0..count)
        .rev()
        .map(|n| StoredSubCheckResult {
            id: id(family, n),
            name: Some(format!("sub-check {n}")),
            description: (n % 2 == 0).then(|| format!("what sub-check {n} validates")),
            references: (0..n % 3)
                .map(|reference| format!("reference-{n}-{reference}"))
                .collect(),
            rule_results: rule_results(family + 1, rules_each),
        })
        .collect()
}

/// Put `results` on the configuration check run `operation_id` names, in the Region this
/// suite drives. See this module's note on why these records are placed rather than
/// produced.
///
/// The run is brought to its terminal status FIRST — the clock is waited out and the
/// record read back over the wire — because that transition is what writes the results
/// the run's own rules produced, and a row that placed its children before it would have
/// them replaced mid-walk.
pub(crate) async fn place_results(
    emulator: &Emulator,
    operation_id: &str,
    results: Vec<StoredSubCheckResult>,
) {
    settle().await;
    let ended = read_record(&emulator.get_check(operation_id).await);
    assert_eq!(
        string_field(&ended, "Status"),
        "SUCCESS",
        "the run a case places results on must have ended first, got {ended:?}"
    );
    emulator.with_state(|state| {
        state
            .region_entry(REGION)
            .configuration_check_operations
            .get_mut(operation_id)
            .expect("the configuration check run the case just started")
            .sub_check_results = results;
    });
}

/// Follow one listing's pages to exhaustion and return every item it reported, in the
/// order the pages reported them.
///
/// Asserts, on every page, the part of the rule that is about the page itself: a 200, at
/// most the requested number of items, and a `NextToken` only alongside a full page.
async fn walk(
    emulator: &Emulator,
    path: &str,
    request: &Value,
    items_member: &str,
    max_results: Option<i32>,
    case: &str,
) -> Vec<Value> {
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE) as usize;
    let mut body = request.clone();
    if let Some(max_results) = max_results {
        body.as_object_mut()
            .expect("the request body is a JSON object")
            .insert("MaxResults".to_string(), json!(max_results));
    }
    let mut seen: Vec<Value> = Vec::new();
    for page in 0..PAGE_LIMIT {
        let reply = emulator.send(path, body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {page}: expected 200, got {}: {:?}",
            reply.status, reply.body
        );
        let items = reply
            .member(items_member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?} page {page}: expected a {items_member} list, got {reply:?}")
            });
        assert!(
            items.len() <= limit,
            "case {case:?} page {page}: expected at most {limit} items, got {}",
            items.len()
        );
        let next_token = reply
            .member("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = next_token.as_deref() {
            assert_eq!(
                items.len(),
                limit,
                "case {case:?} page {page}: a token must only resume after a FULL page, got {} \
                 items with token {token:?}",
                items.len()
            );
        }
        seen.extend(items);
        match next_token {
            Some(token) => {
                body.as_object_mut()
                    .expect("the request body is a JSON object")
                    .insert("NextToken".to_string(), json!(token));
            }
            None => return seen,
        }
    }
    panic!("case {case:?}: the listing handed out tokens for more than {PAGE_LIMIT} pages");
}

/// A member of one reported item, treating JSON `null` as absent.
pub(crate) fn member<'a>(item: &'a Value, name: &str) -> Option<&'a Value> {
    item.get(name).filter(|value| !value.is_null())
}

/// The page sizes every paging row is walked at: absent (the model's default and largest
/// page), the smallest the `MaxResults` range admits, a middling one, and the largest.
fn page_sizes() -> Vec<Option<i32>> {
    vec![None, Some(1), Some(2), Some(MAX_PAGE_SIZE)]
}

/// RULE, over the wire alone: a configuration check run reports the sub-check results it
/// produced — the ones its check's catalogue entry names rules for — in either phase of
/// the run: none while it is still running, and one per sub-check of that entry once it
/// has ended, each holding the results of that sub-check's rules, without the listing
/// changing the record it reports on.
#[tokio::test]
async fn a_run_reports_the_results_its_rules_produced() {
    // One row per check the model defines: the rule is the reader's, not one check's.
    for check in CONFIGURATION_CHECK_TYPE_VALUES {
        let emulator = Emulator::new();
        let application_id = format!("HDB-RESULTS-{check}");
        emulator.with_application(&application_id).await;
        let operation_id = emulator.one_check(&application_id, check).await;
        // What the run is expected to produce is the catalogue's entry for the check it
        // runs, read off `state::CONFIGURATION_CHECK_CATALOGUE` rather than written out
        // here, so a check whose rules change is still covered by this row.
        let catalogued = configuration_check_entry(check)
            .unwrap_or_else(|| panic!("case {check:?}: the service catalogues this check"));

        for phase in ["running", "ended"] {
            let case = format!("{check} ({phase})");
            let before = read_record(&emulator.get_check(&operation_id).await);
            let running = string_field(&before, "Status") == "INPROGRESS";
            let reply = emulator
                .list_sub_check_results(json!({"OperationId": operation_id}))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {}: {:?}",
                reply.status, reply.body
            );
            let results = reply
                .member("SubCheckResults")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a SubCheckResults list, got {reply:?}")
                });
            if running {
                // A run that has not finished has produced nothing yet, so there is
                // nothing to report rather than something invented.
                assert!(
                    results.is_empty(),
                    "case {case:?}: a run that has not ended has produced no sub-check \
                     result, got {results:?}"
                );
            } else {
                assert_eq!(
                    results.len(),
                    catalogued.sub_checks.len(),
                    "case {case:?}: expected one sub-check result per sub-check of the \
                     catalogued check, got {results:?}"
                );
                let names: Vec<String> = results
                    .iter()
                    .map(|result| {
                        member(result, "Name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                for sub_check in catalogued.sub_checks {
                    assert!(
                        names.iter().any(|name| name == sub_check.name),
                        "case {case:?}: expected a result for sub-check {:?}, got {names:?}",
                        sub_check.name
                    );
                }
                // Every rule of every sub-check produced exactly one rule result, which
                // the other reader reports.
                let mut rule_results = 0;
                for result in &results {
                    let sub_check_result_id = member(result, "Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let rules = emulator
                        .list_rule_results(json!({"SubCheckResultId": sub_check_result_id}))
                        .await;
                    assert_eq!(
                        rules.status, 200,
                        "case {case:?}: expected 200 from ListSubCheckRuleResults, got {rules:?}"
                    );
                    rule_results += rules
                        .member("RuleResults")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or_default();
                }
                assert_eq!(
                    rule_results,
                    catalogued.rule_count(),
                    "case {case:?}: expected one rule result per rule of the catalogued \
                     check, got {rule_results}"
                );
            }
            assert!(
                reply.member("NextToken").is_none(),
                "case {case:?}: nothing remains, so nothing is handed out to resume from, got \
                 {reply:?}"
            );
            // A report changes nothing: the run reads back as it did before the listing.
            // In the ended phase the clock owes the record nothing more, so the two reads
            // must be identical.
            let after = read_record(&emulator.get_check(&operation_id).await);
            if phase == "ended" {
                assert_eq!(
                    after, before,
                    "case {case:?}: listing a run's results must leave the run as it was, got \
                     {after:?}"
                );
            }
            if phase == "running" {
                settle().await;
            }
        }
    }
}

/// RULE: `ListSubCheckResults` reports the sub-check results of the run its `OperationId`
/// names, in `SubCheckResultId` order, paged by `MaxResults` — over a table of child
/// counts and page sizes.
#[tokio::test]
async fn sub_check_results_are_reported_in_id_order_and_paged() {

    // Counts: none, one, several, exactly a full page, one past it.
    for count in [0_u32, 1, 7, MAX_PAGE_SIZE as u32, MAX_PAGE_SIZE as u32 + 1] {
        // One emulator per row: the rows are independent runs, not a sequence.
        let emulator = Emulator::new();
        let application_id = format!("HDB-PAGED-{count}");
        emulator.with_application(&application_id).await;
        let operation_id = emulator
            .one_check(&application_id, CONFIGURATION_CHECK_TYPE_VALUES[0])
            .await;
        let stored = sub_check_results(0xBB, count, 2);
        place_results(&emulator, &operation_id, stored.clone()).await;

        let mut expected_ids: Vec<String> = stored.iter().map(|result| result.id.clone()).collect();
        expected_ids.sort();

        for max_results in page_sizes() {
            let case = format!("{count} sub-check results, MaxResults {max_results:?}");
            let reported = walk(
                &emulator,
                SUB_CHECK_RESULTS,
                &json!({"OperationId": operation_id}),
                "SubCheckResults",
                max_results,
                &case,
            )
            .await;

            let reported_ids: Vec<String> = reported
                .iter()
                .map(|item| {
                    member(item, "Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                reported_ids, expected_ids,
                "case {case:?}: expected every stored sub-check result once, in id order, got \
                 {reported_ids:?}"
            );

            // And each row is PROJECTED from the record it was stored as: a member the
            // record holds is reported, one it does not is absent rather than empty.
            for item in &reported {
                let reported_id = member(item, "Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let record = stored
                    .iter()
                    .find(|result| result.id == reported_id)
                    .unwrap_or_else(|| panic!("case {case:?}: unknown id {reported_id:?}"));
                assert_eq!(
                    member(item, "Name").and_then(Value::as_str),
                    record.name.as_deref(),
                    "case {case:?}: Name must be reported as stored, got {item:?}"
                );
                assert_eq!(
                    member(item, "Description").and_then(Value::as_str),
                    record.description.as_deref(),
                    "case {case:?}: Description must be reported as stored, got {item:?}"
                );
                let references: Vec<String> = member(item, "References")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| value.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    references, record.references,
                    "case {case:?}: References must be reported as stored, got {item:?}"
                );
            }
        }
    }
}

/// RULE, the other reader: `ListSubCheckRuleResults` reports the rule results of the
/// sub-check result its `SubCheckResultId` names, in `RuleResultId` order, paged by
/// `MaxResults` — over the same table of child counts and page sizes.
#[tokio::test]
async fn rule_results_are_reported_in_id_order_and_paged() {

    for count in [0_u32, 1, 7, MAX_PAGE_SIZE as u32, MAX_PAGE_SIZE as u32 + 1] {
        // One emulator per row: a `SubCheckResultId` is resolved across the Region's
        // runs, so two rows must not hold results under the same ids.
        let emulator = Emulator::new();
        let application_id = format!("HDB-RULES-{count}");
        emulator.with_application(&application_id).await;
        let operation_id = emulator
            .one_check(&application_id, CONFIGURATION_CHECK_TYPE_VALUES[0])
            .await;
        // Two sub-check results, holding DIFFERENT rule results, so a listing that
        // reported the rule results of the whole RUN rather than of the sub-check asked
        // for would be caught.
        let mut stored = sub_check_results(0xBB, 2, 0);
        stored[0].rule_results = rule_results(0xAA, count);
        stored[1].rule_results = rule_results(0xCC, 3);
        place_results(&emulator, &operation_id, stored.clone()).await;

        // Each sub-check result of the run is asked for in turn, so the reader is shown
        // to report the rule results of the one asked for and no other's.
        for record in &stored {
            let mut expected_ids: Vec<String> = record
                .rule_results
                .iter()
                .map(|rule| rule.id.clone())
                .collect();
            expected_ids.sort();

            for max_results in page_sizes() {
                let case = format!(
                    "{} rule results under {:?}, MaxResults {max_results:?}",
                    record.rule_results.len(),
                    record.id
                );
                let reported = walk(
                    &emulator,
                    RULE_RESULTS,
                    &json!({"SubCheckResultId": record.id}),
                    "RuleResults",
                    max_results,
                    &case,
                )
                .await;

                let reported_ids: Vec<String> = reported
                    .iter()
                    .map(|item| {
                        member(item, "Id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert_eq!(
                    reported_ids, expected_ids,
                    "case {case:?}: expected every rule result of THAT sub-check once, in id \
                     order, got {reported_ids:?}"
                );

                for item in &reported {
                    let reported_id = member(item, "Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let rule = record
                        .rule_results
                        .iter()
                        .find(|rule| rule.id == reported_id)
                        .unwrap_or_else(|| panic!("case {case:?}: unknown id {reported_id:?}"));
                    assert_eq!(
                        member(item, "Status").and_then(Value::as_str),
                        rule.status.as_deref(),
                        "case {case:?}: Status must be reported as stored, got {item:?}"
                    );
                    assert_eq!(
                        member(item, "Description").and_then(Value::as_str),
                        rule.description.as_deref(),
                        "case {case:?}: Description must be reported as stored, got {item:?}"
                    );
                    assert_eq!(
                        member(item, "Message").and_then(Value::as_str),
                        rule.message.as_deref(),
                        "case {case:?}: Message must be reported as stored, got {item:?}"
                    );
                    let metadata: Vec<(String, String)> = member(item, "Metadata")
                        .and_then(Value::as_object)
                        .map(|entries| {
                            let mut pairs: Vec<(String, String)> = entries
                                .iter()
                                .map(|(key, value)| {
                                    (key.clone(), value.as_str().unwrap_or_default().to_string())
                                })
                                .collect();
                            pairs.sort();
                            pairs
                        })
                        .unwrap_or_default();
                    let expected_metadata: Vec<(String, String)> = rule
                        .metadata
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect();
                    assert_eq!(
                        metadata, expected_metadata,
                        "case {case:?}: Metadata must be reported as stored, got {item:?}"
                    );
                }
            }
        }
    }
}

/// RULE: an `OperationId` that names no configuration check operation of this account and
/// Region is refused with the 400 `ValidationException`, naming the id — whether nothing
/// was ever stored under it, it names another resource of this service, or it names a run
/// of another Region.
#[tokio::test]
async fn an_operation_id_that_names_no_run_is_refused() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-REFUSE").await;

    // Well-formed ids (the model's `OperationId` pattern) that name nothing: all-zero,
    // mixed case, and the braced form the pattern allows.
    let mut cases: Vec<(&str, String)> = vec![
        ("an id nothing was stored under", id(0, 0)),
        (
            "the mixed-case form the pattern allows",
            "DEADBEEF-dead-BEEF-dead-beefDEADbeef".to_string(),
        ),
        (
            "the braced form the pattern allows",
            "{11111111-2222-3333-4444-555555555555}".to_string(),
        ),
    ];

    // An application operation and a configuration check operation are separate
    // resources, so the id of one never resolves through the other's listing.
    let application_operation = emulator
        .send("/start-application", json!({"ApplicationId": "HDB-REFUSE"}))
        .await;
    let operation_id = application_operation
        .member("OperationId")
        .and_then(Value::as_str)
        .expect("StartApplication reports an OperationId")
        .to_string();
    cases.push(("an application OperationId", operation_id));

    for (label, operation_id) in cases {
        let reply = emulator
            .list_sub_check_results(json!({"OperationId": operation_id}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {label:?} ({operation_id:?}): expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {label:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        let expected = no_configuration_check_operation_message(&operation_id);
        let actual = message(&reply);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // And a run of THIS Region does not resolve at another regional endpoint.
    let here = emulator
        .one_check("HDB-REFUSE", CONFIGURATION_CHECK_TYPE_VALUES[0])
        .await;
    let crossed = emulator
        .send_in_region(ELSEWHERE, SUB_CHECK_RESULTS, json!({"OperationId": here}))
        .await;
    assert_eq!(
        crossed.status, 400,
        "a run of {REGION:?} must not resolve at {ELSEWHERE:?}, got {crossed:?}"
    );
    assert_eq!(
        message(&crossed),
        no_configuration_check_operation_message(&here),
        "the refusal must name the id it could not resolve, got {:?}",
        message(&crossed)
    );
    let found = emulator
        .list_sub_check_results(json!({"OperationId": here}))
        .await;
    assert_eq!(
        found.status, 200,
        "the same run resolves at the Region it was started in, got {found:?}"
    );
}

/// RULE, the other reader: a `SubCheckResultId` that names no sub-check result of this
/// account and Region is refused the same way — including the id of the RUN that would
/// hold one, and a result of another Region.
#[tokio::test]
async fn a_sub_check_result_id_that_names_nothing_is_refused() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-NO-SUB").await;
    let operation_id = emulator
        .one_check("HDB-NO-SUB", CONFIGURATION_CHECK_TYPE_VALUES[0])
        .await;
    let stored = sub_check_results(0xBB, 1, 2);
    let produced = stored[0].id.clone();
    place_results(&emulator, &operation_id, stored).await;

    for (label, sub_check_result_id) in [
        ("an id nothing was stored under", id(0, 0)),
        (
            "the mixed-case form the pattern allows",
            "DEADBEEF-dead-BEEF-dead-beefDEADbeef".to_string(),
        ),
        (
            "the braced form the pattern allows",
            "{11111111-2222-3333-4444-555555555555}".to_string(),
        ),
        (
            "the id of the RUN that holds the sub-check result",
            operation_id.clone(),
        ),
    ] {
        let reply = emulator
            .list_rule_results(json!({"SubCheckResultId": sub_check_result_id}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {label:?} ({sub_check_result_id:?}): expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {label:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        let expected = no_sub_check_result_message(&sub_check_result_id);
        let actual = message(&reply);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // The id that DOES name a produced sub-check result resolves here, and nowhere else.
    let found = emulator
        .list_rule_results(json!({"SubCheckResultId": produced}))
        .await;
    assert_eq!(
        found.status, 200,
        "a produced sub-check result resolves at its own Region, got {found:?}"
    );
    let crossed = emulator
        .send_in_region(
            ELSEWHERE,
            RULE_RESULTS,
            json!({"SubCheckResultId": produced}),
        )
        .await;
    assert_eq!(
        crossed.status, 400,
        "a sub-check result of {REGION:?} must not resolve at {ELSEWHERE:?}, got {crossed:?}"
    );
}

/// RULE: a `NextToken` this service did not issue resumes nothing — both readers refuse
/// it with the 400 `ValidationException` naming the member, rather than starting over.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-TOKEN").await;
    let operation_id = emulator
        .one_check("HDB-TOKEN", CONFIGURATION_CHECK_TYPE_VALUES[0])
        .await;
    let stored = sub_check_results(0xBB, 3, 3);
    let sub_check_result_id = stored[0].id.clone();
    place_results(&emulator, &operation_id, stored).await;

    // Tokens the `NextToken` pattern (`^.{16,2048}$`) admits but this service never
    // handed out: plain text, hex that decodes to something else, and a resource id.
    for token in [
        "not-a-token-at-all-0123".to_string(),
        ::hex::encode("ssmsap:not-a-token:v1:whatever"),
        format!("{sub_check_result_id}{sub_check_result_id}"),
    ] {
        for (path, body) in [
            (
                SUB_CHECK_RESULTS,
                json!({"OperationId": operation_id, "NextToken": token}),
            ),
            (
                RULE_RESULTS,
                json!({"SubCheckResultId": sub_check_result_id, "NextToken": token}),
            ),
        ] {
            let reply = emulator.send(path, body).await;
            assert_eq!(
                reply.status, 400,
                "case {path:?}/{token:?}: expected 400, got {}: {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.code(),
                VALIDATION_EXCEPTION,
                "case {path:?}/{token:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
            );
            let actual = message(&reply);
            assert!(
                actual.contains("NextToken") && actual.contains(&token),
                "case {path:?}/{token:?}: the refusal must name the member and the value, got \
                 {actual:?}"
            );
        }
    }
}

/// RULE: a result is forgotten exactly when the run that produced it is — deregistering
/// the application drops its runs, so neither reader resolves their ids afterwards, while
/// another application's results are untouched.
#[tokio::test]
async fn deregistering_an_application_forgets_its_results() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-GONE").await;
    emulator.with_application("HDB-STAYS").await;

    let mut runs: Vec<(&str, String, String)> = Vec::new();
    for (family, application_id) in [(0xB0_u32, "HDB-GONE"), (0xB1_u32, "HDB-STAYS")] {
        let operation_id = emulator
            .one_check(application_id, CONFIGURATION_CHECK_TYPE_VALUES[0])
            .await;
        // A family per run, so the two runs' results carry DIFFERENT ids and the lookup
        // cannot resolve one through the other.
        let stored = sub_check_results(family, 1, 2);
        let sub_check_result_id = stored[0].id.clone();
        place_results(&emulator, &operation_id, stored).await;
        runs.push((application_id, operation_id, sub_check_result_id));
    }

    let deregistered = emulator
        .send("/deregister-application", json!({"ApplicationId": "HDB-GONE"}))
        .await;
    assert_eq!(
        deregistered.status, 200,
        "expected 200 from DeregisterApplication, got {deregistered:?}"
    );
    // The deregistration is asynchronous: the runs are dropped with the application, when
    // the removal the clock owes it falls due.
    settle().await;

    for (application_id, operation_id, sub_check_result_id) in &runs {
        let expected = if *application_id == "HDB-GONE" { 400 } else { 200 };
        let run = emulator
            .list_sub_check_results(json!({"OperationId": operation_id}))
            .await;
        assert_eq!(
            run.status, expected,
            "case {application_id:?}: expected {expected} from the run listing, got {}: {:?}",
            run.status, run.body
        );
        let rules = emulator
            .list_rule_results(json!({"SubCheckResultId": sub_check_result_id}))
            .await;
        assert_eq!(
            rules.status, expected,
            "case {application_id:?}: expected {expected} from the rule listing, got {}: {:?}",
            rules.status, rules.body
        );
    }
}
