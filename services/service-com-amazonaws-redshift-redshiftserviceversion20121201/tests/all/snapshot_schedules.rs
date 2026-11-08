// SPDX-License-Identifier: CC0-1.0

//! The snapshot schedule lifecycle, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created schedule is reported and stored exactly as the request
//! configured it, whatever expression type its definitions are written in, how
//! many it carries and how many tags it has; its `NextInvocations` are the
//! instants those definitions name, in order and spaced by the period each
//! states; a definition written in an expression type the service reads no
//! schedule from, and one that states no schedule, are refused with the fault
//! the model declares for each and nothing stored; the IDENTIFIER being what an
//! account's schedules are unique on, so a second create under one identifier is
//! refused and overwrites nothing; the documented identifier syntax; a dry run
//! reporting the schedule it describes and storing nothing; a schedule's
//! associated clusters being read off the clusters that name it; and the
//! creation order, the `ScheduleIdentifier` / `ClusterIdentifier` /
//! `TagKeys` / `TagValues` filters, the service's `MaxRecords` window and its
//! `Marker` cursor.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// One row of the create table: everything a `CreateSnapshotSchedule` request
/// carries, plus how often the schedule it states fires.
struct CreateCase {
    case: &'static str,
    identifier: &'static str,
    /// The `ScheduleDefinitions` the request carries, in order.
    definitions: &'static [&'static str],
    /// The `ScheduleDescription` the request carries, or `None`.
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    /// The seconds between two consecutive invocations of the schedule these
    /// definitions state, when every one of them states the same period; `None`
    /// when the row's definitions do not state one (none at all, or two that
    /// interleave).
    period_seconds: Option<i64>,
    /// The text every invocation of a CALENDAR definition ends with — the
    /// minute and second the expression names — for a row whose definitions name
    /// a wall-clock time.
    invocation_suffix: Option<&'static str>,
}

/// The create table: one row per shape of request, differing along the
/// expression type of its definitions, the value and unit of a rate, how many
/// definitions it carries, whether it carries a description and how many tags.
const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        case: "a rate in hours",
        identifier: "schedule-rate-hours",
        definitions: &["rate(12 hours)"],
        description: Some("every twelve hours"),
        tags: &[],
        period_seconds: Some(12 * 60 * 60),
        invocation_suffix: None,
    },
    CreateCase {
        case: "a rate of one day",
        identifier: "schedule-rate-day",
        definitions: &["rate(1 day)"],
        description: Some("daily"),
        tags: &[("owner", "admin")],
        period_seconds: Some(24 * 60 * 60),
        invocation_suffix: None,
    },
    CreateCase {
        case: "a rate in minutes",
        identifier: "ScheduleRateMinutes",
        definitions: &["rate(90 minutes)"],
        description: None,
        tags: &[("owner", "admin"), ("environment", "production")],
        period_seconds: Some(90 * 60),
        invocation_suffix: None,
    },
    CreateCase {
        case: "a cron expression with the year field",
        identifier: "schedule-cron-daily",
        definitions: &["cron(0 12 * * ? *)"],
        description: Some("every day at noon UTC"),
        tags: &[],
        period_seconds: Some(24 * 60 * 60),
        invocation_suffix: Some("T12:00:00Z"),
    },
    CreateCase {
        case: "a cron expression without the year field",
        identifier: "schedule-cron-hourly",
        definitions: &["cron(15 * * * ?)"],
        description: None,
        tags: &[],
        period_seconds: Some(60 * 60),
        invocation_suffix: Some(":15:00Z"),
    },
    CreateCase {
        // The form the model's own example for `ScheduleDefinitions` is written
        // in: the leading fields alone, each omitted field standing for every
        // value of it.
        case: "a cron expression of its leading fields only",
        identifier: "schedule-cron-leading-fields",
        definitions: &["cron(30 12 *)"],
        description: None,
        tags: &[],
        period_seconds: Some(24 * 60 * 60),
        invocation_suffix: Some("T12:30:00Z"),
    },
    CreateCase {
        case: "two definitions of different types",
        identifier: "schedule-two-definitions",
        definitions: &["rate(12 hours)", "cron(0 0 * * ? *)"],
        description: Some("twice a day and at midnight"),
        tags: &[("owner", "admin")],
        period_seconds: None,
        invocation_suffix: None,
    },
    CreateCase {
        case: "no definitions at all",
        identifier: "schedule-without-definitions",
        definitions: &[],
        description: Some("nothing to compute an invocation from"),
        tags: &[],
        period_seconds: None,
        invocation_suffix: None,
    },
];

/// The form parameters of a `CreateSnapshotSchedule` request.
///
/// Every member is optional on the wire, so each is added only when the case
/// carries it: that is what lets one builder serve the create rows, the dry-run
/// rows and the identifier-syntax rows.
fn create_params(
    identifier: Option<&str>,
    definitions: &[&str],
    description: Option<&str>,
    tags: &[(&str, &str)],
    next_invocations: Option<i32>,
    dry_run: Option<bool>,
) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = Vec::new();
    if let Some(identifier) = identifier {
        params.push(("ScheduleIdentifier".to_string(), identifier.to_string()));
    }
    for (position, definition) in definitions.iter().enumerate() {
        params.push((
            format!("ScheduleDefinitions.ScheduleDefinition.{}", position + 1),
            (*definition).to_string(),
        ));
    }
    if let Some(description) = description {
        params.push(("ScheduleDescription".to_string(), description.to_string()));
    }
    for (position, (key, value)) in tags.iter().enumerate() {
        params.push((
            format!("Tags.Tag.{}.Key", position + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Tags.Tag.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    if let Some(count) = next_invocations {
        params.push(("NextInvocations".to_string(), count.to_string()));
    }
    if let Some(dry_run) = dry_run {
        params.push(("DryRun".to_string(), dry_run.to_string()));
    }
    params
}

/// Drive one action with owned parameters, so a case can build its values.
async fn call_with(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// `CreateSnapshotSchedule` for one row of [`CREATE_CASES`].
async fn create_schedule(emulator: &Emulator, row: &CreateCase) -> Answer {
    let params = create_params(
        Some(row.identifier),
        row.definitions,
        row.description,
        row.tags,
        None,
        None,
    );
    call_with(emulator, "CreateSnapshotSchedule", &params).await
}

/// `DescribeSnapshotSchedules` for one `ScheduleIdentifier`.
async fn describe_one(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "DescribeSnapshotSchedules",
            &[("ScheduleIdentifier", identifier)],
        )
        .await
}

/// The `<ScheduleIdentifier>` of every schedule a body carries, in the order the
/// page lists them.
fn listed_schedules(body: &str) -> Vec<String> {
    all_text(body, "ScheduleIdentifier")
}

/// The `<ScheduleDefinition>` entries a body carries, in document order.
fn listed_definitions(body: &str) -> Vec<String> {
    all_text(body, "ScheduleDefinition")
}

/// The `<SnapshotTime>` entries a body carries, in document order: the schedule's
/// upcoming invocations.
fn listed_invocations(body: &str) -> Vec<String> {
    all_text(body, "SnapshotTime")
}

/// The tag key/value pairs a body carries, in document order.
fn listed_tags(body: &str) -> Vec<(String, String)> {
    all_text(body, "Key")
        .into_iter()
        .zip(all_text(body, "Value"))
        .collect()
}

/// One reported invocation as seconds since the epoch.
///
/// The timestamps travel in the awsQuery date-time format, which is what the
/// framework's serializer writes a `TStamp` as, so the same type parses them
/// back.
fn invocation_seconds(case: &str, reported: &str) -> i64 {
    aws_smithy_types::DateTime::from_str(reported, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case}: expected an invocation time, got {reported:?} ({error})")
        })
        .secs()
}

/// `SystemTime::now()` as seconds since the epoch, for the "strictly in the
/// future" bounds.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs(),
    )
    .expect("the clock is within the range a timestamp represents")
}

/// The definitions the STORE holds for one schedule, read straight from this
/// service's state rather than from a response.
fn stored_definitions(emulator: &Emulator, identifier: &str) -> Option<Vec<String>> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store
        .snapshot_schedule(identifier)
        .map(|schedule| schedule.schedule_definitions.clone())
}

/// How many schedules the store holds.
fn stored_count(emulator: &Emulator) -> usize {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store.snapshot_schedules.len()
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected <Code>{code}</Code>, got body {}",
        answer.body
    );
}

/// Assert one served body carries exactly these element texts.
fn assert_text(case: &str, answer: &Answer, tag: &str, expected: Option<&str>) {
    assert_eq!(
        answer.text(tag).as_deref(),
        expected,
        "case {case}: expected <{tag}> {expected:?}, got {:?} with body {}",
        answer.text(tag),
        answer.body
    );
}

/// A created schedule is reported, and read back, exactly as the request
/// configured it — and its derived members are computed from those definitions
/// and from the clusters the store holds, not stored beside them.
#[tokio::test]
async fn a_created_snapshot_schedule_is_reported_and_stored_as_configured() {
    let emulator = Emulator::new();
    for row in CREATE_CASES {
        let case = row.case;
        let before = now_seconds();
        let created = create_schedule(&emulator, row).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected status 200, got {} with body {}",
            created.status, created.body
        );
        assert_text(case, &created, "ScheduleIdentifier", Some(row.identifier));
        assert_text(case, &created, "ScheduleDescription", row.description);
        assert_eq!(
            listed_definitions(&created.body),
            row.definitions,
            "case {case}: expected the definitions the request carried, got body {}",
            created.body
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed_tags(&created.body),
            expected_tags,
            "case {case}: expected the tags the request carried, got body {}",
            created.body
        );
        // A fresh schedule is associated with no cluster, so the count is zero
        // and the list is empty — both reported, because both are modelled.
        assert_text(case, &created, "AssociatedClusterCount", Some("0"));
        assert!(
            all_text(&created.body, "ClusterIdentifier").is_empty(),
            "case {case}: expected no associated cluster, got body {}",
            created.body
        );

        // One upcoming invocation by default, and none at all for a schedule
        // whose definitions name no instant to compute.
        let invocations = listed_invocations(&created.body);
        let expected_invocations = usize::from(!row.definitions.is_empty());
        assert_eq!(
            invocations.len(),
            expected_invocations,
            "case {case}: expected {expected_invocations} invocation(s), got {invocations:?}"
        );
        for reported in &invocations {
            let at = invocation_seconds(case, reported);
            assert!(
                at >= before,
                "case {case}: expected an invocation at or after {before}, got {at} ({reported})"
            );
            if let Some(period) = row.period_seconds {
                assert!(
                    at <= now_seconds() + period,
                    "case {case}: expected the next invocation within {period}s, got {at} ({reported})"
                );
            }
            if let Some(suffix) = row.invocation_suffix {
                assert!(
                    reported.ends_with(suffix),
                    "case {case}: expected an invocation ending {suffix}, got {reported}"
                );
            }
        }

        // The ROUND TRIP: the same schedule read back through the reader, and
        // the definitions the store itself holds.
        let read = describe_one(&emulator, row.identifier).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the read served with 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            listed_schedules(&read.body),
            vec![row.identifier.to_string()],
            "case {case}: expected the read to report the schedule, got body {}",
            read.body
        );
        assert_text(case, &read, "ScheduleDescription", row.description);
        assert_eq!(
            listed_definitions(&read.body),
            row.definitions,
            "case {case}: expected the stored definitions read back, got body {}",
            read.body
        );
        assert_eq!(
            listed_tags(&read.body),
            expected_tags,
            "case {case}: expected the stored tags read back, got body {}",
            read.body
        );
        assert_eq!(
            stored_definitions(&emulator, row.identifier).as_deref(),
            Some(
                row.definitions
                    .iter()
                    .map(|definition| (*definition).to_string())
                    .collect::<Vec<String>>()
                    .as_slice()
            ),
            "case {case}: expected the store to hold the definitions the request carried"
        );
    }
    assert_eq!(
        stored_count(&emulator),
        CREATE_CASES.len(),
        "every row minted a schedule of its own"
    );
}

/// The invocations a schedule reports are the instants its definitions name: as
/// many as the request asked for, in chronological order, and spaced by the
/// period the definition states.
#[tokio::test]
async fn the_reported_invocations_are_the_instants_the_definitions_name() {
    let emulator = Emulator::new();
    for row in CREATE_CASES {
        let Some(period) = row.period_seconds else {
            continue;
        };
        let case = row.case;
        let asked = 3;
        let params = create_params(
            Some(row.identifier),
            row.definitions,
            row.description,
            row.tags,
            Some(asked),
            // A dry run, so one emulator serves every row without the
            // identifiers colliding: what is asserted here is the computation,
            // which a dry run does exactly as a create does.
            Some(true),
        );
        let answer = call_with(&emulator, "CreateSnapshotSchedule", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected status 200, got {} with body {}",
            answer.status, answer.body
        );
        let invocations = listed_invocations(&answer.body);
        assert_eq!(
            invocations.len(),
            usize::try_from(asked).expect("the asked-for count is positive"),
            "case {case}: expected {asked} invocations, got {invocations:?}"
        );
        let instants: Vec<i64> = invocations
            .iter()
            .map(|reported| invocation_seconds(case, reported))
            .collect();
        for pair in instants.windows(2) {
            assert_eq!(
                pair[1] - pair[0],
                period,
                "case {case}: expected invocations {period}s apart, got {instants:?}"
            );
        }
    }
    assert_eq!(
        stored_count(&emulator),
        0,
        "a dry run stores nothing, whatever it reports"
    );
}

/// A request asking for a number of invocations the service reports is answered
/// with exactly that many, and one asking for a number outside the window —
/// negative, or past the most one response reports — is REFUSED rather than
/// served a different number than it asked for.
#[tokio::test]
async fn the_number_of_reported_invocations_is_the_number_asked_for() {
    let emulator = Emulator::new();
    let bound = i32::try_from(state::MAX_REPORTED_NEXT_INVOCATIONS)
        .expect("the reported-invocation bound is within an i32");
    for (case, asked, expected) in [
        ("none asked for", Some(0), Some(0usize)),
        ("one asked for", Some(1), Some(1)),
        ("ten asked for", Some(10), Some(10)),
        (
            "the bound itself",
            Some(bound),
            Some(state::MAX_REPORTED_NEXT_INVOCATIONS),
        ),
        (
            "no count at all",
            None,
            Some(state::DEFAULT_REPORTED_NEXT_INVOCATIONS),
        ),
        ("a negative count", Some(-1), None),
        ("one past the bound", Some(bound + 1), None),
        ("every invocation there could be", Some(i32::MAX), None),
    ] {
        let params = create_params(
            Some("schedule-invocation-count"),
            &["rate(1 hour)"],
            None,
            &[],
            asked,
            Some(true),
        );
        let answer = call_with(&emulator, "CreateSnapshotSchedule", &params).await;
        match expected {
            Some(expected) => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected status 200, got {} with body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    listed_invocations(&answer.body).len(),
                    expected,
                    "case {case}: expected {expected} invocation(s), got body {}",
                    answer.body
                );
            }
            None => assert_refused(case, &answer, 400, "ValidationException"),
        }
    }
}

/// A definition the service reads no schedule from is refused with the fault the
/// model declares for it, and nothing is stored: an expression TYPE the service
/// does not offer is `ScheduleDefinitionTypeUnsupported`, and an offered type
/// that states no schedule is `InvalidSchedule`.
#[tokio::test]
async fn a_definition_no_schedule_follows_from_is_refused_with_nothing_stored() {
    let emulator = Emulator::new();
    for (case, definitions, code) in [
        // The expression types this service reads a schedule from are `rate` and
        // `cron`; `at(...)` is the one-time type a SCHEDULED ACTION is written
        // in, and a bare expression names no type at all.
        (
            "a one-time at expression",
            vec!["at(2026-03-04T17:27:00)"],
            "ScheduleDefinitionTypeUnsupported",
        ),
        (
            "an expression type that does not exist",
            vec!["every(12 hours)"],
            "ScheduleDefinitionTypeUnsupported",
        ),
        (
            "an expression with no type",
            vec!["12 hours"],
            "ScheduleDefinitionTypeUnsupported",
        ),
        (
            "an unclosed expression",
            vec!["rate(12 hours"],
            "ScheduleDefinitionTypeUnsupported",
        ),
        ("a rate of zero", vec!["rate(0 hours)"], "InvalidSchedule"),
        (
            "a rate in a unit that is not one",
            vec!["rate(12 fortnights)"],
            "InvalidSchedule",
        ),
        ("a rate with no unit", vec!["rate(12)"], "InvalidSchedule"),
        (
            "a signed rate value",
            vec!["rate(+12 hours)"],
            "InvalidSchedule",
        ),
        (
            "a negative rate value",
            vec!["rate(-12 hours)"],
            "InvalidSchedule",
        ),
        (
            "a rate whose value is not a number",
            vec!["rate(twelve hours)"],
            "InvalidSchedule",
        ),
        (
            "a cron expression of too few fields",
            vec!["cron(30 12)"],
            "InvalidSchedule",
        ),
        (
            "a cron expression of too many fields",
            vec!["cron(0 0 12 * * ? *)"],
            "InvalidSchedule",
        ),
        (
            "a cron field outside its range",
            vec!["cron(99 12 * * ? *)"],
            "InvalidSchedule",
        ),
        (
            "one good definition and one bad one",
            vec!["rate(12 hours)", "rate(12 fortnights)"],
            "InvalidSchedule",
        ),
    ] {
        let params = create_params(
            Some("schedule-refused"),
            &definitions,
            None,
            &[],
            None,
            None,
        );
        let answer = call_with(&emulator, "CreateSnapshotSchedule", &params).await;
        assert_refused(case, &answer, 400, code);
        assert_eq!(
            stored_count(&emulator),
            0,
            "case {case}: a refused create stores nothing"
        );
    }

    // The bound on how many definitions one schedule carries: the bound itself is
    // served, one past it is refused, and nothing is stored either way.
    for (case, count, accepted) in [
        ("the definition bound itself", state::MAX_SCHEDULE_DEFINITIONS, true),
        (
            "one definition too many",
            state::MAX_SCHEDULE_DEFINITIONS + 1,
            false,
        ),
    ] {
        // Each definition states its own interval, so the row is a schedule of
        // `count` distinct definitions rather than one repeated.
        let definitions: Vec<String> = (1..=count).map(|hour| format!("rate({hour} hours)")).collect();
        let borrowed: Vec<&str> = definitions.iter().map(String::as_str).collect();
        let params = create_params(
            Some("schedule-definition-bound"),
            &borrowed,
            None,
            &[],
            None,
            None,
        );
        let answer = call_with(&emulator, "CreateSnapshotSchedule", &params).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected {count} definitions served with 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_definitions(&answer.body).len(),
                count,
                "case {case}: expected every definition reported, got body {}",
                answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidSchedule");
        }
    }
}

/// The documented identifier syntax: the alphabet and the bounds a
/// `ScheduleIdentifier` has to satisfy — "Only alphanumeric characters are
/// allowed for the identifier", with the hyphen Amazon Redshift's own schedule
/// identifiers carry — and a create that performs the write needing one at all.
#[tokio::test]
async fn a_schedule_identifier_follows_the_documented_syntax() {
    let emulator = Emulator::new();
    let longest: String = std::iter::once('s')
        .chain(std::iter::repeat_n(
            'x',
            state::SNAPSHOT_SCHEDULE_IDENTIFIER_MAX_LEN - 1,
        ))
        .collect();
    let too_long: String = std::iter::once('s')
        .chain(std::iter::repeat_n(
            'x',
            state::SNAPSHOT_SCHEDULE_IDENTIFIER_MAX_LEN,
        ))
        .collect();
    for (case, identifier, accepted) in [
        ("alphanumeric", "schedule1", true),
        ("mixed case", "ScheduleOne", true),
        ("hyphenated", "my-snapshot-schedule", true),
        ("one character", "s", true),
        ("a leading digit", "2026backups", true),
        ("digits alone", "2026", true),
        ("the longest identifier", longest.as_str(), true),
        ("no identifier at all", "", false),
        ("one character too long", too_long.as_str(), false),
        ("a character outside the alphabet", "schedule_one", false),
        ("a space", "my schedule", false),
        ("a character outside ASCII", "schedulé", false),
    ] {
        let params = create_params(
            Some(identifier),
            &["rate(12 hours)"],
            None,
            &[],
            None,
            None,
        );
        let answer = call_with(&emulator, "CreateSnapshotSchedule", &params).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected {identifier:?} served with 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                stored_definitions(&emulator, identifier).as_deref(),
                Some(["rate(12 hours)".to_string()].as_slice()),
                "case {case}: expected {identifier:?} stored under the identifier as given"
            );
        } else {
            assert_refused(case, &answer, 400, "ValidationException");
            assert!(
                stored_definitions(&emulator, identifier).is_none(),
                "case {case}: a refused create stores nothing"
            );
        }
    }

    // A request that omits the identifier ENTIRELY is refused for the same
    // reason as one that sends it empty: a schedule that is stored is stored
    // under an identifier.
    let answer = call_with(
        &emulator,
        "CreateSnapshotSchedule",
        &create_params(None, &["rate(12 hours)"], None, &[], None, None),
    )
    .await;
    assert_refused("no identifier member", &answer, 400, "ValidationException");
}

/// The IDENTIFIER is what an account's schedules are unique on: a second create
/// under one identifier is refused and overwrites nothing, while a create under
/// another identifier mints a schedule of its own.
#[tokio::test]
async fn the_identifier_is_what_snapshot_schedules_are_unique_on() {
    let emulator = Emulator::new();
    for row in &CREATE_CASES[..2] {
        let created = create_schedule(&emulator, row).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected status 200, got {} with body {}",
            row.case, created.status, created.body
        );
    }
    for (case, identifier, definitions) in [
        (
            "the same definitions again",
            "schedule-rate-hours",
            vec!["rate(12 hours)"],
        ),
        (
            "different definitions under the same identifier",
            "schedule-rate-hours",
            vec!["cron(0 3 * * ? *)"],
        ),
    ] {
        let answer = call_with(
            &emulator,
            "CreateSnapshotSchedule",
            &create_params(Some(identifier), &definitions, None, &[], None, None),
        )
        .await;
        assert_refused(case, &answer, 400, "SnapshotScheduleAlreadyExists");
        assert_eq!(
            stored_definitions(&emulator, identifier).as_deref(),
            Some(["rate(12 hours)".to_string()].as_slice()),
            "case {case}: the refused create overwrote the schedule that was there"
        );
    }
    assert_eq!(
        stored_count(&emulator),
        2,
        "the refused creates added no schedule"
    );
}

/// A dry run reports the schedule the request describes — definitions,
/// description, tags and the invocations computed from it — and stores nothing,
/// so no later read sees it and the identifier stays free. It answers the
/// create's own refusals all the same: a dry run of a create that could not
/// succeed is refused exactly as that create would be.
#[tokio::test]
async fn a_dry_run_reports_the_schedule_it_describes_and_stores_nothing() {
    let emulator = Emulator::new();
    for (case, identifier) in [
        ("a dry run naming an identifier", Some("schedule-dry-run")),
        ("a dry run naming another", Some("ScheduleDryRun2")),
    ] {
        let answer = call_with(
            &emulator,
            "CreateSnapshotSchedule",
            &create_params(
                identifier,
                &["rate(6 hours)"],
                Some("checked, not created"),
                &[("owner", "admin")],
                Some(2),
                Some(true),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected status 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_text(case, &answer, "ScheduleDescription", Some("checked, not created"));
        assert_eq!(
            listed_definitions(&answer.body),
            vec!["rate(6 hours)".to_string()],
            "case {case}: expected the definitions the request carried, got body {}",
            answer.body
        );
        assert_eq!(
            listed_invocations(&answer.body).len(),
            2,
            "case {case}: expected the two invocations asked for, got body {}",
            answer.body
        );
        assert_eq!(
            stored_count(&emulator),
            0,
            "case {case}: a dry run stores nothing"
        );
    }

    // The identifier a dry run named is still free, so a create takes it.
    let created = call_with(
        &emulator,
        "CreateSnapshotSchedule",
        &create_params(
            Some("schedule-dry-run"),
            &["rate(6 hours)"],
            None,
            &[],
            None,
            Some(false),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case a create after a dry run: expected status 200, got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        stored_count(&emulator),
        1,
        "case a create after a dry run: the create stored the schedule"
    );

    // A dry run is the create's own refusals without the write, so a dry run
    // under an identifier the account already holds is refused exactly as the
    // create would be, and still stores nothing.
    let refused = call_with(
        &emulator,
        "CreateSnapshotSchedule",
        &create_params(
            Some("schedule-dry-run"),
            &["rate(6 hours)"],
            None,
            &[],
            None,
            Some(true),
        ),
    )
    .await;
    assert_refused(
        "a dry run under an identifier already held",
        &refused,
        400,
        "SnapshotScheduleAlreadyExists",
    );
    assert_eq!(
        stored_count(&emulator),
        1,
        "case a dry run under an identifier already held: nothing was stored or removed"
    );

    // A dry run that names NO identifier is refused for the same reason the
    // create is: a schedule is stored under an identifier, and a dry run that
    // reported success for a create that could never succeed would be the
    // opposite of a check.
    let nameless = call_with(
        &emulator,
        "CreateSnapshotSchedule",
        &create_params(None, &["rate(6 hours)"], None, &[], None, Some(true)),
    )
    .await;
    assert_refused(
        "a dry run naming no identifier",
        &nameless,
        400,
        "ValidationException",
    );
}

/// A schedule's `AssociatedClusters` are the clusters that NAME it, with the
/// state each association is in read off the cluster's own status: still being
/// applied while the cluster is not serving, active once it is.
#[tokio::test]
async fn the_associated_clusters_are_the_clusters_that_name_the_schedule() {
    let emulator = Emulator::new();
    for identifier in ["schedule-associated", "schedule-unassociated"] {
        let created = call_with(
            &emulator,
            "CreateSnapshotSchedule",
            &create_params(Some(identifier), &["rate(8 hours)"], None, &[], None, None),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {identifier}: expected status 200, got {} with body {}",
            created.status, created.body
        );
    }

    let cluster = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "cluster-on-a-schedule"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("SnapshotScheduleIdentifier", "schedule-associated"),
            ],
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "case a cluster on a schedule: expected status 200, got {} with body {}",
        cluster.status, cluster.body
    );

    // The cluster is still being provisioned, so the association is still being
    // applied: the schedule takes snapshots of a cluster only once it is up.
    let applying = describe_one(&emulator, "schedule-associated").await;
    assert_text(
        "while the cluster is provisioning",
        &applying,
        "AssociatedClusterCount",
        Some("1"),
    );
    assert_eq!(
        all_text(&applying.body, "ClusterIdentifier"),
        vec!["cluster-on-a-schedule".to_string()],
        "case while the cluster is provisioning: expected the cluster associated, got body {}",
        applying.body
    );
    assert_text(
        "while the cluster is provisioning",
        &applying,
        "ScheduleAssociationState",
        Some(state::SCHEDULE_ASSOCIATION_STATE_MODIFYING),
    );

    // The provisioning transition was due this long after the create, and
    // nothing sleeps for it inside the handler.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let active = describe_one(&emulator, "schedule-associated").await;
    assert_text(
        "once the cluster is serving",
        &active,
        "ScheduleAssociationState",
        Some(state::SCHEDULE_ASSOCIATION_STATE_ACTIVE),
    );

    // The other schedule is associated with nothing: an association follows the
    // cluster that names it and no other.
    let unassociated = describe_one(&emulator, "schedule-unassociated").await;
    assert_text(
        "a schedule no cluster names",
        &unassociated,
        "AssociatedClusterCount",
        Some("0"),
    );
    assert!(
        all_text(&unassociated.body, "ClusterIdentifier").is_empty(),
        "case a schedule no cluster names: expected no association, got body {}",
        unassociated.body
    );
}

/// `DescribeSnapshotSchedules` selects by the members the model gives it: the
/// schedule's own identifier, the cluster it is associated with, and the tags it
/// carries — and reports an EMPTY page for an identifier that names no schedule,
/// because the operation models no not-found fault.
#[tokio::test]
async fn snapshot_schedules_are_selected_by_the_documented_filters() {
    let emulator = Emulator::new();
    for (identifier, tags) in [
        ("schedule-owner-admin", &[("owner", "admin")][..]),
        ("schedule-owner-team", &[("owner", "team")][..]),
        ("schedule-untagged", &[][..]),
    ] {
        let created = call_with(
            &emulator,
            "CreateSnapshotSchedule",
            &create_params(
                Some(identifier),
                &["rate(12 hours)"],
                None,
                tags,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {identifier}: expected status 200, got {} with body {}",
            created.status, created.body
        );
    }
    let cluster = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "cluster-with-a-schedule"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("SnapshotScheduleIdentifier", "schedule-owner-team"),
            ],
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "case the cluster a filter selects by: expected status 200, got {} with body {}",
        cluster.status, cluster.body
    );

    for (case, filter, expected) in [
        (
            "no filter at all",
            vec![],
            vec![
                "schedule-owner-admin",
                "schedule-owner-team",
                "schedule-untagged",
            ],
        ),
        (
            "one schedule identifier",
            vec![("ScheduleIdentifier", "schedule-untagged")],
            vec!["schedule-untagged"],
        ),
        (
            "an identifier no schedule carries",
            vec![("ScheduleIdentifier", "schedule-absent")],
            vec![],
        ),
        (
            "the cluster a schedule is associated with",
            vec![("ClusterIdentifier", "cluster-with-a-schedule")],
            vec!["schedule-owner-team"],
        ),
        (
            "a cluster no schedule is associated with",
            vec![("ClusterIdentifier", "cluster-absent")],
            vec![],
        ),
        (
            "one tag key",
            vec![("TagKeys.TagKey.1", "owner")],
            vec!["schedule-owner-admin", "schedule-owner-team"],
        ),
        (
            "one tag value",
            vec![("TagValues.TagValue.1", "team")],
            vec!["schedule-owner-team"],
        ),
        (
            "a tag key no schedule carries",
            vec![("TagKeys.TagKey.1", "absent")],
            vec![],
        ),
        (
            "an identifier and the cluster it is associated with",
            vec![
                ("ScheduleIdentifier", "schedule-owner-team"),
                ("ClusterIdentifier", "cluster-with-a-schedule"),
            ],
            vec!["schedule-owner-team"],
        ),
        (
            "an identifier and a cluster it is not associated with",
            vec![
                ("ScheduleIdentifier", "schedule-owner-admin"),
                ("ClusterIdentifier", "cluster-with-a-schedule"),
            ],
            vec![],
        ),
    ] {
        let answer = emulator.call("DescribeSnapshotSchedules", &filter).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected status 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_schedules(&answer.body),
            expected,
            "case {case}: expected {expected:?}, got body {}",
            answer.body
        );
    }
}

/// `DescribeSnapshotSchedules` hands out one page of the service's documented
/// `MaxRecords` window at a time, with a `Marker` cursor a later call resumes the
/// same sequence from.
#[tokio::test]
async fn snapshot_schedules_are_paginated_over_the_documented_window() {
    let emulator = Emulator::new();
    let minimum = state::MAX_RECORDS_WINDOW.min;
    let total = usize::try_from(minimum).expect("the window's minimum is positive") + 1;
    let mut created_order: Vec<String> = Vec::new();
    for index in 0..total {
        let identifier = format!("schedule-page-{index:02}");
        let answer = call_with(
            &emulator,
            "CreateSnapshotSchedule",
            &create_params(
                Some(&identifier),
                &["rate(12 hours)"],
                None,
                &[],
                None,
                None,
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case schedule {index}: expected status 200, got {} with body {}",
            answer.status, answer.body
        );
        created_order.push(identifier);
    }

    // The window's bounds: below the minimum is refused, every value from the
    // minimum up is served.
    for (case, max_records, accepted) in [
        ("one below the minimum", minimum - 1, false),
        ("the minimum itself", minimum, true),
        ("the maximum itself", state::MAX_RECORDS_WINDOW.max, true),
        ("past the maximum", state::MAX_RECORDS_WINDOW.max + 1, true),
    ] {
        let answer = call_with(
            &emulator,
            "DescribeSnapshotSchedules",
            &[("MaxRecords".to_string(), max_records.to_string())],
        )
        .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} served with 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    let first_page = call_with(
        &emulator,
        "DescribeSnapshotSchedules",
        &[("MaxRecords".to_string(), minimum.to_string())],
    )
    .await;
    let first_names = listed_schedules(&first_page.body);
    assert_eq!(
        first_names.len(),
        usize::try_from(minimum).expect("the window's minimum is positive"),
        "case the first page: expected {minimum} schedules, got {first_names:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case the first page: records remain, so the page carries a cursor");
    let second_page = call_with(
        &emulator,
        "DescribeSnapshotSchedules",
        &[
            ("MaxRecords".to_string(), minimum.to_string()),
            ("Marker".to_string(), marker),
        ],
    )
    .await;
    let second_names = listed_schedules(&second_page.body);
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case the second page: nothing remains, so expected no cursor, got body {}",
        second_page.body
    );
    let walked: Vec<String> = first_names.into_iter().chain(second_names).collect();
    assert_eq!(
        walked, created_order,
        "case the walked pages: expected the creation order, got {walked:?}"
    );

    // A cursor no page handed out is refused rather than served as some other
    // offset.
    for (case, cursor) in [
        ("a cursor of the wrong shape", "not-a-cursor"),
        ("an offset past the collection", "99.deadbeef"),
    ] {
        let answer = emulator
            .call("DescribeSnapshotSchedules", &[("Marker", cursor)])
            .await;
        assert_refused(case, &answer, 400, "InvalidParameterValue");
    }
}
