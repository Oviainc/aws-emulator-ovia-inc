// SPDX-License-Identifier: CC0-1.0

//! The channel ALERT slice of this service's state.
//!
//! One file for the whole slice — the per-channel alert log, the state an alert
//! is in, the `StateFilter` a read narrows it with and the order a read reports
//! it in — so the slice can be read, and merged, on its own. The store itself is
//! the `alerts` member of [`super::ChannelRecord`]: an alert is "An alert on a
//! channel" (`smithy_query(query="shape", shape="ChannelAlert")`), so it lives
//! on that channel's record and goes away with it rather than in a second map
//! beside `channels`.
//!
//! Every rule here comes from the model's own documentation:
//!
//! * `ChannelAlertState`: "The possible states of a channel alert. SET - The
//!   alert is actively happening. CLEARED - The alert is no longer happening."
//!   — the SAME two values every alert surface of this service declares, so the
//!   names, the documented filter set, the state derivation and the filter
//!   predicate come from [`super::alerts`], which owns them once for the channel,
//!   cluster and multiplex listings alike. `State` is therefore DERIVED from
//!   `ClearedTimestamp` ("The time when the alert was cleared") and never
//!   stored, so the two cannot disagree.
//! * `ListAlertsRequest$StateFilter`: "Specifies the set of alerts to return
//!   based on their state. SET - Return only alerts with SET state. CLEARED -
//!   Return only alerts with CLEARED state. ALL - Return all alerts." —
//!   [`CHANNEL_ALERT_FILTERS`] and [`ChannelAlertRecord::admitted_by`].
//! * `ChannelAlert$Id`: "The unique ID for this alert instance" — minted from
//!   the channel's own alert counter, so two alerts of one channel never share
//!   one.
//! * `ChannelAlert$PipelineId`: "The ID of the pipeline this alert is associated
//!   with" — numbered as `PipelineDetail$PipelineId` numbers a channel's
//!   pipelines, so an alert cannot be associated with a pipeline the channel's
//!   class does not give it ([`super::ChannelRecord::raise_alert`] records no
//!   such alert at all).
//!
//! WHO WRITES IT: an alert is the SERVICE's observation of a condition on a
//! channel — the real one raises one when the media it is ingesting or pushing
//! goes wrong. This emulator runs no pipeline and ingests no media, so no
//! operation observes such a condition and nothing here raises an alert on its
//! own: a channel's log is empty unless [`ChannelAlerts::raise`] put something
//! in it, and `ListAlerts` reports exactly what the log holds. Fabricating an
//! alert for a channel that is merely running would be inventing a condition
//! nothing observed, so this slice does not.

use std::time::SystemTime;

use crate::ops::types::ChannelAlert;

/// The `ChannelAlertState` values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="ChannelAlertState")`: SET | CLEARED).
///
/// The same two values every alert-state enum of this service declares, so they
/// live once in [`super::alerts`] and this is the channel slice's name for them,
/// exactly as the cluster slice's `cluster_alert_state` is.
pub use super::alerts::alert_state as channel_alert_state;

/// The `StateFilter` value that asks for every alert whatever its state ("ALL -
/// Return all alerts"), which is NOT one of the two states an alert can be in.
/// Shared with every other alert listing of this service
/// ([`super::alerts::ALERT_STATE_FILTER_ALL`]), whose member documents the same
/// set.
pub const CHANNEL_ALERT_FILTER_ALL: &str = super::alerts::ALERT_STATE_FILTER_ALL;

/// Every `StateFilter` value `ListAlertsRequest$StateFilter` documents, in the
/// order it documents them — the shared
/// [`super::alerts::ALERT_STATE_FILTERS`], so two alert listings of this service
/// cannot accept different values. The member targets bare `__string`, which
/// carries no enum trait, so this is the set `ListAlerts` refuses against.
pub const CHANNEL_ALERT_FILTERS: [&str; 3] = super::alerts::ALERT_STATE_FILTERS;

/// The most alerts this emulator keeps in one channel's log.
///
/// A ceiling on the store, not an AWS quota: an alert is never deleted by a
/// caller (the API has no operation that deletes one) and no doc sentence pins a
/// retention, so without a bound a long-lived channel's log would grow without
/// end. Once the log is full the OLDEST alert is dropped, because an alert log is
/// a history and the recent end of it is the useful one. The same bound as the
/// sibling cluster log ([`super::MAX_CLUSTER_ALERTS`]), so two alert logs of one
/// emulator do not keep different numbers of alerts.
pub const MAX_CHANNEL_ALERTS: usize = super::MAX_CLUSTER_ALERTS;

/// One alert on one channel, as the service observed it.
///
/// The members mirror the model's `ChannelAlert` shape minus the one it derives:
/// `State` comes from [`Self::state`], out of whether the alert has been cleared,
/// rather than from a field that could disagree with `ClearedTimestamp` — the
/// same derivation [`super::ClusterAlertRecord`] uses.
#[derive(Debug, Clone)]
pub struct ChannelAlertRecord {
    /// `ChannelAlert$Id`: "The unique ID for this alert instance".
    pub id: String,
    /// `ChannelAlert$AlertType`: "The type of the alert".
    pub alert_type: String,
    /// `ChannelAlert$Message`: "The user facing alert message which can have
    /// more context". Absent when the observation carried none.
    pub message: Option<String>,
    /// `ChannelAlert$PipelineId`, as a `PipelineDetail$PipelineId` number.
    /// Absent for an alert about the channel rather than one of its pipelines.
    pub pipeline_id: Option<i32>,
    /// `ChannelAlert$SetTimestamp`: "The time when the alert was set".
    pub set_at: SystemTime,
    /// `ChannelAlert$ClearedTimestamp`: "The time when the alert was cleared",
    /// absent while the alert is still happening — which is exactly what makes
    /// it `SET`.
    pub cleared_at: Option<SystemTime>,
    /// Mint order within this channel's log, which breaks a tie between two
    /// alerts set at the same instant so a read's order is stable.
    pub sequence: u64,
}

impl ChannelAlertRecord {
    /// "The state of the alert": `SET` while it is actively happening, `CLEARED`
    /// once it is not. Derived from [`Self::cleared_at`], never stored, through
    /// the one derivation every alert surface of this service shares.
    pub fn state(&self) -> &'static str {
        super::alerts::alert_state_of(self.cleared_at)
    }

    /// Whether `ListAlerts`' `StateFilter` admits this alert: `SET` and
    /// `CLEARED` admit the alerts in that state, `ALL` admits every one, and a
    /// request that named no filter is not filtered at all — the one predicate
    /// every alert listing of this service shares.
    pub fn admitted_by(&self, filter: Option<&str>) -> bool {
        super::alerts::alert_admitted_by(self.cleared_at, filter)
    }
}

/// One channel's alert log.
///
/// A `Vec` rather than a map: an alert is reached only through the channel it is
/// on, and what a read needs is the order the alerts were set in, which is the
/// order they were appended in.
#[derive(Debug, Clone, Default)]
pub struct ChannelAlerts {
    alerts: Vec<ChannelAlertRecord>,
    next_sequence: u64,
}

impl ChannelAlerts {
    /// Record an alert the service has just observed on the channel `channel_id`
    /// names, and answer the `Id` it was given.
    ///
    /// The alert starts SET ("the alert is actively happening") with `now` as
    /// its `SetTimestamp` and no `ClearedTimestamp` — which is what makes its
    /// derived state SET. `pipeline` has already been resolved against the
    /// channel's class by [`super::ChannelRecord::raise_alert`], the only caller.
    ///
    /// Raising past [`MAX_CHANNEL_ALERTS`] drops the oldest alert, so the log is
    /// bounded however long the channel lives.
    pub fn raise(
        &mut self,
        channel_id: &str,
        alert_type: &str,
        message: Option<&str>,
        pipeline: Option<i32>,
        now: SystemTime,
    ) -> String {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        // Scoped by the channel the alert is on, so an id is unique across the
        // account's channels and not only within one log.
        let id = format!("{channel_id}-{sequence}");
        while self.alerts.len() >= MAX_CHANNEL_ALERTS {
            self.alerts.remove(0);
        }
        self.alerts.push(ChannelAlertRecord {
            id: id.clone(),
            alert_type: alert_type.to_string(),
            message: message.map(ToString::to_string),
            pipeline_id: pipeline,
            set_at: now,
            cleared_at: None,
            sequence,
        });
        id
    }

    /// Record that the alert `id` names is no longer happening, as of `now`, and
    /// answer whether it was still happening.
    ///
    /// Clearing an alert that has already been cleared changes nothing, so its
    /// `ClearedTimestamp` stays the instant it actually stopped happening.
    pub fn clear(&mut self, id: &str, now: SystemTime) -> bool {
        let Some(alert) = self.alerts.iter_mut().find(|alert| alert.id == id) else {
            return false;
        };
        if alert.cleared_at.is_some() {
            return false;
        }
        alert.cleared_at = Some(now);
        true
    }

    /// Every alert of this log the `filter` admits, in the order the service set
    /// them: by `SetTimestamp`, then by mint order so the order is total and
    /// stable across reads.
    ///
    /// The order a reader walks the log in, and so the sequence `ListAlerts`
    /// paginates over. Oldest first, which is why appending an alert cannot
    /// change where an already-handed-out page boundary falls.
    pub fn in_set_order(&self, filter: Option<&str>) -> Vec<&ChannelAlertRecord> {
        let mut ordered: Vec<&ChannelAlertRecord> = self
            .alerts
            .iter()
            .filter(|alert| alert.admitted_by(filter))
            .collect();
        ordered.sort_by_key(|alert| (alert.set_at, alert.sequence));
        ordered
    }

    /// How many alerts this log holds, whatever state they are in.
    pub fn len(&self) -> usize {
        self.alerts.len()
    }

    /// Whether this channel has no recorded alert at all — which is every
    /// channel of this emulator until something raises one.
    pub fn is_empty(&self) -> bool {
        self.alerts.is_empty()
    }
}

/// Project one stored alert onto the model's `ChannelAlert` shape.
///
/// Every member comes from the record: nothing is read out of the request that
/// listed it. `State` is derived ([`ChannelAlertRecord::state`]), so it cannot
/// disagree with `ClearedTimestamp`, which is absent exactly while the alert is
/// still happening; `PipelineId` is absent for an alert about the channel rather
/// than one of its pipelines.
pub fn channel_alert_output(record: &ChannelAlertRecord) -> ChannelAlert {
    ChannelAlert {
        alert_type: Some(record.alert_type.clone()),
        cleared_timestamp: record.cleared_at.map(super::timestamp),
        id: Some(record.id.clone()),
        message: record.message.clone(),
        pipeline_id: record.pipeline_id.map(|pipeline| pipeline.to_string()),
        set_timestamp: Some(super::timestamp(record.set_at)),
        state: Some(record.state().to_string()),
    }
}
