// SPDX-License-Identifier: CC0-1.0

//! The account's VDM metric series: what one metric counts, over which of the
//! account's records, for a window and a set of dimension values.
//!
//! One home for the reading, because two surfaces answer it and must not disagree: a
//! metric query (`BatchGetMetricData`) reports the series directly, and a metrics
//! export job writes the same series into its export file
//! ([`super::export_file`]). Both ask the same question — how many events of a metric
//! the account's records hold, per day, for the activity the dimensions select — so
//! the question is answered here once.
//!
//! Split out of the state module root so the records' one derived view lives beside
//! them; every item is re-exported from [`crate::state`], so a handler reaches it as
//! `crate::state::…`.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// `Metric`'s value for messages the account sent, the one metric the records here
/// can count: "SEND - Emails sent eligible for tracking in the VDM dashboard".
pub const METRIC_SEND: &str = "SEND";

/// `MetricDimensionName`'s value for "the email identity used when sending messages".
pub const DIMENSION_EMAIL_IDENTITY: &str = "EMAIL_IDENTITY";

/// `MetricDimensionName`'s value for "the configuration set used when sending
/// messages (if one was used)".
pub const DIMENSION_CONFIGURATION_SET: &str = "CONFIGURATION_SET";

/// `MetricDimensionName`'s value for "the name of the tenant used when sending
/// messages (if one was used)".
pub const DIMENSION_TENANT_NAME: &str = "TENANT_NAME";

/// The domain of the SES mailbox simulator, whose addresses (`success@`, `bounce@`,
/// `complaint@`, `ooto@`, `suppressionlist@`) SES answers itself without delivering
/// anything. `Metric` documents every metric as excluding mail sent to it, so a
/// message addressed there is not counted.
///
/// The one home of the name is the module that decides what those mailboxes answer, so
/// this surface and that one cannot drift.
const MAILBOX_SIMULATOR_DOMAIN: &str =
    crate::state::event_publication::outcome::MAILBOX_SIMULATOR_DOMAIN;

/// The span one data point covers: the VDM dashboard reports sending activity by
/// day, so a data point is one UTC day and its timestamp is the instant that day
/// starts. Seconds since the epoch divide into UTC days exactly — the epoch is
/// itself a UTC midnight — so the bucket is a division.
pub const DATA_POINT_SPAN_SECONDS: i64 = 24 * 60 * 60;

/// The series `metric` yields from the account's records for the window
/// `start..=end` (seconds since the epoch) and the dimension values `dimensions`, as
/// (the instant the day starts, count) pairs in ascending order of instant.
///
/// One record of the account's that a metric counts: the UTC day it falls in, the
/// message, and the single recipient the recipient-side dimensions are read against.
///
/// The records are what BOTH surfaces start from: a metric query filters them by the
/// one set of dimension values it names ([`metric_day_counts`]), and a metrics export
/// places each of them into the dimension combinations it belongs to — which is why
/// they are enumerated once per metric rather than once per combination.
pub struct CountedMessage<'a> {
    /// The instant the UTC day the message was accepted on starts, which is what one
    /// data point covers.
    pub day: i64,
    /// The accepted message.
    pub message: &'a super::SentMessage,
    /// Its single recipient.
    pub recipient: &'a str,
}

/// Every record of `store` the metric `metric` counts inside the window
/// `start..=end` (seconds since the epoch), before any dimension is applied.
pub fn metric_counted<'a>(
    store: &'a super::ComAmazonawsSesv2SimpleemailserviceV2State,
    metric: &str,
    start: f64,
    end: f64,
) -> Vec<CountedMessage<'a>> {
    // Every metric other than `SEND` counts an event in the message's life after
    // SES accepted it — a delivery, a bounce, a complaint, an open, a click.
    // Nothing here delivers a message or observes a recipient reading one, so
    // there is no such event to count and the records are none.
    if metric != METRIC_SEND {
        return Vec::new();
    }
    let mut counted = Vec::new();
    for message in &store.sent_messages {
        let Ok(since_epoch) = message.sent_at.duration_since(SystemTime::UNIX_EPOCH) else {
            continue;
        };
        let at = since_epoch.as_secs_f64();
        if at < start || at > end {
            continue;
        }
        // "This excludes … emails addressed to more than one recipient", so only
        // a message with exactly one recipient is eligible, and that recipient is
        // the one the recipient-side dimensions are read against.
        let Some(recipient) = sole_recipient(message) else {
            continue;
        };
        // "This excludes emails sent to the mailbox simulator".
        if recipient_domain(recipient) == MAILBOX_SIMULATOR_DOMAIN {
            continue;
        }
        let seconds = since_epoch.as_secs() as i64;
        counted.push(CountedMessage {
            day: seconds - seconds.rem_euclid(DATA_POINT_SPAN_SECONDS),
            message,
            recipient,
        });
    }
    counted
}

/// Whether the dimension value `value` of the dimension `name` keeps `counted`.
///
/// A dimension states which of the recorded activity a metric is wanted for, so it is
/// a filter over the records and not a lookup of the resource it names.
pub fn dimension_keeps(
    store: &super::ComAmazonawsSesv2SimpleemailserviceV2State,
    name: &str,
    value: &str,
    counted: &CountedMessage<'_>,
) -> bool {
    Dimension::of(name, value).keeps(store, counted.message, counted.recipient)
}

/// A span the records hold nothing for is not a data point: the account collected
/// no metric for it, which is what an absent point says.
pub fn metric_day_counts(
    store: &super::ComAmazonawsSesv2SimpleemailserviceV2State,
    metric: &str,
    dimensions: &BTreeMap<String, String>,
    start: f64,
    end: f64,
) -> Vec<(i64, i64)> {
    let filters = dimension_filters(dimensions);
    let mut counts: BTreeMap<i64, i64> = BTreeMap::new();
    for counted in metric_counted(store, metric, start, end) {
        if !filters
            .iter()
            .all(|filter| filter.keeps(store, counted.message, counted.recipient))
        {
            continue;
        }
        *counts.entry(counted.day).or_default() += 1;
    }
    counts.into_iter().collect()
}

/// The dimension values `dimensions` as the filters over the records they are.
///
/// A dimension states which of the recorded activity a metric is wanted for, so
/// it is a filter over the records and not a lookup of the resource it names: a
/// value nothing in the records carries — a configuration set or an identity the
/// account does not hold, a provider it never sent to — keeps no message, which
/// is what makes such a query report no data rather than be refused.
fn dimension_filters(dimensions: &BTreeMap<String, String>) -> Vec<Dimension> {
    dimensions
        .iter()
        .map(|(name, value)| Dimension::of(name, value))
        .collect()
}

/// One dimension value, as the filter over the records it is.
enum Dimension {
    /// `EMAIL_IDENTITY`: the identity key (see `crate::state::identity_key`) of the
    /// identity the value names.
    EmailIdentity(String),
    /// `CONFIGURATION_SET`: the name of a configuration set.
    ConfigurationSet(String),
    /// `ISP`: the recipient's mailbox provider.
    Isp(String),
    /// `TENANT_NAME`: a tenant of the account.
    Tenant,
}

impl Dimension {
    /// The filter the dimension value `value` of the dimension `name` is.
    fn of(name: &str, value: &str) -> Self {
        match name {
            // An identity is compared through the key the identity table is
            // keyed by, so the domain part of a value matches the way DNS
            // compares it.
            DIMENSION_EMAIL_IDENTITY => Self::EmailIdentity(crate::state::identity_key(value)),
            DIMENSION_CONFIGURATION_SET => Self::ConfigurationSet(value.to_string()),
            DIMENSION_TENANT_NAME => Self::Tenant,
            // The one remaining name is `ISP`, "the recipient ISP (e.g. Gmail,
            // Yahoo, etc.)" — the keys are checked against the model's value set
            // before a query reaches here.
            _ => Self::Isp(value.to_string()),
        }
    }

    /// Whether this dimension keeps `message`, whose single recipient is `recipient`.
    fn keeps(
        &self,
        store: &super::ComAmazonawsSesv2SimpleemailserviceV2State,
        message: &super::SentMessage,
        recipient: &str,
    ) -> bool {
        match self {
            // "The email identity used when sending messages": the identity a message
            // was sent under is the one that authorised its `From` address — the
            // address identity itself, or the domain identity that covers it — so a
            // value naming no identity the account holds keeps no message.
            Self::EmailIdentity(key) => message
                .from
                .as_deref()
                .and_then(|from| store.sending_identity(from))
                .is_some_and(|used| &crate::state::identity_key(&used.name) == key),
            // "The configuration set used when sending messages (if one was used)".
            Self::ConfigurationSet(name) => {
                message.configuration_set_name.as_deref() == Some(name.as_str())
            }
            Self::Isp(isp) => serves_recipient(isp, recipient),
            // "The name of the tenant used when sending messages (if one was used)":
            // no message in the log was sent through a tenant, because the send
            // operations answer a request carrying a `TenantName` as unwritten, so no
            // tenant accounts for any message.
            Self::Tenant => false,
        }
    }
}

/// The only recipient of `message`, or `None` when it was addressed to none or to
/// more than one.
pub fn sole_recipient(message: &super::SentMessage) -> Option<&String> {
    let mut recipients = message.recipients();
    let only = recipients.next()?;
    recipients.next().is_none().then_some(only)
}

/// The mail domain of `address`, lower-cased, or `""` when the value carries none.
///
/// The value is an address as the send request gave it, so it may be a display-name
/// form (`Name <user@example.com>`); the mailbox is what names the provider. DNS
/// compares domain labels without regard to case.
pub fn recipient_domain(address: &str) -> String {
    crate::state::addr_spec(address)
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Whether the ISP named by an `ISP` dimension is the one that hosts `recipient`.
///
/// The provider that hosts a mailbox is the one its mail domain belongs to, and the
/// documented examples of the dimension name a provider without its public suffix
/// (`Gmail` for `gmail.com`), so either form of the name identifies it. Both sides
/// compare without regard to case.
pub fn serves_recipient(isp: &str, recipient: &str) -> bool {
    let domain = recipient_domain(recipient);
    let isp = isp.trim().to_ascii_lowercase();
    if isp.is_empty() {
        return false;
    }
    domain == isp || domain.split_once('.').is_some_and(|(label, _)| label == isp)
}
