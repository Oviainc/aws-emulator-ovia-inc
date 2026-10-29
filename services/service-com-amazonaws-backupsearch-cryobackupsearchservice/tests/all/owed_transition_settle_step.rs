// SPDX-License-Identifier: CC0-1.0

//! The ONE step every record of this service goes through when the clock has made a
//! transition due, driven end-to-end over HTTP through `GetSearchJob` (a search job) and
//! `GetSearchResultExportJob` (an export job).
//!
//! The rule under test: materialising a transition the clock has made due expires the
//! record's `ClientToken`, writes the new status together with the `StatusMessage` that
//! status comes with, and writes a `CompletionTime` ONLY for a status the record never
//! leaves — and a transition still ahead of the clock writes nothing at all. One rule for
//! every record type this store holds, so the table below spans the record type (both of
//! them), the status owed (one the record never leaves and one it passes through), whether
//! that status comes with a message, and whether the clock has reached the due instant.
//!
//! Nothing here sleeps: the due instant is STORED, so a row moves the clock by choosing
//! the instant the transition is owed from rather than by waiting for one.
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `Answer`) and the exportable search
//! job are the search-job and export-job suites': every suite drives the same emulator the
//! same way, so there is one harness, not two.

use std::time::{Duration, SystemTime};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_jobs::{settled_search_job, Settled};
use crate::search_jobs::{call, emulator_with_engine, Answer};

/// A non-terminal status an export job's own vocabulary declares, which is the branch of
/// the rule an export job can be owed without ever completing. Source: the model's
/// documentation of the member
/// (`smithy_query(query="doc", shape="ExportJobSummary", member="Status")`: the status of
/// the export job is one of `CREATED`, `RUNNING`, `FAILED` or `COMPLETED`).
const EXPORT_CREATED: &str = "CREATED";

/// The `StatusMessage` a record carries BEFORE the transition under test, so a row can
/// tell a message the transition wrote from one that was already there.
const EARLIER_MESSAGE: &str = "the message of the phase before the transition";

/// The `StatusMessage` an owed status comes with, for the rows whose status carries one.
const OWED_MESSAGE: &str = "the message the owed status comes with";

/// Which record of this store a row drives the settle step through, and therefore which
/// operation reads it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Record {
    SearchJob,
    ExportJob,
}

impl Record {
    /// The status the row puts the record in before the transition it is owed.
    fn status_before(self) -> &'static str {
        match self {
            // The phase a search job is created in, before its scan runs.
            Self::SearchJob => state::CREATING,
            // The status a new export job is minted in.
            Self::ExportJob => state::RUNNING,
        }
    }
}

/// Where the clock stands relative to the instant the transition is owed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clock {
    /// The due instant has passed, so the transition is materialised.
    PastTheDueInstant,
    /// The due instant is still ahead, so nothing is materialised.
    BeforeTheDueInstant,
}

/// One row: the record type, the transition it is owed, where the clock stands, and
/// whether the owed status is one the record never leaves (so whether reaching it writes
/// a `CompletionTime`).
struct Row {
    case: &'static str,
    record: Record,
    owed_status: &'static str,
    owed_message: Option<&'static str>,
    clock: Clock,
    /// Whether `owed_status` is a status the record never leaves. Stated per row rather
    /// than read from the crate's own set, so the expectation is independent of it.
    completes: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a search job owed the completion of its scan",
            record: Record::SearchJob,
            owed_status: state::COMPLETED,
            owed_message: None,
            clock: Clock::PastTheDueInstant,
            completes: true,
        },
        Row {
            case: "a search job owed a failure with the text that status comes with",
            record: Record::SearchJob,
            owed_status: state::FAILED,
            owed_message: Some(OWED_MESSAGE),
            clock: Clock::PastTheDueInstant,
            completes: true,
        },
        Row {
            case: "a search job owed the stop it is winding down to",
            record: Record::SearchJob,
            owed_status: state::STOPPED,
            owed_message: Some(OWED_MESSAGE),
            clock: Clock::PastTheDueInstant,
            completes: true,
        },
        Row {
            case: "a search job owed the running phase of its scan, which completes nothing",
            record: Record::SearchJob,
            owed_status: state::RUNNING,
            owed_message: None,
            clock: Clock::PastTheDueInstant,
            completes: false,
        },
        Row {
            case: "a search job owed a status the clock has not reached yet",
            record: Record::SearchJob,
            owed_status: state::COMPLETED,
            owed_message: Some(OWED_MESSAGE),
            clock: Clock::BeforeTheDueInstant,
            completes: true,
        },
        Row {
            case: "an export job owed its completion",
            record: Record::ExportJob,
            owed_status: state::COMPLETED,
            owed_message: None,
            clock: Clock::PastTheDueInstant,
            completes: true,
        },
        Row {
            case: "an export job owed a failure with the text that status comes with",
            record: Record::ExportJob,
            owed_status: state::FAILED,
            owed_message: Some(OWED_MESSAGE),
            clock: Clock::PastTheDueInstant,
            completes: true,
        },
        Row {
            case: "an export job owed a status it passes through, which completes nothing",
            record: Record::ExportJob,
            owed_status: EXPORT_CREATED,
            owed_message: None,
            clock: Clock::PastTheDueInstant,
            completes: false,
        },
        Row {
            case: "an export job owed a status the clock has not reached yet",
            record: Record::ExportJob,
            owed_status: state::FAILED,
            owed_message: Some(OWED_MESSAGE),
            clock: Clock::BeforeTheDueInstant,
            completes: true,
        },
    ]
}

/// Put `identifier`'s record back in the phase before a transition and make `transition`
/// the one the clock owes it, dropping whatever it was owed before. The record's status
/// members are left as the phase before the transition would leave them, so what the
/// transition itself writes is exactly what a row observes.
fn owe_transition(
    engine: &engine::InMemoryEngine,
    record: Record,
    identifier: &str,
    transition: state::OwedTransition,
) {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    match record {
        Record::SearchJob => {
            let job = store
                .search_jobs
                .get_mut(identifier)
                .expect("the search job started above");
            job.status = record.status_before().to_string();
            job.status_message = Some(EARLIER_MESSAGE.to_string());
            job.completion_time = None;
            job.owed_statuses.clear();
            job.owed_statuses.push_back(transition);
        }
        Record::ExportJob => {
            let job = store
                .export_jobs
                .get_mut(identifier)
                .expect("the export job started above");
            job.status = record.status_before().to_string();
            job.status_message = Some(EARLIER_MESSAGE.to_string());
            job.completion_time = None;
            job.next_status_due = Some(transition);
        }
    }
}

/// Whether the store still holds a transition as owed for `identifier`'s record.
fn still_owes_a_transition(
    engine: &engine::InMemoryEngine,
    record: Record,
    identifier: &str,
) -> bool {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    match record {
        Record::SearchJob => !store
            .search_jobs
            .get(identifier)
            .expect("the search job started above")
            .owed_statuses
            .is_empty(),
        Record::ExportJob => store
            .export_jobs
            .get(identifier)
            .expect("the export job started above")
            .next_status_due
            .is_some(),
    }
}

/// Give `identifier`'s record a `ClientToken` minted at `minted_at`, whose validity window
/// therefore closes one published window after that instant.
fn hold_client_token(
    engine: &engine::InMemoryEngine,
    record: Record,
    identifier: &str,
    value: &str,
    minted_at: SystemTime,
) {
    let token = state::ClientToken::minted(value.to_string(), minted_at);
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    match record {
        Record::SearchJob => {
            store
                .search_jobs
                .get_mut(identifier)
                .expect("the search job started above")
                .client_token = Some(token);
        }
        Record::ExportJob => {
            store
                .export_jobs
                .get_mut(identifier)
                .expect("the export job started above")
                .client_token = Some(token);
        }
    }
}

/// The `ClientToken` `identifier`'s record still holds, if any.
fn held_client_token(
    engine: &engine::InMemoryEngine,
    record: Record,
    identifier: &str,
) -> Option<String> {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    match record {
        Record::SearchJob => store
            .search_jobs
            .get(identifier)
            .expect("the search job started above")
            .client_token
            .as_ref()
            .map(|token| token.value().to_string()),
        Record::ExportJob => store
            .export_jobs
            .get(identifier)
            .expect("the export job started above")
            .client_token
            .as_ref()
            .map(|token| token.value().to_string()),
    }
}

/// Read `identifier`'s record back through the operation that reports it.
async fn read_record(
    emulator: &MultiServiceRouter,
    region: &str,
    record: Record,
    identifier: &str,
) -> Answer {
    let uri = match record {
        Record::SearchJob => format!("/search-jobs/{identifier}"),
        Record::ExportJob => format!("/export-search-jobs/{identifier}"),
    };
    call(emulator, region, "GET", &uri, None).await
}

/// `StartSearchResultExportJob` for `search_job`, answering the export job's identifier.
async fn start_export_job(
    emulator: &MultiServiceRouter,
    region: &str,
    search_job: &str,
) -> String {
    let started = call(
        emulator,
        region,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "settle-step-bucket"}
            }
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSearchResultExportJob must accept the request, got {} ({:?})",
        started.status, started.body
    );
    started.string("ExportJobIdentifier")
}

/// One search job and one export job of it, both minted over HTTP exactly as a client
/// mints them — the two records the settle step is driven through below.
async fn both_record_types(
    emulator: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
) -> (String, String) {
    let (search_job, _) =
        settled_search_job(emulator, engine, region, "the settle step", Settled::Completed).await;
    let export_job = start_export_job(emulator, region, &search_job).await;
    (search_job, export_job)
}

#[tokio::test]
async fn one_settle_step_writes_the_status_members_of_every_record_type() {
    let region = "us-west-2";
    let (emulator, engine) = emulator_with_engine();
    let (search_job, export_job) = both_record_types(&emulator, &engine, region).await;

    for row in rows() {
        let identifier = match row.record {
            Record::SearchJob => search_job.clone(),
            Record::ExportJob => export_job.clone(),
        };
        let now = SystemTime::now();
        let due = match row.clock {
            // A due instant the clock has already passed, and one it reaches no sooner
            // than a day from now: in both directions the comparison is the stored
            // instant against the clock, so neither row has to wait for anything.
            Clock::PastTheDueInstant => now - Duration::from_secs(60),
            Clock::BeforeTheDueInstant => now + Duration::from_secs(24 * 60 * 60),
        };
        owe_transition(
            &engine,
            row.record,
            &identifier,
            (due, row.owed_status, row.owed_message.map(ToString::to_string)),
        );

        let answer = read_record(&emulator, region, row.record, &identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the record must be reported, got {} ({:?})",
            row.case, answer.status, answer.body
        );

        let settles = row.clock == Clock::PastTheDueInstant;
        let expected_status = if settles {
            row.owed_status
        } else {
            row.record.status_before()
        };
        assert_eq!(
            answer.string("Status"),
            expected_status,
            "case {:?}: expected status {expected_status}, got {:?}",
            row.case,
            answer.get("Status")
        );

        // The status and the message that status comes with are written TOGETHER: a
        // transition that carries no message leaves the record with none, rather than with
        // the text of the phase it just left.
        let expected_message = if settles {
            row.owed_message
        } else {
            Some(EARLIER_MESSAGE)
        };
        assert_eq!(
            answer.get("StatusMessage").as_str(),
            expected_message,
            "case {:?}: expected StatusMessage {expected_message:?}, got {:?}",
            row.case,
            answer.get("StatusMessage")
        );

        // A `CompletionTime` is written only by reaching a status the record never leaves,
        // and only once the clock has made that status due.
        let completion_time = answer.get("CompletionTime").as_f64();
        assert_eq!(
            completion_time.is_some(),
            settles && row.completes,
            "case {:?}: expected a CompletionTime: {}, got {:?}",
            row.case,
            settles && row.completes,
            answer.get("CompletionTime")
        );

        // What the clock has materialised is no longer owed; what it has not reached still
        // is, so a later call makes it due.
        assert_eq!(
            still_owes_a_transition(&engine, row.record, &identifier),
            !settles,
            "case {:?}: expected the record to still owe a transition: {}, got {}",
            row.case,
            !settles,
            still_owes_a_transition(&engine, row.record, &identifier)
        );
    }
}

#[tokio::test]
async fn one_settle_step_closes_the_client_token_window_of_every_record_type() {
    let region = "eu-central-1";
    let (emulator, engine) = emulator_with_engine();
    let (search_job, export_job) = both_record_types(&emulator, &engine, region).await;

    // Rows spanning the token's age against the published window: inside it (just minted
    // and half a window old) and past it (exactly a window old and far beyond it).
    let ages: [(&str, Duration, bool); 4] = [
        ("a token minted just now", Duration::ZERO, true),
        (
            "a token half the published window old",
            state::CLIENT_TOKEN_VALIDITY / 2,
            true,
        ),
        (
            "a token exactly the published window old",
            state::CLIENT_TOKEN_VALIDITY,
            false,
        ),
        (
            "a token twice the published window old",
            state::CLIENT_TOKEN_VALIDITY * 2,
            false,
        ),
    ];

    for (age_case, age, still_held) in ages {
        for record in [Record::SearchJob, Record::ExportJob] {
            let identifier = match record {
                Record::SearchJob => search_job.clone(),
                Record::ExportJob => export_job.clone(),
            };
            let case = format!("{age_case} on a {record:?}");
            let value = "settle-step-token";
            hold_client_token(
                &engine,
                record,
                &identifier,
                value,
                SystemTime::now() - age,
            );

            let answer = read_record(&emulator, region, record, &identifier).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the record must be reported, got {} ({:?})",
                answer.status, answer.body
            );

            let held = held_client_token(&engine, record, &identifier);
            assert_eq!(
                held.as_deref(),
                if still_held { Some(value) } else { None },
                "case {case:?}: expected the record to still hold its token: \
                 {still_held}, got {held:?}"
            );
        }
    }
}
