// SPDX-License-Identifier: CC0-1.0

//! The creation frequency a schedule must run at to archive the snapshots it
//! creates.
//!
//! `CreateRule`'s documentation is a two-bullet note, and this is its SECOND
//! bullet: "If you need to specify an ArchiveRule for the schedule, then you
//! must specify a creation frequency of at least 28 days"
//! (`smithy_query(query="doc", shape="CreateRule")`). The first bullet — which
//! of the two forms a creation rule states — is `super::create_rule_violation`'s;
//! this module judges the MAGNITUDE of the frequency whichever form stated it,
//! for a schedule that carries an `ArchiveRule` and so falls under the sentence.
//!
//! A creation rule states its frequency in one of two forms, and the frequency
//! is read from the form the rule used:
//!
//! * the interval form, an `Interval` in the `IntervalUnit` it names — and
//!   `IntervalUnitValues` offers `HOURS` alone
//!   (`smithy_query(query="shape", shape="IntervalUnitValues")`), so an interval
//!   form states that many hours;
//! * the cron form, a `CronExpression` whose "schedule interval must be between
//!   1 hour and 1 year" (`smithy_query(query="shape", shape="CreateRule")`,
//!   member `CronExpression`), so the frequency it states is the shortest span
//!   between two of the instants the expression names.
//!
//! An expression this module cannot read a frequency from states none it can
//! judge, so nothing is refused for it: the floor is enforced on what a request
//! decidably asked for.

use super::*;

/// The least creation frequency, in DAYS, a schedule carrying an `ArchiveRule`
/// may run at: `CreateRule` documents "If you need to specify an ArchiveRule for
/// the schedule, then you must specify a creation frequency of at least 28 days"
/// (`smithy_query(query="doc", shape="CreateRule")`).
///
/// Nothing in the model expresses it: the frequency is stated by `Interval` and
/// `IntervalUnit` or by a `CronExpression`, so the floor is a relation between
/// those members and the `ArchiveRule` beside them, and it is enforced here.
pub const ARCHIVE_CREATION_FREQUENCY_MIN_DAYS: i64 = 28;

/// The one unit an interval-form creation rule can state: `IntervalUnitValues`
/// offers `HOURS` alone (`smithy_query(query="shape",
/// shape="IntervalUnitValues")`), and `CreateRule$IntervalUnit` is validated
/// against that enum, so any other spelling is not a unit a request can reach
/// this with.
const INTERVAL_UNIT_HOURS: &str = "HOURS";

/// How many whitespace-separated fields an Amazon Web Services cron expression
/// carries: the minute, hour, day-of-month, month, day-of-week and year.
const CRON_FIELDS: usize = 6;

/// The cron fields a `?` ("no specific value") may stand in, as positions within
/// the six: the day-of-month and the day-of-week.
const NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [2, 4];

/// How many of a cron creation rule's instants are sampled to read the frequency
/// it states. The frequency is the shortest span between two consecutive
/// instants, so the sample has to be long enough to reach the shortest one a
/// calendar expression admits — a monthly expression's February, a yearly one's
/// own period — and it is a fixed bound so that no request can make this walk
/// unbounded.
const SAMPLED_INSTANTS: usize = 64;

/// The instant the sample of a cron expression's instants starts after:
/// 2024-01-01T00:00:00Z in seconds since the Unix epoch, read in UTC as a cron
/// expression is.
///
/// A FIXED instant rather than the time of the request, so one expression states
/// one frequency however often and whenever it is sent, and one the sample
/// reaches a leap February and a common one from, which is what a monthly
/// expression's shortest span is.
const SAMPLE_START_SECONDS: i64 = 1_704_067_200;

/// Why a schedule may not archive the snapshots it creates at the frequency its
/// creation rule states, or `None` when the frequency meets the documented floor,
/// is not one this service can read, or the schedule carries no `ArchiveRule` at
/// all.
///
/// The sentence is about the pairing of the two rules, so it is judged on the
/// SCHEDULE: a schedule carrying no `ArchiveRule` is held to no floor (its
/// creation frequency is judged by `CreateRule$Interval`'s own supported set
/// instead, [`schedule_with_unsupported_create_interval`]), and one carrying it
/// is refused whichever form its creation rule stated the frequency in.
///
/// `label` is how the refusal names the schedule, as every other schedule
/// refusal names it ([`schedule_label`]).
pub fn archive_creation_frequency_violation(
    label: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    schedule.archive_rule.as_ref()?;
    let rule = schedule.create_rule.as_ref()?;
    let floor = ::chrono::TimeDelta::try_days(ARCHIVE_CREATION_FREQUENCY_MIN_DAYS)?;

    // The form the rule stated is the form its frequency is read from. A rule
    // that states neither form, or both, is refused for its form before this
    // ([`create_rule_violation`]), so at most one of the two is read here.
    if let Some(interval) = stated_create_interval(rule) {
        // An interval the member does not support states no frequency this
        // sentence can be asked about: it is refused on its own terms, by the
        // member's documented domain
        // ([`schedule_with_unsupported_create_interval`], which both write paths
        // run beside this), whether or not the schedule archives anything.
        if !SUPPORTED_CREATE_RULE_INTERVALS.contains(&interval) {
            return None;
        }
        let frequency = interval_frequency(interval, rule.interval_unit.as_deref())?;
        return (frequency < floor)
            .then(|| creation_frequency_refused(label, "Interval", &interval.to_string()));
    }
    let expression = rule.cron_expression.as_deref()?;
    let frequency = cron_frequency(expression)?;
    (frequency < floor).then(|| creation_frequency_refused(label, "CronExpression", expression))
}

/// The creation frequency an INTERVAL-form creation rule states — `interval` of
/// `unit` — or `None` when `unit` is not one this service can read a span from.
///
/// [`INTERVAL_UNIT_HOURS`] is the only unit the model offers for the member, so
/// an interval states that many hours.
fn interval_frequency(interval: i32, unit: Option<&str>) -> Option<::chrono::TimeDelta> {
    if unit != Some(INTERVAL_UNIT_HOURS) {
        return None;
    }
    ::chrono::TimeDelta::try_hours(i64::from(interval))
}

/// The creation frequency a CRON-form creation rule states — the shortest span
/// between two of the instants its expression names — or `None` when the
/// expression is not one this service can compute instants from.
fn cron_frequency(expression: &str) -> Option<::chrono::TimeDelta> {
    let schedule = parse_cron_expression(expression)?;
    let after = ::chrono::DateTime::from_timestamp(SAMPLE_START_SECONDS, 0)?;
    let mut shortest: Option<::chrono::TimeDelta> = None;
    let mut previous: Option<::chrono::DateTime<::chrono::Utc>> = None;
    for instant in schedule.after(&after).take(SAMPLED_INSTANTS) {
        if let Some(previous) = previous {
            let span = instant.signed_duration_since(previous);
            shortest = Some(shortest.map_or(span, |held| held.min(span)));
        }
        previous = Some(instant);
    }
    shortest
}

/// A `CronExpression` as the schedule it names, or `None` when it is not an
/// expression this service can compute instants from.
///
/// The expression itself is read by the MAINTAINED cron grammar rather than by a
/// hand-written one; what is written here is only the adapter between the two
/// spellings. The member is written `cron(...)` (its `@pattern` is
/// `^cron\([^\n]{11,100}\)$`) around the six fields of an Amazon Web Services
/// cron expression — minute, hour, day-of-month, month, day-of-week, year. The
/// parser reads the seven-field form that starts with seconds, so a literal `0`
/// seconds field is prepended; a `?` is "no specific value", which that grammar
/// spells `*`, and states nothing outside the two day fields.
fn parse_cron_expression(expression: &str) -> Option<::cron::Schedule> {
    let expression = expression.trim();
    let body = expression
        .strip_prefix("cron(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(expression);
    let fields: Vec<&str> = body.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return None;
    }
    let normalised: Vec<&str> = ::std::iter::once("0")
        .chain(fields.iter().enumerate().map(|(position, field)| {
            if *field == "?" && NO_SPECIFIC_VALUE_FIELDS.contains(&position) {
                "*"
            } else {
                *field
            }
        }))
        .collect();
    normalised.join(" ").parse::<::cron::Schedule>().ok()
}

/// The `InvalidRequestException` for a creation frequency too short to archive
/// the snapshots the schedule creates. The message is built from the request: it
/// names the member the frequency was stated in, the schedule carrying it, the
/// value itself and the floor it falls short of.
fn creation_frequency_refused(
    label: &str,
    member: &str,
    stated: &str,
) -> InvalidRequestException {
    invalid_request(format!(
        "The parameter CreateRule.{member} in schedule {{{label}}} was {stated}. A schedule that \
         specifies an ArchiveRule must specify a creation frequency of at least \
         {ARCHIVE_CREATION_FREQUENCY_MIN_DAYS} days."
    ))
}
