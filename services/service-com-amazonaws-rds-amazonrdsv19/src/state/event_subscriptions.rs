// SPDX-License-Identifier: CC0-1.0

use super::*;

/// A stored RDS event notification subscription: the record `DescribeEventSubscriptions`
/// reports.
///
/// Regional, like the DB instances and snapshots above: a subscription is created in one
/// region and `DescribeEventSubscriptions` lists that region's subscriptions.
#[derive(Debug, Clone)]
pub struct EventSubscriptionRecord {
    /// The caller's `SubscriptionName`, and the second half of this record's key.
    pub subscription_name: String,
    /// The region the subscription was created in; the scope of its ARN.
    pub region: String,
    /// `SnsTopicArn`: the topic RDS publishes the events to.
    pub sns_topic_arn: String,
    /// `Status`: "Can be one of the following: creating | modifying | deleting | active |
    /// no-permission | topic-not-exist" (`EventSubscription$Status`).
    pub status: String,
    /// `SubscriptionCreationTime`, which the model types as a STRING rather than a
    /// timestamp, so the record carries the rendered form its writer chose.
    pub subscription_creation_time: String,
    /// `SourceType`, absent when the subscription covers every source type.
    pub source_type: Option<String>,
    /// `SourceIdsList`.
    pub source_ids: Vec<String>,
    /// `EventCategoriesList`.
    pub event_categories: Vec<String>,
    /// `Enabled`.
    pub enabled: bool,
    /// The status this record owes, and when; see [`PendingStatus`]. A subscription is
    /// `active` from its create onwards and only a modify puts it in a transient status,
    /// so this is `None` for a subscription nothing has changed.
    pub pending_status: Option<PendingStatus>,
    /// The tags `CreateEventSubscription$Tags` assigned to the subscription. The modelled
    /// `EventSubscription` shape carries no tag member, so nothing reports them yet —
    /// `ListTagsForResource`, which does, is still scaffolded — but the caller's input is
    /// kept rather than dropped, so it is here when that operation lands.
    pub tags: Vec<Tag>,
}

impl EventSubscriptionRecord {
    /// A subscription as `CreateEventSubscription` mints it: RDS answers the create with
    /// the subscription already [`EVENT_SUBSCRIPTION_STATUS_ACTIVE`], so there is no
    /// transient status to owe and nothing for the clock to settle later.
    #[allow(clippy::too_many_arguments)]
    pub fn active(
        subscription_name: String,
        region: String,
        sns_topic_arn: String,
        subscription_creation_time: String,
        source_type: Option<String>,
        source_ids: Vec<String>,
        event_categories: Vec<String>,
        enabled: bool,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            subscription_name,
            region,
            sns_topic_arn,
            status: EVENT_SUBSCRIPTION_STATUS_ACTIVE.to_string(),
            subscription_creation_time,
            source_type,
            source_ids,
            event_categories,
            enabled,
            pending_status: None,
            tags,
        }
    }

    /// Accept a modify: the subscription reports [`EVENT_SUBSCRIPTION_STATUS_MODIFYING`]
    /// while the change is applied, and the clock owes it
    /// [`EVENT_SUBSCRIPTION_STATUS_ACTIVE`] [`STATUS_TRANSITION_DELAY`] later — so a
    /// caller reading straight after the modify sees the transient status the real
    /// service reports.
    pub fn begin_modifying(&mut self) {
        self.status = EVENT_SUBSCRIPTION_STATUS_MODIFYING.to_string();
        self.pending_status = Some(PendingStatus::in_a_moment(
            EVENT_SUBSCRIPTION_STATUS_ACTIVE,
        ));
    }

    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// Accept a delete: the subscription's own answer reports the `deleting` status that
    /// says the delete was accepted. A notification rule has nothing provisioned to tear
    /// down, so the record leaves the store with that call rather than a span later — see
    /// `DeleteEventSubscription`.
    pub fn deleting(&self) -> Self {
        Self {
            status: EVENT_SUBSCRIPTION_STATUS_DELETING.to_string(),
            ..self.clone()
        }
    }

    /// `arn:aws:rds:<region>:<account>:es:<subscription name>` — the `es` resource type
    /// RDS gives an event subscription.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("es:{}", self.subscription_name),
        )
    }

    /// Project the stored record onto the modelled `EventSubscription` output shape.
    pub fn to_wire(&self) -> EventSubscription {
        EventSubscription {
            customer_aws_id: Some(emulator_server::emulator_account_id().to_string()),
            cust_subscription_id: Some(self.subscription_name.clone()),
            sns_topic_arn: Some(self.sns_topic_arn.clone()),
            status: Some(self.status.clone()),
            subscription_creation_time: Some(self.subscription_creation_time.clone()),
            source_type: self.source_type.clone(),
            source_ids_list: Some(self.source_ids.clone()),
            event_categories_list: Some(self.event_categories.clone()),
            enabled: Some(self.enabled),
            event_subscription_arn: Some(self.arn()),
        }
    }
}
