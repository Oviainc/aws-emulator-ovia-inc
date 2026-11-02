// SPDX-License-Identifier: CC0-1.0

//! The budget ALERT a notification in alarm owes the subscribers held with it.
//!
//! A notification exists to alert someone: a `Subscriber` is "the subscriber to a
//! budget notification … either an Amazon SNS topic or an email address",
//! `Subscriber$Address` is "the address that Amazon Web Services sends budget
//! notifications to", and `Notification$NotificationState` "specifies whether this
//! notification is in alarm. If a budget notification is in the `ALARM` state, you
//! passed the set threshold for the budget" — so a notification the service holds in
//! that state has an alert owed to every subscriber attached to it, and storing the
//! subscribers without ever contacting one leaves the notification pointless.
//!
//! What is delivered is the half a client can observe. An `SNS` subscriber's address
//! names a topic, so the alert is one `sns:Publish` onto that topic, made the way every
//! emulated service calls another: over the IN-PROCESS callback into this same emulator
//! ([`smithy_server_runtime::OperationContext::callback`]). Nothing here opens a socket
//! or leaves the host — the address a caller supplied is not a URL this code dials, it
//! is a topic the request is routed to INSIDE this process, so the topic has to be one
//! this emulator itself holds or SNS answers that it does not exist. An `EMAIL`
//! subscriber's address is a mailbox no emulated service hosts, so there is nothing to
//! deliver to and nothing is attempted for it.
//!
//! The work splits in two so that no delivery is made while the store is locked:
//! [`owed`] decides which alerts a write leaves owed (under the lock, from the state it
//! wrote) and [`deliver`] publishes them (after the lock is released). An alert is owed
//! only on a topic of the account whose budget it names ([`publishable`]) — which is the
//! same decision the write that establishes such a subscriber is held to, so a budget's
//! figures cannot be deposited on another account's topic and no address that write
//! accepted is skipped here. The number of
//! alerts one call can owe is bounded by what a budget can hold — one `SNS` subscriber
//! per notification ([`super::MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION`]) and at most
//! [`super::MAX_NOTIFICATIONS_PER_BUDGET`] notifications — so a request cannot grow the
//! fan-out.
//!
//! The message is this emulator's own rendering of the alert ([`message`]): the model
//! says a notification is sent and what it is about, not what the published text looks
//! like, so every member is read off the stored budget and notification rather than
//! copied from any one recorded alert, and a client that parses the published AWS text
//! should not expect to match it byte for byte.

use crate::ops::types::{Budget, Notification, Subscriber};

// Amazon SNS's own namespace: the vendor field an `SNS` subscriber's address has to
// carry for that address to name a topic at all (which is what
// `super::subscriber_address_violation` holds every stored subscriber to), and the
// prefix SNS is addressed by in a Region. Defined once beside the store, so the
// validation that admits an address and the publish that uses it read the same vendor.
use super::SNS_NAMESPACE;

/// The budget an alert is published FOR, copied out of the store so the alert can be
/// decided, built and published once the store's lock is released.
///
/// It carries the account the budget belongs to, and the budget as a READ reports it
/// (`crate::state::budget_as_read`): the name and the account identify it to whoever
/// receives the alert, `Budget$BudgetType` and `Budget$TimeUnit` say what it tracks and
/// over what period, and the `BudgetLimit` and `CalculatedSpend` it reads back with are
/// what decides whether a notification of it is in alarm at all
/// ([`super::notification_state`]).
#[derive(Debug, Clone)]
pub struct AlertSource {
    account_id: String,
    budget: Budget,
}

/// The budget `budget` of account `account_id`, as an alert for it names it. `budget` is
/// the budget as a read reports it, so an alert cannot be decided from figures that
/// disagree with `DescribeBudget`.
pub fn source(account_id: &str, budget: &Budget) -> AlertSource {
    AlertSource {
        account_id: account_id.to_string(),
        budget: budget.clone(),
    }
}

/// One alert a write owes: the topic it is published to, and the message published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetAlert {
    /// The topic an `SNS` subscriber's address named.
    pub topic_arn: String,
    /// The alert, as [`message`] renders it.
    pub message: String,
}

/// The alerts a write owes, having left `notification` holding `subscribers`.
///
/// Whether the notification is in alarm is the SERVICE's own reading of the spend behind
/// the budget against the threshold the notification alerts on
/// ([`super::notification_state`]), never a state a request asked for.
///
/// Empty for a notification that is not in alarm: nothing has passed its threshold, so
/// no subscriber of it is owed anything. Empty, too, for a notification in alarm whose
/// subscribers are all addresses no alert of THIS budget can be published to
/// ([`publishable`]) — an email mailbox, the one address form nothing is delivered to.
pub fn owed(
    source: &AlertSource,
    notification: &Notification,
    subscribers: &[Subscriber],
) -> Vec<BudgetAlert> {
    if super::notification_state(&source.budget, notification) != super::NOTIFICATION_STATE_ALARM {
        return Vec::new();
    }
    let message = message(source, notification);
    subscribers
        .iter()
        .filter(|subscriber| subscriber.subscription_type == super::SUBSCRIPTION_TYPE_SNS)
        .filter(|subscriber| publishable(source, &subscriber.address))
        .map(|subscriber| BudgetAlert {
            topic_arn: subscriber.address.clone(),
            message: message.clone(),
        })
        .collect()
}

/// Whether an alert for a budget of the account `source` names can be published to the
/// topic `address` names.
///
/// Decided by the ONE decision that says which topic a notification of an account is
/// published to ([`super::publishable_topic`], over the shared ARN parse
/// `service_common::arn`) — the SAME decision the write that established the subscriber
/// was held to ([`super::subscriber_address_violation`]). A stored `SNS` subscriber
/// therefore cannot fail it: an address no alert of this budget could be published to —
/// one naming no topic, or a topic of another account — is refused before it is stored,
/// so nothing is dropped here that a read reports.
fn publishable(source: &AlertSource, address: &str) -> bool {
    super::publishable_topic(&source.account_id, address).is_ok()
}

/// The alert published for `notification` of the budget `source` names: the budget it
/// is about, what that budget tracks and over what period, and what the notification
/// alerts on — the spend it watches, the comparison it makes, the threshold that was
/// passed and how that threshold is read.
///
/// Every value is the one the budget and the notification hold, so a receiver can tell
/// which budget and which of its notifications alerted, and against which limit. An
/// absent `ThresholdType` is rendered as the [`super::THRESHOLD_TYPE_PERCENTAGE`] it
/// means ("thresholds are always a percentage"), so a notification that named none and
/// one that named `PERCENTAGE` alert alike.
fn message(source: &AlertSource, notification: &Notification) -> String {
    let mut alert = ::serde_json::json!({
        "accountId": source.account_id,
        "budgetName": source.budget.budget_name,
        "budgetType": source.budget.budget_type,
        "timeUnit": source.budget.time_unit,
        "notificationType": notification.notification_type,
        "comparisonOperator": notification.comparison_operator,
        "threshold": notification.threshold,
        "thresholdType": notification
            .threshold_type
            .as_deref()
            .unwrap_or(super::THRESHOLD_TYPE_PERCENTAGE),
        "notificationState": super::NOTIFICATION_STATE_ALARM,
    });
    // A budget that holds no `BudgetLimit` has no limit to name, so the member is left
    // out rather than rendered as a null.
    if let Some(limit) = &source.budget.budget_limit {
        alert["budgetLimit"] = ::serde_json::json!({
            "amount": limit.amount,
            "unit": limit.unit,
        });
    }
    alert.to_string()
}

/// Publish every alert in `alerts` to the topic it names.
///
/// A publish that the topic's own service refuses — most often a topic that does not
/// exist — is not the budget operation's failure: AWS does not refuse a notification
/// write because an alert could not be delivered, so the refusal is logged (without the
/// subscriber's address, which `Subscriber$Address` marks sensitive) and the remaining
/// alerts are still published.
///
/// The client is built once per Region and reused, so a set of alerts spanning one
/// Region builds one client.
pub async fn deliver(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    alerts: &[BudgetAlert],
) {
    let mut client: Option<(String, ::aws_sdk_sns::Client)> = None;
    for alert in alerts {
        // Re-read from the ARN rather than carried alongside it, so the Region this
        // publish is addressed in is always one that came out of the ARN grammar.
        let Some(region) = topic_region(&alert.topic_arn) else {
            continue;
        };
        if client
            .as_ref()
            .is_none_or(|(held, _)| held.as_str() != region)
        {
            client = Some((region.to_string(), sns_client(ctx, region)));
        }
        let Some((_, sns)) = client.as_ref() else {
            continue;
        };
        let published = sns
            .publish()
            .topic_arn(&alert.topic_arn)
            .message(&alert.message)
            .send()
            .await;
        if let Err(error) = published {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a budget alert could not be published to a subscriber's topic"
            );
        }
    }
}

/// The Region of the SNS topic `address` names, or `None` when it names no SNS topic.
///
/// Read through the ONE reading of an `SNS` subscriber's address ([`super::sns_topic`]),
/// which answers a Region only for a label inside the grammar every model states for one,
/// so a caller-supplied address cannot put anything else where a Region goes.
fn topic_region(address: &str) -> Option<&str> {
    super::sns_topic(address).map(|topic| topic.region)
}

/// An Amazon SNS client over the IN-PROCESS callback into this same emulator.
///
/// Nothing here opens a socket or leaves the host: `ctx.callback()` is the framework's
/// seam for calling another emulated service, and the `http_client` it hands over
/// dispatches the request inside this process to whichever mounted service claims it
/// (the SNS emulator, on the request's own `sns` credential scope). The `endpoint_url`
/// below is therefore an ADDRESS the in-process router reads, never a host any resolver
/// looks up, and `region` is the label [`topic_region`] read out of the topic's ARN, so
/// a caller cannot steer that authority.
///
/// The credentials are the emulator's own root credentials, which only sign this
/// operation's in-process calls; they are never put in a response or a log.
fn sns_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{SNS_NAMESPACE}.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}
