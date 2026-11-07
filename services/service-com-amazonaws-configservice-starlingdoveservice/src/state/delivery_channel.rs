// SPDX-License-Identifier: CC0-1.0

//! One stored delivery channel: the channel as it was put, the configuration-snapshot
//! delivery last scheduled through it and what became of that delivery, with the status
//! `DescribeDeliveryChannelStatus` reports it all through.

use super::*;

/// What became of a configuration-snapshot delivery Config PERFORMED: the snapshot
/// object was written into the channel's Amazon S3 bucket, or the write did not happen.
///
/// This is the only place a delivery's outcome is reported — `DeliverConfigSnapshot`
/// models no error for the delivery itself — so a failed delivery carries the code and
/// the message the bucket answered, which `lastErrorCode` and `lastErrorMessage` report.
#[derive(Debug, Clone)]
pub enum SnapshotDeliveryOutcome {
    /// The snapshot object was written at `at`, under `key` in the channel's bucket.
    Delivered {
        /// See [`SnapshotDeliveryOutcome::Delivered`].
        at: SystemTime,
        /// See [`SnapshotDeliveryOutcome::Delivered`].
        key: String,
    },
    /// The write was refused, or could not be made at all, at `at`.
    Failed {
        /// See [`SnapshotDeliveryOutcome::Failed`].
        at: SystemTime,
        /// See [`SnapshotDeliveryOutcome::Failed`].
        code: String,
        /// See [`SnapshotDeliveryOutcome::Failed`].
        message: String,
    },
}

impl SnapshotDeliveryOutcome {
    /// When the delivery reached this outcome, which is when it stopped being in flight.
    pub fn at(&self) -> SystemTime {
        match self {
            Self::Delivered { at, .. } | Self::Failed { at, .. } => *at,
        }
    }

    /// The `lastStatus` this outcome is reported as.
    pub fn last_status(&self) -> &'static str {
        match self {
            Self::Delivered { .. } => DELIVERY_STATUS_SUCCESS,
            Self::Failed { .. } => DELIVERY_STATUS_FAILURE,
        }
    }

    /// When the snapshot object was written, for the delivery that wrote one — the
    /// instant `lastSuccessfulTime` reports.
    pub fn delivered_at(&self) -> Option<SystemTime> {
        match self {
            Self::Delivered { at, .. } => Some(*at),
            Self::Failed { .. } => None,
        }
    }

    /// The error code and message of the delivery that failed, which only a failed one
    /// has.
    pub fn error(&self) -> Option<(&str, &str)> {
        match self {
            Self::Delivered { .. } => None,
            Self::Failed { code, message, .. } => Some((code, message)),
        }
    }
}

/// One configuration-snapshot delivery `DeliverConfigSnapshot` scheduled for a
/// delivery channel.
///
/// The delivery is NOT performed when the call returns (see
/// [`SNAPSHOT_DELIVERY_SETTLE_AFTER`]), so the entry stores the instant the delivery was
/// attempted, the instant the work falls due, and — once that work has run — what became
/// of it. Nothing sleeps for it inside a read and no read moves it: the status is derived
/// from these three.
#[derive(Debug, Clone)]
pub struct SnapshotDeliveryEntry {
    /// The `configSnapshotId` the scheduling call answered with.
    pub id: String,
    /// When the delivery was scheduled, which is when it was attempted.
    pub attempted_at: SystemTime,
    /// When the delivery falls due — [`SNAPSHOT_DELIVERY_SETTLE_AFTER`] after it was
    /// attempted.
    pub completes_at: SystemTime,
    /// What became of the delivery, once it has been performed; `None` while it is still
    /// in flight.
    pub outcome: Option<SnapshotDeliveryOutcome>,
}

impl SnapshotDeliveryEntry {
    /// The delivery `DeliverConfigSnapshot` scheduled at `at`, carrying the snapshot
    /// id it answered with and no outcome yet.
    pub fn scheduled(id: String, at: SystemTime) -> Self {
        Self {
            id,
            attempted_at: at,
            completes_at: at + SNAPSHOT_DELIVERY_SETTLE_AFTER,
            outcome: None,
        }
    }

    /// Whether this delivery's work has fallen due as of `now`.
    pub fn due_by(&self, now: SystemTime) -> bool {
        now >= self.completes_at
    }
}

/// One persisted delivery channel.
#[derive(Debug, Clone)]
pub struct DeliveryChannelEntry {
    pub name: String,
    pub s3_bucket_name: Option<String>,
    pub s3_key_prefix: Option<String>,
    pub s3_kms_key_arn: Option<String>,
    pub sns_topic_arn: Option<String>,
    pub config_snapshot_delivery_properties: Option<ConfigSnapshotDeliveryProperties>,
    /// When this channel was created. It anchors the channel's periodic delivery
    /// schedule, so the first delivery of each kind falls one interval after it and
    /// `DescribeDeliveryChannelStatus` derives every reported instant from it. An
    /// update keeps the instant the channel was first created.
    pub created_at: SystemTime,
    /// The last configuration-snapshot delivery `DeliverConfigSnapshot` scheduled for
    /// this channel, if any.
    ///
    /// ONE entry, not a history: `ConfigExportDeliveryInfo` reports the LAST attempt
    /// and the last success, so an on-demand delivery supersedes the one before it and
    /// however many snapshots a caller schedules, this channel grows no further.
    pub last_snapshot_delivery: Option<SnapshotDeliveryEntry>,
}

impl DeliveryChannelEntry {
    /// The modelled `DeliveryChannel`, read back out of state.
    pub fn to_shape(&self) -> DeliveryChannel {
        DeliveryChannel {
            name: Some(self.name.clone()),
            s3_bucket_name: self.s3_bucket_name.clone(),
            s3_key_prefix: self.s3_key_prefix.clone(),
            s3_kms_key_arn: self.s3_kms_key_arn.clone(),
            sns_topic_arn: self.sns_topic_arn.clone(),
            config_snapshot_delivery_properties: self.config_snapshot_delivery_properties.clone(),
        }
    }

    /// The `deliveryFrequency` this channel delivers configuration snapshots at: the
    /// one its `ConfigSnapshotDeliveryProperties` names, else
    /// [`DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY`].
    pub fn snapshot_delivery_frequency(&self) -> &str {
        self.config_snapshot_delivery_properties
            .as_ref()
            .and_then(|properties| properties.delivery_frequency.as_deref())
            .unwrap_or(DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY)
    }

    /// The interval between two configuration-snapshot deliveries of this channel.
    ///
    /// A `deliveryFrequency` outside the published enum cannot reach state — the
    /// modelled constraint refuses it before a handler runs — so the default interval
    /// stands in for it rather than this returning no schedule at all.
    pub fn snapshot_delivery_interval(&self) -> Duration {
        delivery_frequency_interval(self.snapshot_delivery_frequency()).unwrap_or_else(|| {
            delivery_frequency_interval(DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY)
                .expect("the default snapshot delivery frequency is a published value")
        })
    }

    /// The status of this channel as of `now`: the deliveries a call has had it PERFORM,
    /// and the schedule its creation instant anchors. Reading it changes no state, so two
    /// callers reading at the same instant see the same thing.
    pub fn status_at(&self, now: SystemTime) -> DeliveryChannelStatus {
        DeliveryChannelStatus {
            name: Some(self.name.clone()),
            config_snapshot_delivery_info: Some(self.export_delivery_info(
                now,
                self.snapshot_delivery_interval(),
                self.last_snapshot_delivery.as_ref(),
            )),
            config_history_delivery_info: Some(self.export_delivery_info(
                now,
                CONFIG_HISTORY_DELIVERY_INTERVAL,
                // The configuration history has a schedule only: no operation of this
                // API delivers one on demand, and this emulator performs no scheduled
                // delivery, so the only member this reports is the next delivery time.
                None,
            )),
            config_stream_delivery_info: self.stream_delivery_info(),
        }
    }

    /// The delivery info of one export (the snapshot or the configuration history) as of
    /// `now`, for a schedule that delivers every `interval`.
    ///
    /// Every member but `nextDeliveryTime` reports a delivery that was PERFORMED: the
    /// attempt is the one a call made, and `lastStatus`, `lastSuccessfulTime`,
    /// `lastErrorCode` and `lastErrorMessage` come from the outcome of the write that
    /// delivery made ([`SnapshotDeliveryOutcome`]) and from nothing else. So a delivery
    /// still in flight contributes an attempt time and no status — `DeliveryStatus`
    /// publishes no in-progress value — and an export no delivery has been performed for
    /// reports no attempt, no status and no success, rather than a status for a delivery
    /// nothing made.
    ///
    /// `nextDeliveryTime` is the SCHEDULE: the first instant of the `interval` schedule
    /// strictly after `now`, measured from the last delivery that landed an object and
    /// from the channel's creation while none has — which is what makes the
    /// `deliveryFrequency` the bound on the next delivery time, since the instant it is
    /// measured from is at or before `now` ([`next_scheduled_after`]).
    ///
    /// `on_demand` is the delivery of this kind that a call last scheduled, which for the
    /// configuration history is always none.
    pub(super) fn export_delivery_info(
        &self,
        now: SystemTime,
        interval: Duration,
        on_demand: Option<&SnapshotDeliveryEntry>,
    ) -> ConfigExportDeliveryInfo {
        // What became of the delivery, once Config has performed it.
        let performed = on_demand.and_then(|delivery| delivery.outcome.as_ref());
        let last_success = performed.and_then(SnapshotDeliveryOutcome::delivered_at);
        ConfigExportDeliveryInfo {
            last_status: performed.map(|outcome| outcome.last_status().to_string()),
            last_error_code: performed
                .and_then(SnapshotDeliveryOutcome::error)
                .map(|(code, _)| code.to_string()),
            last_error_message: performed
                .and_then(SnapshotDeliveryOutcome::error)
                .map(|(_, message)| message.to_string()),
            last_attempt_time: on_demand.map(|delivery| timestamp(delivery.attempted_at)),
            last_successful_time: last_success.map(timestamp),
            next_delivery_time: Some(timestamp(next_scheduled_after(
                last_success.unwrap_or(self.created_at),
                interval,
                now,
            ))),
        }
    }

    /// The configuration-stream delivery info of this channel.
    ///
    /// A channel carrying no `snsTopicARN` has SNS delivery turned off, which
    /// `ConfigStreamDeliveryInfo$lastStatus` documents as `Not_Applicable`; that status
    /// was reached when the channel was created without a topic. With a topic
    /// configured the status is the one of the last notification Config streamed, and
    /// this emulator has streamed none, so there is nothing to report.
    pub(super) fn stream_delivery_info(&self) -> Option<ConfigStreamDeliveryInfo> {
        if self.sns_topic_arn.is_some() {
            return None;
        }
        Some(ConfigStreamDeliveryInfo {
            last_status: Some(DELIVERY_STATUS_NOT_APPLICABLE.to_string()),
            last_error_code: None,
            last_error_message: None,
            last_status_change_time: Some(timestamp(self.created_at)),
        })
    }
}

/// The first instant of the schedule `anchor` anchors, stepping by `interval`, that falls
/// strictly after `now`.
///
/// This is what bounds `nextDeliveryTime` by the `deliveryFrequency`: the answer is more
/// than `now` and at most one interval past it, because the instant it steps from is at or
/// before `now`. An interval of zero names no schedule, so one second stands in for it
/// rather than this looping.
fn next_scheduled_after(anchor: SystemTime, interval: Duration, now: SystemTime) -> SystemTime {
    let interval_secs = interval.as_secs().max(1);
    let elapsed = now
        .duration_since(anchor)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let steps = elapsed / interval_secs + 1;
    anchor + Duration::from_secs(steps * interval_secs)
}
