// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sns-amazonsimplenotificationservice service.
//!
//! The records here are the SNS control plane: topics, their subscriptions, and the
//! messages `Publish` delivered to each subscription. They are deliberately named
//! `*Record` so a handler cannot confuse them with the wire shapes of the same name
//! in [`crate::ops::types`] (`Topic`, `Subscription`).

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::MessageAttributeValue;

/// The region an ARN carries when the request names none.
///
/// Every emulator writes `region = us-east-1` into the shared AWS config it hands a
/// client (`vendor/server/src/lib.rs`), so that is the region an unsigned request —
/// one carrying no SigV4 credential scope to read a region from — is addressing.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many items one page of a paginated SNS list holds.
///
/// SNS documents a page of "up to 100" for `ListTopics` and
/// `ListSubscriptionsByTopic`; the token is opaque to the caller either way.
pub const PAGE_SIZE: usize = 100;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores (topics, subscriptions).
///
/// This is NOT AWS's service quota — SNS publishes 100,000 topics and 12.5M
/// subscriptions per account, and nothing here claims to reproduce them. It is one of
/// the two axes of the emulator's own memory ceiling: every successful `CreateTopic`
/// and `Subscribe` inserts a record that lives until a caller deletes it, so without a
/// ceiling a caller could grow the process until the host runs out of memory. A
/// request that would cross it is refused with `TopicLimitExceededException` /
/// `SubscriptionLimitExceededException`, the modelled errors AWS itself uses for
/// "the customer already owns the maximum allowed number", so the refusal is one a
/// real client already handles.
///
/// It bounds the record COUNT; [`MAX_ATTRIBUTE_BYTES_PER_RECORD`] and
/// [`MAX_TAG_BYTES_PER_TOPIC`] bound the bytes ONE record can accumulate, which is
/// the other axis — a record whose attribute map grew without limit would defeat this
/// count on its own.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The attribute names SNS defines for a topic, from the `CreateTopic` /
/// `SetTopicAttributes` / `GetTopicAttributes` docs (the delivery-status logging
/// attributes come from `SetTopicAttributes`). SNS refuses any other name, and so
/// does this emulator: the closed set is also what stops a caller inventing fresh
/// attribute names to grow one topic record for ever.
pub const TOPIC_ATTRIBUTE_NAMES: &[&str] = &[
    "ApplicationFailureFeedbackRoleArn",
    "ApplicationSuccessFeedbackRoleArn",
    "ApplicationSuccessFeedbackSampleRate",
    "ArchivePolicy",
    "ContentBasedDeduplication",
    "DeliveryPolicy",
    "DisplayName",
    "FifoThroughputScope",
    "FifoTopic",
    "FirehoseFailureFeedbackRoleArn",
    "FirehoseSuccessFeedbackRoleArn",
    "FirehoseSuccessFeedbackSampleRate",
    "HTTPFailureFeedbackRoleArn",
    "HTTPSuccessFeedbackRoleArn",
    "HTTPSuccessFeedbackSampleRate",
    "KmsMasterKeyId",
    "LambdaFailureFeedbackRoleArn",
    "LambdaSuccessFeedbackRoleArn",
    "LambdaSuccessFeedbackSampleRate",
    "MaximumMessageSize",
    "Policy",
    "SQSFailureFeedbackRoleArn",
    "SQSSuccessFeedbackRoleArn",
    "SQSSuccessFeedbackSampleRate",
    "SignatureVersion",
    "TracingConfig",
];

/// What `Subscribe` answers in place of the ARN of a subscription that is not
/// confirmed yet: "the string \"pending confirmation\" if the subscription requires
/// confirmation" (`SubscribeResponse$SubscriptionArn` doc), unless
/// `ReturnSubscriptionArn` asks for the ARN in all cases.
pub const PENDING_CONFIRMATION_REPORTED: &str = "pending confirmation";

/// What a subscription LISTING carries in `Subscription$SubscriptionArn` while the
/// subscription is not confirmed yet.
///
/// "A subscription becomes confirmed when the subscriber calls the
/// ConfirmSubscription action" (`SubscribeInput$ReturnSubscriptionArn` doc), and until
/// then SNS publishes no ARN for it: a listing carries this sentinel in the ARN's
/// place. It is the same state as [`PENDING_CONFIRMATION_REPORTED`] but not the same
/// spelling — `Subscribe`'s own doc fixes the wording of its response, and the
/// recorded wire responses of `ListSubscriptionsByTopic` fix this one.
pub const PENDING_CONFIRMATION_LISTED: &str = "PendingConfirmation";

/// Whether `value` is one of the sentinels SNS publishes in place of an unconfirmed
/// subscription's ARN ([`PENDING_CONFIRMATION_REPORTED`],
/// [`PENDING_CONFIRMATION_LISTED`]) rather than an ARN — so a caller that read one out
/// of a `Subscribe` or a listing response and handed it back names a subscription that
/// is still pending, not a subscription that does not exist.
pub fn is_pending_confirmation_sentinel(value: &str) -> bool {
    value == PENDING_CONFIRMATION_REPORTED || value == PENDING_CONFIRMATION_LISTED
}

/// The attribute names SNS defines for a subscription, from the `Subscribe` /
/// `SetSubscriptionAttributes` docs. Closed for the same two reasons as
/// [`TOPIC_ATTRIBUTE_NAMES`].
pub const SUBSCRIPTION_ATTRIBUTE_NAMES: &[&str] = &[
    "DeliveryPolicy",
    "FilterPolicy",
    "FilterPolicyScope",
    "RawMessageDelivery",
    "RedrivePolicy",
    "ReplayPolicy",
    "SubscriptionRoleArn",
];

/// The total attribute bytes (names plus values) ONE topic or subscription record may
/// hold.
///
/// SNS's own documented attribute values are policy documents of at most 30 KB, so a
/// record of the closed attribute set has no legitimate need of more than this; the
/// ceiling is what stops a caller growing a single record without limit by repeating
/// a create or subscribe with ever larger values. A request that would cross it is
/// refused with `InvalidParameterException`.
pub const MAX_ATTRIBUTE_BYTES_PER_RECORD: usize = 64 * 1024;

/// The total tag bytes (keys plus values) one topic may hold — the same ceiling as
/// [`MAX_ATTRIBUTE_BYTES_PER_RECORD`] applied to the other caller-sized map on a
/// topic record, whose entry COUNT is capped at 50 by SNS's own documented tag limit.
pub const MAX_TAG_BYTES_PER_TOPIC: usize = 8 * 1024;

/// "Length Constraints: Maximum length of 30,720"
/// (`CreateTopicInput$DataProtectionPolicy` doc).
pub const MAX_DATA_PROTECTION_POLICY_LEN: usize = 30_720;

/// The bytes an attribute (or tag) map costs: every name and every value, since a
/// caller supplies both.
pub fn attribute_bytes(attributes: &HashMap<String, String>) -> usize {
    attributes
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum()
}

/// Whether `name` is an attribute SNS defines for a topic.
pub fn is_topic_attribute(name: &str) -> bool {
    TOPIC_ATTRIBUTE_NAMES.contains(&name)
}

/// Whether `name` is an attribute SNS defines for a subscription.
pub fn is_subscription_attribute(name: &str) -> bool {
    SUBSCRIPTION_ATTRIBUTE_NAMES.contains(&name)
}

/// How many delivered messages [`ComAmazonawsSnsAmazonsimplenotificationserviceState::delivered`]
/// retains per subscription before dropping the oldest.
///
/// The delivery log is an emulator-side record of what `Publish` fanned out, not a
/// queue any operation drains, so it is bounded rather than kept: a caller that
/// publishes forever sees the most recent messages and no unbounded growth.
pub const MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION: usize = 100;

/// The total bytes of message payload the delivery log retains across every
/// subscription, oldest evicted first.
///
/// This is what bounds the PRODUCT a publish multiplies out — message size x
/// confirmed subscriptions x publish count — which the per-subscription count alone
/// does not: 8 MiB of retained payload holds 32 messages of the documented 256 KiB
/// maximum, however many subscriptions or publishes produced them.
pub const MAX_RETAINED_DELIVERY_BYTES: usize = 8 * 1024 * 1024;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with;
/// `store` names the records it holds, e.g. `"topics"`.
pub fn limit_exceeded_message(store: &str) -> String {
    format!(
        "The customer already owns the maximum allowed number of {store}: \
         this emulator holds at most {MAX_RECORDS_PER_STORE}"
    )
}

/// One topic, as `CreateTopic` minted it.
#[derive(Debug, Clone)]
pub struct TopicRecord {
    pub arn: String,
    pub name: String,
    /// The account id that owns the topic.
    pub owner: String,
    /// The caller-settable attributes (`DisplayName`, `DeliveryPolicy`, `Policy`, …).
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub data_protection_policy: Option<String>,
    /// A FIFO topic: its name ends with `.fifo`.
    pub is_fifo: bool,
    pub created_at: SystemTime,
    /// How many of this topic's subscriptions have been deleted — the
    /// `SubscriptionsDeleted` attribute, which outlives the subscription records.
    pub subscriptions_deleted: u64,
}

/// One subscription of one topic, as `Subscribe` minted it.
#[derive(Debug, Clone)]
pub struct SubscriptionRecord {
    pub arn: String,
    pub topic_arn: String,
    pub protocol: String,
    pub endpoint: Option<String>,
    pub owner: String,
    /// The caller-settable attributes (`FilterPolicy`, `RawMessageDelivery`, …).
    pub attributes: HashMap<String, String>,
    /// The subscriber has not confirmed yet, so SNS delivers nothing to it.
    pub pending_confirmation: bool,
    pub created_at: SystemTime,
}

/// One message `Publish` delivered to one subscription.
#[derive(Debug, Clone)]
pub struct PublishedMessage {
    pub message_id: String,
    pub topic_arn: String,
    pub message: String,
    pub subject: Option<String>,
    pub message_structure: Option<String>,
    pub message_attributes: HashMap<String, MessageAttributeValue>,
    pub message_group_id: Option<String>,
    pub message_deduplication_id: Option<String>,
    /// Set only for a FIFO topic, which is the only kind SNS sequences.
    pub sequence_number: Option<String>,
    pub published_at: SystemTime,
}

impl PublishedMessage {
    /// Every byte retaining this message costs, as [`MAX_RETAINED_DELIVERY_BYTES`]
    /// counts them: EVERY variable-length field of the stored record, since each one
    /// is either caller-supplied or derived from caller input, plus a flat allowance
    /// for the fixed-size parts. A field left out of this sum is a field a caller
    /// could grow without the ceiling noticing.
    pub fn retained_bytes(&self) -> usize {
        /// The fixed-size parts of the record (the timestamp, the `Option`/`String`
        /// headers, the map's own allocation) that no caller varies.
        const FIXED_OVERHEAD: usize = 256;

        let attributes: usize = self
            .message_attributes
            .iter()
            .map(|(name, value)| {
                name.len()
                    + value.data_type.len()
                    + value.string_value.as_ref().map_or(0, String::len)
                    + value.binary_value.as_ref().map_or(0, Vec::len)
            })
            .sum();
        FIXED_OVERHEAD
            + self.message_id.len()
            + self.topic_arn.len()
            + self.message.len()
            + self.subject.as_ref().map_or(0, String::len)
            + self.message_structure.as_ref().map_or(0, String::len)
            + self.message_group_id.as_ref().map_or(0, String::len)
            + self.message_deduplication_id.as_ref().map_or(0, String::len)
            + self.sequence_number.as_ref().map_or(0, String::len)
            + attributes
    }
}

/// Per-service state for com-amazonaws-sns-amazonsimplenotificationservice. A handler reaches it through [`service_state`].
///
/// Every store here is caller-driven and therefore BOUNDED: `topics` and
/// `subscriptions` at [`MAX_RECORDS_PER_STORE`] records each (a request that would
/// cross it is refused with the modelled limit error), and `delivered` at
/// [`MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION`] messages per subscription AND
/// [`MAX_RETAINED_DELIVERY_BYTES`] of payload overall, evicting oldest-first. So the
/// product a publish multiplies out — payload x subscriptions x publishes — is capped
/// at [`MAX_RETAINED_DELIVERY_BYTES`] however the caller shapes the traffic.
#[derive(Debug, Default)]
pub struct ComAmazonawsSnsAmazonsimplenotificationserviceState {
    /// Topics by ARN. At most [`MAX_RECORDS_PER_STORE`].
    pub topics: HashMap<String, TopicRecord>,
    /// Subscriptions by subscription ARN. At most [`MAX_RECORDS_PER_STORE`].
    pub subscriptions: HashMap<String, SubscriptionRecord>,
    /// Delivered messages by subscription ARN, oldest first. Bounded: write through
    /// [`Self::record_delivery`], never by hand.
    pub delivered: HashMap<String, Vec<PublishedMessage>>,
    /// The subscription each retained delivery belongs to, oldest delivery first:
    /// the eviction order of the whole log.
    delivery_order: VecDeque<String>,
    /// The payload bytes currently retained in `delivered`.
    retained_delivery_bytes: usize,
    /// Backs the `SequenceNumber` a FIFO topic's `Publish` returns.
    pub sequence_counter: u64,
    /// Backs [`Self::next_unique_id`]: the monotone part of every minted id.
    id_counter: u64,
}

impl ComAmazonawsSnsAmazonsimplenotificationserviceState {
    /// This topic's subscriptions, oldest first (ARN breaking a tie), so a list
    /// response and its pagination token are stable across calls.
    pub fn subscriptions_of(&self, topic_arn: &str) -> Vec<&SubscriptionRecord> {
        let mut found: Vec<&SubscriptionRecord> = self
            .subscriptions
            .values()
            .filter(|sub| sub.topic_arn == topic_arn)
            .collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        found
    }

    /// Every topic, oldest first (ARN breaking a tie) — see [`Self::subscriptions_of`].
    pub fn topics_in_order(&self) -> Vec<&TopicRecord> {
        let mut found: Vec<&TopicRecord> = self.topics.values().collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        found
    }

    /// `(SubscriptionsConfirmed, SubscriptionsPending)` for a topic, counted from the
    /// subscription records rather than from a stored counter.
    pub fn subscription_counts(&self, topic_arn: &str) -> (usize, usize) {
        let mut confirmed = 0;
        let mut pending = 0;
        for sub in self.subscriptions.values() {
            if sub.topic_arn != topic_arn {
                continue;
            }
            if sub.pending_confirmation {
                pending += 1;
            } else {
                confirmed += 1;
            }
        }
        (confirmed, pending)
    }

    /// The next `SequenceNumber`: strictly increasing, so a later publish to a FIFO
    /// topic always sorts after an earlier one.
    pub fn next_sequence_number(&mut self) -> String {
        self.sequence_counter = self.sequence_counter.saturating_add(1);
        // SNS documents a 128-bit, non-consecutive sequence number; a fixed-width
        // decimal keeps the string order the same as the numeric order.
        format!("{:020}", self.sequence_counter)
    }

    /// A fresh id in the 8-4-4-4-12 hexadecimal form SNS message ids and the last
    /// segment of a subscription ARN carry.
    ///
    /// Unique because this store's own monotone counter goes into it; unpredictable
    /// because the process's randomly seeded hasher does too; shaped like a version-4
    /// UUID because that is what SNS returns. A `uuid` crate would be the natural home
    /// for the formatting, and the first version of this change used one — but that
    /// dependency is the ONE edit that makes this change touch Cargo.lock, the single
    /// file every other fix also edits, and the resulting squash conflict kept this
    /// change from landing at all. The digest comes from `sha2`, which this crate
    /// already depends on.
    pub fn next_unique_id(&mut self) -> String {
        use std::hash::{BuildHasher, Hasher};

        self.id_counter = self.id_counter.saturating_add(1);
        let nanos = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(self.id_counter);
        let entropy = hasher.finish();

        let digest = Sha256::digest(format!("{}:{nanos}:{entropy}", self.id_counter));
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // The version (4) and variant (RFC 4122) nibbles, so the id is well formed and
        // not mistaken for a name-based or time-based one.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Record one delivery of `message` to `subscription_arn`, then bring the log back
    /// within [`MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION`] and
    /// [`MAX_RETAINED_DELIVERY_BYTES`] by dropping the oldest deliveries. This is the
    /// ONLY way to write `delivered`: writing the map directly loses the bound.
    pub fn record_delivery(&mut self, subscription_arn: &str, message: PublishedMessage) {
        self.retained_delivery_bytes = self
            .retained_delivery_bytes
            .saturating_add(message.retained_bytes());
        self.delivered
            .entry(subscription_arn.to_string())
            .or_default()
            .push(message);
        self.delivery_order
            .push_back(subscription_arn.to_string());

        while self
            .delivered
            .get(subscription_arn)
            .is_some_and(|held| held.len() > MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION)
        {
            self.drop_oldest_delivery(subscription_arn);
        }
        while self.retained_delivery_bytes > MAX_RETAINED_DELIVERY_BYTES {
            let Some(oldest) = self.delivery_order.front().cloned() else {
                break;
            };
            self.drop_oldest_delivery(&oldest);
        }
    }

    /// Forget every delivery to `subscription_arn` — the subscription is gone, so its
    /// log is too, and the bytes it held are released.
    pub fn forget_deliveries(&mut self, subscription_arn: &str) {
        if let Some(held) = self.delivered.remove(subscription_arn) {
            let released: usize = held.iter().map(PublishedMessage::retained_bytes).sum();
            self.retained_delivery_bytes =
                self.retained_delivery_bytes.saturating_sub(released);
        }
        self.delivery_order.retain(|arn| arn != subscription_arn);
    }

    /// Drop this subscription's oldest retained delivery, keeping the eviction order
    /// and the retained-byte count in step with it.
    ///
    /// It ALWAYS shortens `delivery_order` when that queue names this subscription,
    /// whatever shape `delivered` is in, so an eviction loop driven by the queue
    /// always terminates.
    fn drop_oldest_delivery(&mut self, subscription_arn: &str) {
        // The first occurrence of this subscription in the order IS its oldest
        // delivery: its own deliveries are queued oldest first.
        if let Some(position) = self
            .delivery_order
            .iter()
            .position(|arn| arn == subscription_arn)
        {
            self.delivery_order.remove(position);
        }
        let Some(held) = self.delivered.get_mut(subscription_arn) else {
            return;
        };
        if held.is_empty() {
            self.delivered.remove(subscription_arn);
            return;
        }
        let dropped = held.remove(0);
        if held.is_empty() {
            self.delivered.remove(subscription_arn);
        }
        self.retained_delivery_bytes = self
            .retained_delivery_bytes
            .saturating_sub(dropped.retained_bytes());
    }

    /// The payload bytes the delivery log currently retains, for a test or a diagnostic.
    pub fn retained_delivery_bytes(&self) -> usize {
        self.retained_delivery_bytes
    }
}

/// The region this request addresses: the one in its SigV4 credential scope, or
/// [`DEFAULT_REGION`] when the request carries no scope to read.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The account id every ARN and `Owner` field this emulator emits carries.
pub fn owner_account_id() -> String {
    ::emulator_server::emulator_account_id().to_string()
}

/// A topic's ARN: `arn:aws:sns:<region>:<account>:<name>`.
pub fn topic_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("sns", region, name)
}

/// A subscription's ARN: the topic's ARN plus a fresh, unique id segment minted by
/// [`ComAmazonawsSnsAmazonsimplenotificationserviceState::next_unique_id`].
pub fn subscription_arn(topic_arn: &str, id: &str) -> String {
    format!("{topic_arn}:{id}")
}

/// The suffix SNS requires on a FIFO topic's name: "For a FIFO (first-in-first-out)
/// topic, the name must end with the .fifo suffix" (`CreateTopicInput$Name` doc).
pub const FIFO_SUFFIX: &str = ".fifo";

/// The longest topic name SNS accepts: "must be between 1 and 256 characters long"
/// (`CreateTopicInput$Name` doc).
pub const MAX_TOPIC_NAME_LEN: usize = 256;

/// Whether `name` satisfies the name rule SNS documents for a topic: 1 to
/// [`MAX_TOPIC_NAME_LEN`] characters of ASCII letters, digits, `_` and `-`, with the
/// [`FIFO_SUFFIX`] of a FIFO topic the one exception.
///
/// Shared, because it is both what `CreateTopic` refuses a bad name with and what
/// tells a topic ARN apart from another SNS ARN (a platform endpoint's, say) whose
/// resource part is not a topic name.
pub fn topic_name_is_valid(name: &str) -> bool {
    let base = name.strip_suffix(FIFO_SUFFIX).unwrap_or(name);
    !base.is_empty()
        && name.len() <= MAX_TOPIC_NAME_LEN
        && base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The topic name an SNS topic ARN names, or `None` when `arn` is not one
/// (`arn:aws:sns:<region>:<account>:<name>` — six colon-separated fields whose last
/// one is a topic name, which is what tells it from an `…:endpoint/GCM/app/…` ARN).
pub fn topic_name_from_arn(arn: &str) -> Option<&str> {
    let fields: Vec<&str> = arn.split(':').collect();
    match fields[..] {
        ["arn", _partition, "sns", _region, _account, name] if topic_name_is_valid(name) => {
            Some(name)
        }
        _ => None,
    }
}

/// The resource part of `arn` when `arn` is an ARN of `service`, or `None` when it is
/// not an ARN at all or names another service.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account-id>:<resource>`, and the
/// resource is the ONLY field that may itself hold `:` separators (a Lambda
/// function's `function:<name>:<qualifier>`, say) — hence the split bounded at six
/// fields. Shared, because "is this string an ARN of that service?" is the question
/// every per-protocol endpoint rule asks.
pub fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    match fields[..] {
        ["arn", partition, found, region, account, resource]
            if found == service
                && !partition.is_empty()
                && !region.is_empty()
                && !account.is_empty()
                && !resource.is_empty() =>
        {
            Some(resource)
        }
        _ => None,
    }
}

/// One page of `items` starting at `token`, plus the token of the page after it.
///
/// `token` is the opaque `NextToken` a previous page returned — the index of the
/// first item of this page. `None` is returned for a token that is not one this
/// function produced, which each caller reports as its own modelled error.
pub fn paginate<T>(
    items: Vec<T>,
    token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<T>, Option<String>)> {
    let start: usize = match token {
        Some(token) => token.parse().ok()?,
        None => 0,
    };
    if start > items.len() {
        return None;
    }
    let end = (start + page_size).min(items.len());
    let next = (end < items.len()).then(|| end.to_string());
    Some((items.into_iter().skip(start).take(end - start).collect(), next))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSnsAmazonsimplenotificationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSnsAmazonsimplenotificationserviceState>)
}
