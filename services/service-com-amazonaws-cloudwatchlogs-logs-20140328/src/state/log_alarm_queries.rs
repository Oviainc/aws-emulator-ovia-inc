// SPDX-License-Identifier: CC0-1.0

//! The scheduled query CloudWatch manages on a LOG ALARM's behalf.
//!
//! "When you create a log alarm, the operation creates a service-managed CloudWatch Logs
//! scheduled query that runs the query string you provide on the schedule you configure"
//! (`PutLogAlarm`), and that alarm's configuration names it by the ARN CloudWatch minted for
//! it (`ScheduledQueryConfiguration$QueryARN`). So this account holds a scheduled query no
//! `CreateScheduledQuery` call ever created, and a reader of scheduled queries must report it.
//!
//! The alarm is the ONE home of that query's configuration — the query string, the log groups,
//! the schedule and the role are members of the alarm's own `ScheduledQueryConfiguration`,
//! stored in the CloudWatch alarm store and reached through the shared engine, the same way an
//! ingested Embedded Metric Format event reaches the metric store ([`super::emf`]). This
//! module therefore DERIVES the query from the alarm rather than keeping a second copy of it
//! beside the queries callers create: one write to the alarm cannot leave the two disagreeing.
//!
//! The alarm store is read with no log-store guard held, so the two services' stores are never
//! locked at once.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use super::{scheduled_query_name_from_identifier, ScheduledQueryRecord};

/// The schedule type of a query the service manages for a log alarm: the other of the two
/// values the model's `ScheduleType` enumerates (`CUSTOMER_MANAGED` | `AWS_MANAGED`), because
/// the schedule belongs to the alarm CloudWatch created the query for and no caller created it.
pub const SCHEDULE_TYPE_AWS_MANAGED: &str = "AWS_MANAGED";

/// The service-managed scheduled query `identifier` names, or `None` when no log alarm of this
/// account owns one under that name — including when the alarm store is not mounted at all.
///
/// A `ScheduledQueryIdentifier` is "The ARN or name of the scheduled query": the managed query
/// carries the name of the alarm it belongs to and the ARN built from that name, so both forms
/// resolve through [`scheduled_query_name_from_identifier`], exactly as they do for a query a
/// caller created. `region` is the one the reading request was signed for, used only for an
/// alarm whose configuration names no ARN of its own.
pub fn service_managed_scheduled_query<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
    region: &str,
) -> Option<ScheduledQueryRecord> {
    let state = cloudwatch::state::service_state(engine)?;
    let store = state.lock();
    let alarm = &store
        .log_alarms
        .get(scheduled_query_name_from_identifier(identifier))?
        .alarm;
    derived(alarm, region)
}

/// EVERY service-managed scheduled query this account holds — one per log alarm whose
/// configuration names one — in ascending name order, which is the order the alarm's name is
/// the query's name in.
///
/// A reader that enumerates scheduled queries must report these beside the ones callers
/// created, because they are scheduled queries of this account that no `CreateScheduledQuery`
/// call created. The alarm store is a hash map, so the derived queries are ordered here rather
/// than left in the store's own iteration order: an enumeration a client pages through must not
/// depend on it.
///
/// Empty when the alarm store is not mounted at all. `region` is used only for an alarm whose
/// configuration names no ARN of its own, exactly as in
/// [`service_managed_scheduled_query`].
pub fn service_managed_scheduled_queries<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<ScheduledQueryRecord> {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let mut queries: Vec<ScheduledQueryRecord> = store
        .log_alarms
        .values()
        .filter_map(|stored| derived(&stored.alarm, region))
        .collect();
    queries.sort_by(|left, right| left.name.cmp(&right.name));
    queries
}

/// The refusal CloudWatch Logs answers a WRITE of a scheduled query the service manages for a
/// log alarm with.
///
/// The alarm's own `ScheduledQueryConfiguration` is the one home of such a query's
/// configuration — which is why its schedule type is [`SCHEDULE_TYPE_AWS_MANAGED`] rather than
/// the caller's — so the query the identifier names is not one a caller's write of a scheduled
/// query configures: the configuration is written by writing the alarm. The message names the
/// query the request asked for, taken from the record derived for it.
///
/// UNVERIFIED WORDING: neither this service's model nor its published documentation states a
/// sentence for this refusal class, so the text below is this emulator's own statement of the
/// rule rather than one taken from the service. Replace it with the service's own sentence when
/// that becomes known.
pub fn managed_query_not_writable_message(name: &str) -> String {
    managed_query_refusal_message(name, "updated")
}

/// The refusal CloudWatch Logs answers a DELETE of a scheduled query the service manages for a
/// log alarm with.
///
/// The same rule as [`managed_query_not_writable_message`], for the write that would remove the
/// query rather than reconfigure it: the query exists because the alarm's
/// `ScheduledQueryConfiguration` states it, so it stops existing when that alarm stops stating
/// it and not because a caller's delete of a scheduled query named it.
///
/// UNVERIFIED WORDING: see [`managed_query_not_writable_message`].
pub fn managed_query_not_deletable_message(name: &str) -> String {
    managed_query_refusal_message(name, "deleted")
}

/// The ONE sentence both refusals above state, so a caller's write of a service-managed query is
/// refused in the same words whichever write it was; `attempted` names what the request asked of
/// the query.
fn managed_query_refusal_message(name: &str, attempted: &str) -> String {
    format!("The scheduled query {name} is managed by CloudWatch and cannot be {attempted}.")
}

/// The scheduled query one log alarm's configuration describes, or `None` when that alarm has
/// no scheduled query configuration (so no managed query belongs to it).
///
/// The ONE derivation both readers above share, so a query reported by a lookup and the same
/// query reported by an enumeration cannot disagree.
fn derived(
    alarm: &cloudwatch::ops::types::LogAlarm,
    region: &str,
) -> Option<ScheduledQueryRecord> {
    let name = alarm.alarm_name.as_deref()?;
    let configuration = alarm.scheduled_query_configuration.as_ref()?;
    let schedule = &configuration.schedule_configuration;

    let mut query = ScheduledQueryRecord::managed(
        name,
        // The ARN is the one the alarm's configuration names the managed query by; an alarm
        // whose configuration carries none leaves the query named by nothing but the alarm, so
        // the ARN that names it is the one this service builds from that name.
        configuration
            .query_arn
            .clone()
            .unwrap_or_else(|| super::scheduled_query_arn(region, name)),
        // The query string the alarm evaluates, which is "the query string you provide".
        &configuration.query_string,
        &schedule.schedule_expression,
        // "The ARN of the IAM role CloudWatch assumes to run the scheduled query"
        // (`ScheduledQueryConfiguration$ScheduledQueryRoleARN`) is the role this query is
        // executed under.
        &configuration.scheduled_query_role_arn,
    );
    query.log_group_identifiers = configuration.log_group_identifiers.clone().unwrap_or_default();
    // `ScheduleConfiguration` states the lookback window as offsets in seconds, which is what
    // the scheduled query's own offsets are.
    query.start_time_offset = Some(schedule.start_time_offset);
    query.end_time_offset = schedule.end_time_offset;
    // "The time stamp of the last update to the alarm configuration"
    // (`LogAlarm$AlarmConfigurationUpdatedTimestamp`) is the instant the managed query was last
    // written, because the alarm's configuration IS that query's configuration. The alarm
    // records no instant of its own creation, so the query reports none either rather than
    // reporting an update instant as one.
    query.last_updated_time_ms = alarm
        .alarm_configuration_updated_timestamp
        .as_ref()
        .and_then(|stamp| stamp.to_millis().ok());

    Some(query)
}
