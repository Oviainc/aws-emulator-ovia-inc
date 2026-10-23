// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sqs-amazonsqs service.
//!
//! The queue is this service's primary resource: `CreateQueue` allocates one,
//! `GetQueueUrl` resolves it by name, `GetQueueAttributes` reports it,
//! `SetQueueAttributes` changes it and `DeleteQueue` removes it, all against the SAME
//! [`Queue`] record in [`ComAmazonawsSqsAmazonsqsState::queues`], so a create is
//! observable by every later read and an attribute change by every later report.
//!
//! The record is the stored half; the wire half (the `Attributes` map
//! `GetQueueAttributes` returns) is PROJECTED from it by
//! [`Queue::reported_attributes`], so no handler assembles a response member by hand.
//!
//! # The message plane
//!
//! A queue also HOLDS its messages: [`Queue::messages`] is the arrival-order deque
//! `SendMessage` appends to and `ReceiveMessage` hands out of, and [`Queue::handles`]
//! maps the receipt handle a receive minted to the message it names, which is what
//! `DeleteMessage` resolves. Visibility and retention are evaluated against the clock
//! by [`Queue::materialise_due`], so the three `ApproximateNumberOfMessages*`
//! attributes are counts over real state rather than made-up values.
//!
//! Everything a caller can make this in-memory store hold is bounded — see the
//! "Store bounds" section — and a bound is REFUSED rather than enforced by eviction,
//! because dropping a message a client was told had been accepted would lose it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use md5::{Digest, Md5};

use crate::ops::types::{
    InvalidAttributeName, InvalidAttributeValue, MessageAttributeValue, QueueDoesNotExist,
    ReceiptHandleIsInvalid, UnsupportedOperation, QUEUE_ATTRIBUTE_NAME_VALUES,
};

/// The region an ARN carries when the request names none (an unsigned call, which
/// carries no SigV4 credential scope). Matches the region the emulator's own
/// generated config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, as `smithy_query(query =
/// "service_info")` reports it (`arnNamespace: sqs`, `sigv4Name: sqs`) — the
/// `service` segment of every queue ARN.
pub const ARN_SERVICE: &str = "sqs";

/// The longest queue name `CreateQueue` accepts: "A queue name can have up to 80
/// characters" (`CreateQueueRequest$QueueName` docs).
pub const MAX_QUEUE_NAME_LENGTH: usize = 80;

/// The first character of `body` that a message body may not hold, if any.
///
/// The accepted set is `SendMessage`'s own: "The following Unicode characters are
/// allowed ... #x9 | #xA | #xD | #x20 to #xD7FF | #xE000 to #xFFFD | #x10000 to
/// #x10FFFF". A Rust `char` can never be a surrogate, so the #xD800–#xDFFF gap between
/// the third and fourth range needs no arm of its own. A body holding anything else is
/// what `SendMessage` answers with the modelled `InvalidMessageContents`.
pub fn invalid_body_character(body: &str) -> Option<char> {
    body.chars().find(|character| {
        !matches!(*character,
            '\u{9}' | '\u{A}' | '\u{D}'
            | '\u{20}'..='\u{D7FF}'
            | '\u{E000}'..='\u{FFFD}'
            | '\u{10000}'..='\u{10FFFF}')
    })
}

/// The suffix a FIFO queue's name must carry: "A FIFO queue name must end with the
/// .fifo suffix" (`CreateQueueRequest$QueueName` docs).
pub const FIFO_QUEUE_NAME_SUFFIX: &str = ".fifo";

/// The attribute that designates a queue as FIFO.
pub const FIFO_QUEUE_ATTRIBUTE: &str = "FifoQueue";

/// How many cost allocation tags one queue may carry.
///
/// The `TagQueue`/`CreateQueueRequest$tags` documentation makes 50 the guideline
/// ("Adding more than 50 tags to a queue isn't recommended") and points at "Quotas
/// related to queues" for "a full list of tag restrictions", which publishes 50 tags
/// per queue. It is enforced here as a bound, over the tag set a call would LEAVE the
/// queue with, so no sequence of tagging calls grows one queue's map without limit.
pub const MAX_TAGS_PER_QUEUE: usize = 50;

/// The longest tag key a queue accepts: 128 characters, per the same "Quotas related
/// to queues" tag restrictions.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value a queue accepts: 256 characters, per the same "Quotas related
/// to queues" tag restrictions.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Attributes `GetQueueAttributes` reports but no caller sets: they are properties of
/// the queue record itself, so [`Queue::reported_attributes`] derives each one.
pub const DERIVED_ATTRIBUTES: &[&str] = &["QueueArn", "CreatedTimestamp", "LastModifiedTimestamp"];

/// Attributes that count MESSAGES ("the approximate number of messages available for
/// retrieval", "... that are delayed", "... that are in flight").
///
/// [`Queue::reported_attributes`] derives each from the queue's own message deque, so
/// they are reported but never settable — which is why they appear here and not in
/// [`SETTABLE_ATTRIBUTES`].
pub const MESSAGE_PLANE_ATTRIBUTES: &[&str] = &[
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesDelayed",
    "ApproximateNumberOfMessagesNotVisible",
];

/// The `AttributeNames` value that asks for every attribute the queue has.
pub const ALL_ATTRIBUTES: &str = "All";

/// What values a queue attribute accepts, from that attribute's own entry in the
/// `CreateQueue`/`SetQueueAttributes` `Attributes` documentation.
#[derive(Debug, Clone, Copy)]
pub enum AttributeKind {
    /// "An integer from <min> to <max>", inclusive on both bounds.
    Integer { min: i64, max: i64 },
    /// "Valid values are `true` and `false`."
    Boolean,
    /// A documented value set.
    OneOf(&'static [&'static str]),
    /// A JSON document ("a valid Amazon Web Services policy", "... as a JSON object").
    Json,
    /// An opaque string the service does not constrain.
    Text,
}

/// One settable queue attribute: what it accepts, when it may be supplied, and the
/// value a queue carries when a create names none.
#[derive(Debug)]
pub struct AttributeSpec {
    /// The attribute name, as the model's `QueueAttributeName` enum spells it.
    pub name: &'static str,
    /// The values it accepts.
    pub kind: AttributeKind,
    /// True when only `CreateQueue` may supply it: "You can provide this attribute
    /// only during queue creation. You can't change it for an existing queue."
    pub create_only: bool,
    /// True when it "applies only to FIFO (first-in-first-out) queues".
    pub fifo_only: bool,
    /// The documented default ("Default: 30."), applied by `CreateQueue` when the
    /// request names no value. `None` for an attribute the docs give no default.
    pub default: Option<&'static str>,
}

/// Every attribute a caller may supply, with the bounds, value sets and defaults the
/// `Attributes` member of `CreateQueue`/`SetQueueAttributes` documents for it.
///
/// `SetQueueAttributes` lists exactly these minus `FifoQueue`, which is why that one
/// entry carries `create_only`. Name order, so a request carrying two bad attributes
/// is always refused for the same one.
pub const SETTABLE_ATTRIBUTES: &[AttributeSpec] = &[
    AttributeSpec {
        // "Enables content-based deduplication. Valid values are true and false."
        name: "ContentBasedDeduplication",
        kind: AttributeKind::Boolean,
        create_only: false,
        fifo_only: true,
        default: None,
    },
    AttributeSpec {
        // "Specifies whether message deduplication occurs at the message group or
        // queue level. Valid values are messageGroup and queue."
        name: "DeduplicationScope",
        kind: AttributeKind::OneOf(&["messageGroup", "queue"]),
        create_only: false,
        fifo_only: true,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 0 to 900 seconds (15 minutes). Default: 0."
        name: DELAY_SECONDS,
        kind: AttributeKind::Integer { min: 0, max: 900 },
        create_only: false,
        fifo_only: false,
        default: Some("0"),
    },
    AttributeSpec {
        // "Designates a queue as FIFO. Valid values are true and false. ... You can
        // provide this attribute only during queue creation."
        name: FIFO_QUEUE_ATTRIBUTE,
        kind: AttributeKind::Boolean,
        create_only: true,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values are perQueue and perMessageGroupId."
        name: "FifoThroughputLimit",
        kind: AttributeKind::OneOf(&["perQueue", "perMessageGroupId"]),
        create_only: false,
        fifo_only: true,
        default: None,
    },
    AttributeSpec {
        // "An integer representing seconds, between 60 seconds (1 minute) and 86,400
        // seconds (24 hours). Default: 300 (5 minutes)." The default is NOT applied at
        // create: it is the period SQS uses once a KMS key is configured, and this
        // slice reports only what the queue carries.
        name: "KmsDataKeyReusePeriodSeconds",
        kind: AttributeKind::Integer {
            min: 60,
            max: 86_400,
        },
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "The ID of an Amazon Web Services managed customer master key (CMK) for
        // Amazon SQS or a custom CMK" — an opaque key id, alias or ARN.
        name: "KmsMasterKeyId",
        kind: AttributeKind::Text,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 1,024 bytes (1 KiB) to 1,048,576 bytes
        // (1 MiB). Default: 1,048,576 bytes (1 MiB)."
        name: MAXIMUM_MESSAGE_SIZE,
        kind: AttributeKind::Integer {
            min: 1_024,
            max: 1_048_576,
        },
        create_only: false,
        fifo_only: false,
        default: Some("1048576"),
    },
    AttributeSpec {
        // "Valid values: An integer from 60 seconds (1 minute) to 1,209,600 seconds
        // (14 days). Default: 345,600 (4 days)."
        name: MESSAGE_RETENTION_PERIOD,
        kind: AttributeKind::Integer {
            min: 60,
            max: 1_209_600,
        },
        create_only: false,
        fifo_only: false,
        default: Some("345600"),
    },
    AttributeSpec {
        // "The queue's policy. A valid Amazon Web Services policy."
        name: "Policy",
        kind: AttributeKind::Json,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 0 to 20 (seconds). Default: 0."
        name: RECEIVE_MESSAGE_WAIT_TIME_SECONDS,
        kind: AttributeKind::Integer { min: 0, max: 20 },
        create_only: false,
        fifo_only: false,
        default: Some("0"),
    },
    AttributeSpec {
        // "The string that includes the parameters for the permissions for the
        // dead-letter queue redrive permission ... as a JSON object."
        name: "RedriveAllowPolicy",
        kind: AttributeKind::Json,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "The string that includes the parameters for the dead-letter queue
        // functionality of the source queue as a JSON object."
        name: "RedrivePolicy",
        kind: AttributeKind::Json,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Enables server-side queue encryption using SQS owned encryption keys."
        // The docs state no default, so a created queue carries none.
        name: "SqsManagedSseEnabled",
        kind: AttributeKind::Boolean,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 0 to 43,200 (12 hours). Default: 30."
        name: VISIBILITY_TIMEOUT,
        kind: AttributeKind::Integer { min: 0, max: 43_200 },
        create_only: false,
        fifo_only: false,
        default: Some("30"),
    },
];

/// The spec for `name`, if a caller may supply that attribute at all.
pub fn settable_attribute(name: &str) -> Option<&'static AttributeSpec> {
    SETTABLE_ATTRIBUTES.iter().find(|spec| spec.name == name)
}

// The message-plane attributes the message handlers read, named once so a handler
// never spells one as a literal.
pub const DELAY_SECONDS: &str = "DelaySeconds";
pub const MAXIMUM_MESSAGE_SIZE: &str = "MaximumMessageSize";
pub const MESSAGE_RETENTION_PERIOD: &str = "MessageRetentionPeriod";
pub const RECEIVE_MESSAGE_WAIT_TIME_SECONDS: &str = "ReceiveMessageWaitTimeSeconds";
pub const VISIBILITY_TIMEOUT: &str = "VisibilityTimeout";

/// The inclusive `(min, max)` the documentation gives integer attribute `name`, from
/// [`SETTABLE_ATTRIBUTES`] — the one place those bounds are written.
///
/// A name that is not an integer attribute is unbounded, so no caller has to unwrap.
pub fn integer_bounds(name: &str) -> (i64, i64) {
    match settable_attribute(name).map(|spec| spec.kind) {
        Some(AttributeKind::Integer { min, max }) => (min, max),
        _ => (0, i64::MAX),
    }
}

// ---------------------------------------------------------------------------
// Store bounds
// ---------------------------------------------------------------------------
//
// The store is in-memory and the emulator authenticates nobody, so everything a
// caller can make it hold is bounded. NOT fidelity claims about any published quota:
// each is the emulator's own ceiling, generous enough that no realistic workflow meets
// it, so a caller looping on a request cannot grow this process's memory without
// bound.

/// How many queues this emulator will hold; `CreateQueue` refuses past it.
pub const MAX_QUEUES: usize = 1_000;

/// How many messages one queue holds. `MessageRetentionPeriod` is an AWS duration
/// (4 days by default, up to 14), far longer than an emulator session, so it does not
/// bound memory on its own: `SendMessage` refuses past this.
pub const MAX_MESSAGES_PER_QUEUE: usize = 1_000;

/// How many message bytes one queue holds, so a few large messages are bounded as well
/// as many small ones.
pub const MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE: usize = 8 * 1024 * 1024;

/// How many message bytes the whole store holds, across every queue: the per-queue
/// caps alone would still scale with the number of queues.
pub const MAX_RETAINED_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// How many bytes one attribute VALUE whose form admits text of any length may carry:
/// the `Json` attributes (`Policy`, `RedrivePolicy`, `RedriveAllowPolicy`) and the
/// `Text` ones (`KmsMasterKeyId`).
///
/// Every other kind is bounded by its own form (an integer, a boolean, one of a list).
/// These are not, and a queue keeps its attributes for as long as it exists, so without
/// this a caller looping on `CreateQueue`/`SetQueueAttributes` could grow the store
/// without end — the one thing the bounds above exist to prevent. The figure is the
/// ceiling AWS publishes for the largest of them: "The policy size is limited to 8,192
/// bytes" (the `Policy` attribute's own documentation).
pub const MAX_ATTRIBUTE_VALUE_BYTES: usize = 8 * 1024;

/// The page size `ListQueues` uses when the caller gives no `MaxResults`, so one
/// unparameterised request cannot materialise a response that grows with the stored
/// queue count. A client walks past it with `NextToken`.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

// The per-queue tag bounds live with the other tag quotas above:
// [`MAX_TAGS_PER_QUEUE`], [`MAX_TAG_KEY_LENGTH`] and [`MAX_TAG_VALUE_LENGTH`].

/// How many attributes one message may carry. Amazon SQS Developer Guide: "Each message
/// can have up to 10 attributes."
pub const MAX_MESSAGE_ATTRIBUTES: usize = 10;

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Where a stored message stands relative to the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Its delay has not elapsed yet, so it is not available for retrieval.
    Delayed,
    /// It was handed to a consumer and its visibility timeout has not lapsed.
    InFlight,
    /// It is available for retrieval now.
    Visible,
}

/// One message a queue holds.
#[derive(Debug, Clone)]
pub struct StoredMessage {
    /// The identifier minted when the message was sent.
    pub message_id: String,
    /// The body exactly as sent.
    pub body: String,
    /// MD5 hex digest of [`body`](Self::body).
    pub md5_of_body: String,
    /// The message attributes the sender supplied.
    pub message_attributes: BTreeMap<String, MessageAttributeValue>,
    /// When `SendMessage` accepted it.
    pub sent_at: SystemTime,
    /// `sent_at` + the effective `DelaySeconds`: before this instant the message is
    /// [`Visibility::Delayed`].
    pub available_at: SystemTime,
    /// Set by a receive to `now + effective visibility timeout`; cleared once the clock
    /// passes it.
    pub invisible_until: Option<SystemTime>,
    /// How many times the message has been handed to a consumer without being deleted.
    pub receive_count: u32,
    /// When it was first handed to a consumer.
    pub first_received_at: Option<SystemTime>,
}

impl StoredMessage {
    /// Everything this message retains, in bytes: the body plus the message attributes,
    /// which hold caller-supplied names, data types, strings and blobs. Amazon SQS
    /// counts message attributes toward a message's size, and this emulator's byte
    /// budgets count them too, so the accounting covers every byte a sender can make
    /// the store hold.
    pub fn retained_bytes(&self) -> usize {
        self.body
            .len()
            .saturating_add(message_attribute_bytes(&self.message_attributes))
    }

    /// Where this message stands at `now`.
    pub fn visibility(&self, now: SystemTime) -> Visibility {
        if now < self.available_at {
            return Visibility::Delayed;
        }
        match self.invisible_until {
            Some(until) if now < until => Visibility::InFlight,
            _ => Visibility::Visible,
        }
    }
}

/// Per-service state for com-amazonaws-sqs-amazonsqs. A handler reaches it through [`service_state`].
///
/// `queues` is a `BTreeMap` rather than a `HashMap` so iteration is in name order:
/// the order a listing pages over has to be the same on every call.
#[derive(Debug, Default)]
pub struct ComAmazonawsSqsAmazonsqsState {
    /// Every queue that exists, keyed by `QueueName`.
    pub queues: BTreeMap<String, Queue>,
}

/// A stored queue: the state the reported `Attributes` map is rebuilt from, plus the
/// messages it holds.
#[derive(Debug, Clone)]
pub struct Queue {
    pub name: String,
    /// The url minted when the queue was created, and the one every later reference
    /// to this queue reports.
    pub url: String,
    pub arn: String,
    /// The configuration attributes the queue carries: what the create supplied,
    /// overlaid on the documented defaults, plus whatever a later
    /// `SetQueueAttributes` changed. Name-ordered so the reported map is stable.
    pub attributes: BTreeMap<String, String>,
    /// The queue's cost allocation tags: what the create carried, plus every tag a
    /// later `TagQueue` added or overwrote, bounded by [`MAX_TAGS_PER_QUEUE`].
    /// Key-ordered so the map `ListQueueTags` reports is stable, and case-sensitive
    /// (keys differing only in case are different tags), as the `TagQueue`
    /// documentation states.
    pub tags: BTreeMap<String, String>,
    /// The messages this queue holds, in arrival order. Bounded: at most
    /// [`MAX_MESSAGES_PER_QUEUE`] messages and [`MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE`]
    /// bytes, with the store-wide [`MAX_RETAINED_MESSAGE_BYTES`] budget bounding every
    /// queue together.
    pub messages: VecDeque<StoredMessage>,
    /// Receipt handle -> message id, written only through
    /// [`record_receipt_handle`](Self::record_receipt_handle). At most ONE live handle
    /// per message: a later receive replaces the handle an earlier one minted, and a
    /// handle is dropped with its message — so this map never holds more entries than
    /// [`messages`](Self::messages), whatever the receive count.
    pub handles: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When an attribute of this queue last changed; equals `created_at` until a
    /// `SetQueueAttributes` changes something.
    pub last_modified_at: SystemTime,
}

impl Queue {
    /// Every attribute this queue has a value for: its stored configuration, the three
    /// the record itself derives (`QueueArn`, `CreatedTimestamp`,
    /// `LastModifiedTimestamp`) and the three that count the messages it holds.
    ///
    /// The two timestamps are reported "in seconds (epoch time)", per their own
    /// entries in the `AttributeNames` documentation.
    pub fn reported_attributes(&self, now: SystemTime) -> BTreeMap<String, String> {
        let mut reported = self.attributes.clone();
        reported.insert("QueueArn".to_string(), self.arn.clone());
        reported.insert(
            "CreatedTimestamp".to_string(),
            epoch_seconds(self.created_at),
        );
        reported.insert(
            "LastModifiedTimestamp".to_string(),
            epoch_seconds(self.last_modified_at),
        );
        let (visible, in_flight, delayed) = self.counts(now);
        reported.insert(
            "ApproximateNumberOfMessages".to_string(),
            visible.to_string(),
        );
        reported.insert(
            "ApproximateNumberOfMessagesNotVisible".to_string(),
            in_flight.to_string(),
        );
        reported.insert(
            "ApproximateNumberOfMessagesDelayed".to_string(),
            delayed.to_string(),
        );
        reported
    }

    /// Whether this queue is a FIFO queue, which is what makes the FIFO-only
    /// attributes applicable to it.
    pub fn is_fifo(&self) -> bool {
        self.attributes
            .get(FIFO_QUEUE_ATTRIBUTE)
            .is_some_and(|value| value == "true")
    }

    /// The configured value of integer attribute `name`, falling back to the
    /// documented default when the queue carries no value (or an unparseable one).
    pub fn integer(&self, name: &str) -> i64 {
        let default = settable_attribute(name)
            .and_then(|spec| spec.default)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        self.attributes
            .get(name)
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .unwrap_or(default)
    }

    /// (visible, in flight, delayed) message counts at `now`.
    pub fn counts(&self, now: SystemTime) -> (usize, usize, usize) {
        let mut counts = (0usize, 0usize, 0usize);
        for message in &self.messages {
            match message.visibility(now) {
                Visibility::Visible => counts.0 += 1,
                Visibility::InFlight => counts.1 += 1,
                Visibility::Delayed => counts.2 += 1,
            }
        }
        counts
    }

    /// Materialise everything the CLOCK already made due on this queue: drop messages
    /// whose retention period has elapsed, and return messages whose visibility
    /// timeout has lapsed to the visible set. Both were owed at `now` whether or not
    /// anybody called, which is why a reporting operation may apply them.
    pub fn materialise_due(&mut self, now: SystemTime) {
        let retention = seconds(self.integer(MESSAGE_RETENTION_PERIOD));
        let mut expired: Vec<String> = Vec::new();
        self.messages.retain(|message| {
            let due = message
                .sent_at
                .checked_add(retention)
                .is_some_and(|due| now >= due);
            if due {
                expired.push(message.message_id.clone());
            }
            !due
        });
        if !expired.is_empty() {
            self.handles.retain(|_, id| !expired.contains(id));
        }
        for message in &mut self.messages {
            if message.invisible_until.is_some_and(|until| now >= until) {
                message.invisible_until = None;
            }
        }
    }

    /// Record the receipt handle a receive just minted for `message_id`, dropping the
    /// handle an earlier receive minted for it.
    ///
    /// Amazon SQS documents exactly this behaviour: "If you receive a message more than
    /// once, each time you receive it, you get a different receipt handle. You must
    /// provide the most recently received receipt handle when you request to delete the
    /// message." Replacing rather than accumulating is also what bounds
    /// [`handles`](Self::handles) by the message count.
    pub fn record_receipt_handle(&mut self, receipt_handle: String, message_id: &str) {
        self.handles.retain(|_, id| id != message_id);
        self.handles.insert(receipt_handle, message_id.to_string());
    }

    /// Drop a message and every receipt handle that referred to it.
    pub fn remove_message(&mut self, message_id: &str) {
        self.messages
            .retain(|message| message.message_id != message_id);
        self.handles.retain(|_, id| id != message_id);
    }

    /// Total bytes this queue holds, counting everything each message retains.
    pub fn retained_message_bytes(&self) -> usize {
        self.messages
            .iter()
            .map(StoredMessage::retained_bytes)
            .sum()
    }

    /// Why this queue cannot take one more message retaining `message_bytes`, given the
    /// store already holds `store_bytes` — or `None` when it can.
    ///
    /// The bounds are refused rather than enforced by eviction: silently dropping the
    /// oldest message would lose a message a client was told had been accepted.
    pub fn message_rejection(&self, message_bytes: usize, store_bytes: usize) -> Option<String> {
        if self.messages.len() >= MAX_MESSAGES_PER_QUEUE {
            return Some(format!(
                "This emulator holds at most {MAX_MESSAGES_PER_QUEUE} messages per queue; \
                 queue {} is full. Delete messages from it first.",
                self.name
            ));
        }
        if self.retained_message_bytes().saturating_add(message_bytes)
            > MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE
        {
            return Some(format!(
                "This emulator holds at most {MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE} message \
                 bytes per queue; queue {} cannot take {message_bytes} more.",
                self.name
            ));
        }
        if store_bytes.saturating_add(message_bytes) > MAX_RETAINED_MESSAGE_BYTES {
            return Some(format!(
                "This emulator holds at most {MAX_RETAINED_MESSAGE_BYTES} message bytes in \
                 total; it cannot take {message_bytes} more."
            ));
        }
        None
    }
}

impl ComAmazonawsSqsAmazonsqsState {
    /// Total message bytes the whole store holds, across every queue.
    pub fn retained_message_bytes(&self) -> usize {
        self.queues.values().map(Queue::retained_message_bytes).sum()
    }

    /// Materialise everything the CLOCK already made due across every queue; see
    /// [`Queue::materialise_due`].
    pub fn materialise_due(&mut self, now: SystemTime) {
        for queue in self.queues.values_mut() {
            queue.materialise_due(now);
        }
    }

    /// The queue a `QueueUrl` addresses, for reading.
    pub fn queue_by_url(&self, queue_url: &str) -> Option<&Queue> {
        self.queues.get(queue_name_from_url(queue_url)?)
    }

    /// The queue a `QueueUrl` addresses, for writing.
    pub fn queue_by_url_mut(&mut self, queue_url: &str) -> Option<&mut Queue> {
        let name = queue_name_from_url(queue_url)?.to_string();
        self.queues.get_mut(&name)
    }

    /// Remove the queue a `QueueUrl` addresses, with every message and handle it held.
    pub fn remove_queue_by_url(&mut self, queue_url: &str) -> Option<Queue> {
        let name = queue_name_from_url(queue_url)?.to_string();
        self.queues.remove(&name)
    }
}

/// A `SystemTime` as the whole seconds since the Unix epoch, which is how SQS reports
/// `CreatedTimestamp`/`LastModifiedTimestamp`.
fn epoch_seconds(at: SystemTime) -> String {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
        .to_string()
}

/// A `SystemTime` as whole milliseconds since the Unix epoch, which is how SQS reports
/// the `SentTimestamp`/`ApproximateFirstReceiveTimestamp` message system attributes
/// ("epoch time in milliseconds").
pub fn epoch_millis(at: SystemTime) -> String {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default()
        .to_string()
}

/// The ARN of the queue named `name` in `region`:
/// `arn:aws:sqs:<region>:<account>:<QueueName>`.
///
/// The account segment comes from the core's single source of truth, never a literal.
pub fn queue_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, name)
}

/// The url of the queue named `name`, as a client should address it:
/// `<endpoint>/<accountId>/<QueueName>`.
///
/// The endpoint is the emulator's OWN — the authority this request arrived on —
/// because the url has to be reachable: a client that re-targets its SQS calls at the
/// queue url must land back on this emulator rather than on the real AWS endpoint.
pub fn queue_url(parts: &::http::request::Parts, region: &str, name: &str) -> String {
    let endpoint = request_endpoint(parts, region);
    format!("{endpoint}/{}/{name}", ::engine::emulator_account_id())
}

/// The emulator endpoint this request arrived on, falling back to the public SQS
/// endpoint for `region` when the request names no authority at all.
fn request_endpoint(parts: &::http::request::Parts, region: &str) -> String {
    let authority = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| !host.is_empty())
        .map(ToString::to_string)
        .or_else(|| parts.uri.authority().map(ToString::to_string));
    match authority {
        Some(authority) => format!("https://{authority}"),
        None => format!("https://sqs.{region}.amazonaws.com"),
    }
}

/// The `QueueName` a `QueueUrl` addresses, or `None` for a value that is not a queue
/// url of this account at all.
///
/// A queue url is `<endpoint>/<accountId>/<QueueName>`, so the name is its last path
/// segment and the account the segment before it. Keying on that pair rather than on
/// the whole string is what lets a client reach its queue through any endpoint this
/// emulator answers on — the url it was handed, or one built for another host name —
/// while a url naming a different account resolves to nothing.
pub fn queue_name_from_url(queue_url: &str) -> Option<&str> {
    let (prefix, name) = queue_url.trim_end_matches('/').rsplit_once('/')?;
    if name.is_empty() {
        return None;
    }
    let (_, account) = prefix.rsplit_once('/')?;
    if account != ::engine::emulator_account_id() {
        return None;
    }
    Some(name)
}

/// The message every `QueueDoesNotExist` refusal carries, whatever the request named.
///
/// Amazon SQS answers this error with ONE fixed string — "The specified queue does not
/// exist." — for every operation and every queue: the error CODE
/// (`AWS.SimpleQueueService.NonExistentQueue` / `QueueDoesNotExist`, from the model's
/// `awsQueryError`) is what identifies the condition, and the message is a constant a
/// client can match on. It deliberately does not name the queue, so a caller probing for
/// a queue it may not address is never handed the name it probed back.
pub const QUEUE_DOES_NOT_EXIST_MESSAGE: &str = "The specified queue does not exist.";

/// The modelled refusal for a `QueueUrl` or `QueueName` no queue of this account answers
/// to, carrying [`QUEUE_DOES_NOT_EXIST_MESSAGE`] verbatim.
///
/// Takes no argument on purpose: nothing of the request may reach this payload, so no
/// caller can word a message of its own that echoes a url, a name or an owner.
pub fn queue_does_not_exist() -> QueueDoesNotExist {
    QueueDoesNotExist {
        message: Some(QUEUE_DOES_NOT_EXIST_MESSAGE.to_string()),
    }
}

/// The modelled refusal for a receipt handle this queue did not mint, or whose message
/// is gone.
///
/// A handle is replaced by the next receive of the same message, so a superseded one is
/// invalid too: "You must provide the most recently received receipt handle when you
/// request to delete the message."
pub fn receipt_handle_is_invalid(receipt_handle: &str) -> ReceiptHandleIsInvalid {
    ReceiptHandleIsInvalid {
        message: Some(format!(
            "The input receipt handle {receipt_handle:?} is not a valid receipt handle for this queue."
        )),
    }
}

/// How many entries one batch request may carry.
///
/// "For Amazon SQS, the maximum number of entries you can include in a single
/// `SendMessageBatch`, `DeleteMessageBatch`, or `ChangeMessageVisibilityBatch` request is
/// 10" (`TooManyEntriesInBatchRequest`'s own documentation). The model puts no `@length`
/// on the entry lists, so every batch operation applies this bound itself.
pub const MAX_BATCH_ENTRIES: usize = 10;

/// How long a batch entry's `Id` may be: "This identifier can have up to 80 characters"
/// (`DeleteMessageBatchRequestEntry$Id`).
pub const MAX_BATCH_ENTRY_ID_LEN: usize = 80;

/// Whether `id` is a batch entry id the specification admits.
///
/// "The following characters are accepted: alphanumeric characters, hyphens(-), and
/// underscores (_)" and "this identifier can have up to 80 characters"
/// (`DeleteMessageBatchRequestEntry$Id`); an empty id names nothing, so it is not one
/// either. A value outside this is what `InvalidBatchEntryId` refuses.
pub fn is_valid_batch_entry_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_BATCH_ENTRY_ID_LEN
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

/// Why an `Attributes` map was refused. Each handler renders it as its own modelled
/// error variant, so the wire code is the one that operation declares.
#[derive(Debug)]
pub enum AttributeRefusal {
    /// The map named an attribute that does not exist, or one this call may not set.
    Name(InvalidAttributeName),
    /// The map gave an attribute a value outside what that attribute accepts.
    Value(InvalidAttributeValue),
}

/// Check an `Attributes` map a caller supplied.
///
/// `on_create` is true for `CreateQueue`, which is the only call that may supply a
/// `create_only` attribute; `fifo` says whether the queue the map applies to is a FIFO
/// queue, which is what makes the FIFO-only attributes applicable. Names are visited
/// in order, so a map with two bad attributes is always refused for the same one.
pub fn validate_attributes(
    supplied: &HashMap<String, String>,
    on_create: bool,
    fifo: bool,
) -> Result<(), AttributeRefusal> {
    let mut names: Vec<&String> = supplied.keys().collect();
    names.sort();
    for name in names {
        let value = &supplied[name];
        if !QUEUE_ATTRIBUTE_NAME_VALUES.contains(&name.as_str()) {
            return Err(AttributeRefusal::Name(InvalidAttributeName {
                message: Some(format!("Unknown Attribute {name}.")),
            }));
        }
        let Some(spec) = settable_attribute(name) else {
            return Err(AttributeRefusal::Name(InvalidAttributeName {
                message: Some(format!(
                    "Attribute {name} is reported by GetQueueAttributes and cannot be set."
                )),
            }));
        };
        if spec.create_only && !on_create {
            return Err(AttributeRefusal::Name(InvalidAttributeName {
                message: Some(format!(
                    "Attribute {name} can be provided only when the queue is created."
                )),
            }));
        }
        if spec.fifo_only && !fifo {
            return Err(AttributeRefusal::Name(InvalidAttributeName {
                message: Some(format!("Attribute {name} applies only to FIFO queues.")),
            }));
        }
        check_attribute_value(spec, value)?;
    }
    Ok(())
}

/// Check one attribute value against what its own documentation says it accepts.
fn check_attribute_value(spec: &AttributeSpec, value: &str) -> Result<(), AttributeRefusal> {
    // Every attribute value is stored for the queue's lifetime, so the two kinds whose
    // form admits text of any length are bounded FIRST: a caller looping on
    // `CreateQueue`/`SetQueueAttributes` must not be able to grow this store without
    // end (see "Store bounds" above). The ceiling is the one AWS publishes for the
    // largest of them — "The policy size is limited to 8,192 bytes" (the `Policy`
    // attribute's own documentation) — applied to each of them.
    let bounded = match spec.kind {
        AttributeKind::Json | AttributeKind::Text => value.len() <= MAX_ATTRIBUTE_VALUE_BYTES,
        _ => true,
    };
    if !bounded {
        return Err(AttributeRefusal::Value(InvalidAttributeValue {
            message: Some(format!(
                "Invalid value for the parameter {}: at most {MAX_ATTRIBUTE_VALUE_BYTES} bytes \
                 were expected, and {} were supplied.",
                spec.name,
                value.len()
            )),
        }));
    }
    let accepted = match spec.kind {
        AttributeKind::Integer { min, max } => match value.parse::<i64>() {
            Ok(parsed) => (min..=max).contains(&parsed),
            Err(_) => false,
        },
        AttributeKind::Boolean => value == "true" || value == "false",
        AttributeKind::OneOf(allowed) => allowed.contains(&value),
        AttributeKind::Json => ::serde_json::from_str::<::serde_json::Value>(value).is_ok(),
        AttributeKind::Text => true,
    };
    if accepted {
        return Ok(());
    }
    let expected = match spec.kind {
        AttributeKind::Integer { min, max } => format!("an integer from {min} to {max}"),
        AttributeKind::Boolean => "true or false".to_string(),
        AttributeKind::OneOf(allowed) => format!("one of {}", allowed.join(", ")),
        AttributeKind::Json => "a valid JSON document".to_string(),
        // `Text` accepts everything, so it never reaches here.
        AttributeKind::Text => "any string".to_string(),
    };
    Err(AttributeRefusal::Value(InvalidAttributeValue {
        message: Some(format!(
            "Invalid value for the parameter {}: {expected} was expected.",
            spec.name
        )),
    }))
}

/// Why a `QueueName` was refused, as the text of the refusal a caller has to fix the
/// request from.
///
/// The rule is the member's own: "A queue name can have up to 80 characters. Valid
/// values: alphanumeric characters, hyphens (-), and underscores (_). A FIFO queue
/// name must end with the .fifo suffix."
pub fn queue_name_refusal(name: &str) -> Option<String> {
    let body = name.strip_suffix(FIFO_QUEUE_NAME_SUFFIX).unwrap_or(name);
    if name.is_empty() || name.chars().count() > MAX_QUEUE_NAME_LENGTH {
        return Some(format!(
            "QueueName must be between 1 and {MAX_QUEUE_NAME_LENGTH} characters long, but was {}",
            name.chars().count()
        ));
    }
    if body.is_empty()
        || !body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Some(format!(
            "QueueName {name} is invalid: a queue name may hold only alphanumeric characters, hyphens (-) and underscores (_), optionally followed by the {FIFO_QUEUE_NAME_SUFFIX} suffix"
        ));
    }
    None
}

/// Why a `tags` map a caller supplied was refused, as the text of the refusal a caller
/// has to fix the request from, or `None` when the map is acceptable.
///
/// `existing` is the tag map the queue carries today, so the count is checked against
/// what the call would LEAVE the queue with: a key the request repeats overwrites the
/// existing tag rather than adding one. Keys are visited in order, so a map with two
/// bad tags is always refused for the same one.
///
/// The rules are the tag restrictions the `tags` member's own documentation points at:
/// at most [`MAX_TAGS_PER_QUEUE`] tags per queue, a key of 1 to
/// [`MAX_TAG_KEY_LENGTH`] characters and a value of up to [`MAX_TAG_VALUE_LENGTH`].
pub fn tag_refusal(
    existing: &BTreeMap<String, String>,
    supplied: &HashMap<String, String>,
) -> Option<String> {
    let mut keys: Vec<&String> = supplied.keys().collect();
    keys.sort();
    for key in &keys {
        let length = key.chars().count();
        if length == 0 || length > MAX_TAG_KEY_LENGTH {
            return Some(format!(
                "Tag key is invalid: a tag key must be between 1 and {MAX_TAG_KEY_LENGTH} characters long, but was {length}"
            ));
        }
        let value_length = supplied[*key].chars().count();
        if value_length > MAX_TAG_VALUE_LENGTH {
            return Some(format!(
                "The value of tag key {key} is invalid: a tag value may hold up to {MAX_TAG_VALUE_LENGTH} characters, but was {value_length}"
            ));
        }
    }
    let merged: BTreeSet<&String> = existing.keys().chain(supplied.keys()).collect();
    if merged.len() > MAX_TAGS_PER_QUEUE {
        return Some(format!(
            "A queue may carry up to {MAX_TAGS_PER_QUEUE} tags, but this request would leave it with {}",
            merged.len()
        ));
    }
    None
}

/// The region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope (an unsigned call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The modelled refusal every handler answers with when [`service_state`] finds no
/// store (this service mounted on an engine that holds none).
///
/// `UnsupportedOperation` ("Error code 400. Unsupported operation.") is the only
/// refusal every one of these operations models that says "this emulator will not
/// serve the call" rather than "your request was malformed"; the payload is shared and
/// each operation wraps it in its OWN generated error variant.
pub fn store_unavailable() -> UnsupportedOperation {
    UnsupportedOperation {
        message: Some("the Amazon SQS queue store is unavailable".to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSqsAmazonsqsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSqsAmazonsqsState>)
}

// ---------------------------------------------------------------------------
// Message content: digests, sizes and identifiers
// ---------------------------------------------------------------------------

/// A non-negative second count as a [`Duration`]; a negative count is zero.
pub fn seconds(value: i64) -> Duration {
    Duration::from_secs(u64::try_from(value).unwrap_or(0))
}

/// `at` plus `value` seconds. Total: `SystemTime`'s `Add` PANICS on overflow and every
/// instant computed here derives from a request value, so overflow yields `at` (the
/// unreachable case — every span this is called with is a validated attribute bounded
/// at 43 200 seconds).
pub fn after(at: SystemTime, value: i64) -> SystemTime {
    at.checked_add(seconds(value)).unwrap_or(at)
}

/// A fresh opaque identifier: a version-4 UUID, so two calls never collide. Used for a
/// `MessageId` and for the receipt handle each receive mints.
pub fn new_identifier() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// MD5 hex digest of `body` — the value SQS returns as `MD5OfMessageBody`/`MD5OfBody`.
pub fn md5_hex(body: &[u8]) -> String {
    hex::encode(Md5::digest(body))
}

/// MD5 hex digest of the message-attribute encoding SQS defines, or `None` when there
/// are no attributes (SQS omits `MD5OfMessageAttributes` then).
///
/// The encoding is Amazon SQS's own, documented as "Calculating the MD5 message digest
/// for message attributes" in the Amazon SQS Developer Guide, and no crate implements
/// it, so it is written here over the `md-5` crate's digest: attributes are taken in
/// name order, and each contributes
///
/// 1. its name, length-prefixed with a 4-byte big-endian length,
/// 2. its `DataType`, length-prefixed the same way,
/// 3. one transport-type byte — `1` for a `StringValue`, `2` for a `BinaryValue`,
/// 4. the value itself, length-prefixed the same way.
///
/// `StringListValues`/`BinaryListValues` contribute NOTHING: the model documents both as
/// "Not implemented. Reserved for future use.", so SQS defines no transport type for
/// them and `SendMessage` refuses a message that carries one rather than guessing a byte.
pub fn md5_of_message_attributes(
    attributes: &BTreeMap<String, MessageAttributeValue>,
) -> Option<String> {
    if attributes.is_empty() {
        return None;
    }
    let mut encoded = Vec::new();
    for (name, value) in attributes {
        append_length_prefixed(&mut encoded, name.as_bytes());
        append_length_prefixed(&mut encoded, value.data_type.as_bytes());
        if let Some(string_value) = &value.string_value {
            encoded.push(1);
            append_length_prefixed(&mut encoded, string_value.as_bytes());
        } else if let Some(binary_value) = &value.binary_value {
            encoded.push(2);
            append_length_prefixed(&mut encoded, binary_value);
        }
    }
    Some(md5_hex(&encoded))
}

/// Append `bytes` preceded by its length as a 4-byte big-endian integer.
fn append_length_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The bytes a message-attribute map holds: each attribute's name, data type and value.
///
/// Amazon SQS counts message attributes toward a message's size ("All parts of a
/// message are included in the ... size limit: the message body, the message
/// attributes"), so they are counted here too, both against the queue's
/// `MaximumMessageSize` and against this emulator's retained-byte budgets.
pub fn message_attribute_bytes(attributes: &BTreeMap<String, MessageAttributeValue>) -> usize {
    attributes
        .iter()
        .map(|(name, value)| {
            name.len()
                .saturating_add(value.data_type.len())
                .saturating_add(value.string_value.as_ref().map_or(0, String::len))
                .saturating_add(value.binary_value.as_ref().map_or(0, Vec::len))
        })
        .fold(0usize, usize::saturating_add)
}

/// Why this message attribute cannot be accepted, or `None` when it can.
///
/// `StringListValues` and `BinaryListValues` are documented as "Not implemented.
/// Reserved for future use.", and an attribute that carries no value at all has no
/// SQS-defined digest encoding either (there is no transport-type byte for "nothing"),
/// so either is refused rather than stored under a layout Amazon SQS does not define.
pub fn message_attribute_refusal(name: &str, value: &MessageAttributeValue) -> Option<String> {
    if value.string_value.is_none() && value.binary_value.is_none() {
        return Some(format!(
            "MessageAttributes.{name} must contain a non-empty StringValue or BinaryValue."
        ));
    }
    let member = [
        value.string_list_values.is_some().then_some("StringListValues"),
        value.binary_list_values.is_some().then_some("BinaryListValues"),
    ]
    .into_iter()
    .flatten()
    .next()?;
    Some(format!(
        "MessageAttributes.{name}.{member} is not implemented and is reserved for future use."
    ))
}
