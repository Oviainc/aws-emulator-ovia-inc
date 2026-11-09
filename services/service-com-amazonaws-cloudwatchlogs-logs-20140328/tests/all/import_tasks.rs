// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeImportTasks`: what the service reports about the
//! account's CloudTrail Lake import tasks.
//!
//! The rule under test: `DescribeImportTasks` reports, from stored state, every import
//! task that satisfies EVERY filter the request names (`importId`, `importStatus`,
//! `importSourceArn`), each one carrying all of the model's `Import` members as that task
//! holds them, enumerated in one stable order and handed back at most `limit` (default 50)
//! per page with a `nextToken` exactly when more remain. An `importId` no task carries is
//! a missing resource, not an empty page, and the status it reports is the one the clock
//! has already made due — a read never moves a task on.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.DescribeImportTasks` through the router that mounts it — so
//! what is asserted here is what a client reads off the wire.
//!
//! The tasks themselves are put in the store through
//! [`ImportTaskRecord::new`] — the SAME constructor `CreateImportTask` calls — so that each
//! case seeds exactly the state it is about, including the statuses no create hands back
//! (cancelled, failed) and a corpus wide enough to page. What a CREATED import task carries
//! is `CreateImportTask`'s own rule, asserted over the wire in
//! `tests/all/import_task_creation.rs`.
//!
//! The table spans what the rule generalises over rather than one request: each value of
//! the model's `ImportStatus` enum, several sources and one no task uses, a filter present
//! at neither / one / both bounds, byte counts from zero up, filters in combination, and
//! every page size from one task to more than the store holds.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::ImportFilter;
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, ComAmazonawsCloudwatchlogsLogs20140328State, ImportTaskRecord,
    IMPORT_COMPLETION_DELAY_MS, IMPORT_STATUS_CANCELLED, IMPORT_STATUS_COMPLETED,
    IMPORT_STATUS_FAILED, IMPORT_STATUS_IN_PROGRESS,
};

use crate::harness::{now_ms, Logs};

/// The operation every case calls.
const OP: &str = "DescribeImportTasks";

/// The role a `CreateImportTask` names ("The ARN of the IAM role that grants CloudWatch
/// Logs permission to import from the CloudTrail Lake Event Data Store"). Stored with the
/// task; no `Import` member reports it.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/CloudWatchLogsImport";

/// Three CloudTrail Lake event data stores to import from, and a fourth no stored task
/// names — `importSourceArn` is "Optional filter to list imports from a specific source".
const SOURCE_ONE: &str =
    "arn:aws:cloudtrail:eu-west-2:123456789012:eventdatastore/11111111-1111-1111-1111-111111111111";
const SOURCE_TWO: &str =
    "arn:aws:cloudtrail:eu-west-2:123456789012:eventdatastore/22222222-2222-2222-2222-222222222222";
const SOURCE_THREE: &str =
    "arn:aws:cloudtrail:eu-west-2:123456789012:eventdatastore/33333333-3333-3333-3333-333333333333";
const SOURCE_UNUSED: &str =
    "arn:aws:cloudtrail:eu-west-2:123456789012:eventdatastore/44444444-4444-4444-4444-444444444444";

/// What a FAILED task reports in `Import$errorMessage` ("Error message related to any
/// failed imports"). Any text does: the rule under test is that the member is reported
/// from state for a failed task and absent for every other.
const FAILURE: &str = "the import source became unavailable";

/// One seeded import task: the state a reporting call must answer from.
struct Seed {
    /// `importId`, also the key the store enumerates by.
    id: &'static str,
    source: &'static str,
    status: &'static str,
    /// The event-time bounds the create named, at neither, one or both ends.
    filter: Option<ImportFilter>,
    bytes_imported: i64,
    /// Milliseconds before the suite's clock reading that the task was created.
    created_ago_ms: i64,
}

/// The managed destination log group's ARN for one import: the resource
/// `CreateImportTask` documents creating, "arn:aws:logs:…:log-group:/aws/cloudtrail/…".
fn destination_arn(id: &str) -> String {
    log_group_arn(
        crate::harness::REGION,
        &format!("/aws/cloudtrail/{id}"),
        true,
    )
}

/// The corpus every filtering, paging and reporting case below reads: six tasks spanning
/// all four `ImportStatus` values, three sources, and each form an `ImportFilter` takes.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            id: "import-a1",
            source: SOURCE_ONE,
            status: IMPORT_STATUS_IN_PROGRESS,
            filter: None,
            bytes_imported: 0,
            created_ago_ms: 6_000,
        },
        Seed {
            id: "import-b2",
            source: SOURCE_ONE,
            status: IMPORT_STATUS_COMPLETED,
            filter: Some(ImportFilter {
                start_event_time: Some(1_600_000_000_000),
                end_event_time: Some(1_700_000_000_000),
            }),
            bytes_imported: 4_096,
            created_ago_ms: 5_000,
        },
        Seed {
            id: "import-c3",
            source: SOURCE_TWO,
            status: IMPORT_STATUS_COMPLETED,
            filter: Some(ImportFilter {
                start_event_time: Some(1_500_000_000_000),
                end_event_time: None,
            }),
            bytes_imported: 1,
            created_ago_ms: 4_000,
        },
        Seed {
            id: "import-d4",
            source: SOURCE_TWO,
            status: IMPORT_STATUS_CANCELLED,
            filter: Some(ImportFilter {
                start_event_time: None,
                end_event_time: Some(1_650_000_000_000),
            }),
            bytes_imported: 512,
            created_ago_ms: 3_000,
        },
        Seed {
            id: "import-e5",
            source: SOURCE_THREE,
            status: IMPORT_STATUS_FAILED,
            filter: Some(ImportFilter {
                start_event_time: Some(0),
                end_event_time: Some(1_234_567_890_123),
            }),
            bytes_imported: 0,
            created_ago_ms: 2_000,
        },
        Seed {
            id: "import-f6",
            source: SOURCE_THREE,
            status: IMPORT_STATUS_IN_PROGRESS,
            filter: None,
            bytes_imported: 123_456_789,
            created_ago_ms: 1_000,
        },
    ]
}

/// Every seeded id, in the order the store enumerates them (by `importId`).
fn all_ids() -> Vec<String> {
    let mut ids: Vec<String> = seeds().into_iter().map(|seed| seed.id.to_string()).collect();
    ids.sort();
    ids
}

/// Put [`seeds`] in the shared store and answer the clock reading they were built against.
///
/// A task the row says is still running keeps a completion instant far enough out that the
/// clock cannot reach it while the suite runs: the transition the clock owes is the subject
/// of its own test below, not a race in these.
fn seed_store(logs: &Logs) -> i64 {
    let now = now_ms();
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    for seed in seeds() {
        let created = now - seed.created_ago_ms;
        let mut record = ImportTaskRecord::new(
            seed.id,
            seed.source,
            ROLE_ARN,
            &destination_arn(seed.id),
            seed.filter.as_ref(),
            created,
        );
        record.bytes_imported = seed.bytes_imported;
        if seed.status == IMPORT_STATUS_IN_PROGRESS {
            record.completion_due_ms = Some(now + 60 * 60 * 1_000);
        } else {
            record.import_status = seed.status.to_string();
            record.completion_due_ms = None;
            record.last_updated_time_ms = created + 1_000;
            if seed.status == IMPORT_STATUS_FAILED {
                record.error_message = Some(FAILURE.to_string());
            }
        }
        store.import_tasks.insert(record.import_id.clone(), record);
    }
    now
}

/// The `imports` member of one successful page.
async fn page(logs: &Logs, body: &Value) -> Vec<Value> {
    let response = logs.ok(OP, body).await;
    response
        .get("imports")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{OP} {body}: the response must carry imports, got {response}"))
        .clone()
}

/// The `importId`s of one successful page, in the order they were reported.
async fn reported_ids(logs: &Logs, body: &Value) -> Vec<String> {
    page(logs, body)
        .await
        .iter()
        .map(|import| {
            import
                .get("importId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every reported import carries an importId, got {import}"))
                .to_string()
        })
        .collect()
}

/// A filter selects exactly the stored tasks that match it — every value of the model's
/// `ImportStatus` enum, each source and one no task uses, an id, and filters in
/// combination — and reports them in the store's one enumeration order.
#[tokio::test]
async fn every_filter_selects_exactly_the_tasks_that_match_it() {
    let logs = Logs::new();
    seed_store(&logs);

    // (case, request, the importIds the rule requires, in order)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec![
                "import-a1",
                "import-b2",
                "import-c3",
                "import-d4",
                "import-e5",
                "import-f6",
            ],
        ),
        // The four values of the model's `ImportStatus` enum: IN_PROGRESS, CANCELLED,
        // COMPLETED, FAILED.
        (
            "importStatus IN_PROGRESS",
            json!({"importStatus": IMPORT_STATUS_IN_PROGRESS}),
            vec!["import-a1", "import-f6"],
        ),
        (
            "importStatus COMPLETED",
            json!({"importStatus": IMPORT_STATUS_COMPLETED}),
            vec!["import-b2", "import-c3"],
        ),
        (
            "importStatus CANCELLED",
            json!({"importStatus": IMPORT_STATUS_CANCELLED}),
            vec!["import-d4"],
        ),
        (
            "importStatus FAILED",
            json!({"importStatus": IMPORT_STATUS_FAILED}),
            vec!["import-e5"],
        ),
        (
            "importSourceArn of the first source",
            json!({"importSourceArn": SOURCE_ONE}),
            vec!["import-a1", "import-b2"],
        ),
        (
            "importSourceArn of the second source",
            json!({"importSourceArn": SOURCE_TWO}),
            vec!["import-c3", "import-d4"],
        ),
        (
            "importSourceArn of the third source",
            json!({"importSourceArn": SOURCE_THREE}),
            vec!["import-e5", "import-f6"],
        ),
        (
            "importSourceArn no stored task imports from",
            json!({"importSourceArn": SOURCE_UNUSED}),
            vec![],
        ),
        (
            "importId of one stored task",
            json!({"importId": "import-c3"}),
            vec!["import-c3"],
        ),
        (
            "importId and the status that task is in",
            json!({"importId": "import-e5", "importStatus": IMPORT_STATUS_FAILED}),
            vec!["import-e5"],
        ),
        (
            "importId and a status that task is not in",
            json!({"importId": "import-e5", "importStatus": IMPORT_STATUS_COMPLETED}),
            vec![],
        ),
        (
            "importStatus and importSourceArn that one task satisfies together",
            json!({"importStatus": IMPORT_STATUS_COMPLETED, "importSourceArn": SOURCE_ONE}),
            vec!["import-b2"],
        ),
        (
            "importStatus and importSourceArn no task satisfies together",
            json!({"importStatus": IMPORT_STATUS_CANCELLED, "importSourceArn": SOURCE_ONE}),
            vec![],
        ),
    ];

    for (name, body, expected) in rows {
        let actual = reported_ids(&logs, &body).await;
        assert_eq!(
            actual, expected,
            "case {name:?}: expected imports {expected:?}, got {actual:?}"
        );
    }
}

/// Every member of the model's `Import` shape is reported as the task holds it: the ids
/// and ARNs, the status, the statistics, the event-time filter at whichever bounds it has,
/// both timestamps, and `errorMessage` on the failed task alone.
#[tokio::test]
async fn a_reported_task_carries_every_member_from_state() {
    let logs = Logs::new();
    let now = seed_store(&logs);

    for seed in seeds() {
        let name = seed.id;
        let reported = page(&logs, &json!({"importId": seed.id})).await;
        assert_eq!(
            reported.len(),
            1,
            "case {name:?}: one id reports one task, got {reported:?}"
        );
        let import = &reported[0];
        let created = now - seed.created_ago_ms;

        let string = |member: &str| import.get(member).and_then(Value::as_str);
        let number = |member: &str| import.get(member).and_then(Value::as_i64);

        assert_eq!(
            string("importId"),
            Some(seed.id),
            "case {name:?}: expected importId {:?}, got {import}",
            seed.id
        );
        assert_eq!(
            string("importSourceArn"),
            Some(seed.source),
            "case {name:?}: expected importSourceArn {:?}, got {import}",
            seed.source
        );
        assert_eq!(
            string("importStatus"),
            Some(seed.status),
            "case {name:?}: expected importStatus {:?}, got {import}",
            seed.status
        );
        let destination = destination_arn(seed.id);
        assert_eq!(
            string("importDestinationArn"),
            Some(destination.as_str()),
            "case {name:?}: expected importDestinationArn {destination:?}, got {import}"
        );
        assert_eq!(
            import
                .get("importStatistics")
                .and_then(|stats| stats.get("bytesImported"))
                .and_then(Value::as_i64),
            Some(seed.bytes_imported),
            "case {name:?}: expected bytesImported {}, got {import}",
            seed.bytes_imported
        );
        assert_eq!(
            number("creationTime"),
            Some(created),
            "case {name:?}: expected creationTime {created}, got {import}"
        );
        let expected_last_updated = if seed.status == IMPORT_STATUS_IN_PROGRESS {
            created
        } else {
            created + 1_000
        };
        assert_eq!(
            number("lastUpdatedTime"),
            Some(expected_last_updated),
            "case {name:?}: expected lastUpdatedTime {expected_last_updated}, got {import}"
        );
        // `errorMessage` is "Error message related to any failed imports": the failed
        // task reports one and no other task carries the member at all.
        let expected_error = (seed.status == IMPORT_STATUS_FAILED).then_some(FAILURE);
        assert_eq!(
            string("errorMessage"),
            expected_error,
            "case {name:?}: expected errorMessage {expected_error:?}, got {import}"
        );
        // `ImportFilter`'s two members are both optional, so a task created with neither
        // bound reports no filter and one created with either reports exactly that bound.
        match &seed.filter {
            None => assert!(
                import.get("importFilter").is_none(),
                "case {name:?}: a task created without event-time bounds reports no \
                 importFilter, got {import}"
            ),
            Some(filter) => {
                let reported_filter = import
                    .get("importFilter")
                    .unwrap_or_else(|| panic!("case {name:?}: expected an importFilter, got {import}"));
                assert_eq!(
                    reported_filter.get("startEventTime").and_then(Value::as_i64),
                    filter.start_event_time,
                    "case {name:?}: expected startEventTime {:?}, got {reported_filter}",
                    filter.start_event_time
                );
                assert_eq!(
                    reported_filter.get("endEventTime").and_then(Value::as_i64),
                    filter.end_event_time,
                    "case {name:?}: expected endEventTime {:?}, got {reported_filter}",
                    filter.end_event_time
                );
            }
        }
    }
}

/// Whatever the page size, paging walks the one enumeration exactly once: every page is at
/// most `limit` long, a `nextToken` comes back exactly when tasks remain, and the pages
/// concatenate to the full enumeration with nothing repeated or skipped.
#[tokio::test]
async fn paging_walks_the_enumeration_once_at_every_page_size() {
    let logs = Logs::new();
    seed_store(&logs);
    let expected = all_ids();

    // One page size below the corpus, one at it, one above, and the modelled maximum.
    for limit in [1_i64, 2, 3, 5, 6, 7, 50] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = match &token {
                Some(next) => json!({"limit": limit, "nextToken": next}),
                None => json!({"limit": limit}),
            };
            let response = logs.ok(OP, &body).await;
            let imports = response
                .get("imports")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case limit {limit}: expected imports, got {response}"))
                .clone();
            assert!(
                i64::try_from(imports.len()).unwrap_or(i64::MAX) <= limit,
                "case limit {limit}: a page holds at most {limit} tasks, got {}",
                imports.len()
            );
            for import in &imports {
                collected.push(
                    import
                        .get("importId")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| panic!("case limit {limit}: expected an importId, got {import}"))
                        .to_string(),
                );
            }
            token = response
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let remaining = expected.len() - collected.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case limit {limit}: after {} of {} tasks the nextToken presence must match \
                 whether more remain, got {response}",
                collected.len(),
                expected.len()
            );
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case limit {limit}: paging must terminate, got {pages} pages for {} tasks",
                expected.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected,
            "case limit {limit}: the pages must concatenate to the whole enumeration, \
             expected {expected:?}, got {collected:?}"
        );
    }
}

/// A `nextToken` is a position within ONE filtered enumeration: continuing a different
/// enumeration with it, or handing over something this service never minted, is refused
/// rather than answered from the wrong place.
#[tokio::test]
async fn a_token_from_another_enumeration_is_refused() {
    let logs = Logs::new();
    seed_store(&logs);

    let first = logs.ok(OP, &json!({"limit": 1})).await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the first page of six tasks must carry a nextToken, got {first}"))
        .to_string();

    // The same token continued under a filter the enumeration it was minted for did not
    // carry, and three values no enumeration of this service ever minted.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a token from the unfiltered enumeration, replayed with a status filter",
            json!({"limit": 1, "nextToken": token, "importStatus": IMPORT_STATUS_COMPLETED}),
        ),
        (
            "a token from the unfiltered enumeration, replayed with a source filter",
            json!({"limit": 1, "nextToken": token, "importSourceArn": SOURCE_ONE}),
        ),
        (
            "a token that is not hex at all",
            json!({"nextToken": "not-a-token"}),
        ),
        (
            "a token that is hex but carries no cursor",
            json!({"nextToken": "0011deadbeef"}),
        ),
    ];
    for (name, body) in rows {
        let code = logs.err(OP, &body).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {name:?}: expected InvalidParameterException, got {code}"
        );
    }

    // The token itself still works for the enumeration it was minted for.
    let continued = reported_ids(&logs, &json!({"limit": 1, "nextToken": token})).await;
    assert_eq!(
        continued,
        vec!["import-b2".to_string()],
        "case \"the token's own enumeration\": expected the second task, got {continued:?}"
    );
}

/// An `importId` no task carries names a resource that does not exist: the operation
/// refuses with `ResourceNotFoundException` and the sentence CloudWatch Logs words that
/// condition in, rather than reporting an empty page.
#[tokio::test]
async fn an_import_id_no_task_carries_is_a_missing_resource() {
    let logs = Logs::new();
    seed_store(&logs);

    // Ids no stored task carries: one of the same shape as a stored id, one that is a
    // stored id with a suffix, and one bearing no resemblance.
    for id in ["import-z9", "import-a1-gone", "NOTHING"] {
        let (code, message) = logs.refusal(OP, &json!({"importId": id})).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {code} {message:?}"
        );
        assert_eq!(
            message, "No matching import found.",
            "case {id:?}: expected the sentence CloudWatch Logs states for an id it holds no \
             import under, got {message:?}"
        );
    }
}

/// `importSourceArn` is an ARN: a value that is not one names no source and is refused
/// with the request-derived parse message, not answered with an empty page.
#[tokio::test]
async fn a_source_filter_that_is_not_an_arn_is_refused() {
    let logs = Logs::new();
    seed_store(&logs);

    for value in ["not-an-arn", "arn:aws:cloudtrail", "eventdatastore/1"] {
        let (code, message) = logs
            .refusal(OP, &json!({"importSourceArn": value}))
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {value:?}: expected InvalidParameterException, got {code} {message:?}"
        );
        let expected = format!("1 validation error detected: Unable to parse importSourceArn {value}");
        assert_eq!(
            message, expected,
            "case {value:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// Both published bounds of `limit` reach the operation, and one past either is refused by
/// the modelled constraint. Source: the model's `DescribeLimit` shape, `@range` 1..50,
/// which `DescribeImportTasks$limit` targets.
#[tokio::test]
async fn the_published_limit_bounds_are_accepted_and_one_past_each_is_refused() {
    let logs = Logs::new();
    seed_store(&logs);

    for limit in [1, 50] {
        let reported = reported_ids(&logs, &json!({"limit": limit})).await;
        let expected = all_ids()
            .into_iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect::<Vec<_>>();
        assert_eq!(
            reported, expected,
            "case \"limit {limit}\": a value on the published bound is answered, \
             expected {expected:?}, got {reported:?}"
        );
    }

    // (case, limit, the constraint clause the refusal must state)
    let rows: [(i64, &str); 2] = [
        (0, "Member must have value greater than or equal to 1"),
        (51, "Member must have value less than or equal to 50"),
    ];
    for (limit, clause) in rows {
        let (code, message) = logs.refusal(OP, &json!({"limit": limit})).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case \"limit {limit}\": expected InvalidParameterException, got {code} {message:?}"
        );
        let expected =
            format!("1 validation error detected: Value at 'limit' failed to satisfy constraint: {clause}");
        assert_eq!(
            message, expected,
            "case \"limit {limit}\": expected {expected:?}, got {message:?}"
        );
    }
}

/// The status a report carries is the one the CLOCK already made due: a running import
/// whose completion instant has passed is reported `COMPLETED`, updated AT that instant,
/// while one whose instant is still ahead stays `IN_PROGRESS` — and reporting twice
/// reports the same thing, because a read moves nothing on.
#[tokio::test]
async fn the_completion_the_clock_owes_is_reported_and_reading_changes_nothing() {
    let logs = Logs::new();
    let now = now_ms();
    // One task created long enough ago that its completion was already due, and one
    // created now, whose completion is still ahead.
    let due = now - IMPORT_COMPLETION_DELAY_MS - 1;
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for (id, created) in [("import-due", due), ("import-running", now)] {
            let record = ImportTaskRecord::new(
                id,
                SOURCE_ONE,
                ROLE_ARN,
                &destination_arn(id),
                None,
                created,
            );
            store.import_tasks.insert(record.import_id.clone(), record);
        }
    }

    // (case, importId, the status owed, the lastUpdatedTime owed)
    let rows: [(&str, &str, &str, i64); 2] = [
        (
            "a completion the clock already made due",
            "import-due",
            IMPORT_STATUS_COMPLETED,
            due + IMPORT_COMPLETION_DELAY_MS,
        ),
        (
            "a completion still ahead of the clock",
            "import-running",
            IMPORT_STATUS_IN_PROGRESS,
            now,
        ),
    ];

    // Twice, so a read that changed what a later caller sees would show up here.
    for round in 1..=2 {
        for (name, id, status, last_updated) in rows {
            let reported = page(&logs, &json!({"importId": id})).await;
            assert_eq!(
                reported.len(),
                1,
                "case {name:?} (round {round}): one id reports one task, got {reported:?}"
            );
            let import = &reported[0];
            assert_eq!(
                import.get("importStatus").and_then(Value::as_str),
                Some(status),
                "case {name:?} (round {round}): expected importStatus {status:?}, got {import}"
            );
            assert_eq!(
                import.get("lastUpdatedTime").and_then(Value::as_i64),
                Some(last_updated),
                "case {name:?} (round {round}): expected lastUpdatedTime {last_updated}, \
                 got {import}"
            );
        }
    }

    // And a status filter sees the same thing the unfiltered report does.
    let completed = reported_ids(&logs, &json!({"importStatus": IMPORT_STATUS_COMPLETED})).await;
    assert_eq!(
        completed,
        vec!["import-due".to_string()],
        "case \"filtering on the materialised status\": expected the completed task, \
         got {completed:?}"
    );
}
