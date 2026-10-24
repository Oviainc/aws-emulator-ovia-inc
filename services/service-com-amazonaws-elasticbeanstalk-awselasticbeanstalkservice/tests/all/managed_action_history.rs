// SPDX-License-Identifier: CC0-1.0

//! `DescribeEnvironmentManagedActionHistory` — "Lists an environment's completed
//! and failed managed actions" — driven end to end over the service's awsQuery
//! HTTP surface.
//!
//! RULE under test: the answer is ONE environment's slice of the store's
//! managed-action history, projected member for member, ordered newest-finished
//! first, cut into pages of at most `MaxItems` records that a `NextToken`
//! resumes exactly after; an environment whose history holds nothing is answered
//! with an empty list and no token, and an identifier that names no environment
//! is refused rather than answered.
//!
//! A managed action is work the SERVICE schedules in an environment's
//! maintenance window, and this emulator runs no such scheduler (which is why
//! `ApplyEnvironmentManagedAction` refuses every `ActionId`), so no implemented
//! operation records one yet. The history is therefore SEEDED through
//! `Emulator::store` — the harness's seam for exactly that — and every assertion
//! below is still made over what the HTTP read answers, so a handler that
//! fabricated items, dropped a member, reported another environment's actions or
//! paged by position rather than by order fails these tests.
//!
//! Each test is a TABLE over the class the rule generalises over: which
//! identifier names the environment, which action type, status and failure a
//! record carries, how many records the history holds and at what page size it
//! is read.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::harness::{instant, Answer, Emulator, SOLUTION_STACK};

/// The epoch second the seeded records are dated from: a fixed instant, so the
/// expected order is a property of the table rather than of when the test ran.
const EPOCH: i64 = 1_700_000_000;

/// One seeded managed action. The rows differ along what the rule generalises
/// over: the action type and status (the modelled
/// `eb::ops::types::ACTION_TYPE_VALUES` / `ACTION_HISTORY_STATUS_VALUES` sets),
/// whether the action failed and so carries a failure type and description,
/// whether it carries a description at all, and when it executed and finished —
/// deliberately NOT in append order, so an answer that reported the store's
/// append order instead of the history order fails.
#[derive(Debug, Clone)]
struct Seed {
    action_id: &'static str,
    action_type: &'static str,
    status: &'static str,
    action_description: Option<&'static str>,
    failure_type: Option<&'static str>,
    failure_description: Option<&'static str>,
    /// Seconds after [`EPOCH`] that the action started executing.
    executed_after: i64,
    /// Seconds after [`EPOCH`] that it finished, or `None` for a record that
    /// carries no finish instant (`FinishedTime` is an optional member, and a
    /// `Unknown`-status record is one whose outcome the service cannot report).
    finished_after: Option<i64>,
}

/// The seeded history of the environment under test.
///
/// The action types, statuses and failure types are the model's own value sets
/// (`smithy_query` shapes `ActionType`, `ActionHistoryStatus`, `FailureType`,
/// mirrored by `eb::ops::types::ACTION_TYPE_VALUES`,
/// `ACTION_HISTORY_STATUS_VALUES` and `FAILURE_TYPE_VALUES`), which the test
/// below checks every row against so this table cannot drift from the model.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            action_id: "action-platform-update",
            action_type: "PlatformUpdate",
            status: "Completed",
            action_description: Some("Platform update from 5.8.0 to 5.9.0"),
            failure_type: None,
            failure_description: None,
            executed_after: 100,
            finished_after: Some(400),
        },
        Seed {
            action_id: "action-instance-refresh",
            action_type: "InstanceRefresh",
            status: "Completed",
            action_description: Some("Instance refresh"),
            failure_type: None,
            failure_description: None,
            executed_after: 600,
            finished_after: Some(900),
        },
        Seed {
            action_id: "action-rolled-back",
            action_type: "PlatformUpdate",
            status: "Failed",
            action_description: Some("Platform update from 5.9.0 to 6.0.0"),
            failure_type: Some("RollbackSuccessful"),
            failure_description: Some("The update failed and was rolled back."),
            executed_after: 200,
            finished_after: Some(1_500),
        },
        Seed {
            action_id: "action-permissions",
            action_type: "PlatformUpdate",
            status: "Failed",
            action_description: None,
            failure_type: Some("PermissionsError"),
            failure_description: Some("The service role is missing a permission."),
            executed_after: 1_000,
            finished_after: Some(1_100),
        },
        Seed {
            action_id: "action-unknown",
            action_type: "Unknown",
            status: "Unknown",
            action_description: None,
            failure_type: None,
            failure_description: None,
            executed_after: 1_200,
            finished_after: None,
        },
        Seed {
            action_id: "action-cancelled",
            action_type: "InstanceRefresh",
            status: "Failed",
            action_description: Some("Instance refresh"),
            failure_type: Some("UpdateCancelled"),
            failure_description: Some("The update was cancelled."),
            executed_after: 50,
            finished_after: Some(2_000),
        },
        Seed {
            action_id: "action-internal",
            action_type: "PlatformUpdate",
            status: "Failed",
            action_description: Some("Platform update from 6.0.0 to 6.1.0"),
            failure_type: Some("InternalFailure"),
            failure_description: Some("An internal error occurred."),
            executed_after: 1_300,
            finished_after: Some(1_400),
        },
    ]
}

/// The action ids of `seeds`, in the order the rule says the service reports
/// them: newest FINISHED first (falling back to the instant the action started,
/// for a record with no finish instant), with the append order breaking a tie.
///
/// Computed here from the table, not read from any answer, so it is an
/// independent statement of the expected order.
fn expected_order(seeds: &[Seed]) -> Vec<&'static str> {
    let mut ordered: Vec<(i64, usize, &'static str)> = seeds
        .iter()
        .enumerate()
        .map(|(appended, seed)| {
            (
                seed.finished_after.unwrap_or(seed.executed_after),
                appended,
                seed.action_id,
            )
        })
        .collect();
    ordered.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
    ordered.into_iter().map(|(_, _, id)| id).collect()
}

/// Create `application` and `environment`, and answer the environment's id.
async fn fixture(emulator: &Emulator, application: &str, environment: &str) -> String {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", application)])
        .await;
    assert_eq!(
        created.status, 200,
        "fixture {application:?}: CreateApplication answered {} with body {}",
        created.status, created.body
    );
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", application),
                ("EnvironmentName", environment),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "fixture {environment:?}: CreateEnvironment answered {} with body {}",
        launched.status, launched.body
    );
    launched
        .expect("EnvironmentId", &format!("fixture {environment}"))
        .to_string()
}

/// Seed `seeds` as `environment_id`'s managed-action history, in table order, and
/// answer the `sequence` each record was stored with.
fn seed_history(emulator: &Emulator, environment_id: &str, seeds: &[Seed]) -> Vec<u64> {
    let state = emulator.store();
    let mut store = state.lock();
    let mut sequences = Vec::new();
    for seed in seeds {
        let sequence = store.next_counter();
        sequences.push(sequence);
        store
            .managed_action_history
            .push(eb::state::ManagedActionRecord {
                sequence,
                environment_id: environment_id.to_string(),
                action_id: seed.action_id.to_string(),
                action_type: seed.action_type.to_string(),
                action_description: seed.action_description.map(str::to_string),
                status: seed.status.to_string(),
                failure_type: seed.failure_type.map(str::to_string),
                failure_description: seed.failure_description.map(str::to_string),
                executed_time: ::aws_smithy_types::DateTime::from_secs(
                    EPOCH + seed.executed_after,
                ),
                finished_time: seed
                    .finished_after
                    .map(|after| ::aws_smithy_types::DateTime::from_secs(EPOCH + after)),
            });
    }
    sequences
}

/// Read the history of the environment `params` names.
async fn history(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator
        .call("DescribeEnvironmentManagedActionHistory", params)
        .await
}

/// The `ActionId`s an answer reports, in the order it reported them.
fn reported_ids(answer: &Answer) -> Vec<String> {
    answer
        .values_at("ManagedActionHistoryItems/member/ActionId")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// RULE, first half: the items reported for an environment are the records its
/// history holds, projected member for member, newest-finished first — and they
/// are the same whichever member of the `EnvironmentId`/`EnvironmentName` pair
/// named the environment.
#[tokio::test]
async fn the_reported_history_is_the_environments_stored_actions_newest_first() {
    let emulator = Emulator::new();
    let environment_id = fixture(&emulator, "history-app", "history-env-1").await;
    let seeds = seeds();
    seed_history(&emulator, &environment_id, &seeds);

    // Every row's action type, status and failure type is a value the MODEL
    // declares, read from the generated value sets rather than written out
    // here, so the table cannot drift from the model.
    for seed in &seeds {
        assert!(
            eb::ops::types::ACTION_TYPE_VALUES.contains(&seed.action_type),
            "case {:?}: {:?} is not a modelled ActionType, the set is {:?}",
            seed.action_id,
            seed.action_type,
            eb::ops::types::ACTION_TYPE_VALUES
        );
        assert!(
            eb::ops::types::ACTION_HISTORY_STATUS_VALUES.contains(&seed.status),
            "case {:?}: {:?} is not a modelled ActionHistoryStatus, the set is {:?}",
            seed.action_id,
            seed.status,
            eb::ops::types::ACTION_HISTORY_STATUS_VALUES
        );
        if let Some(failure_type) = seed.failure_type {
            assert!(
                eb::ops::types::FAILURE_TYPE_VALUES.contains(&failure_type),
                "case {:?}: {failure_type:?} is not a modelled FailureType, the set is {:?}",
                seed.action_id,
                eb::ops::types::FAILURE_TYPE_VALUES
            );
        }
    }

    let expected = expected_order(&seeds);
    for (case, params) in [
        (
            "by name",
            vec![("EnvironmentName", "history-env-1".to_string())],
        ),
        ("by id", vec![("EnvironmentId", environment_id.clone())]),
        (
            "by both",
            vec![
                ("EnvironmentId", environment_id.clone()),
                ("EnvironmentName", "history-env-1".to_string()),
            ],
        ),
    ] {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = history(&emulator, &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_ids(&answer),
            expected,
            "case {case:?}: unexpected history order in {}",
            answer.body
        );
        // The whole history fits in one page, so no continuation token is owed.
        assert_eq!(
            answer.value("NextToken"),
            None,
            "case {case:?}: a complete answer must carry no NextToken, got {}",
            answer.body
        );

        // Every member of every item is the stored record's, and an absent
        // member is absent rather than defaulted.
        let items = answer.elements_at("ManagedActionHistoryItems/member");
        assert_eq!(
            items.len(),
            seeds.len(),
            "case {case:?}: expected {} items, got {}",
            seeds.len(),
            answer.body
        );
        for item in &items {
            let action_id = item.expect("ActionId", case);
            let seed = seeds
                .iter()
                .find(|seed| seed.action_id == action_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: reported an action nothing seeded: {action_id:?}")
                });
            let row = format!("{case} / {action_id}");
            assert_eq!(
                item.value("ActionType"),
                Some(seed.action_type),
                "case {row:?}: unexpected ActionType in {}",
                item.body
            );
            assert_eq!(
                item.value("Status"),
                Some(seed.status),
                "case {row:?}: unexpected Status in {}",
                item.body
            );
            assert_eq!(
                item.value("ActionDescription"),
                seed.action_description,
                "case {row:?}: unexpected ActionDescription in {}",
                item.body
            );
            assert_eq!(
                item.value("FailureType"),
                seed.failure_type,
                "case {row:?}: unexpected FailureType in {}",
                item.body
            );
            assert_eq!(
                item.value("FailureDescription"),
                seed.failure_description,
                "case {row:?}: unexpected FailureDescription in {}",
                item.body
            );
            let executed = instant(item.expect("ExecutedTime", &row), &row);
            assert_eq!(
                executed,
                (EPOCH + seed.executed_after) as f64,
                "case {row:?}: unexpected ExecutedTime in {}",
                item.body
            );
            match seed.finished_after {
                Some(after) => {
                    let finished = instant(item.expect("FinishedTime", &row), &row);
                    assert_eq!(
                        finished,
                        (EPOCH + after) as f64,
                        "case {row:?}: unexpected FinishedTime in {}",
                        item.body
                    );
                }
                None => assert_eq!(
                    item.value("FinishedTime"),
                    None,
                    "case {row:?}: a record with no finish instant must report none, got {}",
                    item.body
                ),
            }
        }
    }
}

/// RULE, second half: one environment's history is ITS OWN. An environment the
/// records do not belong to reports none of them, and a read reports the same
/// history however often it is called.
#[tokio::test]
async fn an_environments_history_is_neither_shared_nor_changed_by_reading_it() {
    let emulator = Emulator::new();
    let first = fixture(&emulator, "history-app", "history-env-1").await;
    let second = fixture(&emulator, "history-app-2", "history-env-2").await;
    let seeds = seeds();
    seed_history(&emulator, &first, &seeds);

    // Rows: the environment the records belong to, and one that holds none.
    for (case, environment, expected) in [
        ("seeded environment", first.as_str(), seeds.len()),
        ("other environment", second.as_str(), 0),
    ] {
        // Read twice: a report must not consume, move or mint anything, so the
        // second answer is the first.
        let once = history(&emulator, &[("EnvironmentId", environment)]).await;
        let twice = history(&emulator, &[("EnvironmentId", environment)]).await;
        assert_eq!(
            once.status, 200,
            "case {case:?}: expected HTTP 200, got {} and body {}",
            once.status, once.body
        );
        assert_eq!(
            reported_ids(&once).len(),
            expected,
            "case {case:?}: expected {expected} items, got {}",
            once.body
        );
        assert_eq!(
            reported_ids(&once),
            reported_ids(&twice),
            "case {case:?}: a second read answered differently: {} then {}",
            once.body,
            twice.body
        );
        assert_eq!(
            once.value("NextToken"),
            None,
            "case {case:?}: a complete answer must carry no NextToken, got {}",
            once.body
        );
    }

    // The empty history is reported as the list it is: the member is present and
    // carries no elements, never a fabricated item.
    let empty = history(&emulator, &[("EnvironmentId", second.as_str())]).await;
    assert!(
        empty
            .values_at("ManagedActionHistoryItems/member/ActionId")
            .is_empty(),
        "case \"empty history\": expected no items, got {}",
        empty.body
    );

    // And reading changed nothing in the store either.
    let stored = emulator.store().lock().managed_action_history.len();
    assert_eq!(
        stored,
        seeds.len(),
        "case \"after four reads\": expected {} stored records, got {stored}",
        seeds.len()
    );
}

/// RULE, third half: `MaxItems` bounds one page and `NextToken` resumes exactly
/// after it, so at EVERY page size the walk yields the whole history once, in
/// the one order.
#[tokio::test]
async fn paging_at_any_page_size_yields_the_whole_history_once_in_one_order() {
    let emulator = Emulator::new();
    let environment_id = fixture(&emulator, "history-app", "history-env-1").await;
    let seeds = seeds();
    seed_history(&emulator, &environment_id, &seeds);
    let expected = expected_order(&seeds);

    // Rows: the modelled minimum page size, sizes that divide and do not divide
    // the history, exactly the history's size, one larger, and the modelled
    // maximum (`ManagedActionHistoryMaxItems`, range 1..100).
    for page_size in [1usize, 2, 3, 6, 7, 8, 100] {
        let case = format!("MaxItems={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more call than pages the history can hold, so a token that never
        // cleared is a failure rather than a hang.
        for _ in 0..=seeds.len() + 1 {
            let size = page_size.to_string();
            let mut params = vec![
                ("EnvironmentId", environment_id.as_str()),
                ("MaxItems", size.as_str()),
            ];
            if let Some(token) = token.as_deref() {
                params.push(("NextToken", token));
            }
            let page = history(&emulator, &params).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {} and body {}",
                page.status, page.body
            );
            let ids = reported_ids(&page);
            assert!(
                ids.len() <= page_size,
                "case {case:?}: a page carried {} items, more than the {page_size} asked for: {}",
                ids.len(),
                page.body
            );
            walked.extend(ids);
            token = page.value("NextToken").map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: the walk did not end, it still carries {token:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: the walked pages are not the history in its order"
        );
    }
}

/// RULE: a page boundary is stable — a token taken from one answer resumes at the
/// same record on a later call, and the two pages it cuts the history into
/// neither repeat nor skip a record.
#[tokio::test]
async fn a_page_boundary_is_stable_across_calls() {
    let emulator = Emulator::new();
    let environment_id = fixture(&emulator, "history-app", "history-env-1").await;
    let seeds = seeds();
    seed_history(&emulator, &environment_id, &seeds);
    let expected = expected_order(&seeds);

    for page_size in [2usize, 3, 4] {
        let case = format!("boundary at {page_size}");
        let size = page_size.to_string();
        let first = history(
            &emulator,
            &[
                ("EnvironmentId", environment_id.as_str()),
                ("MaxItems", size.as_str()),
            ],
        )
        .await;
        let token = first
            .value("NextToken")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a page of {page_size} of {} records owes a NextToken, got {}",
                    seeds.len(),
                    first.body
                )
            })
            .to_string();
        assert_eq!(
            reported_ids(&first),
            &expected[..page_size],
            "case {case:?}: unexpected first page in {}",
            first.body
        );

        // The same token, read twice and at a different page size, resumes at
        // the same record both times.
        for (resumed_case, requested) in [("same size", page_size), ("larger", seeds.len())] {
            let size = requested.to_string();
            let rest = history(
                &emulator,
                &[
                    ("EnvironmentId", environment_id.as_str()),
                    ("MaxItems", size.as_str()),
                    ("NextToken", token.as_str()),
                ],
            )
            .await;
            assert_eq!(
                rest.status, 200,
                "case {case:?}/{resumed_case:?}: expected HTTP 200, got {} and body {}",
                rest.status, rest.body
            );
            let ids = reported_ids(&rest);
            let upto = (page_size + requested).min(expected.len());
            assert_eq!(
                ids,
                &expected[page_size..upto],
                "case {case:?}/{resumed_case:?}: the resumed page is not the rest of the history: {}",
                rest.body
            );
        }
    }
}

/// One refused request: what makes it unanswerable, the awsQuery error code it
/// is refused with, and the message where the service's own template states it.
struct Refusal {
    case: &'static str,
    params: Vec<(&'static str, &'static str)>,
    code: &'static str,
    /// The exact message, where a template of this service builds it from the
    /// request; `None` where only the code is a requirement.
    message: Option<String>,
}

/// RULE: a request this operation cannot answer is REFUSED, with the service's
/// own query error code, rather than answered with an empty history.
#[tokio::test]
async fn a_request_that_names_no_history_is_refused_not_answered_empty() {
    let emulator = Emulator::new();
    let environment_id = fixture(&emulator, "history-app", "history-env-1").await;
    seed_history(&emulator, &environment_id, &seeds());

    // Rows: no identifier at all, an unknown name, an unknown id, and a
    // continuation token that names no record of this environment's history
    // (unparseable, and a plausible sequence nothing in it carries).
    let cases = vec![
        Refusal {
            case: "no identifier",
            params: vec![],
            code: "MissingParameter",
            message: Some(
                "Either EnvironmentName or EnvironmentId must be specified.".to_string(),
            ),
        },
        Refusal {
            case: "unknown name",
            params: vec![("EnvironmentName", "ghost-env")],
            code: "InvalidParameterValue",
            message: Some(eb::state::missing_environment_message(
                None,
                Some("ghost-env"),
            )),
        },
        Refusal {
            case: "unknown id",
            params: vec![("EnvironmentId", "e-ghostghostgho")],
            code: "InvalidParameterValue",
            message: Some(eb::state::missing_environment_message(
                Some("e-ghostghostgho"),
                None,
            )),
        },
        Refusal {
            case: "token that is not a token",
            params: vec![
                ("EnvironmentName", "history-env-1"),
                ("NextToken", "not-a-token"),
            ],
            code: "InvalidParameterValue",
            message: None,
        },
        Refusal {
            case: "token of no record of this history",
            params: vec![
                ("EnvironmentName", "history-env-1"),
                ("NextToken", "999999"),
            ],
            code: "InvalidParameterValue",
            message: None,
        },
    ];

    for Refusal {
        case,
        params,
        code,
        message,
    } in cases
    {
        let refused = history(&emulator, &params).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: unexpected error code in {}",
            refused.body
        );
        if let Some(message) = message {
            assert_eq!(
                refused.error_message(),
                message,
                "case {case:?}: unexpected message in {}",
                refused.body
            );
        }
        assert!(
            refused
                .values_at("ManagedActionHistoryItems/member/ActionId")
                .is_empty(),
            "case {case:?}: a refusal must report no items, got {}",
            refused.body
        );
    }

    // The other side of the same rule: `MaxItems` outside its modelled range
    // (1..100) is refused by the boundary validator before the handler runs.
    for (case, max_items) in [("below the minimum", "0"), ("above the maximum", "101")] {
        let refused = history(
            &emulator,
            &[
                ("EnvironmentName", "history-env-1"),
                ("MaxItems", max_items),
            ],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationError",
            "case {case:?}: unexpected error code in {}",
            refused.body
        );
    }
}
