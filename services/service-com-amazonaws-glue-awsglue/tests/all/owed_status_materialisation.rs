// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for every Glue record whose next status the CLOCK owes: a blueprint
//! registration (`CreateBlueprint`/`GetBlueprint`), an interactive session
//! (`CreateSession`/`GetSession`) and a blueprint run
//! (`StartBlueprintRun`/`GetBlueprintRun`).
//!
//! RULE: the status such a record reports is a function of the DUE INSTANT its write
//! recorded compared with the instant of the call — the status the write put it in while that
//! instant is still ahead, the status the clock owes it from the instant it is reached — so
//! every read of one record reports the same status however many times it is read, and the
//! members the write carried survive the change.
//!
//! Every call the first suite makes goes over the wire through the shared [`crate::wire`]
//! harness, so what it asserts is HTTP behaviour and not a direct call into a handler. The
//! second pins where the boundary itself lies, over the records the handlers settle: a wire
//! call is served at whatever instant it reaches the service, which no caller picks, so the
//! instants either side of the due instant are chosen there instead of raced for.
//!
//! The rows differ along what the rule generalises over: the kind of record — three, each
//! written by its own operation and read back by its own — and where the due instant sits
//! when the reads are made (ahead of the clock, exactly at it, behind it). Each row POSITIONS
//! that instant rather than leaving a margin against the wall clock, which is what makes the
//! side of it a read lands on the row's own choice and not the scheduler's.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    BlueprintEntry, BlueprintRunEntry, ComAmazonawsGlueAwsglueState, SessionEntry,
    BLUEPRINT_RUN_SETTLES_AFTER, BLUEPRINT_RUN_STATE_RUNNING, BLUEPRINT_RUN_STATE_SUCCEEDED,
    BLUEPRINT_SETTLES_AFTER, BLUEPRINT_STATUS_ACTIVE, BLUEPRINT_STATUS_CREATING,
    SESSION_SETTLES_AFTER, SESSION_STATUS_PROVISIONING, SESSION_STATUS_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, far_future};
use crate::wire::{emulator_on, ok, REGION};

/// The one name — and session id — every record here is written under.
const NAME: &str = "owed";

/// How many times each positioned record is read: more than once, so a read that changed what
/// the next read reports is caught.
const READS: usize = 3;

/// The description the blueprint and session writes carry, which the status change must leave
/// alone.
const DESCRIPTION: &str = "Owed a status by the clock.";

/// An IAM role ARN of the form the modelled `OrchestrationRoleArn` and
/// `OrchestrationIAMRoleArn` patterns require (`^arn:aws[^:]*:iam::[0-9]*:role/.+$`).
const ROLE: &str = "arn:aws:iam::000000000000:role/GlueOwedStatusRole";

/// Where the blueprint archive every registration here points at is published, as the
/// modelled `OrchestrationS3Location` pattern `^s3://([^/]+)/([^/]+/)*([^/]+)$` requires.
const LOCATION: &str = "s3://blueprint-bucket/owed.zip";

/// One row's position of the due instant: its case name, the instant itself, and whether the
/// clock owes the record its next status by then.
type Position = (&'static str, fn() -> SystemTime, bool);

/// The records whose next status the clock owes, each written and read by its own operations.
#[derive(Debug, Clone, Copy)]
enum Record {
    /// Written by `CreateBlueprint`, read back by `GetBlueprint`.
    BlueprintRegistration,
    /// Written by `CreateSession`, read back by `GetSession`.
    Session,
    /// Written by `StartBlueprintRun`, read back by `GetBlueprintRun`.
    BlueprintRun,
}

/// Every record kind this suite drives.
const RECORDS: [Record; 3] = [
    Record::BlueprintRegistration,
    Record::Session,
    Record::BlueprintRun,
];

impl Record {
    /// The status this record carries while its due instant is still ahead of the clock, and
    /// the one the clock owes it from that instant on.
    fn statuses(self) -> (&'static str, &'static str) {
        match self {
            Record::BlueprintRegistration => (BLUEPRINT_STATUS_CREATING, BLUEPRINT_STATUS_ACTIVE),
            Record::Session => (SESSION_STATUS_PROVISIONING, SESSION_STATUS_READY),
            Record::BlueprintRun => (BLUEPRINT_RUN_STATE_RUNNING, BLUEPRINT_RUN_STATE_SUCCEEDED),
        }
    }

    /// The member of the reported record the status change must not disturb, and the value
    /// the write gave it.
    fn surviving_member(self) -> (&'static str, String) {
        match self {
            Record::BlueprintRegistration | Record::Session => {
                ("Description", DESCRIPTION.to_string())
            }
            Record::BlueprintRun => ("Parameters", run_parameters()),
        }
    }

    /// The span this record's write puts between the instant it was written and the instant
    /// the clock owes it its next status.
    fn span(self) -> Duration {
        match self {
            Record::BlueprintRegistration => BLUEPRINT_SETTLES_AFTER,
            Record::Session => SESSION_SETTLES_AFTER,
            Record::BlueprintRun => BLUEPRINT_RUN_SETTLES_AFTER,
        }
    }
}

/// The `Parameters` the blueprint run is started with.
fn run_parameters() -> String {
    format!("{{\"WorkflowName\":\"{NAME}-workflow\"}}")
}

/// The body that registers the blueprint both blueprint rows need.
fn registration() -> Value {
    json!({
        "Name": NAME,
        "BlueprintLocation": LOCATION,
        "Description": DESCRIPTION,
    })
}

/// Write the record a row reads, answering the id a blueprint run is addressed by.
async fn write(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    record: Record,
) -> Option<String> {
    match record {
        Record::BlueprintRegistration => {
            ok(router, "CreateBlueprint", &registration()).await;
            None
        }
        Record::Session => {
            ok(
                router,
                "CreateSession",
                &json!({
                    "Id": NAME,
                    "Role": ROLE,
                    "Command": { "Name": "glueetl" },
                    "Description": DESCRIPTION,
                }),
            )
            .await;
            None
        }
        Record::BlueprintRun => {
            // A run may only be started of a registration the clock has already finished, so
            // the blueprint's own due instant goes behind the clock before the start.
            ok(router, "CreateBlueprint", &registration()).await;
            hold_due_at(engine, Record::BlueprintRegistration, already_past());
            let started = ok(
                router,
                "StartBlueprintRun",
                &json!({
                    "BlueprintName": NAME,
                    "RoleArn": ROLE,
                    "Parameters": run_parameters(),
                }),
            )
            .await;
            Some(
                started["RunId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("StartBlueprintRun must answer a RunId, got {started}")
                    })
                    .to_string(),
            )
        }
    }
}

/// Read the record back, answering the whole record a read reports and the status it carries.
async fn read(
    router: &MultiServiceRouter,
    record: Record,
    run_id: Option<&str>,
) -> (Value, String) {
    let (operation, request, reported, member) = match record {
        Record::BlueprintRegistration => (
            "GetBlueprint",
            json!({ "Name": NAME }),
            "Blueprint",
            "Status",
        ),
        Record::Session => ("GetSession", json!({ "Id": NAME }), "Session", "Status"),
        Record::BlueprintRun => (
            "GetBlueprintRun",
            json!({
                "BlueprintName": NAME,
                "RunId": run_id.expect("a blueprint run row starts a run"),
            }),
            "BlueprintRun",
            "State",
        ),
    };
    let answer = ok(router, operation, &request).await;
    let record = answer
        .get(reported)
        .cloned()
        .unwrap_or_else(|| panic!("{operation} must report a {reported}, got {answer}"));
    let status = record[member]
        .as_str()
        .unwrap_or_else(|| panic!("{operation} must report a {member}, got {record}"))
        .to_string();
    (record, status)
}

/// Put the instant the clock owes `record` its next status at `at`, pulling the instants its
/// write recorded back with it when `at` is already behind them, so what is stored stays the
/// record a write made before `at` would have left.
///
/// This is how a row fixes which side of the due instant its reads land on: the status turns
/// on the stored instant, so positioning that instant leaves nothing for the wall clock to
/// decide between one call and the next.
fn hold_due_at(engine: &Arc<engine::InMemoryEngine>, record: Record, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    match record {
        Record::BlueprintRegistration => {
            let entry = store
                .blueprint_mut(NAME)
                .unwrap_or_else(|| panic!("{NAME} was registered"));
            entry.created_on = entry.created_on.min(at);
            entry.last_modified_on = entry.last_modified_on.min(at);
            entry.settles_at = Some(at);
        }
        Record::Session => {
            let entry = store
                .session_mut(REGION, NAME)
                .unwrap_or_else(|| panic!("{NAME} was created"));
            entry.created_on = entry.created_on.min(at);
            entry.last_active_on = entry.last_active_on.min(at);
            entry.settles_at = Some(at);
        }
        Record::BlueprintRun => {
            let entry = store
                .blueprint_mut(NAME)
                .unwrap_or_else(|| panic!("{NAME} was registered"));
            for run in &mut entry.runs {
                run.started_on = run.started_on.min(at);
                run.settles_at = Some(at);
            }
        }
    }
}

/// Every read of one record reports the status its DUE instant says — the one the write put
/// it in while that instant is ahead of the clock, the one the clock owes it from the instant
/// it is reached — and repeated reads report the very same record.
#[tokio::test]
async fn the_status_a_record_reports_is_the_one_its_due_instant_says() {
    // (case, where the due instant sits when the reads are made, whether the clock owes the
    //  record its next status by then)
    let positions: &[Position] = &[
        ("an hour ahead of the clock", far_future, false),
        ("at the instant it was positioned", SystemTime::now, true),
        ("a second behind the clock", already_past, true),
    ];

    for record in RECORDS {
        let (in_flight, owed) = record.statuses();
        let (member, survives) = record.surviving_member();
        for (position, due_at, settled) in positions {
            let case = format!("{record:?} with its due instant {position}");
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = emulator_on(engine.clone());
            let run_id = write(&router, &engine, record).await;
            hold_due_at(&engine, record, due_at());
            let expected = if *settled { owed } else { in_flight };

            let mut first: Option<Value> = None;
            for read_number in 1..=READS {
                let (reported, status) = read(&router, record, run_id.as_deref()).await;
                assert_eq!(
                    status, expected,
                    "case {case}: read {read_number} of {READS} must report {expected}, got \
                     {reported}"
                );
                assert_eq!(
                    reported[member].as_str(),
                    Some(survives.as_str()),
                    "case {case}: read {read_number} of {READS} must keep {member} as \
                     {survives}, got {reported}"
                );
                if let Some(first) = &first {
                    assert_eq!(
                        &reported, first,
                        "case {case}: read {read_number} of {READS} must report what the first \
                         read did, got {reported}"
                    );
                }
                first = Some(reported);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The gate is the due instant itself
// ---------------------------------------------------------------------------

/// A copy of a written record, so the instant it is settled at can be CHOSEN.
///
/// A wire call is served at the instant it reaches the service, which no caller picks, so the
/// exact boundary — owed from the due instant, not a tick before it — is pinned here over the
/// records the handlers settle, at instants this suite names.
#[derive(Debug, Clone)]
enum Settling {
    BlueprintRegistration(BlueprintEntry),
    Session(Box<SessionEntry>),
    BlueprintRun(BlueprintRunEntry),
}

impl Settling {
    /// The instant the write recorded this record at, which its due instant is a span after.
    fn written_at(&self) -> SystemTime {
        match self {
            Settling::BlueprintRegistration(entry) => entry.created_on,
            Settling::Session(entry) => entry.created_on,
            Settling::BlueprintRun(entry) => entry.started_on,
        }
    }

    /// Take this copy's clock to `at` and answer the status it then reports.
    fn status_at(&self, at: SystemTime) -> String {
        match self.clone() {
            Settling::BlueprintRegistration(mut entry) => {
                entry.settle_if_due(at);
                entry.status
            }
            Settling::Session(mut entry) => {
                entry.settle_if_due(at);
                entry.status
            }
            Settling::BlueprintRun(mut entry) => {
                entry.settle_if_due(at);
                entry.state
            }
        }
    }
}

/// The written record and the instant the clock owes it its next status.
fn settling(engine: &Arc<engine::InMemoryEngine>, record: Record) -> (Settling, SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    let (settling, due) = match record {
        Record::BlueprintRegistration => {
            let entry = store
                .blueprint(NAME)
                .unwrap_or_else(|| panic!("{NAME} was registered"))
                .clone();
            let due = entry.settles_at;
            (Settling::BlueprintRegistration(entry), due)
        }
        Record::Session => {
            let entry = store
                .session(REGION, NAME)
                .unwrap_or_else(|| panic!("{NAME} was created"))
                .clone();
            let due = entry.settles_at;
            (Settling::Session(Box::new(entry)), due)
        }
        Record::BlueprintRun => {
            let entry = store
                .blueprint(NAME)
                .unwrap_or_else(|| panic!("{NAME} was registered"))
                .runs
                .first()
                .unwrap_or_else(|| panic!("{NAME} holds the run that was started"))
                .clone();
            let due = entry.settles_at;
            (Settling::BlueprintRun(entry), due)
        }
    };
    (
        settling,
        due.unwrap_or_else(|| panic!("{record:?} is owed a status at a stored instant")),
    )
}

/// Where a case takes the clock, relative to the record's own due instant.
#[derive(Debug, Clone, Copy)]
enum Clock {
    Before(Duration),
    At,
    After(Duration),
}

impl Clock {
    /// The instant this case settles the record at.
    fn instant(self, due: SystemTime) -> SystemTime {
        match self {
            Clock::Before(by) => due - by,
            Clock::At => due,
            Clock::After(by) => due + by,
        }
    }
}

/// The span a write records is NOT zero: the instant the clock owes a record its next status
/// sits that record's whole span after the instant it was written, which is what leaves a
/// caller reading straight after the write the status the write put the record in.
///
/// Read off the stored record, so no call has to land inside the span for this to hold.
#[tokio::test]
async fn a_write_records_a_due_instant_a_whole_span_out() {
    for record in RECORDS {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        write(&router, &engine, record).await;
        let (settling, due) = settling(&engine, record);
        let written_at = settling.written_at();
        let span = due.duration_since(written_at).unwrap_or_else(|error| {
            panic!("case {record:?}: the due instant must follow the write, got {error}")
        });
        assert_eq!(
            span,
            record.span(),
            "case {record:?}: expected a due instant {:?} after the write, got {span:?}",
            record.span()
        );
    }
}

/// The owed status falls due AT the due instant and not a tick before it: the comparison the
/// handlers settle a record by is `due <= now`, so a clock anywhere before that instant leaves
/// the record in the status its write put it in, and a clock at it or after it reports the
/// status the clock owes.
///
/// The rows differ along the kind of record and how far from the due instant the clock is
/// taken, down to the single nanosecond either side of it.
#[tokio::test]
async fn the_owed_status_falls_due_at_the_due_instant_and_not_before() {
    // (case, where the clock is taken, whether the owed status is reported there)
    let cases: &[(&str, Clock, bool)] = &[
        ("an hour before the due instant", Clock::Before(Duration::from_secs(3_600)), false),
        ("a second before it", Clock::Before(Duration::from_secs(1)), false),
        ("a millisecond before it", Clock::Before(Duration::from_millis(1)), false),
        ("a nanosecond before it", Clock::Before(Duration::from_nanos(1)), false),
        ("exactly at it", Clock::At, true),
        ("a nanosecond after it", Clock::After(Duration::from_nanos(1)), true),
        ("an hour after it", Clock::After(Duration::from_secs(3_600)), true),
    ];

    for record in RECORDS {
        let (in_flight, owed) = record.statuses();
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        write(&router, &engine, record).await;
        let (settling, due) = settling(&engine, record);

        for (case, clock, is_owed) in cases {
            let expected = if *is_owed { owed } else { in_flight };
            let status = settling.status_at(clock.instant(due));
            assert_eq!(
                status, expected,
                "case {record:?} settled {case}: expected {expected}, got {status}"
            );
        }
    }
}
