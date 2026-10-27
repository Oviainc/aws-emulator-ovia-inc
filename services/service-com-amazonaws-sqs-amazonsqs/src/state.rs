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
//! Server-side encryption is part of that projection: which of the two mutually
//! exclusive options a queue is on, and what it therefore reports, lives in
//! [`encryption`], as does the KMS permission a call that uses the queue's key needs.
//!
//! # The message plane
//!
//! A queue also HOLDS its messages: [`Queue::messages`] is the arrival-order deque
//! `SendMessage` appends to and `ReceiveMessage` hands out of, and [`Queue::handles`]
//! maps the receipt handle a receive minted to the message it names, which is what
//! `DeleteMessage` resolves — and [`Queue::deleted_handles`] remembers the handles a
//! delete has already consumed, so repeating a delete that succeeded is idempotent
//! rather than refused. Visibility and retention are evaluated against the clock
//! by [`Queue::materialise_due`], so the three `ApproximateNumberOfMessages*`
//! attributes are counts over real state rather than made-up values. A message carries
//! the visibility timeout it was RECEIVED under, so a later change to the queue's
//! attribute spares it, while a FIFO queue's delay is re-derived from the queue's
//! current value by [`Queue::reapply_delay_seconds`], which is where the
//! FIFO-versus-standard retroactivity of `DelaySeconds` lives.
//!
//! Which of those messages a receive may hand over is [`Queue::delivery_order`]: on a
//! standard queue whatever is visible, and on a FIFO queue each message group in strict
//! send order, one group member in flight at a time. A FIFO send also remembers the
//! deduplication token it accepted a message under, in [`Queue::deduplication`], so a
//! resend inside the interval is accepted without delivering a second copy, and
//! sequences the message from [`Queue::sequenced`].
//!
//! A FIFO RECEIVE is idempotent the same way: the batch it handed over under a
//! `ReceiveRequestAttemptId` is remembered in [`Queue::receive_attempts`], so a retry
//! carrying that same token hands the identical messages and receipt handles back
//! instead of a second, different batch.
//!
//! `PurgeQueue` empties that plane through [`Queue::purge`]: the messages, the receipt
//! handles naming them, the remembered receive attempts that named them and the
//! deduplication tokens that would suppress a resend all go, and the instant of the
//! purge is remembered, so a further purge inside [`PURGE_QUEUE_INTERVAL`] of it is
//! refused ([`Queue::purge_in_progress`]).
//!
//! # Message movement tasks
//!
//! `StartMessageMoveTask` takes the messages a dead-letter queue can hand out and puts
//! them in the destination queue, each one deliverable at the instant the task's rate
//! schedules it, and records a [`MessageMoveTask`] in
//! [`ComAmazonawsSqsAmazonsqsState::message_move_tasks`]. The task's progress is a
//! function of that record and the clock — [`MessageMoveTask::messages_moved`] and
//! [`MessageMoveTask::status`] — so no handler stores a status it then has to keep up
//! to date. `ListMessageMoveTasks` reports those records through
//! [`ComAmazonawsSqsAmazonsqsState::reported_message_move_tasks`], which projects the
//! wire entry from the record and the clock exactly as
//! [`Queue::reported_attributes`] projects a queue's attributes.
//!
//! `CancelMessageMoveTask` stops such a task through
//! [`ComAmazonawsSqsAmazonsqsState::cancel_message_move_task`]: the messages whose
//! delivery slot is still ahead go back to the source queue and the cancellation instant
//! is recorded on the task, which is all every later reader needs to report its status
//! and its progress.
//!
//! # Queue permissions
//!
//! A permission is a statement of the queue's own `Policy` attribute rather than a
//! record beside it: `AddPermission` "generates a policy", so
//! [`Queue::granted_policy`] assembles the document the grant leaves the queue
//! carrying and [`Queue::revoke_permission`] takes the statement a label identifies
//! back out of it. Every grant is therefore reported by `GetQueueAttributes` and
//! replaced by an uploaded `SetQueueAttributes` policy, because there is only ever one
//! policy.
//!
//! Everything a caller can make this in-memory store hold is bounded — see the
//! "Store bounds" section — and a bound is REFUSED rather than enforced by eviction,
//! because dropping a message a client was told had been accepted would lose it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use md5::{Digest, Md5};
use sha2::Sha256;

pub mod encryption;

use crate::ops::types::{
    InvalidAddress, InvalidAttributeName, InvalidAttributeValue, ListMessageMoveTasksResultEntry,
    MessageAttributeValue, MessageSystemAttributeValue, QueueDoesNotExist, ReceiptHandleIsInvalid,
    ResourceNotFoundException, UnsupportedOperation,
    MESSAGE_SYSTEM_ATTRIBUTE_NAME_FOR_SENDS_VALUES, QUEUE_ATTRIBUTE_NAME_VALUES,
};

/// Whether the policies in force let a request act on the queue it addresses — the gate
/// every operation that acts on a queue runs before it reports or changes anything — and
/// what a `Policy` attribute value has to look like to be stored at all.
pub mod authorization;

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

/// How a refusal states that the body holds `character`, which is outside the allowed
/// set: the character that was found, as the code point the published set is written
/// in, and that set.
///
/// One wording for every call that takes a message body, so `SendMessage` and a
/// `SendMessageBatch` entry cannot drift on how they state the same defect.
pub fn invalid_body_character_message(character: char) -> String {
    format!(
        "Invalid binary character '#x{:X}' was found in the message body, the set of allowed \
         characters is #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF].",
        character as u32
    )
}

/// The suffix a FIFO queue's name must carry: "A FIFO queue name must end with the
/// .fifo suffix" (`CreateQueueRequest$QueueName` docs).
pub const FIFO_QUEUE_NAME_SUFFIX: &str = ".fifo";

/// The attribute that designates a queue as FIFO.
pub const FIFO_QUEUE_ATTRIBUTE: &str = "FifoQueue";

/// The attribute that makes a FIFO queue derive a message's deduplication token from
/// its body: "Enables content-based deduplication."
pub const CONTENT_BASED_DEDUPLICATION: &str = "ContentBasedDeduplication";

/// The attribute that says whether deduplication is per message group or per queue:
/// "Specifies whether message deduplication occurs at the message group or queue
/// level. Valid values are messageGroup and queue."
pub const DEDUPLICATION_SCOPE: &str = "DeduplicationScope";

/// The [`DEDUPLICATION_SCOPE`] value that scopes a deduplication token to its message
/// group; any other value (including none) scopes it to the whole queue, which is the
/// level the attribute's own documentation names as the alternative.
pub const DEDUPLICATION_SCOPE_MESSAGE_GROUP: &str = "messageGroup";

/// The attribute that says whether the FIFO throughput quota "applies to the entire
/// queue or per message group": "Valid values are perQueue and perMessageGroupId."
pub const FIFO_THROUGHPUT_LIMIT: &str = "FifoThroughputLimit";

/// The [`FIFO_THROUGHPUT_LIMIT`] value that scopes the quota to each message group. It
/// "is allowed only when the value for DeduplicationScope is messageGroup", which is
/// what [`attribute_combination_refusal`] enforces.
pub const FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP: &str = "perMessageGroupId";

/// How long a deduplication token suppresses a resend: a message carrying a
/// `MessageDeduplicationId` already seen is "accepted successfully but aren't delivered
/// during the 5-minute deduplication interval"
/// (`SendMessageRequest$MessageDeduplicationId`).
pub const DEDUPLICATION_INTERVAL: Duration = Duration::from_secs(300);

/// How long a `ReceiveRequestAttemptId` can be retried under: "You can use
/// ReceiveRequestAttemptId only for 5 minutes after a ReceiveMessage action"
/// (`ReceiveMessageRequest$ReceiveRequestAttemptId`).
pub const RECEIVE_REQUEST_ATTEMPT_INTERVAL: Duration = Duration::from_secs(300);

/// How long a deleted queue's NAME stays unavailable: "If you delete a queue, you must
/// wait at least 60 seconds before creating a queue with the same name" (`CreateQueue`'s
/// own documentation).
///
/// The sentence `CreateQueue` answers with is Amazon SQS's own for the
/// `QueueDeletedRecently` class — "You must wait 60 seconds after deleting a queue before
/// you can create another with the same name." — which is the model's documentation of
/// that error shape with its second "queue" dropped, as the recorded refusal words it.
pub const QUEUE_DELETED_RECENTLY_INTERVAL: Duration = Duration::from_secs(60);

/// How long an accepted `PurgeQueue` keeps the next one on the same queue refused:
/// "the specified queue previously received a PurgeQueue request within the last 60
/// seconds (the time it can take to delete the messages in the queue)"
/// (`PurgeQueueInProgress`), while "the message deletion process takes up to 60 seconds"
/// (`PurgeQueue`). The interval is part of the fixed text of that refusal too, through
/// [`purge_queue_in_progress_message`].
pub const PURGE_QUEUE_INTERVAL: Duration = Duration::from_secs(60);

/// The longest `MessageGroupId` / `MessageDeduplicationId` a message may carry: "The
/// length of MessageGroupId is 128 characters" and "The maximum length of
/// MessageDeduplicationId is 128 characters" (their own member documentation), which is
/// also "the maximum length of ReceiveRequestAttemptId".
pub const MAX_MESSAGE_TOKEN_LENGTH: usize = 128;

/// The value the first sequence number a FIFO queue assigns counts up from.
///
/// `SendMessageResult$SequenceNumber` documents the value as "the large,
/// non-consecutive number that Amazon SQS assigns to each message. The length of
/// SequenceNumber is 128 bits. SequenceNumber continues to increase for a particular
/// MessageGroupId" — so what a client may rely on is a decimal integer inside 128 bits
/// that increases. Counting up from here makes them all the same width, which is what a
/// client comparing them as text needs.
pub const SEQUENCE_NUMBER_BASE: u128 = 10_u128.pow(19);

/// Why the FIFO token called `member` — a `MessageGroupId`, a `MessageDeduplicationId`
/// or a `ReceiveRequestAttemptId` — cannot be accepted, as the text of the refusal a
/// caller has to fix the request from, or `None` when it can.
///
/// The rule is the one all three members publish: at most [`MAX_MESSAGE_TOKEN_LENGTH`]
/// characters, "alphanumeric characters and punctuation
/// (!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~)" — which is exactly the ASCII punctuation set.
/// An empty token names no group, no send and no receive attempt, so it is not one
/// either.
pub fn message_token_refusal(member: &str, token: &str) -> Option<String> {
    let length = token.chars().count();
    let within_length = (1..=MAX_MESSAGE_TOKEN_LENGTH).contains(&length);
    let printable = token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c.is_ascii_punctuation());
    if within_length && printable {
        return None;
    }
    // One wording for both halves of the one rule the member publishes, because that is
    // how the rule is published: the accepted characters and the accepted length in a
    // single sentence naming the member.
    Some(invalid_parameter_message(
        token,
        member,
        &format!(
            "{member} can only include alphanumeric and punctuation characters. 1 to \
             {MAX_MESSAGE_TOKEN_LENGTH} in length."
        ),
    ))
}

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

/// The tag-key prefix reserved for the provider's own tags, which a caller may not
/// supply: the same "Quotas related to queues" tag restrictions state "Do not use the
/// `aws:` prefix in your tag names or values because it is reserved for Amazon Web
/// Services use", and a reserved key cannot be created, edited or deleted by a caller.
///
/// Matched case-insensitively, because the reservation is on the prefix itself rather
/// than on one spelling of it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, compared
/// without allocating and without assuming a spelling: the bytes of an ASCII prefix are
/// a char boundary of whatever follows them, so the comparison is safe on any key.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

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
        name: CONTENT_BASED_DEDUPLICATION,
        kind: AttributeKind::Boolean,
        create_only: false,
        fifo_only: true,
        default: None,
    },
    AttributeSpec {
        // "Specifies whether message deduplication occurs at the message group or
        // queue level. Valid values are messageGroup and queue."
        name: DEDUPLICATION_SCOPE,
        kind: AttributeKind::OneOf(&[DEDUPLICATION_SCOPE_MESSAGE_GROUP, "queue"]),
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
        name: FIFO_THROUGHPUT_LIMIT,
        kind: AttributeKind::OneOf(&["perQueue", FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP]),
        create_only: false,
        fifo_only: true,
        default: None,
    },
    AttributeSpec {
        // "An integer representing seconds, between 60 seconds (1 minute) and 86,400
        // seconds (24 hours). Default: 300 (5 minutes)." The default is NOT stored at
        // create: it is the period a KMS key is used under, so it is reported for a
        // queue that is ON a KMS key — see [`encryption::report`].
        name: encryption::KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
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
        // Amazon SQS or a custom CMK" — an opaque key id, alias or ARN. An EMPTY value
        // takes the queue off its key, which is why the form accepts any text; which
        // option the queue is then on is [`encryption::apply_encryption_choice`].
        name: encryption::KMS_MASTER_KEY_ID,
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
        name: POLICY,
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
        name: REDRIVE_POLICY,
        kind: AttributeKind::Json,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Enables server-side queue encryption using SQS owned encryption keys."
        // No value is STORED for a create that names none — a queue carries only what
        // its create supplied — and which option the queue is on, SQS-owned key or KMS
        // key, is what [`encryption::report`] answers from.
        name: encryption::SQS_MANAGED_SSE_ENABLED,
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
/// The attribute carrying the queue's access policy ("The queue's policy. A valid
/// Amazon Web Services policy.").
pub const POLICY: &str = "Policy";

/// The attributes a re-create of an existing name does NOT have to match.
///
/// `CreateQueue` "will return the URL of the existing queue instead of creating a new
/// one" for a request that asks for nothing different about the queue, and refuses one
/// that does — but the queue's ACCESS POLICY is not part of what it compares: a request
/// that re-asserts an existing queue under a different (or no) `Policy` is served with
/// the existing url, and leaves the stored policy as it was. Every other settable
/// attribute is compared.
pub const ATTRIBUTES_NOT_COMPARED_ON_RECREATE: &[&str] = &[POLICY];
/// The attribute that makes a queue a dead-letter SOURCE queue: it names the queue its
/// messages are driven to.
pub const REDRIVE_POLICY: &str = "RedrivePolicy";

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

/// How many recently-deleted queue names the store remembers (see
/// [`ComAmazonawsSqsAmazonsqsState::recently_deleted`]).
///
/// An entry lapses with [`QUEUE_DELETED_RECENTLY_INTERVAL`] and is forgotten as soon as
/// anything touches the ledger, so this bound is only reached by a caller deleting
/// names faster than they lapse. Past it the oldest records are forgotten first: unlike
/// a message, a deletion record is derived data whose only effect is deferring a
/// re-create for a minute, so forgetting one loses nothing a client was told had been
/// accepted.
pub const MAX_RECENTLY_DELETED_NAMES: usize = MAX_QUEUES;

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

/// The inclusive range `ListQueues` accepts for `MaxResults`: "Maximum number of results
/// to include in the response. Value range is 1 to 1000"
/// (`ListQueuesRequest$MaxResults`). A value outside it is refused rather than clamped,
/// so a paginating client is told its request was wrong.
pub const LIST_QUEUES_MAX_RESULTS_RANGE: (i32, i32) = (1, 1_000);

/// The inclusive range `ListDeadLetterSourceQueues` accepts for `MaxResults`: "Maximum
/// number of results to include in the response. Value range is 1 to 1000"
/// (`ListDeadLetterSourceQueuesRequest$MaxResults`) — the same range `ListQueues`
/// publishes for its own page size, so the two name ONE value rather than two that have
/// to be kept equal. Refused rather than clamped, as every page size this service takes
/// is.
pub const LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE: (i32, i32) =
    LIST_QUEUES_MAX_RESULTS_RANGE;

/// How many source queues `ListDeadLetterSourceQueues` reports when the caller names no
/// `MaxResults`: "If you do not set MaxResults, the response includes a maximum of 1,000
/// results" (`ListDeadLetterSourceQueues`), which is the upper end of the range that
/// parameter publishes. It is at least [`MAX_QUEUES`], so an unparameterised request
/// cannot leave a source queue behind.
pub const LIST_DEAD_LETTER_SOURCE_QUEUES_DEFAULT_RESULTS: usize =
    LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE.1 as usize;

/// Where in a listing the `NextToken` a request carried continues it, or the text of the
/// refusal a token this listing never issued is answered with.
///
/// Every paged listing of this service carries its cursor the same way: the COUNT of
/// entries already handed over. `total` is how many entries the listing holds now, and a
/// token that is not such a count — unparseable, or past the end of the listing it claims
/// to continue — is refused rather than silently treated as "start again", which would
/// loop a paging client forever. A request carrying no token starts at the beginning.
///
/// One place for the rule and its wording, so two listings cannot drift on either.
pub fn count_cursor(parameter: &str, token: Option<&str>, total: usize) -> Result<usize, String> {
    match token {
        None => Ok(0),
        Some(token) => match token.parse() {
            Ok(start) if start <= total => Ok(start),
            _ => Err(invalid_parameter_message(
                token,
                parameter,
                "it was not issued by this listing.",
            )),
        },
    }
}

/// The `NextToken` a page that began at `start` and handed over `handed` of `total`
/// entries answers with: the count the NEXT page continues from, and nothing at all for a
/// page that reached the end of the listing, which is how a paging client stops.
pub fn next_count_cursor(start: usize, handed: usize, total: usize) -> Option<String> {
    let next = start + handed;
    (next < total).then(|| next.to_string())
}

/// Why the value a listing's page-size parameter carried was refused, as the text of the
/// refusal a caller has to fix the request from, or `None` when `requested` is inside the
/// inclusive `(min, max)` that parameter publishes.
///
/// One template for every listing of this service — `ListQueues`'
/// [`LIST_QUEUES_MAX_RESULTS_RANGE`] and `ListMessageMoveTasks`' own documented upper
/// limit — so two operations cannot drift on how they word the same refusal. The wording
/// is how Amazon SQS words a parameter whose value it will not take: the value, the
/// parameter it was given for, and the reason it was refused.
pub fn max_results_refusal(parameter: &str, (min, max): (i32, i32), requested: i32) -> Option<String> {
    (requested < min || requested > max).then(|| {
        invalid_parameter_message(
            &requested.to_string(),
            parameter,
            &format!("must be between {min} and {max}."),
        )
    })
}

// The per-queue tag bounds live with the other tag quotas above:
// [`MAX_TAGS_PER_QUEUE`], [`MAX_TAG_KEY_LENGTH`] and [`MAX_TAG_VALUE_LENGTH`].

/// How many attributes one message may carry. Amazon SQS Developer Guide: "Each message
/// can have up to 10 attributes."
pub const MAX_MESSAGE_ATTRIBUTES: usize = 10;

/// How many receipt handles one queue remembers as deleted (see
/// [`Queue::deleted_handles`]).
///
/// A delete is idempotent, so the handle a delete consumed outlives the message it named;
/// past this bound the queue forgets the handles it retired first. Like a deduplication
/// token, a retired handle is derived data whose only effect is answering a repeat of a
/// delete that already succeeded, so forgetting one loses nothing a client was told had
/// been accepted.
pub const MAX_DELETED_HANDLES_PER_QUEUE: usize = MAX_MESSAGES_PER_QUEUE;

/// How many deduplication tokens one queue remembers. A token outlives the message it
/// suppressed ("Amazon SQS continues to keep track of the message deduplication ID even
/// after the message is received and deleted"), so the message count does not bound it:
/// past this, the queue forgets the oldest tokens first. Unlike a message, a token is
/// derived data whose only effect is suppressing a resend inside
/// [`DEDUPLICATION_INTERVAL`], so forgetting one loses nothing a client was told had
/// been accepted — which is why this bound is enforced by eviction and the message
/// bounds are not.
pub const MAX_DEDUPLICATION_TOKENS_PER_QUEUE: usize = 1_000;

/// How many `ReceiveRequestAttemptId` batches one queue remembers. Each one lapses with
/// [`RECEIVE_REQUEST_ATTEMPT_INTERVAL`] and names at most the messages one receive can
/// hand over, and its only effect is replaying that batch to a retry — derived data, so
/// past this bound the queue forgets the oldest attempts first rather than refusing the
/// receive, exactly as it forgets the oldest deduplication tokens.
pub const MAX_RECEIVE_ATTEMPTS_PER_QUEUE: usize = 1_000;

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

/// Why a visibility change was refused. Each handler renders it as its own modelled error,
/// so the wire code is the one that operation declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityRefusal {
    /// The value is not formed as a receipt handle at all, so no receive can have minted
    /// it and it names no message of any queue.
    MalformedHandle,
    /// It IS a receipt handle, but this queue holds no message under it: the message was
    /// deleted, purged or expired, or a later receive superseded the handle.
    NoSuchMessage,
    /// The message the handle names is not in flight, so it has no visibility timeout to
    /// change.
    NotInFlight,
    /// The span asked for reaches past what ONE receive may hide the message for, counted
    /// from that receive.
    BeyondLimit,
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
    /// The message system attributes the sender supplied — "currently, the only
    /// supported message system attribute is AWSTraceHeader", which a receive reports
    /// under that name when the request selects it.
    pub message_system_attributes: BTreeMap<String, MessageSystemAttributeValue>,
    /// The `MessageGroupId` the send carried, if any: the group whose messages a FIFO
    /// queue hands over "one at a time, in strict order". A standard queue stores it
    /// too — the send may carry it there — but does not order by it.
    pub message_group_id: Option<String>,
    /// The `MessageDeduplicationId` this message was accepted under, either the one the
    /// send supplied or the one `ContentBasedDeduplication` derived from the body. Only
    /// a FIFO queue has one, and it "is available to the consumer of the message".
    pub message_deduplication_id: Option<String>,
    /// The sequence number a FIFO queue assigned this message; `None` on a standard
    /// queue, which assigns none.
    pub sequence_number: Option<String>,
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
    /// When it was LAST handed to a consumer, which is the instant the receipt handle
    /// that receive minted was minted at. The visibility a caller may still ask for is
    /// counted from here, so a change of this message's visibility timeout cannot keep
    /// it hidden for longer than the span one receive may hide it for.
    pub last_received_at: Option<SystemTime>,
    /// The handle of the message movement task that still owes this message's delivery
    /// into the queue holding it: set while the task has not reached its
    /// [`available_at`](Self::available_at) slot, and cleared by
    /// [`Queue::materialise_due`] once the clock has. A cancelled task takes back
    /// exactly the messages still carrying its handle, and leaves the ones it has
    /// already delivered where they are.
    pub moved_by_task: Option<String>,
}

impl StoredMessage {
    /// Everything this message retains, in bytes: the body plus both attribute maps,
    /// which hold caller-supplied names, data types, strings and blobs. Amazon SQS
    /// counts message attributes toward a message's size and system attributes not at
    /// all, but this emulator's byte budgets exist to bound the store, so they count
    /// every byte a sender can make it hold.
    pub fn retained_bytes(&self) -> usize {
        self.body
            .len()
            .saturating_add(message_attribute_bytes(&self.message_attributes))
            .saturating_add(message_system_attribute_bytes(&self.message_system_attributes))
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

    /// The message group this message belongs to, as the empty group when it carries no
    /// `MessageGroupId` — which is every message of a standard queue that named none.
    pub fn group(&self) -> &str {
        self.message_group_id.as_deref().unwrap_or_default()
    }
}

/// What a queue remembers about a deduplication token it has already accepted: the
/// message that token admitted, so a resend inside [`DEDUPLICATION_INTERVAL`] is
/// "accepted successfully but aren't delivered" and still answers for that message.
#[derive(Debug, Clone)]
pub struct Deduplication {
    /// The `MessageId` of the message the token first admitted.
    pub message_id: String,
    /// The sequence number that message was assigned.
    pub sequence_number: String,
    /// When the token was accepted; the interval runs from here.
    pub accepted_at: SystemTime,
}

/// One message a receive handed over under a `ReceiveRequestAttemptId`, as the retry
/// carrying that token has to see it again.
#[derive(Debug, Clone)]
pub struct DeliveredMessage {
    /// The `MessageId` handed over.
    pub message_id: String,
    /// The receipt handle minted for it: "subsequent calls with the same
    /// ReceiveRequestAttemptId return the same messages and receipt handles".
    pub receipt_handle: String,
    /// The instant that receive made the message invisible until. The clock may lapse
    /// it, but only a caller can move it elsewhere, which is what makes a visibility
    /// change recognisable — see [`Queue::receive_attempt_was_modified`].
    pub invisible_until: SystemTime,
}

/// The batch one `ReceiveRequestAttemptId` handed over, so a retry carrying that token
/// "retrieve[s] the same set of messages, even if their visibility timeout has not yet
/// expired".
#[derive(Debug, Clone)]
pub struct ReceiveAttempt {
    /// The messages handed over, in the order they were handed over.
    pub delivered: Vec<DeliveredMessage>,
    /// When the receive was served; [`RECEIVE_REQUEST_ATTEMPT_INTERVAL`] runs from here.
    pub served_at: SystemTime,
}

/// Per-service state for com-amazonaws-sqs-amazonsqs. A handler reaches it through [`service_state`].
///
/// `queues` is a `BTreeMap` rather than a `HashMap` so iteration is in name order:
/// the order a listing pages over has to be the same on every call.
#[derive(Debug, Default)]
pub struct ComAmazonawsSqsAmazonsqsState {
    /// Every queue that exists, keyed by `QueueName`.
    pub queues: BTreeMap<String, Queue>,
    /// The message movement tasks `StartMessageMoveTask` has started, oldest first.
    /// Bounded per source queue by [`MAX_MESSAGE_MOVE_TASKS_PER_QUEUE`].
    pub message_move_tasks: Vec<MessageMoveTask>,
    /// The names `DeleteQueue` has removed, each with the instant it was removed: a name
    /// stays unavailable to `CreateQueue` for [`QUEUE_DELETED_RECENTLY_INTERVAL`] after
    /// its queue was deleted. Entries lapse with that interval and are bounded by
    /// [`MAX_RECENTLY_DELETED_NAMES`].
    pub recently_deleted: BTreeMap<String, SystemTime>,
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
    /// The receipt handles this queue minted and a delete has since CONSUMED, in the
    /// order they were retired. A delete is idempotent — "the request succeeds, but the
    /// message might not be deleted" — so a handle whose message a delete already removed
    /// still names a deletion this queue has performed, which is what makes a repeat of
    /// that delete succeed instead of being refused. Bounded by
    /// [`MAX_DELETED_HANDLES_PER_QUEUE`]; a purge empties it with the message plane.
    pub deleted_handles: VecDeque<String>,
    /// The deduplication tokens this queue has accepted, keyed by
    /// `(deduplication scope, token)` — see [`Queue::deduplication_key`]. Only a FIFO
    /// queue writes here; entries lapse with [`DEDUPLICATION_INTERVAL`] and are bounded
    /// by [`MAX_DEDUPLICATION_TOKENS_PER_QUEUE`].
    pub deduplication: BTreeMap<(String, String), Deduplication>,
    /// The batch each `ReceiveRequestAttemptId` this queue has served handed over, keyed
    /// by that token. Only a FIFO receive writes here, and only when it handed something
    /// over; entries lapse with [`RECEIVE_REQUEST_ATTEMPT_INTERVAL`] and are bounded by
    /// [`MAX_RECEIVE_ATTEMPTS_PER_QUEUE`].
    pub receive_attempts: BTreeMap<String, ReceiveAttempt>,
    /// How many messages this queue has sequenced. The sequence number of the next one
    /// is derived from it, so "SequenceNumber continues to increase" across the queue
    /// and therefore across each of its message groups.
    pub sequenced: u128,
    pub created_at: SystemTime,
    /// When an attribute of this queue last changed; equals `created_at` until a
    /// `SetQueueAttributes` changes something.
    pub last_modified_at: SystemTime,
    /// When this queue last had a `PurgeQueue` accepted, if it ever has: a further
    /// purge inside [`PURGE_QUEUE_INTERVAL`] of that instant is what
    /// `PurgeQueueInProgress` reports.
    pub purged_at: Option<SystemTime>,
}

impl Queue {
    /// Every attribute this queue has a value for: its stored configuration, the three
    /// the record itself derives (`QueueArn`, `CreatedTimestamp`,
    /// `LastModifiedTimestamp`), the three that count the messages it holds and the
    /// encryption the queue is under ([`encryption::report`], since a queue is always
    /// on one of the two server-side encryption options whether or not its create named
    /// one).
    ///
    /// The two timestamps are reported "in seconds (epoch time)", per their own
    /// entries in the `AttributeNames` documentation.
    pub fn reported_attributes(&self, now: SystemTime) -> BTreeMap<String, String> {
        let mut reported = self.attributes.clone();
        encryption::report(&self.attributes, &mut reported);
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
        self.boolean(FIFO_QUEUE_ATTRIBUTE)
    }

    /// This queue's access policy — the document its `Policy` attribute carries — or
    /// `None` when it carries none, which is a queue no resource-based statement is in
    /// force for.
    pub fn policy(&self) -> Option<&str> {
        self.attributes.get(POLICY).map(String::as_str)
    }

    /// The customer master key this queue's messages are encrypted under, or `None` for
    /// a queue on the Amazon SQS owned key — see
    /// [`encryption::configured_master_key`]. A call that has to use the key asks
    /// [`encryption::authorize_master_key`] about it first.
    pub fn master_key_id(&self) -> Option<&str> {
        encryption::configured_master_key(&self.attributes)
    }

    /// Whether boolean attribute `name` is set on this queue. "Valid values are true
    /// and false", so anything else — including no value at all — is false.
    pub fn boolean(&self, name: &str) -> bool {
        self.attributes
            .get(name)
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

    /// Whether this queue derives a message's deduplication token from its body rather
    /// than requiring the sender to supply one.
    pub fn content_based_deduplication(&self) -> bool {
        self.boolean(CONTENT_BASED_DEDUPLICATION)
    }

    /// The key the deduplication token `token` of a message in group `group` is
    /// remembered under.
    ///
    /// `DeduplicationScope` "specifies whether message deduplication occurs at the
    /// message group or queue level": under `messageGroup` the same token in two groups
    /// is two tokens, so the group is part of the key; at queue level — the default —
    /// the token alone is the key, whatever group it arrived in.
    pub fn deduplication_key(&self, group: &str, token: &str) -> (String, String) {
        let per_group = self
            .attributes
            .get(DEDUPLICATION_SCOPE)
            .is_some_and(|value| value == DEDUPLICATION_SCOPE_MESSAGE_GROUP);
        let scope = if per_group {
            group.to_string()
        } else {
            String::new()
        };
        (scope, token.to_string())
    }

    /// Remember that `key` admitted the message `record` names, forgetting the oldest
    /// tokens if that takes the queue past
    /// [`MAX_DEDUPLICATION_TOKENS_PER_QUEUE`](crate::state::MAX_DEDUPLICATION_TOKENS_PER_QUEUE).
    pub fn record_deduplication(&mut self, key: (String, String), record: Deduplication) {
        self.deduplication.insert(key, record);
        while self.deduplication.len() > MAX_DEDUPLICATION_TOKENS_PER_QUEUE {
            let Some(oldest) = self
                .deduplication
                .iter()
                .min_by_key(|(_, remembered)| remembered.accepted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.deduplication.remove(&oldest);
        }
    }

    /// Remember that the receive served under `attempt_id` handed `attempt` over,
    /// forgetting the oldest attempts if that takes the queue past
    /// [`MAX_RECEIVE_ATTEMPTS_PER_QUEUE`](crate::state::MAX_RECEIVE_ATTEMPTS_PER_QUEUE).
    pub fn record_receive_attempt(&mut self, attempt_id: String, attempt: ReceiveAttempt) {
        self.receive_attempts.insert(attempt_id, attempt);
        while self.receive_attempts.len() > MAX_RECEIVE_ATTEMPTS_PER_QUEUE {
            let Some(oldest) = self
                .receive_attempts
                .iter()
                .min_by_key(|(_, remembered)| remembered.served_at)
                .map(|(attempt_id, _)| attempt_id.clone())
            else {
                break;
            };
            self.receive_attempts.remove(&oldest);
        }
    }

    /// Whether the batch `attempt` handed over has been modified since, so it can no
    /// longer be handed over again.
    ///
    /// A retry works "if none of the messages have been modified (deleted or had their
    /// visibility changes)": a message the queue no longer holds was deleted, and one
    /// whose receipt handle or invisible-until instant is no longer the one the receive
    /// left had its visibility changed. An instant the CLOCK lapsed is not a
    /// modification — nobody touched the message, so the retry still owes the same batch.
    pub fn receive_attempt_was_modified(&self, attempt: &ReceiveAttempt) -> bool {
        attempt.delivered.iter().any(|delivered| {
            let held = self
                .messages
                .iter()
                .find(|message| message.message_id == delivered.message_id);
            let Some(message) = held else {
                return true;
            };
            self.handles.get(&delivered.receipt_handle) != Some(&delivered.message_id)
                || message
                    .invisible_until
                    .is_some_and(|until| until != delivered.invisible_until)
        })
    }

    /// Mint the next sequence number for this queue: a decimal integer inside 128 bits
    /// that is larger than every one it has already assigned.
    pub fn next_sequence_number(&mut self) -> String {
        self.sequenced = self.sequenced.saturating_add(1);
        SEQUENCE_NUMBER_BASE
            .saturating_add(self.sequenced)
            .to_string()
    }

    /// The indexes, in the order they are to be handed over, of at most `max` messages
    /// this queue may deliver at `now`.
    ///
    /// A standard queue hands over whatever is visible, oldest first. A FIFO queue
    /// hands over each message group's messages "one at a time, in strict order,
    /// ensuring that no two messages from the same group are processed simultaneously":
    /// the oldest message of a group that is not visible — because it is in flight with
    /// another consumer, or because its delay has not elapsed — stops that group, so
    /// nothing behind it is handed over until it is deleted or becomes visible again.
    /// Other groups are unaffected, which is what lets several ordered streams share
    /// one queue.
    pub fn delivery_order(&self, now: SystemTime, max: usize) -> Vec<usize> {
        let fifo = self.is_fifo();
        let mut stopped: BTreeSet<&str> = BTreeSet::new();
        let mut taking: Vec<usize> = Vec::new();
        for (index, message) in self.messages.iter().enumerate() {
            if taking.len() >= max {
                break;
            }
            if fifo && stopped.contains(message.group()) {
                continue;
            }
            if message.visibility(now) == Visibility::Visible {
                taking.push(index);
            } else if fifo {
                stopped.insert(message.group());
            }
        }
        taking
    }

    /// Materialise everything the CLOCK already made due on this queue: drop messages
    /// whose retention period has elapsed, return messages whose visibility timeout has
    /// lapsed to the visible set, and forget deduplication tokens whose interval has
    /// run out. All three were owed at `now` whether or not anybody called, which is
    /// why a reporting operation may apply them.
    pub fn materialise_due(&mut self, now: SystemTime) {
        let retention = seconds(self.integer(MESSAGE_RETENTION_PERIOD));
        let mut expired: Vec<String> = Vec::new();
        self.messages.retain(|message| {
            let due = lapsed(message.sent_at, retention, now);
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
            // A message whose delivery slot has passed has been moved: the delivery was
            // owed at that instant whether or not anybody called, so no movement task
            // still owes it and no cancellation can take it back.
            if now >= message.available_at {
                message.moved_by_task = None;
            }
        }
        // A token suppresses a resend only "during the 5-minute deduplication
        // interval"; past it, "Amazon SQS can't detect duplicate messages".
        self.deduplication.retain(|_, remembered| {
            !lapsed(remembered.accepted_at, DEDUPLICATION_INTERVAL, now)
        });
        // "You can use ReceiveRequestAttemptId only for 5 minutes after a ReceiveMessage
        // action", so past that a token names no batch and a receive carrying it is a
        // fresh one.
        self.receive_attempts.retain(|_, remembered| {
            remembered
                .served_at
                .checked_add(RECEIVE_REQUEST_ATTEMPT_INTERVAL)
                .is_none_or(|lapses| now < lapses)
        });
    }

    /// Re-derive, from this queue's CURRENT `DelaySeconds`, when each message it
    /// already holds becomes available — which is what a `DelaySeconds` change does to
    /// a FIFO queue and does not do to a standard one.
    ///
    /// "For standard queues, the per-queue delay setting is not retroactive — changing
    /// the setting doesn't affect the delay of messages already in the queue. For FIFO
    /// queues, the per-queue delay setting is retroactive — changing the setting affects
    /// the delay of messages already in the queue" (the `DelaySeconds` attribute's own
    /// documentation). A FIFO send carries no per-message delay either ("you can't set
    /// DelaySeconds per message ... only on a queue level"), so the queue's value is the
    /// only delay a stored FIFO message has and re-deriving it from the instant the
    /// message was sent loses nothing.
    ///
    /// A delay governs a message's FIRST delivery, so a message already handed to a
    /// consumer keeps the availability it was delivered under; a message a movement task
    /// still owes carries that task's delivery slot rather than a delay of its own.
    pub fn reapply_delay_seconds(&mut self) {
        if !self.is_fifo() {
            return;
        }
        let delay = self.integer(DELAY_SECONDS);
        for message in &mut self.messages {
            if message.first_received_at.is_some() || message.moved_by_task.is_some() {
                continue;
            }
            message.available_at = after(message.sent_at, delay);
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

    /// The id of the message `receipt_handle` names, or why it names none.
    ///
    /// Only a handle this queue holds TODAY names a message: a handle a later receive
    /// superseded, and one whose message has been deleted, purged or expired, name
    /// nothing, and a value that is not formed as a handle at all names nothing either —
    /// which is the distinction a caller recovers from differently.
    pub fn message_id_for_handle(
        &self,
        receipt_handle: &str,
    ) -> Result<String, ReceiptHandleRefusal> {
        if let Some(message_id) = self.handles.get(receipt_handle) {
            return Ok(message_id.clone());
        }
        Err(if is_receipt_handle(receipt_handle) {
            ReceiptHandleRefusal::NoSuchMessage
        } else {
            ReceiptHandleRefusal::Malformed
        })
    }

    /// The message `receipt_handle` names, to be changed in place, or why it names none.
    pub fn message_for_handle_mut(
        &mut self,
        receipt_handle: &str,
    ) -> Result<&mut StoredMessage, ReceiptHandleRefusal> {
        let message_id = self.message_id_for_handle(receipt_handle)?;
        self.messages
            .iter_mut()
            .find(|message| message.message_id == message_id)
            // A handle is dropped with its message, so a handle this queue holds always
            // names a message it holds; this arm is the same answer either way.
            .ok_or(ReceiptHandleRefusal::NoSuchMessage)
    }

    /// Discard everything this queue holds, and remember that it was purged at `at`.
    ///
    /// `PurgeQueue` "deletes available messages in a queue (including in-flight
    /// messages)", so no message survives whatever its visibility, and every receipt
    /// handle an earlier receive minted names a message that is gone — as does every
    /// batch a `ReceiveRequestAttemptId` remembers, so neither can be replayed against
    /// messages the purge discarded. The deduplication tokens go with them: a token's
    /// only effect is suppressing a resend of a message this queue holds, and "you
    /// can't retrieve any messages deleted from a queue", so a resend after a purge is a
    /// message the queue never accepted and is delivered rather than suppressed.
    ///
    /// The queue's configuration — its attributes, its tags and the sequence counter
    /// whose numbers "continue to increase" — is untouched: a purge empties a queue, it
    /// does not re-create it.
    pub fn purge(&mut self, at: SystemTime) {
        self.messages.clear();
        self.handles.clear();
        self.deleted_handles.clear();
        self.receive_attempts.clear();
        self.deduplication.clear();
        self.purged_at = Some(at);
    }

    /// Whether a `PurgeQueue` on this queue is refused at `now` because the purge it
    /// accepted inside [`PURGE_QUEUE_INTERVAL`] is still deleting.
    pub fn purge_in_progress(&self, now: SystemTime) -> bool {
        self.purged_at
            .is_some_and(|at| !lapsed(at, PURGE_QUEUE_INTERVAL, now))
    }

    /// Drop a message and every receipt handle that referred to it, remembering those
    /// handles as handles this queue has already deleted under — which is what makes a
    /// repeat of the same delete idempotent.
    pub fn remove_message(&mut self, message_id: &str) {
        self.messages
            .retain(|message| message.message_id != message_id);
        let retired: Vec<String> = self
            .handles
            .iter()
            .filter(|(_, id)| id.as_str() == message_id)
            .map(|(handle, _)| handle.clone())
            .collect();
        self.handles.retain(|_, id| id != message_id);
        for handle in retired {
            self.remember_deleted_handle(handle);
        }
    }

    /// Remember `receipt_handle` as one whose message is gone, bounded by
    /// [`MAX_DELETED_HANDLES_PER_QUEUE`]: past the bound the handles retired earliest are
    /// forgotten first.
    fn remember_deleted_handle(&mut self, receipt_handle: String) {
        if self.deleted_handle(&receipt_handle) {
            return;
        }
        self.deleted_handles.push_back(receipt_handle);
        while self.deleted_handles.len() > MAX_DELETED_HANDLES_PER_QUEUE {
            self.deleted_handles.pop_front();
        }
    }

    /// Whether `receipt_handle` is one this queue minted and a delete has already
    /// consumed: it names a message this queue no longer holds, so there is nothing left
    /// for a further delete to do.
    pub fn deleted_handle(&self, receipt_handle: &str) -> bool {
        self.deleted_handles
            .iter()
            .any(|handle| handle == receipt_handle)
    }

    /// Give the message a receipt handle names a new visibility timeout, counted from `now`:
    /// "if you send a ChangeMessageVisibility call with VisibilityTimeout set to 10 seconds,
    /// the 10 seconds begin to count from the time that you make the
    /// ChangeMessageVisibility call."
    ///
    /// `Err` is why the change was refused, which each handler renders as its own modelled
    /// error (or, in a batch, as that entry's own result). The handle decides the first two:
    /// a value that is not a receipt handle at all is a different fault from one that is,
    /// but names no message this queue still holds — see
    /// [`message_for_handle_mut`](Self::message_for_handle_mut). Past it, a message that is
    /// not IN FLIGHT has no visibility timeout to change — "the specified message isn't in
    /// flight" — and the span is bounded by what ONE receive may hide a message for, since
    /// "if you attempt to set the VisibilityTimeout to a value greater than the maximum time
    /// left, Amazon SQS returns an error. Amazon SQS doesn't automatically recalculate and
    /// increase the timeout to the maximum remaining time." The caller materialises what the
    /// clock already owed first, so a message whose timeout has lapsed is not in flight here.
    pub fn change_visibility(
        &mut self,
        receipt_handle: &str,
        timeout: i64,
        now: SystemTime,
    ) -> Result<(), VisibilityRefusal> {
        let message = self
            .message_for_handle_mut(receipt_handle)
            .map_err(|refusal| match refusal {
                ReceiptHandleRefusal::Malformed => VisibilityRefusal::MalformedHandle,
                ReceiptHandleRefusal::NoSuchMessage => VisibilityRefusal::NoSuchMessage,
            })?;
        if message.visibility(now) != Visibility::InFlight {
            return Err(VisibilityRefusal::NotInFlight);
        }
        // The ceiling of the span one receive may hide a message for is the ceiling of the
        // range the timeout itself takes, and it is counted from the receipt handle's own
        // receive — the LAST one, since that is the receive whose handle this call carries
        // and an earlier handle names no message any more, so counting from the first
        // receive would charge this consumer for a delivery it does not hold. Counted in
        // whole seconds, the unit the timeout is in.
        let (_, limit) = integer_bounds(VISIBILITY_TIMEOUT);
        let held = message.last_received_at.map_or(0, |received_at| {
            now.duration_since(received_at)
                .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        });
        if held.saturating_add(timeout) > limit {
            return Err(VisibilityRefusal::BeyondLimit);
        }
        // A timeout of zero makes the message visible at once, which is how a consumer hands
        // one back; anything else holds it for that long from now.
        message.invisible_until = Some(after(now, timeout));
        Ok(())
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
        self.batch_rejection(1, message_bytes, store_bytes)
    }

    /// Why this queue cannot take `count` more messages retaining `message_bytes`
    /// together, given the store already holds `store_bytes` — or `None` when it can.
    ///
    /// The whole batch is judged at once, so a call that adds several messages (a
    /// batch send, or the messages a message movement task hands over) is either
    /// accepted entirely or refused entirely, never half-applied.
    pub fn batch_rejection(
        &self,
        count: usize,
        message_bytes: usize,
        store_bytes: usize,
    ) -> Option<String> {
        if self.messages.len().saturating_add(count) > MAX_MESSAGES_PER_QUEUE {
            return Some(format!(
                "This emulator holds at most {MAX_MESSAGES_PER_QUEUE} messages per queue; \
                 queue {} cannot take {count} more. Delete messages from it first.",
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

    // -----------------------------------------------------------------------
    // The queue's access policy
    // -----------------------------------------------------------------------

    /// The access policy document this queue carries, as the JSON object a policy is,
    /// with the members a generated one declares filled in where the stored value has
    /// none.
    ///
    /// A queue that carries no `Policy` attribute, or one whose value is not a JSON
    /// object, grants nothing: the document a grant is added to is then the generated
    /// one, whose `Id` is this queue's ARN and the suffix a generated queue policy
    /// carries.
    fn policy_document(&self) -> ::serde_json::Map<String, ::serde_json::Value> {
        let stored = self
            .attributes
            .get(POLICY)
            .and_then(|policy| ::serde_json::from_str::<::serde_json::Value>(policy).ok());
        let mut document = match stored {
            Some(::serde_json::Value::Object(members)) => members,
            _ => ::serde_json::Map::new(),
        };
        document
            .entry(POLICY_VERSION_MEMBER)
            .or_insert_with(|| POLICY_VERSION.into());
        document
            .entry(POLICY_ID_MEMBER)
            .or_insert_with(|| format!("{}{DEFAULT_POLICY_ID_SUFFIX}", self.arn).into());
        document
    }

    /// The statements this queue's policy carries, in policy order.
    ///
    /// `Statement` is "an array of individual statements", and the policy language also
    /// takes a lone statement object, so both forms answer the same list here and
    /// neither is lost by a grant that appends to it.
    fn policy_statements(&self) -> Vec<::serde_json::Value> {
        match self.policy_document().get(POLICY_STATEMENT_MEMBER) {
            Some(::serde_json::Value::Array(statements)) => statements.clone(),
            Some(statement @ ::serde_json::Value::Object(_)) => vec![statement.clone()],
            _ => Vec::new(),
        }
    }

    /// The label of every permission this queue's policy grants, in policy order: a
    /// permission's label is the `Sid` of the statement that carries it, which is what
    /// makes a label the handle `RemovePermission` revokes one by.
    pub fn permission_labels(&self) -> Vec<String> {
        self.policy_statements()
            .iter()
            .filter_map(|statement| {
                statement
                    .get(POLICY_SID_MEMBER)
                    .and_then(::serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
            .collect()
    }

    /// The policy document this queue would carry once `label` granted `actions` on it
    /// to each account in `account_numbers`, or why that grant cannot be served.
    ///
    /// The statement is added to whatever the queue's policy already carries, so a
    /// policy a caller uploaded keeps every statement it had and a second grant does not
    /// displace the first.
    pub fn granted_policy(
        &self,
        label: &str,
        account_numbers: &[String],
        actions: &[String],
    ) -> Result<String, PermissionRefusal> {
        if let Some(reason) = permission_label_refusal(label) {
            return Err(PermissionRefusal::Parameter(reason));
        }
        if let Some(reason) = principals_refusal(account_numbers) {
            return Err(reason);
        }
        if let Some(reason) = actions_refusal(actions) {
            return Err(reason);
        }
        let labels = self.permission_labels();
        if labels.iter().any(|existing| existing == label) {
            return Err(PermissionRefusal::Parameter(invalid_parameter_message(
                label,
                PERMISSION_LABEL_PARAMETER,
                LABEL_ALREADY_EXISTS,
            )));
        }
        let mut statements = self.policy_statements();
        if statements.len() >= MAX_POLICY_STATEMENTS {
            return Err(PermissionRefusal::Limit(format!(
                "An Amazon SQS policy can have a maximum of {MAX_POLICY_STATEMENTS} statements, \
                 and the policy of queue {} already has {}.",
                self.name,
                statements.len()
            )));
        }
        statements.push(permission_statement(
            label,
            account_numbers,
            actions,
            &self.arn,
        ));
        let document = self.policy_with_statements(statements);
        // The policy is stored for the queue's lifetime, so the document a grant leaves
        // behind is held to the ceiling AWS publishes for it — "The policy size is
        // limited to 8,192 bytes" (the `Policy` attribute's own documentation) — which
        // is the same bound an uploaded policy is held to.
        if document.len() > MAX_ATTRIBUTE_VALUE_BYTES {
            return Err(PermissionRefusal::Limit(format!(
                "The policy size is limited to {MAX_ATTRIBUTE_VALUE_BYTES} bytes, and the policy \
                 of queue {} would be {} bytes.",
                self.name,
                document.len()
            )));
        }
        Ok(document)
    }

    /// This queue's policy document carrying `statements`, as the string the `Policy`
    /// attribute holds — or no document at all when nothing is left to grant, which is
    /// what makes a queue whose last permission was revoked carry no policy again.
    fn policy_with_statements(&self, statements: Vec<::serde_json::Value>) -> String {
        let mut document = self.policy_document();
        document.insert(
            POLICY_STATEMENT_MEMBER.to_string(),
            ::serde_json::Value::Array(statements),
        );
        ::serde_json::Value::Object(document).to_string()
    }

    /// Record `policy` as this queue's access policy as of `now`; `None` leaves the
    /// queue carrying no policy at all.
    fn set_policy(&mut self, policy: Option<String>, now: SystemTime) {
        match policy {
            Some(policy) => {
                self.attributes.insert(POLICY.to_string(), policy);
            }
            None => {
                self.attributes.remove(POLICY);
            }
        }
        self.last_modified_at = now;
    }

    /// Grant the permission `policy` describes, as of `now`.
    pub fn grant_permission(&mut self, policy: String, now: SystemTime) {
        self.set_policy(Some(policy), now);
    }

    /// Revoke the permission labelled `label`, as of `now`, answering whether the policy
    /// carried such a permission at all.
    ///
    /// Only the statement that label identifies goes; every other statement stays, and a
    /// policy left with no statement is dropped rather than stored empty, so a queue
    /// whose last permission was revoked reports no `Policy` exactly as one that was
    /// never granted a permission does.
    pub fn revoke_permission(&mut self, label: &str, now: SystemTime) -> bool {
        let statements = self.policy_statements();
        let remaining: Vec<::serde_json::Value> = statements
            .iter()
            .filter(|statement| {
                statement
                    .get(POLICY_SID_MEMBER)
                    .and_then(::serde_json::Value::as_str)
                    != Some(label)
            })
            .cloned()
            .collect();
        if remaining.len() == statements.len() {
            return false;
        }
        let policy = (!remaining.is_empty()).then(|| self.policy_with_statements(remaining));
        self.set_policy(policy, now);
        true
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

    /// Where `receipt_handle` came from, for the queue at `queue_url` that cannot resolve
    /// it: another queue of this store minted it, or no queue did and it is not a receipt
    /// handle at all. A handle a delete has already consumed still counts as minted by
    /// the queue that issued it.
    pub fn receipt_handle_origin<'a>(
        &self,
        queue_url: &str,
        receipt_handle: &'a str,
    ) -> ReceiptHandleOrigin<'a> {
        let elsewhere = self.queues.values().any(|queue| {
            queue.url != queue_url
                && (queue.handles.contains_key(receipt_handle)
                    || queue.deleted_handle(receipt_handle))
        });
        if elsewhere {
            ReceiptHandleOrigin::AnotherQueue(receipt_handle)
        } else {
            ReceiptHandleOrigin::Unrecognised
        }
    }

    /// Remove the queue a `QueueUrl` addresses, with every message and handle it held,
    /// and remember that its NAME was deleted at `at` — which is what makes a re-create
    /// of the same name inside [`QUEUE_DELETED_RECENTLY_INTERVAL`] refusable.
    pub fn remove_queue_by_url(&mut self, queue_url: &str, at: SystemTime) -> Option<Queue> {
        let name = queue_name_from_url(queue_url)?.to_string();
        let removed = self.queues.remove(&name)?;
        self.forget_lapsed_deletions(at);
        self.recently_deleted.insert(name, at);
        // The ledger is bounded; the oldest record goes first, so the names deleted most
        // recently — the ones a re-create is most likely to name — are the ones kept.
        while self.recently_deleted.len() > MAX_RECENTLY_DELETED_NAMES {
            if let Some(oldest) = self
                .recently_deleted
                .iter()
                .min_by_key(|(_, deleted_at)| **deleted_at)
                .map(|(name, _)| name.clone())
            {
                self.recently_deleted.remove(&oldest);
            }
        }
        Some(removed)
    }

    /// Forget every deletion record whose [`QUEUE_DELETED_RECENTLY_INTERVAL`] has
    /// elapsed by `now`: the name became available again at that instant whether or not
    /// anybody called, so materialising it is owed by the clock rather than by this call.
    pub fn forget_lapsed_deletions(&mut self, now: SystemTime) {
        self.recently_deleted
            .retain(|_, deleted_at| !lapsed(*deleted_at, QUEUE_DELETED_RECENTLY_INTERVAL, now));
    }

    /// When the name `name` was deleted, if a delete removed it inside
    /// [`QUEUE_DELETED_RECENTLY_INTERVAL`] of `now`.
    ///
    /// The ledger is keyed by name and says nothing about whether a queue carries that
    /// name again, so a caller that needs both — `CreateQueue` — resolves the live queue
    /// first and reaches here only for a name no queue answers to.
    pub fn deleted_recently(&self, name: &str, now: SystemTime) -> Option<SystemTime> {
        self.recently_deleted
            .get(name)
            .copied()
            .filter(|deleted_at| !lapsed(*deleted_at, QUEUE_DELETED_RECENTLY_INTERVAL, now))
    }

    /// The names of the queues whose `RedrivePolicy` names the queue called
    /// `dead_letter_queue_name` as their dead-letter target, in queue-name order.
    ///
    /// These are that queue's dead-letter SOURCE queues, and having at least one is
    /// what makes it "configured as a dead-letter queue (DLQ) of other Amazon SQS
    /// queues" — the only kind of source `StartMessageMoveTask` supports.
    pub fn dead_letter_source_queue_names(&self, dead_letter_queue_name: &str) -> Vec<String> {
        self.queues
            .values()
            .filter(|queue| {
                queue
                    .attributes
                    .get(REDRIVE_POLICY)
                    .and_then(|policy| dead_letter_target_arn(policy))
                    .and_then(|arn| queue_name_from_arn(&arn).map(str::to_string))
                    .is_some_and(|target| target == dead_letter_queue_name)
            })
            .map(|queue| queue.name.clone())
            .collect()
    }

    /// The message movement task that is still running for the queue called
    /// `source_queue_name` at `now`, if any: "Only one active message movement task is
    /// supported per queue at any given time" (`StartMessageMoveTask`).
    pub fn active_message_move_task(
        &self,
        source_queue_name: &str,
        now: SystemTime,
    ) -> Option<&MessageMoveTask> {
        self.message_move_tasks.iter().find(|task| {
            task.source_queue_name == source_queue_name && task.status(now) == MESSAGE_MOVE_RUNNING
        })
    }

    /// The message movement task the handle a start call returned names, if this store
    /// still holds its record.
    pub fn message_move_task(&self, task_handle: &str) -> Option<&MessageMoveTask> {
        self.message_move_tasks
            .iter()
            .find(|task| task.task_handle == task_handle)
    }

    /// Stop the message movement task called `task_handle` at `now`, and answer how many
    /// of its messages had been moved by then.
    ///
    /// Cancelling stops the messages the task has not moved yet and leaves the ones it
    /// has already moved where they are: the messages still owed a delivery slot go back
    /// to the task's source queue, so the pair of queues holds every message it held
    /// before the task started, and the ones already handed to the destination queue stay
    /// its own. The cancellation is recorded on the task record, which is what every
    /// later reader projects the task's status and progress from.
    ///
    /// Putting messages back into a queue is bounded like every other path that adds
    /// messages to one — see [`Queue::batch_rejection`] — because the source queue may
    /// have been refilled while the task ran: what it cannot take back keeps its place in
    /// the destination queue, which still leaves the pair holding every message.
    ///
    /// `None` when no task answers to the handle.
    pub fn cancel_message_move_task(&mut self, task_handle: &str, now: SystemTime) -> Option<u64> {
        let task = self
            .message_move_tasks
            .iter_mut()
            .find(|task| task.task_handle == task_handle)?;
        task.cancelled_at = Some(now);
        let moved = task.messages_moved(now);
        let source_queue_name = task.source_queue_name.clone();
        let destination_queue_name = task.destination_queue_name.clone();

        // What the task has not moved yet is exactly what still carries its handle and
        // whose delivery slot is still ahead; a message not yet deliverable has never
        // been handed to a consumer, so no receipt handle refers to it.
        //
        // The source queue may have been refilled while the task ran, so what it takes
        // back is judged against the SAME bounds every other path that adds messages to
        // a queue is: without that, a caller looping on start / refill / cancel could
        // grow one queue past its allowance each cycle. The oldest are taken back first,
        // which is the order they would be handed out in, and a message the queue cannot
        // take back is NOT dropped — dropping one would lose a message a client was told
        // had been accepted. It stays in the place it holds in the destination queue,
        // with the task owing it nothing any more.
        let store_bytes = self.retained_message_bytes();
        let stopped: Vec<(String, usize)> = self
            .queues
            .get(&destination_queue_name)
            .map(|destination| {
                destination
                    .messages
                    .iter()
                    .filter(|message| {
                        message.moved_by_task.as_deref() == Some(task_handle)
                            && now < message.available_at
                    })
                    .map(|message| (message.message_id.clone(), message.retained_bytes()))
                    .collect()
            })
            .unwrap_or_default();
        let mut returning: BTreeSet<String> = BTreeSet::new();
        if let Some(source) = self.queues.get(&source_queue_name) {
            let mut taken_bytes = 0usize;
            for (message_id, message_bytes) in &stopped {
                let bytes = taken_bytes.saturating_add(*message_bytes);
                // The bytes leave the destination queue as they reach the source one, so
                // the store-wide budget is judged on what the move would leave behind.
                if source
                    .batch_rejection(
                        returning.len().saturating_add(1),
                        bytes,
                        store_bytes.saturating_sub(bytes),
                    )
                    .is_none()
                {
                    taken_bytes = bytes;
                    returning.insert(message_id.clone());
                }
            }
        }

        let mut moving: Vec<StoredMessage> = Vec::with_capacity(returning.len());
        if let Some(destination) = self.queues.get_mut(&destination_queue_name) {
            let mut staying: VecDeque<StoredMessage> = VecDeque::new();
            for message in std::mem::take(&mut destination.messages) {
                if returning.contains(&message.message_id) {
                    moving.push(message);
                } else if message.moved_by_task.as_deref() == Some(task_handle)
                    && now < message.available_at
                {
                    // This one the source queue cannot take back: its delivery stands, so
                    // it keeps its place here, is deliverable from now, and no later
                    // cancellation can take it.
                    staying.push_back(StoredMessage {
                        available_at: now,
                        moved_by_task: None,
                        ..message
                    });
                } else {
                    staying.push_back(message);
                }
            }
            destination.messages = staying;
        }

        // They go back in front of whatever the source queue took while the task ran,
        // keeping the order it held them in: their sends preceded all of those. Retention
        // runs from the return, which is when that queue took the message back, exactly as
        // it runs from the move for a message the task delivers.
        if let Some(source) = self.queues.get_mut(&source_queue_name) {
            for message in moving.into_iter().rev() {
                source.messages.push_front(StoredMessage {
                    sent_at: now,
                    available_at: now,
                    invisible_until: None,
                    moved_by_task: None,
                    ..message
                });
            }
        }
        Some(moved)
    }

    /// Record a started message movement task, keeping at most
    /// [`MAX_MESSAGE_MOVE_TASKS_PER_QUEUE`] records per source queue and
    /// [`MAX_MESSAGE_MOVE_TASKS`] in all.
    ///
    /// Beyond either bound the OLDEST record is dropped, not refused:
    /// `ListMessageMoveTasks` reports "the most recent message movement tasks (up to 10)
    /// under a specific source queue", so nothing a caller can still list is lost, and a
    /// caller looping on `StartMessageMoveTask` cannot grow this store without end.
    ///
    /// The per-queue bound is counted on [`MessageMoveTask::source_queue_name`] — the
    /// queue the start call RESOLVED, never a string the request supplied — so every
    /// request naming one queue shares one allowance. The store-wide bound is the
    /// backstop for the names of queues that no longer exist, which no per-queue count
    /// can limit.
    pub fn record_message_move_task(&mut self, task: MessageMoveTask) {
        let source_queue_name = task.source_queue_name.clone();
        self.message_move_tasks.push(task);
        while self
            .message_move_tasks
            .iter()
            .filter(|recorded| recorded.source_queue_name == source_queue_name)
            .count()
            > MAX_MESSAGE_MOVE_TASKS_PER_QUEUE
        {
            let Some(oldest) = self
                .message_move_tasks
                .iter()
                .position(|recorded| recorded.source_queue_name == source_queue_name)
            else {
                break;
            };
            self.message_move_tasks.remove(oldest);
        }
        while self.message_move_tasks.len() > MAX_MESSAGE_MOVE_TASKS {
            self.message_move_tasks.remove(0);
        }
    }

    /// The message movement tasks to report for the queue called `source_queue_name`:
    /// "the most recent message movement tasks (up to 10) under a specific source queue"
    /// (`ListMessageMoveTasks`), most recent FIRST and at most `max_results` of them.
    ///
    /// Every member is PROJECTED from the task record and `now`, the way
    /// [`Queue::reported_attributes`] projects a queue's reported attributes: the status
    /// and both `ApproximateNumberOfMessages*` counts are functions of the record and the
    /// clock, and each ARN is the one the queue record itself reports, so two readers
    /// cannot disagree about a task and nothing stores a progress value it would have to
    /// keep up to date. `FailureReason` is left unset because no task this store records
    /// can fail: it takes its messages when it starts, and a status it never reaches owes
    /// no reason.
    pub fn reported_message_move_tasks(
        &self,
        source_queue_name: &str,
        now: SystemTime,
        region: &str,
        max_results: usize,
    ) -> Vec<ListMessageMoveTasksResultEntry> {
        // `message_move_tasks` is append-ordered, so reversing it is start-time order,
        // newest first.
        self.message_move_tasks
            .iter()
            .rev()
            .filter(|task| task.source_queue_name == source_queue_name)
            .take(max_results)
            .map(|task| ListMessageMoveTasksResultEntry {
                task_handle: Some(task.task_handle.clone()),
                status: Some(task.status(now).to_string()),
                source_arn: Some(self.reported_queue_arn(&task.source_queue_name, region)),
                destination_arn: Some(
                    self.reported_queue_arn(&task.destination_queue_name, region),
                ),
                max_number_of_messages_per_second: task.max_number_of_messages_per_second,
                approximate_number_of_messages_moved: count_as_i64(task.messages_moved(now)),
                approximate_number_of_messages_to_move: Some(count_as_i64(
                    task.messages_to_move(now),
                )),
                failure_reason: None,
                started_timestamp: epoch_millis_number(task.started_at),
            })
            .collect()
    }

    /// The ARN to report for the queue called `name`: the one the queue record carries,
    /// or the ARN that name resolves to in `region` when no queue answers to it any more
    /// — a task record outlives the queues it names.
    fn reported_queue_arn(&self, name: &str, region: &str) -> String {
        self.queues
            .get(name)
            .map(|queue| queue.arn.clone())
            .unwrap_or_else(|| queue_arn(region, name))
    }
}

/// A message count as the `long` a response carries it in. Every count here is bounded by
/// the messages one queue may hold, so the saturation is unreachable.
fn count_as_i64(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------------
// Message movement tasks
// ---------------------------------------------------------------------------

/// The largest message movement rate `StartMessageMoveTask` accepts: "The maximum value
/// for messages per second is 500"
/// (`StartMessageMoveTaskRequest$MaxNumberOfMessagesPerSecond`).
pub const MAX_MESSAGES_PER_SECOND: i32 = 500;

/// How long after the start call the first moved message becomes deliverable in the
/// destination queue.
///
/// `StartMessageMoveTask` "starts an asynchronous task": it answers with a task handle
/// before the task has delivered anything, so this emulator owes the first delivery a
/// short span after the call rather than at it — a consumer polling the destination
/// queue straight away still sees the task running and none of its messages yet.
pub const MESSAGE_MOVE_TASK_START_DELAY: Duration = Duration::from_millis(500);

/// How many message movement task records one source queue keeps, and the most
/// `ListMessageMoveTasks` reports for it: "Gets the most recent message movement tasks
/// (up to 10) under a specific source queue" / "The upper limit is 10"
/// (`ListMessageMoveTasksRequest$MaxResults`).
pub const MAX_MESSAGE_MOVE_TASKS_PER_QUEUE: usize = 10;

/// How many message movement tasks `ListMessageMoveTasks` reports when the request names
/// no `MaxResults`: "The default is 1, which provides the most recent message movement
/// task" (`ListMessageMoveTasksRequest$MaxResults`).
pub const DEFAULT_MESSAGE_MOVE_TASK_RESULTS: usize = 1;

/// How many message movement task records the whole store holds. The per-queue bound
/// alone would still scale with the number of queue NAMES that have ever existed, which
/// a caller creating and deleting queues can grow; this bounds them all together.
pub const MAX_MESSAGE_MOVE_TASKS: usize = MAX_QUEUES * MAX_MESSAGE_MOVE_TASKS_PER_QUEUE;

/// Nanoseconds in one second, so a rate in messages per second becomes a span per
/// message without a floating-point division.
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A message movement task's status while it still has messages to deliver. The status
/// set is the member's own: "Possible values are: RUNNING, COMPLETED, CANCELLING,
/// CANCELLED, and FAILED" (`ListMessageMoveTasksResultEntry$Status`).
pub const MESSAGE_MOVE_RUNNING: &str = "RUNNING";

/// A message movement task's status once every message it took has been delivered.
pub const MESSAGE_MOVE_COMPLETED: &str = "COMPLETED";

/// A message movement task's status while its cancellation is being applied.
pub const MESSAGE_MOVE_CANCELLING: &str = "CANCELLING";

/// A message movement task's status once its cancellation has been applied.
pub const MESSAGE_MOVE_CANCELLED: &str = "CANCELLED";

/// How long a cancelled task reports [`MESSAGE_MOVE_CANCELLING`] before it reports
/// [`MESSAGE_MOVE_CANCELLED`].
///
/// `CancelMessageMoveTask` answers while the task is still winding down — the status set
/// has a distinct `CANCELLING` state before `CANCELLED`
/// (`ListMessageMoveTasksResultEntry$Status`) — so a caller listing the task straight
/// after the cancel call sees it cancelling and only later sees it cancelled. The span
/// itself is this emulator's choice, not a documented duration.
pub const MESSAGE_MOVE_CANCELLATION_SPAN: Duration = Duration::from_millis(500);

/// One message movement task `StartMessageMoveTask` started.
///
/// The record holds only what the start call RESOLVED — the queues it found, the rate and
/// how many messages the task took — and nothing a request supplied verbatim: each queue
/// is held as the `QueueName` its ARN resolved to, which is the identity every reader and
/// every bound keys on, and the ARNs a reader reports are projected from those queue
/// records rather than stored a second time. How far the task has got is computed from
/// this record and the clock by [`messages_moved`](Self::messages_moved) and
/// [`status`](Self::status), so no two readers can disagree about a task's progress.
#[derive(Debug, Clone)]
pub struct MessageMoveTask {
    /// The opaque handle the start call returned, which `CancelMessageMoveTask` names.
    pub task_handle: String,
    /// The name of the dead-letter queue the messages were taken from.
    pub source_queue_name: String,
    /// The name of the queue they are being delivered to.
    pub destination_queue_name: String,
    /// The message movement rate the caller fixed, or `None` when it left the rate to
    /// the service ("the system will optimize the rate based on the queue message
    /// backlog size").
    pub max_number_of_messages_per_second: Option<i32>,
    /// When the start call accepted the task.
    pub started_at: SystemTime,
    /// How many messages the task took from the source queue.
    pub message_count: u64,
    /// When `CancelMessageMoveTask` stopped the task, if it was cancelled: from that
    /// instant the task delivers nothing more, so its progress stops there too.
    pub cancelled_at: Option<SystemTime>,
}

impl MessageMoveTask {
    /// When the `index`-th message of this task becomes deliverable: the start delay,
    /// then one message per rate slot. With no rate fixed, every message is deliverable
    /// at the end of the start delay.
    pub fn delivery_instant(&self, index: u64) -> SystemTime {
        let first = self
            .started_at
            .checked_add(MESSAGE_MOVE_TASK_START_DELAY)
            .unwrap_or(self.started_at);
        let Some(rate) = self
            .max_number_of_messages_per_second
            .and_then(|rate| u64::try_from(rate).ok())
            .filter(|rate| *rate > 0)
        else {
            return first;
        };
        let nanos =
            u64::try_from(u128::from(index) * NANOS_PER_SECOND / u128::from(rate))
                .unwrap_or(u64::MAX);
        first
            .checked_add(Duration::from_nanos(nanos))
            .unwrap_or(first)
    }

    /// When the last message this task took becomes deliverable, which is when the task
    /// is done.
    pub fn completes_at(&self) -> SystemTime {
        self.delivery_instant(self.message_count.saturating_sub(1))
    }

    /// How many of this task's messages have been delivered by `now`.
    ///
    /// A cancelled task moved nothing after it was stopped, so its progress is measured
    /// at the instant of the cancellation rather than at `now`.
    pub fn messages_moved(&self, now: SystemTime) -> u64 {
        let now = match self.cancelled_at {
            Some(cancelled_at) if cancelled_at < now => cancelled_at,
            _ => now,
        };
        let Ok(elapsed) = now.duration_since(self.delivery_instant(0)) else {
            return 0;
        };
        let Some(rate) = self
            .max_number_of_messages_per_second
            .and_then(|rate| u64::try_from(rate).ok())
            .filter(|rate| *rate > 0)
        else {
            return self.message_count;
        };
        let delivered = u64::try_from(elapsed.as_nanos() * u128::from(rate) / NANOS_PER_SECOND)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        delivered.min(self.message_count)
    }

    /// How many of this task's messages are still to be delivered at `now`.
    pub fn messages_to_move(&self, now: SystemTime) -> u64 {
        self.message_count.saturating_sub(self.messages_moved(now))
    }

    /// This task's status at `now`: it runs until the last message it took is
    /// deliverable, and is completed from that instant on. A cancelled task is
    /// cancelling for [`MESSAGE_MOVE_CANCELLATION_SPAN`] after the cancel call and
    /// cancelled from then on, whatever it had still to deliver.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if let Some(cancelled_at) = self.cancelled_at {
            let settled = cancelled_at
                .checked_add(MESSAGE_MOVE_CANCELLATION_SPAN)
                .unwrap_or(cancelled_at);
            return if now >= settled {
                MESSAGE_MOVE_CANCELLED
            } else {
                MESSAGE_MOVE_CANCELLING
            };
        }
        if now >= self.completes_at() {
            MESSAGE_MOVE_COMPLETED
        } else {
            MESSAGE_MOVE_RUNNING
        }
    }
}

/// The `deadLetterTargetArn` a `RedrivePolicy` attribute value names, or `None` when
/// the value is not a JSON object carrying one.
///
/// "The string that includes the parameters for the dead-letter queue functionality of
/// the source queue as a JSON object", whose `deadLetterTargetArn` is "the Amazon
/// Resource Name (ARN) of the dead-letter queue to which Amazon SQS moves messages
/// after the value of maxReceiveCount is exceeded" (the `RedrivePolicy` attribute's own
/// documentation).
pub fn dead_letter_target_arn(policy: &str) -> Option<String> {
    ::serde_json::from_str::<::serde_json::Value>(policy)
        .ok()?
        .get("deadLetterTargetArn")?
        .as_str()
        .map(ToString::to_string)
}

// ---------------------------------------------------------------------------
// Queue permissions
// ---------------------------------------------------------------------------
//
// A permission is not a record of its own: `AddPermission` "generates a policy for you",
// so a granted permission IS a statement of the queue's `Policy` attribute — the same
// attribute `SetQueueAttributes` uploads a policy into and `GetQueueAttributes` reports.
// Keeping it there is what makes a grant observable by every later reader, and lets
// `RemovePermission` "revoke any permissions in the queue policy that matches the
// specified Label parameter" whether the statement came from a grant or from an upload.
// The document and the statement are assembled and taken apart by the [`Queue`] methods
// above and the helpers at the end of this module; the bounds and the wording of every
// refusal live here. The statement SHAPE is the one every service whose `AddPermission`
// generates a policy writes, and Amazon SNS's own state assembles its own copy of it;
// the two are kept apart deliberately, because each carries the statement in its own
// representation — this one as the JSON a `Policy` attribute string holds, that one as a
// typed record with the condition and passthrough members a topic policy keeps — so a
// change to the shape has to be made in both.

/// The policy language version a generated queue policy declares.
pub const POLICY_VERSION: &str = "2012-10-17";

/// The `Version` member of a policy document.
pub const POLICY_VERSION_MEMBER: &str = "Version";

/// The `Id` member of a policy document.
pub const POLICY_ID_MEMBER: &str = "Id";

/// The `Statement` member of a policy document: "an array of individual statements".
pub const POLICY_STATEMENT_MEMBER: &str = "Statement";

/// The `Sid` member of a policy statement, which carries the `Label` the permission was
/// granted under.
pub const POLICY_SID_MEMBER: &str = "Sid";

/// What the `Id` of a generated queue policy is: the queue's ARN followed by this.
pub const DEFAULT_POLICY_ID_SUFFIX: &str = "/SQSDefaultPolicy";

/// The prefix a granted action carries in the generated policy, qualifying the action
/// name with the service it belongs to.
pub const POLICY_ACTION_PREFIX: &str = "SQS:";

/// The action name that stands for every action: "Valid values: the name of any action
/// or *" (the `Actions` member's own documentation).
pub const ANY_ACTION: &str = "*";

/// The longest `Label` a permission may carry: "Maximum 80 characters"
/// (`AddPermissionRequest$Label`).
pub const MAX_PERMISSION_LABEL_LENGTH: usize = 80;

/// How many digits an Amazon Web Services account number has, which is the only
/// principal form this call takes: "Amazon SQS AddPermission does not support adding a
/// non-account principal."
pub const ACCOUNT_NUMBER_DIGITS: usize = 12;

/// How many statements one queue policy may carry — therefore how many permissions a
/// queue can have, since each grant adds one statement. The ceiling AWS publishes for a
/// queue policy ("The maximum limit is 8,192 bytes, 20 statements, 50 principals, or 10
/// conditions"), and what `OverLimit` means for this call: "AddPermission returns this
/// error if the maximum number of permissions for the queue is reached".
pub const MAX_POLICY_STATEMENTS: usize = 20;

/// How many principals one statement may name, from the same published ceiling.
pub const MAX_PRINCIPALS_PER_STATEMENT: usize = 50;

/// How many actions one statement may allow: "An Amazon SQS policy can have a maximum of
/// seven actions per statement" (`AddPermission`'s own documentation).
pub const MAX_ACTIONS_PER_STATEMENT: usize = 7;

/// The parameter a label is supplied for, as the request carries it.
pub const PERMISSION_LABEL_PARAMETER: &str = "Label";

/// The parameter a principal is supplied for, as the request carries it: the
/// `AWSAccountIds` list is flattened to repeated `AWSAccountId` entries on the wire
/// (`xmlName: AWSAccountId`, `xmlFlattened`).
pub const PRINCIPAL_PARAMETER: &str = "AWSAccountId";

/// The parameter an action is supplied for, as the request carries it: the `Actions`
/// list is flattened to repeated `ActionName` entries on the wire (`xmlName: ActionName`,
/// `xmlFlattened`).
pub const ACTION_PARAMETER: &str = "ActionName";

/// The reason a grant is refused for a label the queue's policy already grants: a label
/// is "the unique identification of the permission you're setting", so the policy cannot
/// carry two.
pub const LABEL_ALREADY_EXISTS: &str = "Already exists.";

/// The reason a revoke is refused for a label the queue's policy does not grant: there
/// is no statement for it to revoke.
pub const LABEL_NOT_FOUND: &str = "can't find label on existing policy.";

/// The action names a permission may allow, which is "the name of any action" of this
/// service (`AddPermissionRequest$Actions`), beside [`ANY_ACTION`].
///
/// Source: `smithy_query(query="list_operations",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`
/// — the operations bound to this service, in the order it reports them.
pub const ACTION_NAMES: &[&str] = &[
    "AddPermission",
    "CancelMessageMoveTask",
    "ChangeMessageVisibility",
    "ChangeMessageVisibilityBatch",
    "CreateQueue",
    "DeleteMessage",
    "DeleteMessageBatch",
    "DeleteQueue",
    "GetQueueAttributes",
    "GetQueueUrl",
    "ListDeadLetterSourceQueues",
    "ListMessageMoveTasks",
    "ListQueueTags",
    "ListQueues",
    "PurgeQueue",
    "ReceiveMessage",
    "RemovePermission",
    "SendMessage",
    "SendMessageBatch",
    "SetQueueAttributes",
    "StartMessageMoveTask",
    "TagQueue",
    "UntagQueue",
];

/// Why a permission cannot be granted, as the text of the refusal a caller has to fix
/// the request from. The two kinds answer under different error codes, which is why they
/// are told apart here rather than in a handler.
#[derive(Debug)]
pub enum PermissionRefusal {
    /// A parameter's value is not one the call accepts.
    Parameter(String),
    /// A published limit on what a queue policy may hold.
    Limit(String),
}

/// Why `label` is not a `Label` this service accepts, or `None` when it is.
///
/// The rule is the member's own: "Maximum 80 characters. Allowed characters include
/// alphanumeric characters, hyphens (-), and underscores (_)."
pub fn permission_label_refusal(label: &str) -> Option<String> {
    let length = label.chars().count();
    let well_formed = (1..=MAX_PERMISSION_LABEL_LENGTH).contains(&length)
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if well_formed {
        return None;
    }
    // The rule, not which half of it this label broke: a caller fixes the label against
    // the whole rule either way. The value is echoed only where it is already known to
    // be inside its length bound, so a refusal never hands an unbounded string back.
    let reason = format!(
        "Label can only include alphanumeric characters, hyphens, or underscores. 1 to \
         {MAX_PERMISSION_LABEL_LENGTH} in length"
    );
    Some(if length > MAX_PERMISSION_LABEL_LENGTH {
        invalid_parameter_message_without_value(PERMISSION_LABEL_PARAMETER, &reason)
    } else {
        invalid_parameter_message(label, PERMISSION_LABEL_PARAMETER, &reason)
    })
}

/// Why the principals a grant names cannot receive it, or `None` when they can.
///
/// Each one is "the Amazon Web Services account number of a principal", and nothing
/// else: "Amazon SQS AddPermission does not support adding a non-account principal."
fn principals_refusal(account_numbers: &[String]) -> Option<PermissionRefusal> {
    for account_number in account_numbers {
        let account = account_number.chars().count() == ACCOUNT_NUMBER_DIGITS
            && account_number.chars().all(|c| c.is_ascii_digit());
        if !account {
            let reason = "Amazon SQS AddPermission does not support adding a non-account \
                          principal.";
            return Some(PermissionRefusal::Parameter(
                if account_number.chars().count() > ACCOUNT_NUMBER_DIGITS {
                    invalid_parameter_message_without_value(PRINCIPAL_PARAMETER, reason)
                } else {
                    invalid_parameter_message(account_number, PRINCIPAL_PARAMETER, reason)
                },
            ));
        }
    }
    (account_numbers.len() > MAX_PRINCIPALS_PER_STATEMENT).then(|| {
        PermissionRefusal::Limit(format!(
            "An Amazon SQS policy statement can name a maximum of \
             {MAX_PRINCIPALS_PER_STATEMENT} principals, and this one names {}.",
            account_numbers.len()
        ))
    })
}

/// Why the actions a grant allows cannot be allowed, or `None` when they can.
///
/// The per-statement ceiling is refused as a PARAMETER fault rather than as a limit: the
/// condition `OverLimit` is documented for on this call is the other one — "AddPermission
/// returns this error if the maximum number of permissions for the queue is reached" —
/// and a request naming more actions than one statement may allow is a request this call
/// could never serve, whatever the queue already carries.
fn actions_refusal(actions: &[String]) -> Option<PermissionRefusal> {
    for action in actions {
        if action != ANY_ACTION && !ACTION_NAMES.contains(&action.as_str()) {
            // The action is named back QUALIFIED with the service it was looked up in,
            // the way the statement a grant would have added names it, because that is
            // the name no action of this service answers to.
            let qualified = format!("{POLICY_ACTION_PREFIX}{action}");
            return Some(PermissionRefusal::Parameter(
                if action.chars().count() > MAX_ACTION_NAME_LENGTH {
                    invalid_parameter_message_without_value(ACTION_PARAMETER, UNKNOWN_ACTION_REASON)
                } else {
                    invalid_parameter_message(&qualified, ACTION_PARAMETER, UNKNOWN_ACTION_REASON)
                },
            ));
        }
    }
    (actions.len() > MAX_ACTIONS_PER_STATEMENT).then(|| {
        PermissionRefusal::Parameter(invalid_parameter_message_without_value(
            ACTION_PARAMETER,
            &format!(
                "An Amazon SQS policy can have a maximum of {MAX_ACTIONS_PER_STATEMENT} actions \
                 per statement, and this one has {}.",
                actions.len()
            ),
        ))
    })
}

/// The reason a grant is refused for an action name this service has no action for: the
/// caller is pointed at the service's own interface description, which is where the names
/// a grant may allow are published.
const UNKNOWN_ACTION_REASON: &str =
    "Please refer to the appropriate WSDL for a list of valid actions.";

/// The longest action name a refusal echoes back: the longest name this service has, so
/// a value no longer than any action name is reported and anything longer is not.
const MAX_ACTION_NAME_LENGTH: usize = longest_action_name();

/// The length of the longest entry of [`ACTION_NAMES`].
const fn longest_action_name() -> usize {
    let mut longest = 0;
    let mut index = 0;
    while index < ACTION_NAMES.len() {
        let length = ACTION_NAMES[index].len();
        if length > longest {
            longest = length;
        }
        index += 1;
    }
    longest
}

/// The statement that grants `actions` on the queue ARN `resource` to each account in
/// `account_numbers`, labelled `label`.
///
/// A member naming one value carries it alone and several as a list, which is how a
/// policy document states the two cases.
fn permission_statement(
    label: &str,
    account_numbers: &[String],
    actions: &[String],
    resource: &str,
) -> ::serde_json::Value {
    let principals: Vec<String> = account_numbers
        .iter()
        .map(|account_number| account_principal(account_number))
        .collect();
    let allowed: Vec<String> = actions
        .iter()
        .map(|action| format!("{POLICY_ACTION_PREFIX}{action}"))
        .collect();
    ::serde_json::json!({
        POLICY_SID_MEMBER: label,
        "Effect": "Allow",
        "Principal": { "AWS": one_or_many(principals) },
        "Action": one_or_many(allowed),
        "Resource": resource,
    })
}

/// The principal a policy names for the account numbered `account_number`: an account
/// number identifies that account's root user, which is the principal the grant is
/// recorded for.
fn account_principal(account_number: &str) -> String {
    format!("arn:aws:iam::{account_number}:root")
}

/// `values` as a policy member states them: the value alone when there is exactly one,
/// the list otherwise.
fn one_or_many(mut values: Vec<String>) -> ::serde_json::Value {
    if values.len() == 1 {
        return ::serde_json::Value::String(values.remove(0));
    }
    ::serde_json::Value::Array(
        values
            .into_iter()
            .map(::serde_json::Value::String)
            .collect(),
    )
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
    epoch_millis_number(at).to_string()
}

/// The same instant as a NUMBER of milliseconds since the Unix epoch, for the members a
/// response carries as a `long` rather than as a string (`StartedTimestamp`).
///
/// An instant before the epoch, and one too far past it to count in an `i64`, are both
/// unreachable for a value this store derives from the clock.
pub fn epoch_millis_number(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default()
        .try_into()
        .unwrap_or(i64::MAX)
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

/// The modelled refusal for a `SourceArn` or `DestinationArn` that is not an ARN naming
/// a queue at all (`InvalidAddress`: "The specified ID is invalid.").
///
/// Only the member NAME reaches the payload, never the value it carried.
pub fn invalid_address(member: &str) -> InvalidAddress {
    InvalidAddress {
        message: Some(format!(
            "The address supplied for the {member} parameter is not a valid Amazon SQS queue ARN."
        )),
    }
}

/// The modelled refusal for a `SourceArn` or `DestinationArn` no queue of this account
/// answers to, naming the member the caller has to fix.
///
/// Only the member NAME reaches the payload, never the value it carried, so a caller
/// probing for a queue it may not address is never handed its own input back.
pub fn resource_not_found(member: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "The resource that you specified for the {member} parameter doesn't exist."
        )),
    }
}

/// The modelled refusal for a `TaskHandle` no recorded message movement task answers to
/// (`ResourceNotFoundException`: "One or more specified resources don't exist.").
///
/// The service words this one for the task rather than for the parameter, and carries no
/// value the request supplied.
pub fn unknown_message_move_task() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Task does not exist.".to_string()),
    }
}

/// What a `SourceArn` or `DestinationArn` value names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueArn<'a> {
    /// An Amazon SQS queue ARN of this account, naming the queue called this.
    OfThisAccount(&'a str),
    /// A well-formed ARN, but not one of this account's Amazon SQS queues: no resource
    /// this emulator holds can answer to it.
    Elsewhere,
    /// Not an ARN that names a queue at all.
    Malformed,
}

/// Classify the ARN a request supplied.
///
/// A queue ARN is `arn:<partition>:sqs:<region>:<accountId>:<QueueName>`, and a queue
/// name "can have up to 80 characters" (`CreateQueueRequest$QueueName`), so a value with
/// another segment count, an empty segment or a longer last segment names no queue in any
/// account. The service and account segments are what single out this account's queues;
/// the region segment is deliberately NOT matched, exactly as [`queue_name_from_url`]
/// does not match the endpoint, so a client whose SigV4 scope names one region still
/// reaches the queue it created under another.
pub fn parse_queue_arn(arn: &str) -> QueueArn<'_> {
    let segments: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, name] = segments[..] else {
        return QueueArn::Malformed;
    };
    if prefix != "arn"
        || partition.is_empty()
        || service.is_empty()
        || region.is_empty()
        || account.is_empty()
        || name.is_empty()
        || name.chars().count() > MAX_QUEUE_NAME_LENGTH
    {
        return QueueArn::Malformed;
    }
    if service != ARN_SERVICE || account != ::engine::emulator_account_id() {
        return QueueArn::Elsewhere;
    }
    QueueArn::OfThisAccount(name)
}

/// The `QueueName` a queue ARN of this account addresses, or `None` for any other value.
pub fn queue_name_from_arn(arn: &str) -> Option<&str> {
    match parse_queue_arn(arn) {
        QueueArn::OfThisAccount(name) => Some(name),
        QueueArn::Elsewhere | QueueArn::Malformed => None,
    }
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

/// What Amazon SQS answers a `PurgeQueue` on the queue called `name` with while the one
/// before it is still deleting: the interval is part of the sentence, so the caller is
/// told how long the deletion can take, and the queue the request named is the only
/// value filled in.
pub fn purge_queue_in_progress_message(name: &str) -> String {
    format!(
        "Only one PurgeQueue operation on {name} is allowed every {} seconds.",
        PURGE_QUEUE_INTERVAL.as_secs(),
    )
}

/// What Amazon SQS answers for a value supplied where a receipt handle was expected that
/// is not a receipt handle at all: ONE fixed sentence, with nothing of the request in it,
/// so a client can match on it and no caller is handed its own value back.
pub const RECEIPT_HANDLE_IS_INVALID_MESSAGE: &str = "The input receipt handle is invalid.";

/// How the same refusal is worded for a handle that IS a receipt handle, minted by
/// another queue: the handle is named, because what the caller has to fix is which queue
/// it sent it to rather than the handle itself.
pub fn receipt_handle_of_another_queue_message(receipt_handle: &str) -> String {
    format!("The receipt handle {receipt_handle:?} is not valid for this queue.")
}

/// Where a receipt handle a queue cannot resolve came from, which is what decides how the
/// refusal is worded.
#[derive(Debug, Clone, Copy)]
pub enum ReceiptHandleOrigin<'a> {
    /// No queue minted it, so it is not a receipt handle at all.
    Unrecognised,
    /// Another queue minted it: it is a receipt handle, just not one of this queue's.
    AnotherQueue(&'a str),
}

/// The modelled refusal for a receipt handle this queue cannot resolve.
///
/// A handle is replaced by the next receive of the same message, so a superseded one is
/// invalid too: "You must provide the most recently received receipt handle when you
/// request to delete the message."
pub fn receipt_handle_is_invalid(origin: ReceiptHandleOrigin<'_>) -> ReceiptHandleIsInvalid {
    ReceiptHandleIsInvalid {
        message: Some(match origin {
            ReceiptHandleOrigin::Unrecognised => RECEIPT_HANDLE_IS_INVALID_MESSAGE.to_string(),
            ReceiptHandleOrigin::AnotherQueue(receipt_handle) => {
                receipt_handle_of_another_queue_message(receipt_handle)
            }
        }),
    }
}

/// The parameter a receipt handle is supplied for, as every operation that takes one
/// names it.
pub const RECEIPT_HANDLE_PARAMETER: &str = "ReceiptHandle";

/// Why a receipt handle a request carries names no message of the queue it was sent to.
///
/// The two are told apart because a caller recovers from them differently: a string that
/// is not a receipt handle at all is a request no retry can fix, while a handle this
/// queue no longer holds a message under means the message is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptHandleRefusal {
    /// The value is not formed as a receipt handle, so no receive can have minted it.
    Malformed,
    /// It is formed as one, but names no message this queue still holds: the message was
    /// deleted, purged or expired, or a later receive superseded the handle.
    NoSuchMessage,
}

/// Whether `receipt_handle` is formed as a receipt handle at all — a handle is an opaque
/// token a receive handed a consumer, so what can be judged of a value is its FORM.
///
/// A handle this emulator minted is one [`new_identifier`] produced, and another queue
/// service's is a longer opaque token of the same alphabet, so both are handles here: a
/// consumer replaying one is told its MESSAGE is gone rather than that what it holds is
/// not a handle. A value shorter than a minted one, one holding a character no opaque
/// token does, or one longer than any handle a consumer can have been handed is not one.
pub fn is_receipt_handle(receipt_handle: &str) -> bool {
    let length = receipt_handle.chars().count();
    (MINTED_RECEIPT_HANDLE_LENGTH..=MAX_RECEIPT_HANDLE_LENGTH).contains(&length)
        && receipt_handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '='))
}

/// The length of a receipt handle this service mints: [`new_identifier`] produces the
/// hyphenated 36-character form, and nothing shorter is a handle.
const MINTED_RECEIPT_HANDLE_LENGTH: usize = 36;

/// The longest value this service treats as a receipt handle. A handle is held by a
/// consumer and handed back, so this bounds what one refusal can be made to echo; it is
/// this emulator's bound, as the other `MAX_*` bounds here are.
const MAX_RECEIPT_HANDLE_LENGTH: usize = 1_024;

/// How a refusal states that the handle names no message whose visibility there is to
/// change. The value is echoed only because it is already known to be a handle, and so
/// bounded by the form one has.
pub fn no_message_for_receipt_handle_message(receipt_handle: &str) -> String {
    invalid_parameter_message(
        receipt_handle,
        RECEIPT_HANDLE_PARAMETER,
        "Message does not exist or is not available for visibility timeout change.",
    )
}

/// How a refusal states that the visibility asked for reaches past what the message has
/// left: "if you attempt to set the VisibilityTimeout to a value greater than the maximum
/// time left, Amazon SQS returns an error. Amazon SQS doesn't automatically recalculate
/// and increase the timeout to the maximum remaining time." The limit is the span one
/// receive may hide a message for, counted from that receive.
pub fn visibility_beyond_limit_message(requested: i64, limit: i64) -> String {
    invalid_parameter_message(
        &requested.to_string(),
        VISIBILITY_TIMEOUT,
        &format!("Total VisibilityTimeout for the message is beyond the limit [{limit} seconds]."),
    )
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

/// How a refusal of an empty batch states it: the operation's own request-entry shape
/// is what the request should have carried at least one of.
///
/// One wording for every batch operation, so two of them cannot drift on how they state
/// the same defect; `operation` is the operation's name, whose request entry shape is
/// `<operation>RequestEntry`.
pub fn empty_batch_message(operation: &str) -> String {
    format!("There should be at least one {operation}RequestEntry in the request.")
}

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

/// The per-entry error code for a batch entry that carries a required member with no
/// value at all: the member is there, so nothing is missing, but it holds nothing the
/// operation can act on.
pub const EMPTY_VALUE: &str = "EmptyValue";

/// How a batch result entry states that `member` of one entry carried no value: the
/// member is named by its PATH in the request — the operation's request-entry shape, the
/// entry's position in `Entries` counted from one, and the member — because a batch
/// carries several entries and the caller has to know which one to repair.
///
/// One wording for every batch operation, so two of them cannot drift on how they state
/// the same defect; `operation` is the operation's name, whose request entry shape is
/// `<operation>RequestEntry`, and `position` is one-based.
pub fn empty_batch_entry_value_message(operation: &str, position: usize, member: &str) -> String {
    format!("No Value Found for {operation}RequestEntry.{position}.{member}.")
}

/// The largest payload one batch request may carry, in bytes.
///
/// `SendMessageBatch`'s own documentation gives it as a FIXED service limit — "the
/// maximum allowed individual message size and the maximum total payload size (the sum
/// of the individual lengths of all of the batched messages)" — which is the largest
/// value [`MAXIMUM_MESSAGE_SIZE`] admits, so it is read from that attribute's documented
/// range rather than written twice. A queue's own (possibly lowered)
/// `MaximumMessageSize` bounds EACH message, not the sum of a batch's entries.
pub fn maximum_batch_payload_bytes() -> i64 {
    integer_bounds(MAXIMUM_MESSAGE_SIZE).1
}

// ---------------------------------------------------------------------------
// Parameter-level refusals
// ---------------------------------------------------------------------------
//
// This service's model shapes an error for an ATTRIBUTE fault — `InvalidAttributeName`,
// `InvalidAttributeValue` — and one for each batch fault, but a fault in a PARAMETER is
// carried by two codes it documents without shaping: one for a parameter the call
// requires and did not get, one for a parameter whose value it does not accept. The
// error channel owns those two codes and their variants
// ([`crate::scaffolded::Scaffolded::missing_parameter`] and
// [`crate::scaffolded::Scaffolded::invalid_parameter_value`]); the TEXT they carry is
// worded HERE, once, in the templates the service builds from the request, so no handler
// and no batch entry invents a second wording for the same class of refusal.

/// How a refusal states that `parameter` was absent. The one wording for that class:
/// [`crate::scaffolded::MissingParameter`] builds its payload from it, and a batch entry
/// reporting the condition per entry uses it too.
pub fn missing_parameter_message(parameter: &str) -> String {
    format!("The request must contain the parameter {parameter}.")
}

/// How a refusal states that `value`, supplied for `parameter`, is not one this service
/// accepts: the offending value, the parameter it was supplied for, and the rule it
/// broke. `reason` is a whole sentence and carries its own terminal period.
pub fn invalid_parameter_message(value: &str, parameter: &str, reason: &str) -> String {
    format!("Value {value} for parameter {parameter} is invalid. Reason: {reason}")
}

/// The same template for a refusal that does NOT name the value: the one to use where
/// the value is a string whose length is the very thing being refused, so echoing it
/// would hand an unbounded value back to the caller.
pub fn invalid_parameter_message_without_value(parameter: &str, reason: &str) -> String {
    format!("Value for parameter {parameter} is invalid. Reason: {reason}")
}

/// How a refusal states a rule the request broke as a whole rather than through one
/// named parameter. `reason` is a whole sentence and carries its own terminal period.
pub fn invalid_parameters_message(reason: &str) -> String {
    format!("One or more parameters are invalid. Reason: {reason}")
}

/// The same class of fault as [`invalid_parameters_message`], as a batch RESULT ENTRY
/// states it: the entry's own members could not be validated, so the sentence is about
/// the entry rather than about the request.
pub fn unvalidated_parameters_message(reason: &str) -> String {
    format!("One or more parameters cannot be validated. Reason: {reason}")
}

/// Why a message is larger than the queue will carry, as the reason of either sentence
/// above: the bound is the queue's own `MaximumMessageSize`, which applies to ONE message
/// whether it was sent on its own or as one entry of a batch.
pub fn message_too_long_reason(maximum: i64) -> String {
    format!("Message must be shorter than {maximum} bytes.")
}

/// The message system attribute names in the order the model DECLARES them, which is the
/// order a refusal lists the accepted set in.
///
/// Source: `smithy_query(query="shape", shape="MessageSystemAttributeName",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`.
/// It holds exactly the values the generated
/// `MESSAGE_SYSTEM_ATTRIBUTE_NAME_VALUES` carries — membership is still checked against
/// that generated set — but the generated one is sorted, and a published value set is
/// stated in the order it is published.
pub const MESSAGE_SYSTEM_ATTRIBUTE_NAMES: &[&str] = &[
    "All",
    "SenderId",
    "SentTimestamp",
    "ApproximateReceiveCount",
    "ApproximateFirstReceiveTimestamp",
    "SequenceNumber",
    "MessageDeduplicationId",
    "MessageGroupId",
    "AWSTraceHeader",
    "DeadLetterQueueSourceArn",
];

/// How a refusal states that the queue a message movement task names as its source is
/// not one it can move messages out of.
///
/// The sentence states the pre-condition the named source does not meet — the queue has
/// to be the dead-letter queue of another queue — and nothing else: the whole message is
/// fixed text, because the parameter whose value is refused is already named by the
/// parameter-refusal class this is answered as. The documentation sentence of the
/// service's `UnsupportedOperation` class ("This action is currently limited to
/// supporting message redrive from queues that are configured as dead-letter queues
/// (DLQs) ... only") belongs to that OTHER class and is not reused here.
pub const DEAD_LETTER_SOURCE_REQUIRED: &str =
    "Source queue must be configured as a Dead Letter Queue.";

/// How a refusal states that the source queue a message movement task names already has
/// one running: "only one active message movement task is supported per queue at any
/// given time", so the request is refused for the source arn it named rather than for the
/// kind of queue that arn is. Fixed text — the condition clears on its own, and nothing of
/// the request tells the caller anything the sentence does not.
pub const TASK_ALREADY_RUNNING: &str =
    "There is already a task running. Only one active task is allowed for a source queue \
     arn at a given time.";

/// The reason a refusal gives for a parameter that does not apply to the KIND of queue
/// the call addresses: a FIFO-only member supplied to a standard queue, or a
/// per-message member a FIFO queue accepts only as a queue attribute.
pub const QUEUE_TYPE_PARAMETER_REASON: &str =
    "The request include parameter that is not valid for this queue type.";

/// How a refusal states that a FIFO send carried no deduplication token and the queue it
/// addresses derives none: "If you don't provide a MessageDeduplicationId and the queue
/// doesn't have ContentBasedDeduplication set, the action fails with an error"
/// (`SendMessageRequest$MessageDeduplicationId`).
pub const NO_DEDUPLICATION_ID: &str = "The queue should either have ContentBasedDeduplication \
                                       enabled or MessageDeduplicationId provided explicitly";

/// How a refusal states that a parameter's value is not one this service accepts, where
/// it names the parameter and nothing else: what that parameter accepts is published
/// with the parameter, not restated per call.
pub fn invalid_value_message(parameter: &str) -> String {
    format!("Invalid value for the parameter {parameter}.")
}

/// The same template with the rule the value broke stated after it, in the `Reason:`
/// clause every refusal of this service that HAS a rule to state carries.
///
/// A value refused for what ANOTHER attribute holds cannot be repaired from the refused
/// attribute's own published value set, so the rule that rejected the pair is the only
/// thing that tells the caller what to change. `reason` is a whole sentence and carries
/// its own terminal period.
pub fn invalid_value_message_with_reason(parameter: &str, reason: &str) -> String {
    format!("{} Reason: {reason}", invalid_value_message(parameter))
}

/// The rule the FIFO throughput limit and the deduplication scope have to satisfy
/// TOGETHER, as the reason a refusal of either half states: "The perMessageGroupId value
/// is allowed only when the value for DeduplicationScope is messageGroup"
/// ([`FIFO_THROUGHPUT_LIMIT`]'s own documentation), stated as the change the caller has
/// to make.
pub fn fifo_throughput_pair_reason() -> String {
    format!(
        "To set {FIFO_THROUGHPUT_LIMIT} to {FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP}, the \
         {DEDUPLICATION_SCOPE} must be {DEDUPLICATION_SCOPE_MESSAGE_GROUP}."
    )
}

/// Why a retry under a `ReceiveRequestAttemptId` can no longer be served, as the reason
/// of the refusal that turns it away: the token replays its batch only "if none of the
/// messages have been modified (deleted or had their visibility changes)"
/// (`ReceiveMessageRequest$ReceiveRequestAttemptId`), so once one of them was, the
/// receive that token names is not a retryable call any more.
pub const RECEIVE_ATTEMPT_NOT_RETRYABLE: &str = "The receiveRequestAttemptId is not valid \
                                                 anymore since the receive call is not \
                                                 retryable.";

/// The modelled refusal for an attribute value the call does not accept: it names the
/// attribute whose value was refused, which is all this refusal states.
pub fn invalid_attribute_value(attribute: &str) -> InvalidAttributeValue {
    InvalidAttributeValue {
        message: Some(invalid_value_message(attribute)),
    }
}

/// The modelled refusal for an attribute name the call does not accept: it names no
/// attribute of the value set that member carries.
///
/// One wording for every call that takes attribute names — a queue attribute map, a
/// queue attribute name list, or a message system attribute name list — so two
/// operations cannot drift on how they word the same defect.
pub fn unknown_attribute_name(name: &str) -> InvalidAttributeName {
    InvalidAttributeName {
        message: Some(format!("Unknown Attribute {name}.")),
    }
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
            return Err(AttributeRefusal::Name(unknown_attribute_name(name)));
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

/// Apply the attribute map a call supplied to a queue's stored attributes, with the two
/// rules that are about the VALUE rather than about the attribute.
///
/// * An attribute whose form admits text of any length is REMOVED by a value holding no
///   text ("You can remove the ... attribute by setting it to an empty string"), so a
///   cleared policy leaves the queue carrying no `Policy` at all rather than one that is
///   not a document.
/// * A `Policy` is stored in its normalised form
///   ([`authorization::normalised_policy`]), so every later read reports the principals
///   in the spelling AWS canonicalises them to.
///
/// Both `CreateQueue` and `SetQueueAttributes` store through this, so the two cannot
/// disagree about what a supplied value means.
pub fn apply_attributes<'a>(
    attributes: &mut BTreeMap<String, String>,
    supplied: impl IntoIterator<Item = (&'a String, &'a String)>,
) {
    for (name, value) in supplied {
        match stored_attribute(name, value) {
            Some(stored) => attributes.insert(name.clone(), stored),
            None => attributes.remove(name),
        };
    }
}

/// The form attribute `name` is STORED in when a call supplies `value` for it, or `None`
/// when that value removes the attribute. See [`apply_attributes`].
pub fn stored_attribute(name: &str, value: &str) -> Option<String> {
    let removable = settable_attribute(name)
        .is_some_and(|spec| matches!(spec.kind, AttributeKind::Json | AttributeKind::Text));
    if value.is_empty() && removable {
        return None;
    }
    if name == POLICY {
        return Some(authorization::normalised_policy(value));
    }
    Some(value.to_string())
}

/// Why the attribute set a call would LEAVE a queue carrying cannot hold together, or
/// `None` when it can.
///
/// Some attributes are each individually valid yet constrain one another, so the pair
/// can only be judged over the resulting set rather than over the map one call supplies:
/// the per-message-group FIFO throughput limit "is allowed only when the value for
/// DeduplicationScope is messageGroup".
pub fn attribute_combination_refusal(
    attributes: &BTreeMap<String, String>,
) -> Option<InvalidAttributeValue> {
    let per_message_group = attributes
        .get(FIFO_THROUGHPUT_LIMIT)
        .is_some_and(|value| value == FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP);
    let message_group_scope = attributes
        .get(DEDUPLICATION_SCOPE)
        .is_some_and(|value| value == DEDUPLICATION_SCOPE_MESSAGE_GROUP);
    (per_message_group && !message_group_scope).then(|| InvalidAttributeValue {
        // The fixed text Amazon SQS words a refused attribute value in, instantiated with
        // the attribute whose value the pair rule rejects, and carrying the `Reason:`
        // clause stating that rule — without it the refusal names no way to repair the
        // request, because the value is outside nothing the named attribute itself
        // publishes. The attribute named is the one the rule is stated ON, whichever half
        // of the pair the call supplied: it is the value that is only conditionally
        // allowed, and the scope is what has to change for it.
        message: Some(invalid_value_message_with_reason(
            FIFO_THROUGHPUT_LIMIT,
            &fifo_throughput_pair_reason(),
        )),
    })
}

/// Check one attribute value against what its own documentation says it accepts.
fn check_attribute_value(spec: &AttributeSpec, value: &str) -> Result<(), AttributeRefusal> {
    // An attribute whose form admits text of any length is also the kind a caller REMOVES
    // by supplying no text for it: "You can remove the ... attribute by setting it to an
    // empty string", so the empty value is accepted and stored as the attribute's absence
    // ([`stored_attribute`]) rather than refused as a malformed document.
    if value.is_empty() && matches!(spec.kind, AttributeKind::Json | AttributeKind::Text) {
        return Ok(());
    }
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
        return Err(AttributeRefusal::Value(invalid_attribute_value(spec.name)));
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
    // The refusal names the attribute and nothing else: what that attribute accepts is
    // published with the attribute, not restated per call.
    Err(AttributeRefusal::Value(invalid_attribute_value(spec.name)))
}

/// Why a `QueueName` was refused, as the text of the refusal a caller has to fix the
/// request from, or `None` when the name is one this service accepts.
///
/// The rule is the member's own: "A queue name can have up to 80 characters. Valid
/// values: alphanumeric characters, hyphens (-), and underscores (_). A FIFO queue
/// name must end with the .fifo suffix." `fifo` says whether the request asks for a
/// FIFO queue, which is what makes the suffix part of the rule — and makes the refusal
/// state the whole rule a FIFO name has to satisfy, suffix included.
pub fn queue_name_refusal(name: &str, fifo: bool) -> Option<String> {
    // The suffix is part of a FIFO queue's name and of nothing else: a standard queue's
    // name may hold only the characters the member lists, so a `.fifo` name is accepted
    // for a FIFO queue and refused for a standard one.
    let body = if fifo {
        name.strip_suffix(FIFO_QUEUE_NAME_SUFFIX).unwrap_or(name)
    } else {
        name
    };
    let well_formed = (1..=MAX_QUEUE_NAME_LENGTH).contains(&name.chars().count())
        && !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let suffixed = !fifo || name.ends_with(FIFO_QUEUE_NAME_SUFFIX);
    if well_formed && suffixed {
        return None;
    }
    // The refusal states the rule, not which half of it this name broke: a caller fixes
    // the name against the whole rule either way.
    Some(if fifo {
        format!(
            "The name of a FIFO queue can only include alphanumeric characters, hyphens, or \
             underscores, must end with {FIFO_QUEUE_NAME_SUFFIX} suffix and be 1 to \
             {MAX_QUEUE_NAME_LENGTH} in length."
        )
    } else {
        format!(
            "Can only include alphanumeric characters, hyphens, or underscores. 1 to \
             {MAX_QUEUE_NAME_LENGTH} in length"
        )
    })
}

/// Why a `tags` map a caller supplied was refused, as the text of the refusal a caller
/// has to fix the request from, or `None` when the map is acceptable.
///
/// `existing` is the tag map the queue carries today, so the count is checked against
/// what the call would LEAVE the queue with: a key the request repeats overwrites the
/// existing tag rather than adding one. `queue` names the queue the call addresses, the
/// way a refusal of its tag count states it. Keys are visited in order, so a map with
/// two bad tags is always refused for the same one.
///
/// The rules are the tag restrictions the `tags` member's own documentation points at:
/// at most [`MAX_TAGS_PER_QUEUE`] tags per queue, a key of 1 to
/// [`MAX_TAG_KEY_LENGTH`] characters, a value of up to [`MAX_TAG_VALUE_LENGTH`], and no
/// key in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
pub fn tag_refusal(
    queue: &str,
    existing: &BTreeMap<String, String>,
    supplied: &HashMap<String, String>,
) -> Option<String> {
    let mut keys: Vec<&String> = supplied.keys().collect();
    keys.sort();
    for key in &keys {
        // Each refusal is worded as Amazon SQS words a parameter whose value it will
        // not take: the parameter it was given for, and the reason. The VALUE is named
        // only where it is already known to be inside its length bound, so a refusal
        // never echoes an unbounded string back to the caller.
        let length = key.chars().count();
        if length == 0 || length > MAX_TAG_KEY_LENGTH {
            return Some(invalid_parameter_message_without_value(
                "TagKey",
                &format!(
                    "must be between 1 and {MAX_TAG_KEY_LENGTH} characters long, but was {length}."
                ),
            ));
        }
        if is_reserved_tag_key(key) {
            return Some(invalid_parameter_message(
                key,
                "TagKey",
                &format!(
                    "must not start with the reserved prefix {RESERVED_TAG_KEY_PREFIX}."
                ),
            ));
        }
        let value_length = supplied[*key].chars().count();
        if value_length > MAX_TAG_VALUE_LENGTH {
            return Some(invalid_parameter_message_without_value(
                "TagValue",
                &format!(
                    "the value of tag key {key} must be at most {MAX_TAG_VALUE_LENGTH} \
                     characters long, but was {value_length}."
                ),
            ));
        }
    }
    let merged: BTreeSet<&String> = existing.keys().chain(supplied.keys()).collect();
    if merged.len() > MAX_TAGS_PER_QUEUE {
        return Some(format!("Too many tags added for queue {queue}."));
    }
    None
}

/// Why `CreateQueue` must refuse a request that names an EXISTING queue, as the text of
/// the `QueueNameExists` refusal, or `None` when the request asks for nothing different
/// about that queue and is therefore idempotent.
///
/// "If you specify the name of an existing queue and provide the exact same names and
/// values for all its attributes, the CreateQueue action will return the URL of the
/// existing queue instead of creating a new one. If you attempt to create a queue with a
/// name that already exists but with different attribute names or values, the CreateQueue
/// action will return an error" — and `QueueNameExists` itself: "Amazon SQS returns this
/// error only if the request includes attributes whose values differ from those of the
/// existing queue."
///
/// `requested` is the attribute map the request would leave the queue with (the
/// documented defaults overlaid with what it supplied), compared attribute by attribute
/// in name order so a request differing in two is always refused for the same one.
/// [`ATTRIBUTES_NOT_COMPARED_ON_RECREATE`] is left out of that comparison. `tags` is the
/// tag map the request supplied, and `None` when it supplied no `tags` member at all:
/// tags carry no default, so a request that supplies none includes nothing about them,
/// while one that supplies them has to name the set the queue already holds.
pub fn recreate_refusal(
    existing: &Queue,
    requested: &BTreeMap<String, String>,
    tags: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    let compared: BTreeSet<&String> = existing
        .attributes
        .keys()
        .chain(requested.keys())
        .filter(|name| !ATTRIBUTES_NOT_COMPARED_ON_RECREATE.contains(&name.as_str()))
        .collect();
    for name in compared {
        if existing.attributes.get(name) != requested.get(name) {
            return Some(format!(
                "A queue already exists with the same name and a different value for attribute {name}"
            ));
        }
    }
    if tags.is_some_and(|supplied| *supplied != existing.tags) {
        return Some(
            "A queue already exists with the same name and a different value for attribute Tags"
                .to_string(),
        );
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

/// Whether `interval` has run out, measured from `since`, by `now`.
///
/// An instant so far in the future that adding the interval overflows has not lapsed —
/// the same reading the message and deduplication spans take of an unrepresentable
/// deadline.
pub fn lapsed(since: SystemTime, interval: Duration, now: SystemTime) -> bool {
    since
        .checked_add(interval)
        .is_some_and(|lapses| now >= lapses)
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

/// The deduplication token a queue with `ContentBasedDeduplication` derives for a
/// message: "Amazon SQS uses a SHA-256 hash to generate the MessageDeduplicationId
/// using the body of the message (but not the attributes of the message)", so two sends
/// of identical content inside the interval are "treated as duplicates and only one
/// copy of the message is delivered".
pub fn content_deduplication_token(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}

/// The parts of ONE attribute the validation and the digest below read.
///
/// `MessageAttributeValue` (what a sender puts in `MessageAttributes`) and
/// `MessageSystemAttributeValue` (what it puts in `MessageSystemAttributes`) declare
/// exactly the same members, so both are measured, judged and digested through this one
/// borrowed view: the rules are written once and a send cannot apply them to one map and
/// not the other.
#[derive(Debug, Clone, Copy)]
pub struct AttributeParts<'a> {
    /// The attribute's `DataType`, custom label included.
    pub data_type: &'a str,
    /// Its `StringValue`, when it carries one.
    pub string_value: Option<&'a str>,
    /// Its `BinaryValue`, when it carries one.
    pub binary_value: Option<&'a [u8]>,
    /// Which of the two list members it carries, when it carries one.
    pub list_member: Option<&'static str>,
}

/// Which list member an attribute carries, for the two shapes that declare both.
fn list_member(strings: bool, binaries: bool) -> Option<&'static str> {
    [
        strings.then_some("StringListValues"),
        binaries.then_some("BinaryListValues"),
    ]
    .into_iter()
    .flatten()
    .next()
}

impl<'a> From<&'a MessageAttributeValue> for AttributeParts<'a> {
    fn from(value: &'a MessageAttributeValue) -> Self {
        Self {
            data_type: value.data_type.as_str(),
            string_value: value.string_value.as_deref(),
            binary_value: value.binary_value.as_deref(),
            list_member: list_member(
                value.string_list_values.is_some(),
                value.binary_list_values.is_some(),
            ),
        }
    }
}

impl<'a> From<&'a MessageSystemAttributeValue> for AttributeParts<'a> {
    fn from(value: &'a MessageSystemAttributeValue) -> Self {
        Self {
            data_type: value.data_type.as_str(),
            string_value: value.string_value.as_deref(),
            binary_value: value.binary_value.as_deref(),
            list_member: list_member(
                value.string_list_values.is_some(),
                value.binary_list_values.is_some(),
            ),
        }
    }
}

/// MD5 hex digest of the message-attribute encoding SQS defines, or `None` when there
/// are no attributes (SQS omits `MD5OfMessageAttributes` then).
pub fn md5_of_message_attributes(
    attributes: &BTreeMap<String, MessageAttributeValue>,
) -> Option<String> {
    md5_of_attribute_encoding(attributes)
}

/// The same digest over the message SYSTEM attributes, which is what
/// `MD5OfMessageSystemAttributes` reports: the encoding is defined on an attribute's
/// name, type and value, and a system attribute has all three.
pub fn md5_of_message_system_attributes(
    attributes: &BTreeMap<String, MessageSystemAttributeValue>,
) -> Option<String> {
    md5_of_attribute_encoding(attributes)
}

/// MD5 hex digest of the attribute encoding SQS defines, or `None` for an empty map.
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
/// them and a send refuses a message that carries one rather than guessing a byte.
fn md5_of_attribute_encoding<'a, V>(attributes: &'a BTreeMap<String, V>) -> Option<String>
where
    AttributeParts<'a>: From<&'a V>,
{
    if attributes.is_empty() {
        return None;
    }
    let mut encoded = Vec::new();
    for (name, value) in attributes {
        let parts = AttributeParts::from(value);
        append_length_prefixed(&mut encoded, name.as_bytes());
        append_length_prefixed(&mut encoded, parts.data_type.as_bytes());
        if let Some(string_value) = parts.string_value {
            encoded.push(1);
            append_length_prefixed(&mut encoded, string_value.as_bytes());
        } else if let Some(binary_value) = parts.binary_value {
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
/// Amazon SQS counts message attributes toward a message's size ("All parts of the
/// message attribute, including Name, Type, and Value, are part of the message size
/// restriction"), so they are counted here too, both against the queue's
/// `MaximumMessageSize` and against this emulator's retained-byte budgets.
pub fn message_attribute_bytes(attributes: &BTreeMap<String, MessageAttributeValue>) -> usize {
    attribute_bytes(attributes)
}

/// The bytes a message-SYSTEM-attribute map holds, measured the same way.
///
/// These do NOT count toward a message's size — "the size of a message system attribute
/// doesn't count towards the total size of a message"
/// (`SendMessageRequest$MessageSystemAttributes`) — so this is used only for this
/// emulator's retained-byte budgets, which bound every byte a sender can make the store
/// hold.
pub fn message_system_attribute_bytes(
    attributes: &BTreeMap<String, MessageSystemAttributeValue>,
) -> usize {
    attribute_bytes(attributes)
}

/// The bytes an attribute map holds: each attribute's name, data type and value.
fn attribute_bytes<'a, V>(attributes: &'a BTreeMap<String, V>) -> usize
where
    AttributeParts<'a>: From<&'a V>,
{
    attributes
        .iter()
        .map(|(name, value)| {
            let parts = AttributeParts::from(value);
            name.len()
                .saturating_add(parts.data_type.len())
                .saturating_add(parts.string_value.map_or(0, str::len))
                .saturating_add(parts.binary_value.map_or(0, <[u8]>::len))
        })
        .fold(0usize, usize::saturating_add)
}

/// The longest a message attribute name may be. Amazon SQS Developer Guide, message
/// attribute components: the name "can contain up to 256 characters" — so 256 is taken
/// and 257 refused, even though the refusal Amazon SQS words for it says "shorter than
/// 256 bytes".
pub const MAX_MESSAGE_ATTRIBUTE_NAME_LEN: usize = 256;

/// The prefixes a message attribute name may not start with, in any casing: they are
/// "reserved for use by Amazon Web Services" (Amazon SQS Developer Guide, message
/// attribute components). Held lower-cased, because the rule ignores casing.
const RESERVED_MESSAGE_ATTRIBUTE_PREFIXES: [&str; 2] = ["aws.", "amazon."];

/// The logical data types a message attribute may carry: "Amazon SQS supports the
/// following logical data types: String, Number, and Binary"
/// (`MessageAttributeValue$DataType`), each optionally followed by a custom label ("You
/// can also append custom labels").
pub const MESSAGE_ATTRIBUTE_DATA_TYPES: [&str; 3] = ["String", "Number", "Binary"];

/// How many digits of precision the `Number` data type holds: "a number can have up to
/// 38 digits of precision" (Amazon SQS Developer Guide, message attribute data types).
pub const MAX_NUMBER_PRECISION_DIGITS: usize = 38;

/// Where a `Number` value's decimal point may fall: the type "must be within the range
/// -10^128 to 10^126" (the same entry), so the point stands at most 127 digits past the
/// first one and at most 128 before it. It also bounds how many zeros writing the value
/// out plainly can take.
pub const NUMBER_DECIMAL_POINT_RANGE: (i64, i64) = (-128, 127);

/// Why Amazon SQS will not take `name` as a message attribute name, or `None` when it
/// will.
///
/// The documented rules (Amazon SQS Developer Guide, message attribute components): the
/// name "can contain alphanumeric characters and the underscore (_), hyphen (-), and
/// period (.)", "can contain up to 256 characters", "must not start or end with a
/// period" and "must not have periods in succession", and the `AWS.` and `Amazon.`
/// prefixes are reserved whatever their casing. A name breaking any of them is refused
/// and nothing is enqueued for it.
pub fn message_attribute_name_refusal(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("The request must contain non-empty message attribute names.".to_string());
    }
    if name.len() > MAX_MESSAGE_ATTRIBUTE_NAME_LEN {
        return Some(format!(
            "Message (user) attribute name must be shorter than \
             {MAX_MESSAGE_ATTRIBUTE_NAME_LEN} bytes."
        ));
    }
    let lowered = name.to_lowercase();
    if RESERVED_MESSAGE_ATTRIBUTE_PREFIXES
        .iter()
        .any(|reserved| lowered.starts_with(reserved))
    {
        return Some(
            "You can't use message attribute names beginning with 'AWS.' or 'Amazon'. \
             These strings are reserved for internal use."
                .to_string(),
        );
    }
    if let Some(character) = name
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')))
    {
        return Some(format!(
            "Message (user) attribute name contains invalid non-alphanumeric character \
             '#x{:X}'. You can use only alphanumeric characters, hyphens (-), underscores (_), \
             or periods (.).",
            character as u32
        ));
    }
    if name.starts_with('.') {
        return Some(
            "Message (user) attribute name must not begin with character '.'.".to_string(),
        );
    }
    if name.ends_with('.') {
        return Some(
            "Message (user) attribute name must not end with character '.'.".to_string(),
        );
    }
    if name.contains("..") {
        return Some(
            "Message (user) attribute name must not have successive '.' characters.".to_string(),
        );
    }
    None
}

/// Why this message attribute cannot be accepted, or `None` when it can: the name rules
/// above, then the type and value rules below.
pub fn message_attribute_refusal(name: &str, value: &MessageAttributeValue) -> Option<String> {
    message_attribute_name_refusal(name)
        .or_else(|| attribute_value_refusal(name, AttributeParts::from(value)))
}

/// Why Amazon SQS will not take this attribute's `DataType` and value, or `None`.
///
/// The documented rules: the type is one of the logical types in
/// [`MESSAGE_ATTRIBUTE_DATA_TYPES`], optionally with a non-empty custom label; a
/// `String` or `Number` carries its value in `StringValue` and a `Binary` in
/// `BinaryValue` ("for the Number data type, you must use StringValue"), so a value in
/// the other field is not a value of the declared type at all;
/// "Name, type, value and the message body must not be empty or null", so a value
/// carrying no byte is no value either; and a `Number`'s value is a number.
fn attribute_value_refusal(name: &str, parts: AttributeParts<'_>) -> Option<String> {
    if let Some(member) = parts.list_member {
        return Some(format!(
            "Message (user) attribute '{name}' carries {member}, which is not implemented \
             and is reserved for future use."
        ));
    }
    let Some(logical_type) = attribute_logical_type(parts.data_type) else {
        return Some(format!(
            "The type of message (user) attribute '{name}' is invalid. You must use only the \
             following supported type prefixes: Binary, Number, String."
        ));
    };
    let binary = logical_type == "Binary";
    // Which FIELD carries the value of that type: "for the Number data type, you must
    // use StringValue", so only `Binary` is carried in `BinaryValue`. A value supplied
    // in the other field is not a value of the type the attribute declares, and one
    // absent from its field or empty in it is no value at all.
    let (field, carried, in_the_other_field) = if binary {
        (
            "Binary",
            parts.binary_value.map(<[u8]>::len),
            parts.string_value.is_some(),
        )
    } else {
        (
            "String",
            parts.string_value.map(str::len),
            parts.binary_value.is_some(),
        )
    };
    if in_the_other_field {
        return Some(format!(
            "Message (user) attribute '{name}' with type '{logical_type}' must use field \
             '{field}'."
        ));
    }
    if carried.is_none_or(|length| length == 0) {
        return Some(format!(
            "Message (user) attribute '{name}' must contain a non-empty value of type \
             '{field}'."
        ));
    }
    if logical_type == "Number" {
        if let Some(value) = parts.string_value {
            return number_value_refusal(name, value);
        }
    }
    None
}

/// Why Amazon SQS will not take `value` as a `Number` attribute's value, or `None`.
///
/// The documented rules for the type (Amazon SQS Developer Guide, message attribute data
/// types): "numbers are positive or negative integers or floating-point numbers", "a
/// number can have up to 38 digits of precision, and it must be within the range
/// -10^128 to 10^126". The value is reported back as the request wrote it, never as the
/// number it names, because that is the text the caller has to fix.
fn number_value_refusal(name: &str, value: &str) -> Option<String> {
    let Some(number) = DecimalNumber::parse(value) else {
        return Some(format!(
            "Can't cast the value of message (user) attribute '{name}' to a number."
        ));
    };
    if number.precision_digits() > MAX_NUMBER_PRECISION_DIGITS {
        return Some(format!(
            "Message (user) attribute '{name}' number value [{value}] must contain fewer \
             than {MAX_NUMBER_PRECISION_DIGITS} precision digits."
        ));
    }
    let (low, high) = NUMBER_DECIMAL_POINT_RANGE;
    if number.point < low || number.point > high {
        return Some(format!(
            "Message (user) attribute '{name}' number value [{value}] must be within the \
             range -10^128 to 10^126."
        ));
    }
    None
}

/// Store every `Number` attribute's value in the plain decimal form Amazon SQS keeps it
/// in, so a value written with an exponent comes back as the number it names and both
/// digests cover what the queue actually holds.
///
/// A `Number` attribute carries a NUMBER, not the text of one, so the value a receive
/// reports is that number written out plainly — `1e3` is the number one thousand, and
/// that is what comes back. Every other data type is text or bytes and is kept exactly
/// as sent. Applied after validation, which reports the value as the request wrote it.
pub fn store_message_attributes_canonically(
    attributes: &mut BTreeMap<String, MessageAttributeValue>,
) {
    for value in attributes.values_mut() {
        if attribute_logical_type(&value.data_type) != Some("Number") {
            continue;
        }
        let plain = value
            .string_value
            .as_deref()
            .and_then(DecimalNumber::parse)
            .filter(|number| {
                let (low, high) = NUMBER_DECIMAL_POINT_RANGE;
                number.point >= low && number.point <= high
            })
            .map(DecimalNumber::plain);
        if let Some(plain) = plain {
            value.string_value = Some(plain);
        }
    }
}

/// One `Number` attribute value, taken apart: its sign, its digits in order, and where
/// its decimal point falls among them once the exponent has moved it.
///
/// The digits are MOVED, never parsed into a float, so a value carrying more precision
/// than any float holds keeps every digit it was sent with.
#[derive(Debug, Clone)]
struct DecimalNumber {
    /// Whether the value carried a `-` sign.
    negative: bool,
    /// Every digit of the significand, in order, leading zeros included.
    digits: String,
    /// How many of those digits stand before the decimal point. Zero or negative puts
    /// the point before the first digit.
    point: i64,
}

impl DecimalNumber {
    /// `value` taken apart, or `None` when it is not a number of the kind the `Number`
    /// data type carries: "numbers are positive or negative integers or floating-point
    /// numbers", written plainly or with a decimal exponent.
    fn parse(value: &str) -> Option<Self> {
        let all_digits =
            |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        let negative = value.starts_with('-');
        let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
        let (significand, exponent) = match unsigned.split_once(['e', 'E']) {
            Some((significand, exponent)) => {
                let unsigned_exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
                if !all_digits(unsigned_exponent) {
                    return None;
                }
                (significand, exponent.parse::<i64>().ok()?)
            }
            None => (unsigned, 0),
        };
        let (whole, fraction) = match significand.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (significand, None),
        };
        if !all_digits(whole) || !fraction.is_none_or(all_digits) {
            return None;
        }
        Some(Self {
            negative,
            digits: format!("{whole}{}", fraction.unwrap_or_default()),
            point: i64::try_from(whole.len()).ok()?.checked_add(exponent)?,
        })
    }

    /// How many digits of precision the value carries: its digits without the leading
    /// zeros, which stand for no precision.
    fn precision_digits(&self) -> usize {
        self.digits.trim_start_matches('0').len()
    }

    /// The value in plain decimal notation: no exponent, no leading `+`, and no leading
    /// zero before another digit — `1e3` is `1000`, `007` is `7` and `1e-3` is `0.001`.
    ///
    /// Call it only on a value inside [`NUMBER_DECIMAL_POINT_RANGE`], which is what
    /// bounds how many zeros writing it out plainly takes.
    fn plain(self) -> String {
        let digits = &self.digits;
        let length = i64::try_from(digits.len()).unwrap_or(i64::MAX);
        let written = if self.point <= 0 {
            let zeros = "0".repeat(usize::try_from(-self.point).unwrap_or(0));
            format!("0.{zeros}{digits}")
        } else if self.point >= length {
            let zeros = "0".repeat(usize::try_from(self.point - length).unwrap_or(0));
            format!("{digits}{zeros}")
        } else {
            let at = usize::try_from(self.point).unwrap_or(0);
            format!("{}.{}", &digits[..at], &digits[at..])
        };
        // A leading zero only stands before the decimal point: `007` is `7`, `0.5` stays
        // `0.5`.
        let (integral, fraction) = match written.split_once('.') {
            Some((integral, fraction)) => (integral, Some(fraction)),
            None => (written.as_str(), None),
        };
        let trimmed = integral.trim_start_matches('0');
        let integral = if trimmed.is_empty() { "0" } else { trimmed };
        let sign = if self.negative { "-" } else { "" };
        match fraction {
            Some(fraction) => format!("{sign}{integral}.{fraction}"),
            None => format!("{sign}{integral}"),
        }
    }
}

/// The logical type `data_type` names, or `None` when it names none.
///
/// "You can also append custom labels" — `Number.float` is a `Number` — so the logical
/// type is what stands before the first period, and a label that is present must hold
/// something. The whole type is bounded like an attribute name (Amazon SQS Developer
/// Guide, message attribute components: the type "can be up to 256 characters long"),
/// and holds the same characters a name may.
pub fn attribute_logical_type(data_type: &str) -> Option<&'static str> {
    if data_type.chars().count() > MAX_MESSAGE_ATTRIBUTE_NAME_LEN {
        return None;
    }
    if !data_type
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return None;
    }
    let (logical, label) = match data_type.split_once('.') {
        Some((logical, label)) => (logical, Some(label)),
        None => (data_type, None),
    };
    if label.is_some_and(|label| label.split('.').any(str::is_empty)) {
        return None;
    }
    MESSAGE_ATTRIBUTE_DATA_TYPES
        .into_iter()
        .find(|supported| *supported == logical)
}


/// Why this message SYSTEM attribute cannot be accepted, or `None` when it can.
///
/// "Currently, the only supported message system attribute is AWSTraceHeader. Its type
/// must be String and its value must be a correctly formatted X-Ray trace header string"
/// (`SendMessageRequest$MessageSystemAttributes`), and the map's own key shape
/// ([`MESSAGE_SYSTEM_ATTRIBUTE_NAME_FOR_SENDS_VALUES`]) publishes that one name — so any
/// other name is refused, and the one name takes a non-empty `StringValue` and nothing
/// else.
pub fn message_system_attribute_refusal(
    name: &str,
    value: &MessageSystemAttributeValue,
) -> Option<String> {
    if !MESSAGE_SYSTEM_ATTRIBUTE_NAME_FOR_SENDS_VALUES.contains(&name) {
        return Some(format!("Message system attribute name '{name}' is invalid."));
    }
    let parts = AttributeParts::from(value);
    let carries_a_string = parts
        .string_value
        .is_some_and(|string_value| !string_value.is_empty());
    if parts.data_type != "String"
        || !carries_a_string
        || parts.binary_value.is_some()
        || parts.list_member.is_some()
    {
        return Some(format!(
            "Message system attribute '{name}' must contain a non-empty value of type 'String'."
        ));
    }    None
}
