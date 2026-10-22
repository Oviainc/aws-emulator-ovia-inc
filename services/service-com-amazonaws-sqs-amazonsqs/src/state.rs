// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sqs-amazonsqs service.
//!
//! The queue is this service's primary resource: `CreateQueue` allocates one,
//! `GetQueueUrl` resolves it by name, `GetQueueAttributes` reports it and
//! `SetQueueAttributes` changes it, all against the SAME [`Queue`] record in
//! [`ComAmazonawsSqsAmazonsqsState::queues`], so a create is observable by every
//! later read and an attribute change by every later report.
//!
//! The record is the stored half; the wire half (the `Attributes` map
//! `GetQueueAttributes` returns) is PROJECTED from it by
//! [`Queue::reported_attributes`], so no handler assembles a response member by hand.
//!
//! # What this slice holds, and what it does not
//!
//! The queue's identity (name, url, ARN) and its CONFIGURATION attributes. It holds no
//! messages: `SendMessage`/`ReceiveMessage`/`DeleteMessage` are still scaffolded in
//! this crate, so nothing can put a message in a queue. The three
//! `ApproximateNumberOfMessages*` attributes are counts over that message plane, so
//! they are reported by NO value here rather than by a made-up one — see
//! [`MESSAGE_PLANE_ATTRIBUTES`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    InvalidAttributeName, InvalidAttributeValue, QueueDoesNotExist, UnsupportedOperation,
    QUEUE_ATTRIBUTE_NAME_VALUES,
};

/// The region an ARN carries when the request names none (an unsigned call, which
/// carries no SigV4 credential scope). Matches the region the emulator's own
/// generated config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, as `smithy_query(query =
/// "service_info")` reports it (`arnNamespace: sqs`, `sigv4Name: sqs`) — the
/// `service` segment of every queue ARN.
pub const ARN_SERVICE: &str = "sqs";

/// How many queues this emulator will hold.
///
/// NOT a fidelity claim about any published quota: it is the emulator's own ceiling,
/// generous enough that no realistic workflow meets it, so a caller looping on
/// `CreateQueue` cannot grow this process's memory without bound.
pub const MAX_QUEUES: usize = 1_000;

/// The longest queue name `CreateQueue` accepts: "A queue name can have up to 80
/// characters" (`CreateQueueRequest$QueueName` docs).
pub const MAX_QUEUE_NAME_LENGTH: usize = 80;

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
/// This crate has no message plane yet — `SendMessage`, `ReceiveMessage` and
/// `DeleteMessage` all still answer HTTP 501 — so there is no store to count and these
/// are reported by no value at all. A queue omits an attribute it has no value for
/// (exactly as it omits `Policy` when no policy was ever set), which the model's own
/// `AttributeNames` note tells a client to expect: "In the future, new attributes might
/// be added. If you write code that calls this action, we recommend that you structure
/// your code so that it can handle new attributes gracefully."
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
        name: "DelaySeconds",
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
        name: "MaximumMessageSize",
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
        name: "MessageRetentionPeriod",
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
        name: "ReceiveMessageWaitTimeSeconds",
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
        name: "VisibilityTimeout",
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

/// Per-service state for com-amazonaws-sqs-amazonsqs. A handler reaches it through [`service_state`].
///
/// `queues` is a `BTreeMap` rather than a `HashMap` so iteration is in name order:
/// the order a listing pages over has to be the same on every call.
#[derive(Debug, Default)]
pub struct ComAmazonawsSqsAmazonsqsState {
    /// Every queue that exists, keyed by `QueueName`.
    pub queues: BTreeMap<String, Queue>,
}

/// A stored queue: the state the reported `Attributes` map is rebuilt from.
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
    pub created_at: SystemTime,
    /// When an attribute of this queue last changed; equals `created_at` until a
    /// `SetQueueAttributes` changes something.
    pub last_modified_at: SystemTime,
}

impl Queue {
    /// Every attribute this queue has a value for: its stored configuration plus the
    /// three the record itself derives (`QueueArn`, `CreatedTimestamp`,
    /// `LastModifiedTimestamp`).
    ///
    /// The two timestamps are reported "in seconds (epoch time)", per their own
    /// entries in the `AttributeNames` documentation.
    pub fn reported_attributes(&self) -> BTreeMap<String, String> {
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
        reported
    }

    /// Whether this queue is a FIFO queue, which is what makes the FIFO-only
    /// attributes applicable to it.
    pub fn is_fifo(&self) -> bool {
        self.attributes
            .get(FIFO_QUEUE_ATTRIBUTE)
            .is_some_and(|value| value == "true")
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

/// The modelled refusal for a `QueueUrl` no queue answers to.
///
/// The payload's wording follows the shape's own documentation: "Ensure that the
/// `QueueUrl` is correct and that the queue has not been deleted."
pub fn queue_does_not_exist(queue_url: &str) -> QueueDoesNotExist {
    QueueDoesNotExist {
        message: Some(format!(
            "The queue referred to by {queue_url} does not exist. Ensure that the QueueUrl is correct and that the queue has not been deleted."
        )),
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
