// SPDX-License-Identifier: CC0-1.0

//! The rules a schedule's own definition has to satisfy, and the normalisation
//! EventBridge Scheduler applies to the instants it stores.
//!
//! One concern, shared by the two operations that WRITE a schedule
//! (`CreateSchedule` and `UpdateSchedule`): both carry the same members, so both
//! are held to the same rules here rather than each spelling its own.
//!
//! * `ScheduleExpression` must state one of the three forms the service
//!   documents — "at expression - at(yyyy-mm-ddThh:mm:ss) rate expression -
//!   rate(value unit) cron expression - cron(fields)"
//!   (`CreateSchedule$ScheduleExpression`) — so an expression that states none of
//!   them, or states one of them malformed, is not a schedule at all
//!   ([`schedule_expression_violation`]).
//! * `ScheduleExpressionTimezone` names "the timezone in which the scheduling
//!   expression is evaluated" (`CreateSchedule$ScheduleExpressionTimezone`),
//!   which is a time zone of the IANA database: a value no zone answers to leaves
//!   the firing instants undefined, so it is not accepted
//!   ([`schedule_expression_timezone_violation`]).
//! * `StartDate` is "the date, in UTC, after which the schedule can begin
//!   invoking its target" and `EndDate` "the date, in UTC, before which the
//!   schedule can invoke its target" (`CreateSchedule$StartDate` /
//!   `$EndDate`): together they are an interval, and an interval that holds no
//!   instant is a schedule that can never fire ([`schedule_window_violation`]).
//! * both instants are stored at MILLISECOND resolution
//!   ([`to_millisecond_precision`]), which is the resolution a schedule's
//!   timestamps are reported back at.
//!
//! The expression grammars are read by maintained parsers — the `cron` crate for
//! a `cron(...)` body and `chrono` for an `at(...)` instant — rather than by a
//! hand-written one, and a zone name by `chrono-tz`, which carries the IANA
//! database.

use std::time::SystemTime;

use ::aws_smithy_types::DateTime;

/// The units a `rate(value unit)` expression may name, and what each is worth in
/// seconds: "rate(value unit) […] unit: The unit of time. Required: Yes. Values:
/// minute | minutes | hour | hours | day | days" (the service's schedule-types
/// reference). Both spellings of each unit are accepted, as the service accepts
/// them.
const RATE_UNITS: &[(&str, u64)] = &[
    ("minute", 60),
    ("minutes", 60),
    ("hour", 3_600),
    ("hours", 3_600),
    ("day", 86_400),
    ("days", 86_400),
];

/// How many whitespace-separated fields a `cron(...)` body carries: "cron(fields)
/// […] Minutes Hours Day-of-month Month Day-of-week Year" — six.
const CRON_FIELDS: usize = 6;

/// The indexes of the two fields a `?` ("no specific value") is a statement in —
/// day-of-month and day-of-week — which is the same statement `*` makes in the
/// grammar the `cron` crate reads.
const NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [2, 4];

/// The field values the normalised seven-field expression carries for every field
/// but the one under test, so a field is judged on its own.
const NEUTRAL_CRON_FIELDS: [&str; CRON_FIELDS + 1] = ["0", "0", "0", "*", "*", "*", "*"];

/// The wildcards the service's own cron dialect documents on top of the ones the
/// `cron` crate's grammar reads: `L` (last), `W` (weekday) and `#` (nth weekday
/// of the month), from the wildcards table of the service's schedule-types
/// reference. A field that carries one of them states something the shared parser
/// cannot read, so it is left to the service's grammar rather than refused for a
/// grammar it does not use.
const SERVICE_CRON_WILDCARDS: [char; 3] = ['L', 'W', '#'];

/// The format an `at(...)` body states its instant in:
/// "at(yyyy-mm-ddThh:mm:ss)" — a local date and time with no zone offset, since
/// the zone is `ScheduleExpressionTimezone`'s to state.
const AT_EXPRESSION_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// A nanosecond count of one millisecond: the resolution a schedule's instants
/// are held at.
const NANOS_PER_MILLISECOND: u32 = 1_000_000;

/// `at` instant, `rate` period or `cron` fields: the three forms a
/// `ScheduleExpression` may state, keyed by the keyword that introduces each.
///
/// The keyword decides which grammar reads the body, so an expression that
/// states none of the three keywords states no schedule at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpressionForm {
    At,
    Rate,
    Cron,
}

/// The form `expression` states and the body it carries, or `None` when it states
/// no form this service defines.
fn form_of(expression: &str) -> Option<(ExpressionForm, &str)> {
    for (keyword, form) in [
        ("at", ExpressionForm::At),
        ("rate", ExpressionForm::Rate),
        ("cron", ExpressionForm::Cron),
    ] {
        if let Some(body) = expression
            .strip_prefix(keyword)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return Some((form, body));
        }
    }
    None
}

/// How this service states an expression that is not a schedule: the class of the
/// failure, with the value the request carried, which is the only place the
/// service reports the rejected expression back.
fn invalid_expression(expression: &str) -> String {
    format!("Invalid Schedule Expression {expression}.")
}

/// Refuse a `ScheduleExpression` that states none of the three documented forms,
/// or states one of them malformed; `None` for an expression the service can read.
///
/// The length bound (1..256) is modelled, so the boundary validator has already
/// refused anything outside it by the time a handler runs.
pub fn schedule_expression_violation(expression: &str) -> Option<String> {
    let readable = match form_of(expression) {
        Some((ExpressionForm::At, body)) => at_expression_states_an_instant(body),
        Some((ExpressionForm::Rate, body)) => rate_expression_states_a_period(body),
        Some((ExpressionForm::Cron, body)) => cron_expression_states_its_instants(body),
        None => false,
    };
    (!readable).then(|| invalid_expression(expression))
}

/// Whether an `at(...)` body states an instant in the documented format.
fn at_expression_states_an_instant(body: &str) -> bool {
    ::chrono::NaiveDateTime::parse_from_str(body.trim(), AT_EXPRESSION_FORMAT).is_ok()
}

/// Whether a `rate(...)` body states a period: a positive integer of a unit the
/// service names ("value: The positive number […] unit: minute | minutes | hour |
/// hours | day | days"), and nothing else.
fn rate_expression_states_a_period(body: &str) -> bool {
    let mut fields = body.split_whitespace();
    let (Some(value), Some(unit), None) = (fields.next(), fields.next(), fields.next()) else {
        return false;
    };
    let states_a_count = value.parse::<u64>().is_ok_and(|value| value > 0);
    states_a_count && RATE_UNITS.iter().any(|(name, _)| *name == unit)
}

/// Whether a `cron(...)` body states the instants it has to: six fields, each one
/// the cron grammar reads in the place it stands.
///
/// The `cron` crate reads the seven-field form (seconds first), so a literal `0`
/// seconds field is prepended, and a `?` in the day-of-month or day-of-week field
/// is the `*` that grammar spells the same statement with. A field carrying one of
/// the service's own wildcards ([`SERVICE_CRON_WILDCARDS`]) is a statement that
/// grammar does not read at all, so it is accepted on the service's grammar
/// instead of being refused on the parser's.
fn cron_expression_states_its_instants(body: &str) -> bool {
    let fields: Vec<&str> = body.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return false;
    }
    let service_dialect = fields.iter().any(|field| names_service_wildcard(field));
    for (index, field) in fields.iter().enumerate() {
        if !cron_field_parses(index, field) && !names_service_wildcard(field) {
            return false;
        }
    }
    // Every field reads on its own; the whole expression is read once more so a
    // combination of fields the grammar refuses is refused too — except for a body
    // in the service's own dialect, which that grammar cannot read as a whole
    // either.
    service_dialect || normalised_cron(&fields).parse::<::cron::Schedule>().is_ok()
}

/// Whether `field` carries one of the wildcards this service's cron dialect
/// documents and the shared parser's grammar does not read.
fn names_service_wildcard(field: &str) -> bool {
    SERVICE_CRON_WILDCARDS
        .iter()
        .any(|wildcard| field.contains(*wildcard))
}

/// The six documented fields as the seven-field expression the `cron` crate reads.
fn normalised_cron(fields: &[&str]) -> String {
    let mut expression = String::from(NEUTRAL_CRON_FIELDS[0]);
    for (index, field) in fields.iter().enumerate() {
        expression.push(' ');
        expression.push_str(cron_field_as_parsed(index, field));
    }
    expression
}

/// One field as the `cron` crate's grammar reads it: `?` ("no specific value") is
/// `*` in that grammar, in the two fields it is a statement in.
fn cron_field_as_parsed(index: usize, field: &str) -> &str {
    if field == "?" && NO_SPECIFIC_VALUE_FIELDS.contains(&index) {
        "*"
    } else {
        field
    }
}

/// Whether `field` is one the cron grammar reads in the place `index` names,
/// judged by parsing it there with every other field neutral — so the answer is
/// about that field and not about the rest of the expression.
fn cron_field_parses(index: usize, field: &str) -> bool {
    let mut expression = NEUTRAL_CRON_FIELDS;
    expression[index + 1] = cron_field_as_parsed(index, field);
    expression
        .join(" ")
        .parse::<::cron::Schedule>()
        .is_ok()
}

/// How this service states a time zone no IANA zone answers to.
fn invalid_timezone(timezone: &str) -> String {
    format!("Invalid Schedule Expression Timezone {timezone}.")
}

/// Refuse a `ScheduleExpressionTimezone` that names no time zone; `None` for an
/// absent member (the expression is then evaluated in UTC) or a zone the IANA
/// database carries.
///
/// The length bound (1..50) is modelled, so the boundary validator has already
/// refused anything outside it.
pub fn schedule_expression_timezone_violation(timezone: Option<&str>) -> Option<String> {
    let timezone = timezone?;
    timezone
        .parse::<::chrono_tz::Tz>()
        .err()
        .map(|_| invalid_timezone(timezone))
}

/// How this service states a window whose boundaries are in the wrong order.
const START_DATE_BEFORE_END_DATE: &str = "StartDate must be earlier than EndDate.";

/// How this service states a window that closed before it was asked for.
const END_DATE_IN_THE_FUTURE: &str = "EndDate must be in the future.";

/// Refuse a `StartDate`/`EndDate` pair that names an EMPTY invocation window —
/// one holding no instant at which the schedule could invoke its target.
///
/// The window opens at `StartDate` ("after which the schedule can begin invoking
/// its target") when the request carries one, and otherwise at the instant of the
/// request, since nothing bounds it earlier; it closes at `EndDate` ("before
/// which the schedule can invoke its target"). So a pair whose start is after its
/// end is refused, and an end already behind the clock is refused when no start
/// was given.
pub fn schedule_window_violation(
    start_date: Option<DateTime>,
    end_date: Option<DateTime>,
    now: SystemTime,
) -> Option<String> {
    let end_date = end_date?;
    match start_date {
        Some(start_date) => {
            (instant(start_date) > instant(end_date)).then(|| START_DATE_BEFORE_END_DATE.to_string())
        }
        None => (instant(end_date) <= instant(DateTime::from(now)))
            .then(|| END_DATE_IN_THE_FUTURE.to_string()),
    }
}

/// One wire instant as the pair that orders it: `DateTime` carries no ordering of
/// its own, and a second count with its sub-second remainder compares exactly.
fn instant(at: DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// One of a schedule's instants at the resolution the service holds it: the
/// sub-second remainder truncated to whole milliseconds.
///
/// EventBridge Scheduler reports `StartDate`/`EndDate` at millisecond resolution,
/// so a request stating a finer instant is stored at the millisecond it falls in —
/// which is the instant every later read of the schedule reports.
pub fn to_millisecond_precision(at: DateTime) -> DateTime {
    let whole_milliseconds = at.subsec_nanos() / NANOS_PER_MILLISECOND;
    DateTime::from_secs_and_nanos(at.secs(), whole_milliseconds * NANOS_PER_MILLISECOND)
}

#[cfg(test)]
mod tests {
    //! The rules this module owns, each over a SET of inputs rather than one
    //! recorded expression. The same rules are driven through real requests in
    //! `tests/all/schedule_definition_rules.rs`.

    use super::*;

    /// Every documented form, well-formed and malformed, is read as the service
    /// documents it.
    #[test]
    fn an_expression_states_one_of_the_three_documented_forms() {
        // (case, expression, states a schedule)
        let rows: [(&str, &str, bool); 26] = [
            ("a rate of minutes", "rate(5 minutes)", true),
            ("a rate of one minute", "rate(1 minute)", true),
            // A singular unit with a plural count, and the reverse, are both
            // forms the unit set carries.
            ("a plural unit with one", "rate(1 minutes)", true),
            ("a singular unit with five", "rate(5 minute)", true),
            ("a rate of hours", "rate(12 hours)", true),
            ("a rate of days", "rate(1 day)", true),
            ("a rate of zero", "rate(0 minutes)", false),
            ("a rate of nothing", "rate(bogus)", false),
            ("a rate with no unit", "rate(5)", false),
            ("a rate in a unit nothing measures", "rate(5 fortnights)", false),
            ("a negative rate", "rate(-5 minutes)", false),
            ("a fractional rate", "rate(1.5 hours)", false),
            ("a rate with a trailing field", "rate(5 minutes please)", false),
            ("a six-field cron", "cron(0 8 1 * ? *)", true),
            ("a cron naming weekdays", "cron(0 18 ? * MON-FRI *)", true),
            ("a cron over a step", "cron(0/15 * * * ? *)", true),
            // The wildcards the service's own dialect documents.
            ("a cron on the last day of the month", "cron(0 8 L * ? *)", true),
            ("a cron on the nearest weekday", "cron(0 8 15W * ? *)", true),
            ("a cron on the second Tuesday", "cron(0 8 ? * 3#2 *)", true),
            ("a cron of five fields", "cron(0 8 1 * ?)", false),
            ("a cron of prose", "cron(not a cron)", false),
            ("a cron with a field nothing parses", "cron(0 8 1 * QQ *)", false),
            ("an empty cron", "cron()", false),
            ("an at instant", "at(2027-01-01T00:00:00)", true),
            ("an at instant with a zone", "at(2027-01-01T00:00:00Z)", false),
            ("no form at all", "every 5 minutes", false),
        ];

        for (case, expression, states_a_schedule) in rows {
            let violation = schedule_expression_violation(expression);
            assert_eq!(
                violation.is_none(),
                states_a_schedule,
                "case {case:?}: expected states_a_schedule={states_a_schedule}, got \
                 {violation:?}"
            );
            if let Some(message) = violation {
                assert!(
                    message.contains(expression),
                    "case {case:?}: the refusal states the expression the request \
                     carried, got {message:?}"
                );
            }
        }
    }

    /// A time zone is one the IANA database carries, and an absent member is no
    /// violation at all.
    #[test]
    fn a_timezone_names_an_iana_zone() {
        // (case, timezone, names a zone)
        let rows: [(&str, Option<&str>, bool); 8] = [
            ("no zone at all", None, true),
            ("UTC", Some("UTC"), true),
            ("a European zone", Some("Europe/London"), true),
            ("an American zone", Some("America/New_York"), true),
            ("an Etc zone", Some("Etc/GMT+5"), true),
            ("a zone nothing names", Some("Not/AZone"), false),
            ("an offset rather than a zone", Some("+01:00"), false),
            ("prose", Some("here"), false),
        ];

        for (case, timezone, names_a_zone) in rows {
            let violation = schedule_expression_timezone_violation(timezone);
            assert_eq!(
                violation.is_none(),
                names_a_zone,
                "case {case:?}: expected names_a_zone={names_a_zone}, got {violation:?}"
            );
        }
    }

    /// The window a schedule may fire in holds at least one instant.
    #[test]
    fn a_window_holds_an_instant() {
        let now = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_000_000);
        let at = |secs: i64| DateTime::from_secs(secs);
        // (case, start, end, holds an instant)
        let rows: [(&str, Option<DateTime>, Option<DateTime>, bool); 8] = [
            ("neither boundary", None, None, true),
            ("a start only", Some(at(2_000_000)), None, true),
            ("an end ahead of the clock", None, Some(at(2_000_000)), true),
            ("an end behind the clock", None, Some(at(999_999)), false),
            ("an end at the clock", None, Some(at(1_000_000)), false),
            ("an ordered pair", Some(at(2_000_000)), Some(at(3_000_000)), true),
            ("a reversed pair", Some(at(3_000_000)), Some(at(2_000_000)), false),
            // A single-instant window is one the schedule can fire in.
            ("one instant", Some(at(2_000_000)), Some(at(2_000_000)), true),
        ];

        for (case, start_date, end_date, holds_an_instant) in rows {
            let violation = schedule_window_violation(start_date, end_date, now);
            assert_eq!(
                violation.is_none(),
                holds_an_instant,
                "case {case:?}: expected holds_an_instant={holds_an_instant}, got \
                 {violation:?}"
            );
        }
    }

    /// An instant is held at the millisecond it falls in, whatever finer value the
    /// request stated.
    #[test]
    fn an_instant_is_held_to_the_millisecond() {
        // (case, submitted nanos, held nanos)
        let rows: [(&str, u32, u32); 6] = [
            ("a whole second", 0, 0),
            ("whole milliseconds", 123_000_000, 123_000_000),
            ("microseconds", 123_456_000, 123_000_000),
            ("nanoseconds", 123_456_789, 123_000_000),
            ("under a millisecond", 999_999, 0),
            ("one nanosecond under a millisecond", 1_999_999, 1_000_000),
        ];

        for (case, submitted, held) in rows {
            let normalised = to_millisecond_precision(DateTime::from_secs_and_nanos(7, submitted));
            assert_eq!(
                (normalised.secs(), normalised.subsec_nanos()),
                (7, held),
                "case {case:?}: expected the instant held at {held} nanos, got \
                 {normalised:?}"
            );
        }
    }
}
