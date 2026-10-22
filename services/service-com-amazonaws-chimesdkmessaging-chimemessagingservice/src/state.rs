// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkmessaging-chimemessagingservice service.
//!
//! The channel-messaging slice keeps every channel this service minted, keyed by its
//! ARN, and the messages sent to each one inside that channel's record — so deleting a
//! channel drops its messages with it and no second map can disagree about either.
//!
//! Timestamps are held as epoch MILLISECONDS rather than as
//! [`aws_smithy_types::DateTime`], because the list operations have to order and range
//! filter them and `DateTime` carries no total order. [`date_time`] converts one back
//! for the wire and [`millis_of`] converts an inbound filter bound in.

use std::collections::{HashMap, VecDeque};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Channel, ChannelMessage, ChannelMessageStatusStructure, ChannelMessageSummary, ChannelSummary,
    ElasticChannelConfiguration, ExpirationSettings, Identity, MessageAttributeValue, Tag, Target,
};

/// How many `CreateChannel` idempotency records this emulator keeps.
///
/// The records are the emulator's own bookkeeping rather than an emulated resource, and
/// no operation in the API deletes one, so the map needs a bound of its own or a caller
/// could grow it without limit: past this many records the OLDEST is dropped. The worst
/// consequence is that a replay of a very old `CreateChannel` mints a new channel
/// instead of answering with the first one's ARN. A record is a `ChimeArn` (modelled
/// `len 5..1600`) keyed by an `AppInstanceArn` (1600) and a `ClientRequestToken`
/// (modelled `len 2..64`), so 4096 records bound this map at about 13 MiB.
pub const MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS: usize = 4096;

/// How many channels this emulator stores at once.
///
/// `CreateChannel` is the only way to add one and `DeleteChannel` the only way to remove
/// one, and that reclamation is the CALLER's option — so without a cap of its own the
/// map grows with the request stream. Past this many live channels `CreateChannel`
/// answers the modelled `ResourceLimitExceededException` instead of minting another,
/// which is also how the real service reports a quota it will not exceed.
///
/// Memory: a `ChannelRecord`'s own members are at most `Name` 256 + `Metadata` 1024 +
/// `ChannelId` 64 + three `ChimeArn`s (3 x 1600) + 50 tags of 128 + 256, so on the order
/// of 26 KiB; 256 channels bound the records themselves at about 7 MiB, plus at most
/// [`MAX_MESSAGE_BYTES_PER_CHANNEL`] of messages each (256 MiB in total) — a bound every
/// retention path respects, because a message larger than [`MAX_MESSAGE_BYTES`] is
/// refused rather than stored.
pub const MAX_STORED_CHANNELS: usize = 256;

/// How many messages one channel keeps.
///
/// `SendChannelMessage` appends one record per request and this slice implements no
/// message delete, so the Vec needs a bound of its own: past this many the OLDEST
/// message is dropped, together with the `message_idempotency` record that named it — so
/// that map can never outgrow the messages it points at. An evicted message is simply no
/// longer there: `GetChannelMessage` answers its modelled 404 for it.
pub const MAX_MESSAGES_PER_CHANNEL: usize = 1024;

/// How many caller-supplied bytes one message may carry.
///
/// `Content` is modelled `NonEmptyContent, len 1..` — no MAXIMUM — so nothing in the
/// model stops one request from carrying a gigabyte, and a retained message that large
/// would break the aggregate bound below however the eviction is written. This emulator
/// therefore caps the message it will ACCEPT: `SendChannelMessage` refuses a larger one
/// with the modelled `BadRequestException` naming this limit, so every stored message
/// fits inside [`MAX_MESSAGE_BYTES_PER_CHANNEL`] and can be read back by the
/// `GetChannelMessage` that follows it. The value is this emulator's own choice, not a
/// modelled or documented service limit.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// How many caller-supplied bytes one channel's messages keep.
///
/// The oldest messages are dropped until the channel's stored bytes (see
/// [`ChannelMessageRecord::stored_bytes`]) are inside this budget — UNCONDITIONALLY, so
/// there is no retention path a request can take that this budget does not cover. It is
/// a whole multiple of [`MAX_MESSAGE_BYTES`], so a channel always keeps at least the
/// message it just accepted.
pub const MAX_MESSAGE_BYTES_PER_CHANNEL: usize = 16 * MAX_MESSAGE_BYTES;

/// How many entries one request's `MessageAttributes` map may carry.
///
/// `MessageAttributeMap` carries no modelled length, and an entry costs memory even when
/// its key and values are empty, so the count needs a cap of its own or a single request
/// could retain millions of near-zero-byte entries inside the byte budget.
/// `SendChannelMessage` refuses a larger map with the modelled `BadRequestException`.
/// This emulator's own choice, not a modelled or documented service limit.
pub const MAX_MESSAGE_ATTRIBUTES: usize = 64;

/// How many `StringValues` one message attribute may carry, for the same reason and with
/// the same refusal as [`MAX_MESSAGE_ATTRIBUTES`].
pub const MAX_MESSAGE_ATTRIBUTE_VALUES: usize = 64;

/// What one element of a stored collection costs beyond its own bytes: the `String` /
/// `Target` header the element itself occupies. Counting it is what keeps a request of
/// many EMPTY elements from being accounted at zero and so escaping the retention
/// budget.
const COLLECTION_ELEMENT_OVERHEAD: usize = std::mem::size_of::<String>();

/// The caller-supplied footprint a `SendChannelMessage` request would store, which is
/// what [`MAX_MESSAGE_BYTES`] refuses on and [`MAX_MESSAGE_BYTES_PER_CHANNEL`] budgets.
///
/// One function so the size the handler refuses on and the size the store accounts for
/// can never disagree, and it measures RETAINED FOOTPRINT rather than string lengths
/// alone: every map entry and every collection element is charged for the storage it
/// occupies, so a request made of empty strings cannot be accounted at zero.
pub fn message_bytes(
    content: &str,
    metadata: Option<&str>,
    content_type: Option<&str>,
    attributes: Option<&HashMap<String, MessageAttributeValue>>,
    target: Option<&[Target]>,
) -> usize {
    let attribute_bytes = attributes.map_or(0, |attributes| {
        attributes
            .iter()
            .map(|(key, value)| {
                std::mem::size_of::<String>()
                    + std::mem::size_of::<MessageAttributeValue>()
                    + key.len()
                    + value.string_values.as_ref().map_or(0, |values| {
                        values
                            .iter()
                            .map(|value| value.len() + COLLECTION_ELEMENT_OVERHEAD)
                            .sum::<usize>()
                    })
            })
            .sum()
    });
    // `TargetList` is modelled `len 1..1`, so the model itself bounds this collection;
    // it is charged for anyway, so no stored member escapes the accounting.
    let target_bytes = target.map_or(0, |targets| {
        targets
            .iter()
            .map(|target| {
                std::mem::size_of::<Target>()
                    + target.member_arn.as_ref().map_or(0, String::len)
            })
            .sum::<usize>()
    });
    content.len()
        + metadata.map_or(0, str::len)
        + content_type.map_or(0, str::len)
        + attribute_bytes
        + target_bytes
}

/// Per-service state for com-amazonaws-chimesdkmessaging-chimemessagingservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkmessagingChimemessagingserviceState {
    /// Every live channel, keyed by its `ChannelArn`. Channels and the messages they own
    /// are emulated RESOURCES, reclaimed by `DeleteChannel`; because that reclamation is
    /// the caller's option, the map is additionally capped at [`MAX_STORED_CHANNELS`]
    /// and each channel's messages at [`MAX_MESSAGES_PER_CHANNEL`] /
    /// [`MAX_MESSAGE_BYTES_PER_CHANNEL`], so the store cannot grow with the request
    /// stream. No response returns more than the modelled `MaxResults` maximum at once.
    pub channels: HashMap<String, ChannelRecord>,
    /// `(AppInstanceArn, ClientRequestToken) -> ChannelArn`: the result the FIRST
    /// `CreateChannel` carrying that `@idempotencyToken` produced, so a replay answers
    /// with it instead of minting a second channel. The record is the recorded outcome
    /// of that request, so it outlives the channel itself — which is why it is read and
    /// written through [`Self::remembered_channel`] / [`Self::remember_channel`], which
    /// keep the map inside [`MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS`].
    create_channel_idempotency: HashMap<(String, String), String>,
    /// The order those records were written in, so the oldest can be dropped first.
    create_channel_idempotency_order: VecDeque<(String, String)>,
    /// Draws [`Self::mint_id`], so two ids minted in the same nanosecond differ.
    id_counter: u64,
}

impl ComAmazonawsChimesdkmessagingChimemessagingserviceState {
    /// Mint an id for a resource this service creates: a channel id or a message id.
    ///
    /// The value is the first 16 bytes of a SHA-256 over `salt`, a monotonic counter and
    /// the current instant, hex-encoded — 32 characters that satisfy both `ChannelId`'s
    /// `^[A-Za-z0-9]([A-Za-z0-9:\-_.@]{0,62}[A-Za-z0-9])?$` and `MessageId`'s
    /// `^[-_a-zA-Z0-9]*$`. The counter is what makes two ids minted in the same
    /// nanosecond differ; nothing the caller sent is recoverable from the result.
    pub fn mint_id(&mut self, salt: &str) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut hasher, salt.as_bytes());
        sha2::Digest::update(&mut hasher, self.id_counter.to_be_bytes());
        sha2::Digest::update(&mut hasher, now_nanos().to_be_bytes());
        let digest = sha2::Digest::finalize(hasher);
        hex::encode(&digest[..16])
    }

    /// The `ChannelArn` an earlier `CreateChannel` with this `(AppInstanceArn,
    /// ClientRequestToken)` returned, if the record is still held.
    pub fn remembered_channel(&self, key: &(String, String)) -> Option<&String> {
        self.create_channel_idempotency.get(key)
    }

    /// Record what this `CreateChannel` returned, dropping the oldest record once the
    /// map is at [`MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS`].
    pub fn remember_channel(&mut self, key: (String, String), channel_arn: String) {
        if self
            .create_channel_idempotency
            .insert(key.clone(), channel_arn)
            .is_none()
        {
            self.create_channel_idempotency_order.push_back(key);
        }
        while self.create_channel_idempotency_order.len() > MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS {
            if let Some(oldest) = self.create_channel_idempotency_order.pop_front() {
                self.create_channel_idempotency.remove(&oldest);
            }
        }
    }
}

/// A channel minted by `CreateChannel`, with the messages sent to it.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub arn: String,
    pub app_instance_arn: String,
    pub channel_id: String,
    pub name: String,
    /// `ChannelMode` wire value — see `CHANNEL_MODE_VALUES`.
    pub mode: String,
    /// `ChannelPrivacy` wire value — see `CHANNEL_PRIVACY_VALUES`.
    pub privacy: String,
    pub metadata: Option<String>,
    pub created_by: Identity,
    pub created_millis: i64,
    pub last_updated_millis: i64,
    /// Set by the last PERSISTENT message sent to the channel; `None` until then.
    pub last_message_millis: Option<i64>,
    pub elastic_channel_configuration: Option<ElasticChannelConfiguration>,
    pub expiration_settings: Option<ExpirationSettings>,
    pub tags: Vec<Tag>,
    /// Messages in the order they were accepted, which is ASCENDING `CreatedTimestamp`.
    pub messages: Vec<ChannelMessageRecord>,
    /// `ClientRequestToken -> MessageId` for this channel's `SendChannelMessage` calls.
    pub message_idempotency: HashMap<String, String>,
}

/// One message appended to a channel by `SendChannelMessage`.
#[derive(Debug, Clone)]
pub struct ChannelMessageRecord {
    pub message_id: String,
    pub content: String,
    pub content_type: Option<String>,
    pub metadata: Option<String>,
    /// `ChannelMessageType` wire value — see `CHANNEL_MESSAGE_TYPE_VALUES`.
    pub r#type: String,
    /// `ChannelMessagePersistenceType` wire value — see
    /// `CHANNEL_MESSAGE_PERSISTENCE_TYPE_VALUES`.
    pub persistence: String,
    pub created_millis: i64,
    pub last_updated_millis: i64,
    pub last_edited_millis: Option<i64>,
    pub sender: Identity,
    pub redacted: bool,
    /// `ChannelMessageStatus` wire value — see `CHANNEL_MESSAGE_STATUS_VALUES`.
    pub status: String,
    pub message_attributes: Option<HashMap<String, MessageAttributeValue>>,
    pub target: Option<Vec<Target>>,
}

impl ChannelRecord {
    /// The modelled `Channel` this record describes, as `DescribeChannel` returns it.
    pub fn to_channel(&self) -> Channel {
        Channel {
            name: Some(self.name.clone()),
            channel_arn: Some(self.arn.clone()),
            mode: Some(self.mode.clone()),
            privacy: Some(self.privacy.clone()),
            metadata: self.metadata.clone(),
            created_by: Some(self.created_by.clone()),
            created_timestamp: Some(date_time(self.created_millis)),
            last_message_timestamp: self.last_message_millis.map(date_time),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
            // No channel flow is associated until `AssociateChannelFlow` is implemented.
            channel_flow_arn: None,
            elastic_channel_configuration: self.elastic_channel_configuration.clone(),
            expiration_settings: self.expiration_settings.clone(),
        }
    }

    /// The modelled `ChannelSummary` this record describes, as `ListChannels` returns it.
    pub fn to_summary(&self) -> ChannelSummary {
        ChannelSummary {
            name: Some(self.name.clone()),
            channel_arn: Some(self.arn.clone()),
            mode: Some(self.mode.clone()),
            privacy: Some(self.privacy.clone()),
            metadata: self.metadata.clone(),
            last_message_timestamp: self.last_message_millis.map(date_time),
        }
    }

    /// The message with `message_id`, if this channel holds it.
    pub fn message(&self, message_id: &str) -> Option<&ChannelMessageRecord> {
        self.messages.iter().find(|m| m.message_id == message_id)
    }

    /// Append `message`, recording `client_request_token` as the token that produced it.
    ///
    /// The OLDEST messages are then dropped until the channel is inside both
    /// [`MAX_MESSAGES_PER_CHANNEL`] and [`MAX_MESSAGE_BYTES_PER_CHANNEL`], and each
    /// eviction takes the `message_idempotency` record that named the evicted message
    /// with it — so neither collection grows with the request stream, and the map can
    /// never outgrow the messages it points at. Both bounds apply unconditionally; a
    /// message too large to sit inside the byte budget is refused by
    /// `SendChannelMessage` ([`MAX_MESSAGE_BYTES`]) rather than stored here, so the
    /// message just appended is always still there for the get that follows.
    pub fn append_message(
        &mut self,
        message: ChannelMessageRecord,
        client_request_token: String,
    ) {
        let message_id = message.message_id.clone();
        self.messages.push(message);
        self.message_idempotency
            .insert(client_request_token, message_id);
        while !self.messages.is_empty()
            && (self.messages.len() > MAX_MESSAGES_PER_CHANNEL
                || self.stored_bytes() > MAX_MESSAGE_BYTES_PER_CHANNEL)
        {
            let evicted = self.messages.remove(0);
            self.message_idempotency
                .retain(|_, id| *id != evicted.message_id);
        }
    }

    /// The caller-supplied bytes this channel's messages hold.
    pub fn stored_bytes(&self) -> usize {
        self.messages
            .iter()
            .map(ChannelMessageRecord::stored_bytes)
            .sum()
    }
}

impl ChannelMessageRecord {
    /// The modelled `ChannelMessage` this record describes, as `GetChannelMessage`
    /// returns it. The channel ARN is the owning channel's, which the record does not
    /// repeat.
    pub fn to_channel_message(&self, channel_arn: &str) -> ChannelMessage {
        ChannelMessage {
            channel_arn: Some(channel_arn.to_string()),
            message_id: Some(self.message_id.clone()),
            content: Some(self.content.clone()),
            metadata: self.metadata.clone(),
            r#type: Some(self.r#type.clone()),
            created_timestamp: Some(date_time(self.created_millis)),
            last_edited_timestamp: self.last_edited_millis.map(date_time),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
            sender: Some(self.sender.clone()),
            redacted: self.redacted,
            persistence: Some(self.persistence.clone()),
            status: Some(self.status_structure()),
            message_attributes: self.message_attributes.clone(),
            // Sub-channels belong to elastic channels, which this slice does not serve.
            sub_channel_id: None,
            content_type: self.content_type.clone(),
            target: self.target.clone(),
        }
    }

    /// The modelled `ChannelMessageSummary`, as `ListChannelMessages` returns it.
    pub fn to_summary(&self) -> ChannelMessageSummary {
        ChannelMessageSummary {
            message_id: Some(self.message_id.clone()),
            content: Some(self.content.clone()),
            metadata: self.metadata.clone(),
            r#type: Some(self.r#type.clone()),
            created_timestamp: Some(date_time(self.created_millis)),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
            last_edited_timestamp: self.last_edited_millis.map(date_time),
            sender: Some(self.sender.clone()),
            redacted: self.redacted,
            status: Some(self.status_structure()),
            message_attributes: self.message_attributes.clone(),
            content_type: self.content_type.clone(),
            target: self.target.clone(),
        }
    }

    /// This message's delivery status as the modelled structure.
    pub fn status_structure(&self) -> ChannelMessageStatusStructure {
        ChannelMessageStatusStructure {
            value: Some(self.status.clone()),
            // `Detail` carries the reason a channel flow denied or failed a message;
            // nothing in this slice can produce one.
            detail: None,
        }
    }

    /// The caller-supplied footprint this record holds, which is what
    /// [`MAX_MESSAGE_BYTES_PER_CHANNEL`] budgets. Every stored caller-supplied member is
    /// charged, `Target` included.
    pub fn stored_bytes(&self) -> usize {
        self.message_id.len()
            + message_bytes(
                &self.content,
                self.metadata.as_deref(),
                self.content_type.as_deref(),
                self.message_attributes.as_ref(),
                self.target.as_deref(),
            )
    }
}

/// Now, as epoch milliseconds — the resolution the restJson1 timestamps carry.
pub fn now_millis() -> i64 {
    let now = std::time::SystemTime::now();
    match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(since_epoch) => i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX),
        Err(before_epoch) => -i64::try_from(before_epoch.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// Now, as epoch nanoseconds, which is what [`ComAmazonawsChimesdkmessagingChimemessagingserviceState::mint_id`]
/// hashes beside its counter.
fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos())
}

/// An epoch-milliseconds instant as the modelled timestamp type.
pub fn date_time(millis: i64) -> DateTime {
    DateTime::from_millis(millis)
}

/// An inbound modelled timestamp as epoch milliseconds, for range comparison.
pub fn millis_of(value: &DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// The `Identity` of the caller named by the `x-amz-chime-bearer` ARN.
///
/// Only `Arn` is filled: an `AppInstanceUser`'s `Name` is held by the Chime SDK Identity
/// service, which this emulator cannot read from here, and the member is optional — so
/// inventing a name would be inventing state.
pub fn identity_of(bearer_arn: &str) -> Identity {
    Identity {
        arn: Some(bearer_arn.to_string()),
        name: None,
    }
}

/// Encode a pagination cursor as an opaque `NextToken`.
///
/// Hex keeps the token opaque (a caller cannot read a resource id out of it) while
/// staying inside `NextToken`'s `.*` pattern and 2048-character bound.
pub fn encode_next_token(cursor: &str) -> String {
    hex::encode(cursor.as_bytes())
}

/// The cursor inside a `NextToken`, or `None` when the token was not one this
/// service issued.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkmessagingChimemessagingserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkmessagingChimemessagingserviceState>)
}
