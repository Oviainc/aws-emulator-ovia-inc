// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-timestreamquery-timestream-20181101 service.
//!
//! Timestream Query's only durable resource is the **scheduled query**, so that is what
//! this store holds: one [`ScheduledQueryRecord`] per live query, keyed by its ARN, plus
//! the two indexes the API's uniqueness rules need (live name -> ARN, and client token ->
//! ARN for `CreateScheduledQuery`'s `@idempotencyToken` replay).
//!
//! Everything a response reports about a scheduled query is built from the record here —
//! `DescribeScheduledQuery` and `ListScheduledQueries` render it through
//! [`ScheduledQueryRecord::description`] / [`ScheduledQueryRecord::summary`] so the two
//! views cannot drift.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ErrorReportConfiguration, NotificationConfiguration, ScheduleConfiguration, ScheduledQuery,
    ScheduledQueryDescription, ScheduledQueryRunSummary, Tag, TargetConfiguration,
    TargetDestination, TimestreamDestination,
};

/// The region an ARN carries when the request is not signed, so nothing in the store
/// depends on a client having sent a SigV4 scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` this service's model declares — the service segment of every ARN
/// it mints (see `OPERATIONS.md`, "ARN namespace").
pub const ARN_NAMESPACE: &str = "timestream";

/// The resource segment a scheduled query's ARN carries, before its name.
pub const SCHEDULED_QUERY_RESOURCE_PREFIX: &str = "scheduled-query/";

/// Whether `candidate` is shaped like an ARN this service mints for a scheduled query:
/// `arn:aws:timestream:<region>:<account>:scheduled-query/<name>`.
///
/// Used to refuse a pagination cursor this service cannot have issued; resolving the ARN
/// to a record is the store's job, not this predicate's.
pub fn is_scheduled_query_arn(candidate: &str) -> bool {
    candidate
        .strip_prefix("arn:aws:")
        .and_then(|rest| rest.strip_prefix(ARN_NAMESPACE))
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| rest.contains(&format!(":{SCHEDULED_QUERY_RESOURCE_PREFIX}")))
}

/// The `ScheduledQueryState` a newly created scheduled query is in. The enum models only
/// `ENABLED` and `DISABLED` (see `SCHEDULED_QUERY_STATE_VALUES`), so a created query is
/// live at once — there is no transient state for the clock to move it out of.
pub const SCHEDULED_QUERY_STATE_ENABLED: &str = "ENABLED";

/// The most scheduled queries this store holds at once. A create past it is refused with
/// the modelled `ServiceQuotaExceededException` (HTTP 402, "You have exceeded the service
/// quota.") — the same shape the real service refuses an over-quota create with.
///
/// CHOSEN, not documented: Timestream caps scheduled queries per account, but the model
/// states no number, so this one is the emulator's own. It is what bounds this store's
/// growth: every record is itself bounded by the modelled `QueryString` maximum
/// (262144 bytes) and the rest of a create's modelled lengths, and a record is released by
/// `DeleteScheduledQuery`, so the store cannot grow past this many records however many
/// creates arrive. `names` and `create_tokens` only ever hold entries pointing at a live
/// record, so they are bounded by the same cap.
pub const MAX_SCHEDULED_QUERIES: usize = 1_000;

/// Per-service state for com-amazonaws-timestreamquery-timestream-20181101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTimestreamqueryTimestream20181101State {
    /// Every live scheduled query, keyed by ARN. A `BTreeMap` so `ListScheduledQueries`
    /// pages in a stable, total order and an opaque page token resumes exactly.
    pub scheduled_queries: BTreeMap<String, ScheduledQueryRecord>,
    /// Live `Name` -> ARN. A name may name at most one live scheduled query, which is
    /// what makes a second create under a live name a `ConflictException`.
    pub names: BTreeMap<String, String>,
    /// `ClientToken` -> ARN, so `CreateScheduledQuery`'s `@idempotencyToken` replay
    /// answers with the ARN the first call minted instead of minting a second record.
    pub create_tokens: BTreeMap<String, String>,
    /// Counter behind the unique suffix of a minted scheduled-query ARN.
    pub arn_sequence: u64,
}

impl ComAmazonawsTimestreamqueryTimestream20181101State {
    /// Mint the ARN for a new scheduled query:
    /// `arn:aws:timestream:<region>:<account>:scheduled-query/<name>-<unique suffix>`.
    ///
    /// The suffix is derived from a per-store counter through a bijective 64-bit mix, so
    /// two queries created under the same name never collide while the value still looks
    /// like the opaque id the real service appends.
    pub fn mint_scheduled_query_arn(&mut self, region: &str, name: &str) -> String {
        self.arn_sequence = self.arn_sequence.wrapping_add(1);
        let suffix = mix64(self.arn_sequence);
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("{SCHEDULED_QUERY_RESOURCE_PREFIX}{name}-{suffix:016x}"),
        )
    }

    /// Insert a minted record and index it by name (and by client token, when the
    /// request carried one).
    pub fn insert_scheduled_query(
        &mut self,
        record: ScheduledQueryRecord,
        client_token: Option<&str>,
    ) {
        self.names.insert(record.name.clone(), record.arn.clone());
        if let Some(token) = client_token {
            self.create_tokens
                .insert(token.to_string(), record.arn.clone());
        }
        self.scheduled_queries.insert(record.arn.clone(), record);
    }

    /// Remove a scheduled query and every index entry that pointed at it, so a later
    /// `DescribeScheduledQuery` answers `ResourceNotFoundException` and the name is free
    /// again. Returns the removed record, or `None` when the ARN named no live query.
    pub fn remove_scheduled_query(&mut self, arn: &str) -> Option<ScheduledQueryRecord> {
        let record = self.scheduled_queries.remove(arn)?;
        if self.names.get(&record.name).is_some_and(|a| a == arn) {
            self.names.remove(&record.name);
        }
        self.create_tokens.retain(|_, bound| bound != arn);
        Some(record)
    }
}

/// The splitmix64 finalizer: a bijection on `u64`, so distinct counter values give
/// distinct ARN suffixes (a hash of the counter mixed with the clock would not).
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One scheduled query, as created and then reported by Describe/List.
#[derive(Debug, Clone)]
pub struct ScheduledQueryRecord {
    pub arn: String,
    pub name: String,
    pub query_string: String,
    /// The `ScheduleConfiguration.ScheduleExpression` exactly as submitted, for reporting.
    pub schedule_expression: String,
    /// The same expression, parsed ONCE when the query was created. The next invocation
    /// time is computed from it against the current clock on every read, so a read still
    /// writes nothing — it just does not re-parse per record per page.
    pub schedule: ScheduleSpec,
    pub notification_configuration: NotificationConfiguration,
    pub target_configuration: Option<TargetConfiguration>,
    pub error_report_configuration: ErrorReportConfiguration,
    pub execution_role_arn: String,
    pub kms_key_id: Option<String>,
    /// `ENABLED` or `DISABLED` (`ScheduledQueryState`).
    pub state: String,
    pub creation_time: SystemTime,
    /// When a run of this query last started. No run is emulated yet (`Query` and
    /// `ExecuteScheduledQuery` are still scaffolded), so this stays `None` rather than
    /// reporting a run that never happened.
    pub previous_invocation_time: Option<SystemTime>,
    /// `ScheduledQueryRunStatus` of the last run, for the same reason `None` until a run
    /// is emulated.
    pub last_run_status: Option<String>,
    pub last_run_summary: Option<ScheduledQueryRunSummary>,
    pub recently_failed_runs: Vec<ScheduledQueryRunSummary>,
    pub tags: BTreeMap<String, String>,
}

impl ScheduledQueryRecord {
    /// The `TargetDestination` summary View that `ListScheduledQueries` reports, derived
    /// from the stored `TargetConfiguration` so the two cannot disagree.
    pub fn target_destination(&self) -> Option<TargetDestination> {
        let timestream = &self.target_configuration.as_ref()?.timestream_configuration;
        Some(TargetDestination {
            timestream_destination: Some(TimestreamDestination {
                database_name: Some(timestream.database_name.clone()),
                table_name: Some(timestream.table_name.clone()),
            }),
        })
    }

    /// The next time this query is due to run, as of `now`: `None` when the stored
    /// expression has no computable next occurrence.
    pub fn next_invocation_time(&self, now: SystemTime) -> Option<SystemTime> {
        self.schedule.next_after(self.creation_time, now)
    }

    /// `ScheduledQueryDescription`, the full view `DescribeScheduledQuery` returns.
    pub fn description(&self, now: SystemTime) -> ScheduledQueryDescription {
        ScheduledQueryDescription {
            arn: self.arn.clone(),
            name: self.name.clone(),
            query_string: self.query_string.clone(),
            creation_time: Some(to_date_time(self.creation_time)),
            state: self.state.clone(),
            previous_invocation_time: self.previous_invocation_time.map(to_date_time),
            next_invocation_time: self.next_invocation_time(now).map(to_date_time),
            schedule_configuration: ScheduleConfiguration {
                schedule_expression: self.schedule_expression.clone(),
            },
            notification_configuration: self.notification_configuration.clone(),
            target_configuration: self.target_configuration.clone(),
            scheduled_query_execution_role_arn: Some(self.execution_role_arn.clone()),
            kms_key_id: self.kms_key_id.clone(),
            error_report_configuration: Some(self.error_report_configuration.clone()),
            last_run_summary: self.last_run_summary.clone(),
            recently_failed_runs: (!self.recently_failed_runs.is_empty())
                .then(|| self.recently_failed_runs.clone()),
        }
    }

    /// `ScheduledQuery`, the summary view `ListScheduledQueries` returns.
    pub fn summary(&self, now: SystemTime) -> ScheduledQuery {
        ScheduledQuery {
            arn: self.arn.clone(),
            name: self.name.clone(),
            creation_time: Some(to_date_time(self.creation_time)),
            state: self.state.clone(),
            previous_invocation_time: self.previous_invocation_time.map(to_date_time),
            next_invocation_time: self.next_invocation_time(now).map(to_date_time),
            error_report_configuration: Some(self.error_report_configuration.clone()),
            target_destination: self.target_destination(),
            last_run_status: self.last_run_status.clone(),
        }
    }
}

/// A modelled `TagList` as the map the store keeps. A repeated key keeps the last value,
/// the way a map-valued tag set behaves.
pub fn tag_map(tags: Option<&Vec<Tag>>) -> BTreeMap<String, String> {
    tags.into_iter()
        .flatten()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect()
}

/// A `SystemTime` as the modelled `timestamp` the wire carries.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The region the request was signed for, which an ARN this request mints carries.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The two forms AWS documents for a scheduled query's `ScheduleExpression`.
#[derive(Debug, Clone)]
pub enum ScheduleSpec {
    /// `rate(<value> <unit>)`: a fixed interval, counted from the creation time. Its next
    /// occurrence is documented arithmetic, so it is computed.
    Rate(Duration),
    /// `cron(<minute> <hour> <day-of-month> <month> <day-of-week> <year>)`.
    ///
    /// Stored and reported verbatim, with NO next occurrence: evaluating a cron
    /// expression faithfully takes a cron engine, and a hand-written subset would answer
    /// every expression it does not cover wrongly while reporting success. So
    /// `NextInvocationTime` — an optional member — is left absent for a cron schedule
    /// rather than fabricated.
    Cron,
}

impl ScheduleSpec {
    /// The first invocation strictly after `after`, for a query created at `created`, or
    /// `None` when this emulator cannot compute one.
    pub fn next_after(&self, created: SystemTime, after: SystemTime) -> Option<SystemTime> {
        match self {
            // A rate expression fires at `created + n * interval`; the next one due is the
            // first of those strictly after `after`.
            Self::Rate(interval) => {
                let step = interval.as_secs().max(1);
                let elapsed = after.duration_since(created).unwrap_or_default().as_secs();
                let periods = elapsed.checked_div(step)?.checked_add(1)?;
                created.checked_add(Duration::from_secs(step.checked_mul(periods)?))
            }
            Self::Cron => None,
        }
    }
}

/// The units a `rate(...)` expression accepts, and the seconds each one is worth.
///
/// Source: the `ScheduleExpression` forms AWS documents for a scheduled query — a rate
/// expression's unit is minute(s), hour(s) or day(s). Both the singular and the plural
/// spelling are accepted for every value, because refusing a spelling the real service
/// accepts would reject a valid create.
const RATE_UNITS: &[(&str, u64)] = &[
    ("minute", 60),
    ("minutes", 60),
    ("hour", 3_600),
    ("hours", 3_600),
    ("day", 86_400),
    ("days", 86_400),
];

/// A cron expression's six fields, in the order AWS documents them.
const CRON_FIELD_NAMES: [&str; 6] = [
    "minute",
    "hour",
    "day-of-month",
    "month",
    "day-of-week",
    "year",
];

/// Which of those fields accept `?` ("no specific value"): AWS allows it only in the
/// day-of-month / day-of-week pair, where one field stands down for the other.
const CRON_FIELD_ACCEPTS_NO_SPECIFIC_VALUE: [bool; 6] = [false, false, true, false, true, false];

/// Parse a `ScheduleExpression`, or say why it is not one.
///
/// The `Err` is the `ValidationException` message the caller needs to fix the request:
/// it names the member and the form expected.
pub fn parse_schedule_expression(expression: &str) -> Result<ScheduleSpec, String> {
    let expression = expression.trim();
    if let Some(body) = bracketed(expression, "rate") {
        return parse_rate(body);
    }
    if let Some(body) = bracketed(expression, "cron") {
        return parse_cron(body);
    }
    Err(format!(
        "ScheduleConfiguration.ScheduleExpression must be a rate expression \
         (rate(<value> <unit>)) or a cron expression (cron(<minute> <hour> <day-of-month> \
         <month> <day-of-week> <year>)), got {expression:?}"
    ))
}

/// The body of `<keyword>( .. )`, or `None` when `expression` is not that call.
fn bracketed<'a>(expression: &'a str, keyword: &str) -> Option<&'a str> {
    expression
        .strip_prefix(keyword)?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn parse_rate(body: &str) -> Result<ScheduleSpec, String> {
    let fields: Vec<&str> = body.split_whitespace().collect();
    let [value, unit] = fields.as_slice() else {
        return Err(format!(
            "ScheduleConfiguration.ScheduleExpression rate expression takes a value and a \
             unit, as rate(<value> <unit>), got {} field(s)",
            fields.len()
        ));
    };
    let value: u64 = value.parse().map_err(|_| {
        format!(
            "ScheduleConfiguration.ScheduleExpression rate value must be a positive \
             integer, got {value:?}"
        )
    })?;
    if value == 0 {
        return Err(
            "ScheduleConfiguration.ScheduleExpression rate value must be greater than 0"
                .to_string(),
        );
    }
    let lowered = unit.to_ascii_lowercase();
    let seconds = RATE_UNITS
        .iter()
        .find(|(name, _)| *name == lowered)
        .map(|(_, seconds)| *seconds)
        .ok_or_else(|| {
            format!(
                "ScheduleConfiguration.ScheduleExpression rate unit must be one of \
                 minute(s), hour(s) or day(s), got {unit:?}"
            )
        })?;
    let total = value.checked_mul(seconds).ok_or_else(|| {
        format!("ScheduleConfiguration.ScheduleExpression rate interval is too large: {value} {unit}")
    })?;
    Ok(ScheduleSpec::Rate(Duration::from_secs(total)))
}

fn parse_cron(body: &str) -> Result<ScheduleSpec, String> {
    let fields: Vec<&str> = body.split_whitespace().collect();
    if fields.len() != 6 {
        return Err(format!(
            "ScheduleConfiguration.ScheduleExpression cron expression takes 6 fields \
             (<minute> <hour> <day-of-month> <month> <day-of-week> <year>), got {}",
            fields.len()
        ));
    }
    // `?` ("no specific value") stands one of the day fields down for the other, so AWS
    // accepts it only in that pair. Everything the field grammar says beyond this is left
    // to the service: this emulator does not evaluate cron expressions (see
    // [`ScheduleSpec::Cron`]), so it does not claim to judge a field's contents either.
    for (index, field) in fields.iter().enumerate() {
        if field.contains('?') && !CRON_FIELD_ACCEPTS_NO_SPECIFIC_VALUE[index] {
            return Err(format!(
                "ScheduleConfiguration.ScheduleExpression cron {} field accepts no '?': \
                 it is allowed only in the day-of-month and day-of-week fields, got {field:?}",
                CRON_FIELD_NAMES[index]
            ));
        }
    }
    Ok(ScheduleSpec::Cron)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTimestreamqueryTimestream20181101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTimestreamqueryTimestream20181101State>)
}
