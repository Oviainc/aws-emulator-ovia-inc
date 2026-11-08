// SPDX-License-Identifier: CC0-1.0

//! The STOP CONDITIONS of a running experiment, judged against the alarms they name.
//!
//! A stop condition is the blast-radius guardrail of an experiment: FIS documents it as
//! "the CloudWatch alarm that will be monitored to determine if the experiment should be
//! stopped", so while an experiment runs the SERVICE watches that alarm and ends the run
//! itself when the alarm is in its alarming state — no `StopExperiment` from the caller,
//! and whatever is left of the actions' duration.
//!
//! The alarm's state is not something this service holds: it belongs to the account's
//! CloudWatch store in the SAME shared engine — the records `PutMetricAlarm`,
//! `PutCompositeAlarm`, `PutLogAlarm` and `SetAlarmState` wrote — so the states are READ
//! from there ([`AlarmBreaches::read`], the same way the execution-role gate reads the
//! account's IAM store in [`super::action_permissions`]) and never guessed at. A breach is
//! therefore a change the CLOCK made due: the alarm went to its alarming state at an
//! instant of its own, FIS's watch of it was owed from that instant, and
//! [`AlarmBreaches::breached_alarm`] is what the settle of an experiment spends it with.
//!
//! An engine carrying no CloudWatch store, or a condition naming an alarm that store holds
//! none of, shows nothing: the alarm may belong to another account or to a client that
//! never told this emulator about it, so the run goes ahead rather than being stopped for
//! an alarm nothing can be read from — the same reading the privileges gate takes of a role
//! the account's IAM store does not hold.

use std::collections::HashSet;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::{ExperimentStopCondition, STOP_CONDITION_SOURCE_ALARM};

/// The CloudWatch store this service reads an alarm's state from: that service's OWN state
/// struct, reached through the SAME shared engine this service's store lives in.
type CloudWatchState = cloudwatch::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The alarm state a stop condition is BREACHED by — one of CloudWatch's three
/// `StateValue`s, read through THAT service's own constant so the two services cannot
/// disagree about how the alarming state is spelled.
pub const ALARM_STATE_BREACHED: &str = cloudwatch::ALARM_STATE_ALARM;

/// The most alarms one read of the CloudWatch store keeps.
///
/// Only the alarms reported as BREACHED are kept, and this bounds how many of those one
/// settle is judged against: a store holding more alarming alarms than this contributes the
/// first ones it walks, which is the same "what cannot be finished being read shows
/// nothing" reading [`super::action_permissions`] takes of a role with too many documents.
/// Well above CloudWatch's own published alarm quota for one account in one Region being
/// simultaneously in alarm in any realistic store.
pub const MAX_BREACHED_ALARMS: usize = 256;

/// The ARNs of the alarms the account's CloudWatch store reports in
/// [`ALARM_STATE_BREACHED`], as one snapshot.
///
/// Built with the CloudWatch guard held and holding none afterwards, so the experiments'
/// stop conditions are judged with that store free — and judged against ONE snapshot, so
/// two experiments settled by the same call cannot disagree about an alarm.
#[derive(Debug, Clone, Default)]
pub struct AlarmBreaches {
    /// The ARN of every alarm reported in [`ALARM_STATE_BREACHED`].
    breached: HashSet<String>,
}

impl AlarmBreaches {
    /// The snapshot in which NO alarm is breached: what an engine carrying no CloudWatch
    /// store shows, and what a sweep whose stop is already decided by another rule is
    /// judged against.
    pub fn none() -> Self {
        Self::default()
    }

    /// The alarms the account's CloudWatch store reports in [`ALARM_STATE_BREACHED`] right
    /// now.
    ///
    /// `None` from the store accessor means the engine carries no CloudWatch store, which
    /// shows nothing about any alarm: the snapshot is then empty and no run is stopped for
    /// a state nothing could be read from.
    pub fn read<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        let Some(state) = cloudwatch::service_state(engine) else {
            return Self::none();
        };
        let guard = state.lock();
        let store: &CloudWatchState = &guard;
        let mut breached = HashSet::new();
        // Each of the three kinds of alarm CloudWatch holds reports a `StateValue` and its
        // own ARN, which is what a stop condition names, so all three are read: a stop
        // condition carries an alarm ARN and not the kind of alarm behind it.
        let metric = store
            .alarms
            .values()
            .map(|stored| (&stored.alarm.state_value, &stored.alarm.alarm_arn));
        let composite = store
            .composite_alarms
            .values()
            .map(|stored| (&stored.alarm.state_value, &stored.alarm.alarm_arn));
        let logs = store
            .log_alarms
            .values()
            .map(|stored| (&stored.alarm.state_value, &stored.alarm.alarm_arn));
        for (state_value, arn) in metric.chain(composite).chain(logs) {
            if breached.len() >= MAX_BREACHED_ALARMS {
                break;
            }
            if state_value.as_deref() != Some(ALARM_STATE_BREACHED) {
                continue;
            }
            if let Some(arn) = arn.as_deref().filter(|arn| !arn.is_empty()) {
                breached.insert(arn.to_string());
            }
        }
        Self { breached }
    }

    /// The alarm ARN of the first of `conditions` that is breached, or `None` when none of
    /// them is.
    ///
    /// Only a condition naming the CloudWatch alarm source carries an alarm to watch:
    /// FIS's other documented source is the explicit `none` ([`STOP_CONDITION_SOURCE_NONE`]
    /// — "specify `none` if there is no stop condition"), which can never halt a run. The
    /// conditions are examined in the order the template declared them, so the alarm a
    /// halted experiment names is the same at every read.
    ///
    /// [`STOP_CONDITION_SOURCE_NONE`]: super::STOP_CONDITION_SOURCE_NONE
    pub fn breached_alarm<'conditions>(
        &self,
        conditions: &'conditions [ExperimentStopCondition],
    ) -> Option<&'conditions str> {
        conditions.iter().find_map(|condition| {
            if condition.source.as_deref() != Some(STOP_CONDITION_SOURCE_ALARM) {
                return None;
            }
            condition
                .value
                .as_deref()
                .filter(|alarm_arn| self.breached.contains(*alarm_arn))
        })
    }
}
