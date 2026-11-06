// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotdataplane-iotmoonrakerservice service.
//!
//! Three stores, and every handler computes its whole response from them:
//!
//! * `shadows` — one [`ShadowSlot`] per `(thingName, shadowName)`, with the empty
//!   shadow name standing for the unnamed ("classic") shadow. The slot holds the
//!   shadow's VERSION COUNTER and the stored document, which is the merge of every
//!   `UpdateThingShadow` that has been applied to it; `state.delta` is NOT stored, it is
//!   derived from `desired` vs `reported` whenever a document is rendered. The counter
//!   is the slot's, not the document's, so it survives the document's deletion.
//! * `retained_messages` — the MQTT retained message of each topic, as the last
//!   `Publish` with `retain=true` left it.
//! * `connections` — the MQTT session the broker holds per client id, which is what the
//!   connection operations (`GetConnection`, `ListSubscriptions`, `SendDirectMessage`,
//!   `DeleteConnection`) report on; a client id with no session is their shared
//!   not-found condition.
//!
//! All three are `BTreeMap`s so a list operation walks them in a stable key order and an
//! offset-style `nextToken` keeps meaning across calls.
//!
//! Beside the two stores, this module holds the service's RESPONSE TEXT: the fixed
//! message it answers each condition with, and the two templates it renders from the
//! request. They live here, not in the handlers, because several operations answer the
//! same condition and a message composed per handler drifts into describing the
//! emulator's internals instead of the service's contract.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// How many `traceId`s this process has minted. Part of what makes the next one
/// unique: two calls in the same clock tick still differ.
static TRACE_IDS_MINTED: AtomicU64 = AtomicU64::new(0);

/// A fresh `traceId` for one response: the per-request correlation id this data plane
/// returns beside `message` in every envelope it writes.
///
/// Minted per CALL and stored nowhere: the id identifies the request, not the
/// resource, so two calls that read the same shadow carry different ids and neither is
/// derived from the request. Unique without a random source — the clock, the process
/// and a monotone per-process counter cannot repeat together — and rendered in the
/// 8-4-4-4-12 hexadecimal form the member carries, which is what the digest of those
/// three is sliced into.
pub fn new_trace_id() -> String {
    let mut digest = Sha256::new();
    digest.update(duration_since_epoch().as_nanos().to_be_bytes());
    digest.update(TRACE_IDS_MINTED.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    digest.update(std::process::id().to_be_bytes());
    let hex = hex::encode(digest.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The service's fixed rejection for a state document over the size limit, which is
/// the one message this service builds from the request: the size of the state the
/// document carries (see [`state_sections_bytes`]) and the limit it exceeded. Rendered
/// exactly as the data plane renders it, two spaces after the period included.
pub fn json_document_too_large(size: usize, max_allowed: usize) -> String {
    format!("Json document too large.  Size is {size}, max allowed: {max_allowed}")
}

/// The size the state-document quota weighs, in bytes: the STATE a document carries —
/// its `desired` and `reported` nodes as canonical JSON.
///
/// What the quota bounds is the state itself, so none of the framing around it counts:
/// not the document's own `state`/`version` keys, not the insignificant whitespace a
/// client spaced the request out with, not the `state.delta` the service DERIVES from
/// the two sections, and not the `metadata` mirror the service maintains beside them.
/// The same measure weighs one request's document and the document a merge would
/// leave stored ([`Shadow::state_bytes`]), so one limit means one thing throughout.
pub fn state_sections_bytes(desired: Option<&Value>, reported: Option<&Value>) -> usize {
    [desired, reported]
        .into_iter()
        .flatten()
        .map(|section| section.to_string().len())
        .sum()
}

/// This emulator's refusal of a request larger than one of its OWN size caps. The
/// service's `Json document too large.  …` template reports the service's published
/// quota, so a cap this emulator chose says it is the emulator's instead of borrowing
/// that text. `subject` reads as the verb phrase after "this emulator".
pub fn emulator_size_cap(subject: &str, size: usize, max_allowed: usize) -> String {
    format!(
        "this emulator {subject} of at most {max_allowed} bytes, and this request would need {size}"
    )
}

/// The member of an update document carrying the caller's correlation token, which the
/// accepted reply echoes and the stored document never keeps — see
/// [`reply_with_client_token`].
pub const CLIENT_TOKEN: &str = "clientToken";

/// The reply document for an accepted update: the document the merge produced, plus the
/// request's own [`CLIENT_TOKEN`] where it carried one.
///
/// `clientToken` is the caller's correlation key, not shadow state: the acknowledgement
/// carries it back so a client that multiplexes updates can match this reply to the
/// request that produced it, while the STORED document — what [`Shadow::document`]
/// renders for every later reader — never holds it. A request that named no token gets a
/// reply with no `clientToken` member, so the absence still means "you sent none".
pub fn reply_with_client_token(mut document: Value, client_token: Option<&Value>) -> Value {
    if let (Some(token), Some(members)) = (client_token, document.as_object_mut()) {
        members.insert(CLIENT_TOKEN.to_string(), token.clone());
    }
    document
}

/// The service's fixed rejection for a payload that is not a JSON document.
pub const PAYLOAD_INVALID_JSON: &str = "Payload contains invalid json";

/// The service's fixed rejection for a document that names no `state` node — the one
/// node a state document must carry.
pub const MISSING_STATE_NODE: &str = "Missing required node: state";

/// The service's fixed rejection for a `version` that is not a document version.
pub const INVALID_VERSION: &str = "Invalid version";

/// The service's fixed rejection for a node of the state document that must be an
/// object and is not. `node` is the node's name as the service writes it (`State`,
/// `Desired`, `Reported`).
pub fn node_must_be_an_object(node: &str) -> String {
    format!("{node} node must be an object")
}

/// The service's fixed rejection for a node under `state` that the service does not
/// recognise: it merges `desired` and `reported` and derives everything else.
pub fn invalid_state_node(node: &str) -> String {
    format!("State contains an invalid node: '{node}'")
}

/// The service's fixed rejection for an update whose stated `version` is not the
/// version stored. The condition, not the two versions, is what the caller branches
/// on — a conflicted writer re-reads the document rather than parsing this text.
pub const VERSION_CONFLICT: &str = "Version conflict";

/// The service's fixed rejection for a publish topic that is not a single, concrete
/// topic name — an MQTT wildcard makes it a topic FILTER, which only a subscription
/// may be.
pub const INVALID_TOPIC_FILTER: &str = "Invalid topic filter";

/// How many `/`-separated levels one topic name may carry. The IoT Core message-broker
/// quota "Maximum number of slashes in topic and topic filter" is 7, which is 8 levels.
pub const MAX_TOPIC_PARTS: usize = 8;

/// The service's fixed rejection for a topic with more levels than the broker allows.
pub fn too_many_topic_parts(max_parts: usize) -> String {
    format!("Topic can't contain more than {max_parts} parts")
}

/// How many BYTES one topic name may carry, measured on the UTF-8 topic. The IoT Core
/// message-broker quota "Topic size" is 256 bytes, and it is independent of
/// [`MAX_TOPIC_PARTS`]: a topic of legal depth is still refused once its name is longer
/// than this.
pub const MAX_TOPIC_BYTES: usize = 256;

/// The service's fixed rejection for a topic name longer than the broker allows.
pub fn too_many_topic_bytes(max_bytes: usize) -> String {
    format!("Topic can't contain more than {max_bytes} bytes")
}

/// Why the broker will not publish to `topic`, or `None` when the name is one it will.
///
/// The rule is the MQTT topic-NAME rule, which holds wherever a topic names the
/// destination of one message rather than a subscription's filter: a name carrying a
/// wildcard is a FILTER and not a name, a name may not carry more levels than
/// [`MAX_TOPIC_PARTS`], and a name may not be longer than [`MAX_TOPIC_BYTES`]. Each
/// condition has its own message naming its own limit, so a caller is told which ceiling
/// it crossed; the limits are independent, so a name of legal depth is still weighed
/// against the byte ceiling.
pub fn topic_name_refusal(topic: &str) -> Option<String> {
    // MQTT forbids the single-level (`+`) and multi-level (`#`) wildcards in a publish
    // topic: a topic carrying one is a topic FILTER, which only a subscription may be.
    if topic.contains('+') || topic.contains('#') {
        return Some(INVALID_TOPIC_FILTER.to_string());
    }
    // A topic name is a path of levels separated by `/`, and the broker bounds how many
    // levels one carries.
    if topic.split('/').count() > MAX_TOPIC_PARTS {
        return Some(too_many_topic_parts(MAX_TOPIC_PARTS));
    }
    // …and, independently, how long the whole name is.
    if topic.len() > MAX_TOPIC_BYTES {
        return Some(too_many_topic_bytes(MAX_TOPIC_BYTES));
    }
    None
}

/// The service's fixed answer when a topic holds no retained message.
pub const RETAINED_MESSAGE_NOT_FOUND: &str = "Retained message not found";

/// The service's fixed answer when the addressed shadow does not exist. `name` is the
/// shadow the request addressed: its `name` query parameter, or — for the unnamed
/// ("classic") shadow, which is addressed by the thing alone — the thing's own name.
pub fn no_shadow_exists(name: &str) -> String {
    format!("No shadow exists with name: '{name}'")
}

/// The shadow a request addresses, as the service names it in a message: the request's
/// `shadowName`, or the thing's own name for the classic shadow.
pub fn addressed_shadow_name<'a>(thing_name: &'a str, shadow_name: Option<&'a str>) -> &'a str {
    match shadow_name {
        Some(name) if name != CLASSIC_SHADOW => name,
        _ => thing_name,
    }
}

/// The first character of the RESERVED shadow-name namespace. This service keeps every
/// shadow name beginning with `$` for its own use, exactly as it keeps the `$` topic
/// namespace (`crate::ops::publish::RESERVED_TOPIC_PREFIX`), and refuses a request that
/// addresses one — even though the modelled `shadowName` pattern admits `$` as a
/// character, which makes the namespace the service's rule rather than the model's. Only
/// the FIRST character is reserved: a `$` later in the name (`fleet$a`) is ordinary.
pub const RESERVED_SHADOW_PREFIX: char = '$';

/// The service's rejection of a shadow name in the reserved namespace, naming the shadow
/// the request addressed — which is the one thing the caller has to change.
pub fn invalid_shadow_name(name: &str) -> String {
    format!("Invalid shadow name : {name}")
}

/// Why this service will not address the shadow `shadow_name`, or `None` when the name is
/// one it will.
///
/// The rule is the shadow-NAME rule, and it holds wherever a request names one shadow —
/// a write and a read alike, so a name the write path refuses is never reported by the
/// read path as a shadow that merely holds nothing: a name in the reserved namespace
/// ([`RESERVED_SHADOW_PREFIX`]) is not a name a client may address. A request that names
/// no shadow addresses the unnamed ("classic") shadow, which is never refused here.
/// Every other bound on the name — its length and its character set — is the MODEL's, and
/// is weighed at this service's front door before a handler runs (see [`BAD_REQUEST`]).
pub fn shadow_name_refusal(shadow_name: Option<&str>) -> Option<String> {
    let name = shadow_name?;
    if name.starts_with(RESERVED_SHADOW_PREFIX) {
        return Some(invalid_shadow_name(name));
    }
    None
}

/// The service's fixed refusal of a request its front door will not take at all: a value
/// outside a bound the model declares, or a required member the request left out.
///
/// Terse on purpose. Unlike the conditions a handler reports — each of which names the
/// node, the shadow or the limit at fault — the front-door refusal carries NOTHING of the
/// request: not the member, not the bound it crossed, and not the value the caller sent.
/// It is one sentence a client can match on whichever member of whichever operation was
/// at fault. See `crate::service::EmulatedService::override_refusal`, which is where this
/// service answers such a refusal.
pub const BAD_REQUEST: &str = "Bad Request";

/// The shadow-name key of the unnamed ("classic") shadow — the shadow a request that
/// carries no `name` query parameter addresses. The model constrains `shadowName` to
/// 1..64 characters, so the empty string can never collide with a named shadow.
pub const CLASSIC_SHADOW: &str = "";

/// The largest shadow state document this service accepts, in bytes. The AWS IoT Core
/// Device Shadow service quota "Maximum size of a JSON state document" is 8 KB; it
/// bounds the ACCUMULATED `state` node of a stored shadow, not just one request, so
/// an update whose merged result would exceed it is refused with the operation's
/// modelled `RequestEntityTooLargeException` and changes nothing.
pub const MAX_SHADOW_DOCUMENT_BYTES: usize = 8 * 1024;

// There is deliberately NO emulator-chosen ceiling on the request entity an update may
// carry, weighed before the document is parsed. The size verdict on a state document is
// the SERVICE's, built from the state the document carries and the published quota
// ([`json_document_too_large`]), and a second ceiling of the emulator's own would answer
// a document AWS refuses with that message in the emulator's own words instead — the
// caller would be told the limit is the emulator's ceiling rather than 8 KB. The entity
// is bounded where it is read instead: the transport buffers at most
// `vendor/server`'s `MAX_PRODUCTION_BODY_BYTES` (16 MiB) per request and answers a
// larger one 413 itself, and the payload is already wholly in memory by the time this
// handler runs, so parsing it to weigh its state costs a constant factor of a request
// that is already bounded.

/// The largest stored shadow document this service keeps, in bytes, counting the
/// `metadata` mirror and the version/timestamp alongside the `state` node that
/// [`MAX_SHADOW_DOCUMENT_BYTES`] bounds. Emulator-chosen: the metadata mirror has one
/// `{"timestamp": …}` leaf per state leaf, so it grows in proportion to a state node
/// that is already bounded, and this is the belt-and-braces cap that makes the whole
/// stored record — and therefore a `GetThingShadow` response — bounded outright.
pub const MAX_STORED_SHADOW_BYTES: usize = 64 * 1024;

/// How many shadow SLOTS this emulator keeps, across every thing and shadow name.
/// Emulator-chosen, for the same reason as [`MAX_RETAINED_TOPICS`]: both the thing name
/// and the shadow name come straight from the request, so without a cap a caller could
/// name a new shadow per request forever. An update of a shadow name that already has a
/// slot is never refused by this cap — including one whose document was deleted, since
/// the slot (and with it the version counter) outlives the document, so `DeleteThingShadow`
/// does NOT free a slot.
pub const MAX_SHADOWS: usize = 1024;

/// The largest message payload this service accepts, in bytes. The AWS IoT Core
/// message-broker quota "Maximum size of a message payload" is 128 KB, measured
/// inclusively: a payload of exactly this many bytes is accepted and one byte more is
/// refused with `RequestEntityTooLargeException` (413) — the entity is too large for the
/// service, which is what tells a publisher to split or shrink the message, as against a
/// malformed request. It bounds every message the broker takes: a `Publish` and a
/// `SendDirectMessage` alike.
pub const MAX_PUBLISH_PAYLOAD_BYTES: usize = 128 * 1024;

/// The service's rejection of a message payload over [`MAX_PUBLISH_PAYLOAD_BYTES`]:
/// fixed text naming the CEILING the message crossed and nothing else. The size the
/// request carried is deliberately absent — the broker states the one limit a publisher
/// has to shrink to, not the request it just refused, so the sentence is the same for
/// every over-size message and a caller parsing it reads one number.
pub fn payload_too_large(max_allowed: usize) -> String {
    format!("payload larger than {max_allowed} bytes")
}

/// How many topics may hold a retained message at once. Emulator-chosen: the real
/// service's quota is far larger than an in-memory emulator should be asked to hold,
/// and a bound is what keeps `Publish` from growing the store without limit. Clearing
/// a retained message (an empty retained payload) always frees a slot.
pub const MAX_RETAINED_TOPICS: usize = 1024;

/// The page size `ListSubscriptions` uses when the caller names no `maxResults`.
///
/// Source: `ListSubscriptionsRequest$maxResults`'s own documentation — "By default, this
/// is set to 20" (`smithy_query(query="operation", operation="ListSubscriptions")` gives
/// its modelled `range {min: 1, max: 200}`, which bounds what a caller may ask for).
pub const DEFAULT_SUBSCRIPTION_PAGE_SIZE: usize = 20;

/// The largest page `ListSubscriptions` answers with: the top of `maxResults`' modelled
/// range, so a caller cannot ask for more than the model allows.
pub const MAX_SUBSCRIPTION_PAGE_SIZE: usize = 200;

/// The page size `ListNamedShadowsForThing` uses when the caller gives no `pageSize`,
/// so one unparameterised request cannot return a page that grows with the number of
/// stored shadows. It is the top of the modelled `pageSize` range (1..100).
pub const DEFAULT_SHADOW_PAGE_SIZE: usize = 100;

/// The largest page `ListRetainedMessages` answers with, which is also the page size it
/// uses when a request names no `maxResults`: the top of that member's modelled range.
///
/// Source: `smithy_query(query="operation", operation="ListRetainedMessages")` — its
/// `maxResults` carries `range {min: 1, max: 200}`. Reusing the modelled ceiling as the
/// default is what keeps one unparameterised request from returning a page that grows
/// with the number of retained topics.
pub const MAX_RETAINED_MESSAGE_PAGE_SIZE: usize = 200;

/// One section (`desired` or `reported`) of a shadow document: the merged state and
/// the metadata mirror that carries each leaf's last-written timestamp.
#[derive(Debug, Clone, Default)]
pub struct ShadowSection {
    /// The section's state, always a JSON object.
    pub state: Map<String, Value>,
    /// The same tree with every leaf replaced by `{"timestamp": <epoch seconds>}`.
    pub metadata: Map<String, Value>,
}

/// A stored shadow document. A section is `None` while the shadow has never carried
/// it — that is what makes `state.desired` absent from the rendered document rather
/// than present and empty.
///
/// The document carries no version of its own: the version is the SLOT's
/// ([`ShadowSlot::version`]), because the counter outlives the document, and
/// [`Shadow::document`] is handed the version the slot stands at.
#[derive(Debug, Clone, Default)]
pub struct Shadow {
    pub desired: Option<ShadowSection>,
    pub reported: Option<ShadowSection>,
    /// When the document was last accepted, in epoch SECONDS — the unit shadow
    /// documents use for `timestamp` and for every metadata leaf.
    pub timestamp_secs: i64,
}

/// One shadow slot: everything the service keeps for a `(thingName, shadowName)`.
///
/// The slot exists from the first update of that shadow name and is never dropped: the
/// VERSION COUNTER lives here, so it survives `DeleteThingShadow`, which takes the
/// document away and consumes a version of its own. That is what makes the counter
/// monotone per shadow name — a device holding a version from before a delete is still
/// refused with `ConflictException` afterwards, and a recreated document resumes above
/// the deleted one instead of starting again at 1.
#[derive(Debug, Clone, Default)]
pub struct ShadowSlot {
    /// The highest version this shadow name has consumed: 0 before anything has, 1
    /// after the first accepted update, and one more for every accepted update and
    /// every delete.
    pub version: i64,
    /// The stored document, or `None` once it has been deleted.
    pub document: Option<Shadow>,
}

impl ShadowSlot {
    /// Consume the next version of this shadow name and answer it. The ONE place the
    /// counter moves, so no caller can move it by a different step.
    pub fn consume_version(&mut self) -> i64 {
        self.version += 1;
        self.version
    }
}

impl Shadow {
    /// Merge one update's `state.desired` / `state.reported` into this shadow, the way
    /// the Device Shadow service merges an update: objects merge key by key, a `null`
    /// leaf deletes that key, and any other value replaces what was there. Every leaf
    /// the patch writes takes `timestamp_secs` as its metadata timestamp; untouched
    /// leaves keep the timestamp they already had.
    pub fn apply(
        &mut self,
        desired: Option<&Value>,
        reported: Option<&Value>,
        timestamp_secs: i64,
    ) {
        for (patch, section) in [
            (desired, &mut self.desired),
            (reported, &mut self.reported),
        ] {
            let Some(patch) = patch else { continue };
            let target = section.get_or_insert_with(ShadowSection::default);
            match patch {
                Value::Object(patch) => merge_object(
                    &mut target.state,
                    &mut target.metadata,
                    patch,
                    timestamp_secs,
                ),
                // A section whose patch is not an object cannot be merged key by key;
                // the handler refuses that before calling in, so this arm only guards
                // the invariant.
                _ => {
                    target.state.clear();
                    target.metadata.clear();
                }
            }
            // A patch that deleted every key leaves the section empty, and an empty
            // section is not part of the document.
            if target.state.is_empty() {
                *section = None;
            }
        }
    }

    /// The size of the state this shadow STORES, in bytes, as
    /// [`state_sections_bytes`] measures it: its `desired` and `reported` sections as
    /// canonical JSON, with the derived `delta` and the `metadata` mirror excluded.
    ///
    /// Serializing each section's map directly measures the same bytes
    /// `state_sections_bytes` would, without copying the section to wrap it in a
    /// `Value` first.
    pub fn state_bytes(&self) -> usize {
        [self.desired.as_ref(), self.reported.as_ref()]
            .into_iter()
            .flatten()
            .map(|section| {
                serde_json::to_string(&section.state)
                    .expect("a state section serializes to JSON")
                    .len()
            })
            .sum()
    }

    /// The shadow document as a client reads it: `state` (with `delta` derived now),
    /// `metadata`, `version` and `timestamp`. `version` is the slot's counter, which
    /// the caller passes in.
    pub fn document(&self, version: i64) -> Value {
        let mut state = Map::new();
        let mut metadata = Map::new();
        if let Some(desired) = &self.desired {
            state.insert("desired".to_string(), Value::Object(desired.state.clone()));
            metadata.insert(
                "desired".to_string(),
                Value::Object(desired.metadata.clone()),
            );
        }
        if let Some(reported) = &self.reported {
            state.insert("reported".to_string(), Value::Object(reported.state.clone()));
            metadata.insert(
                "reported".to_string(),
                Value::Object(reported.metadata.clone()),
            );
        }
        if let Some(delta) = self.delta() {
            state.insert("delta".to_string(), delta);
        }
        let mut document = Map::new();
        document.insert("state".to_string(), Value::Object(state));
        document.insert("metadata".to_string(), Value::Object(metadata));
        document.insert("version".to_string(), Value::from(version));
        document.insert("timestamp".to_string(), Value::from(self.timestamp_secs));
        Value::Object(document)
    }

    /// The desired state that `reported` does not already match — the `state.delta`
    /// node. `None` when there is nothing desired, or when everything desired is
    /// already reported.
    pub fn delta(&self) -> Option<Value> {
        let desired = Value::Object(self.desired.as_ref()?.state.clone());
        match &self.reported {
            // Nothing has been reported, so the whole desired state is outstanding.
            None => Some(desired),
            Some(reported) => delta(&desired, &Value::Object(reported.state.clone())),
        }
    }
}

/// The part of `desired` that `reported` does not match, compared leaf by leaf.
fn delta(desired: &Value, reported: &Value) -> Option<Value> {
    if desired == reported {
        return None;
    }
    match (desired, reported) {
        (Value::Object(desired), Value::Object(reported)) => {
            let mut out = Map::new();
            for (key, wanted) in desired {
                match reported.get(key) {
                    // Not reported at all: the whole desired subtree is outstanding.
                    None => {
                        out.insert(key.clone(), wanted.clone());
                    }
                    Some(have) => {
                        if let Some(difference) = delta(wanted, have) {
                            out.insert(key.clone(), difference);
                        }
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(Value::Object(out))
            }
        }
        // Keys reported but not desired are never part of the delta, and a non-object
        // mismatch is outstanding as a whole.
        _ => Some(desired.clone()),
    }
}

/// Merge `patch` into `target`, keeping `meta` a mirror of `target` whose leaves are
/// `{"timestamp": timestamp_secs}`.
fn merge_object(
    target: &mut Map<String, Value>,
    meta: &mut Map<String, Value>,
    patch: &Map<String, Value>,
    timestamp_secs: i64,
) {
    for (key, value) in patch {
        match value {
            // A null leaf deletes the key, and its metadata with it.
            Value::Null => {
                target.remove(key);
                meta.remove(key);
            }
            Value::Object(sub) => {
                // An object patch merges into an object node, so a node that is
                // absent or holds a non-object becomes a fresh object first —
                // writing `{"a": {"b": 1}}` over `{"a": 7}` replaces the scalar, and
                // its metadata leaf goes with it rather than surviving beside the
                // new sub-tree.
                let branch = target
                    .entry(key.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                let replaced = !branch.is_object();
                if replaced {
                    *branch = Value::Object(Map::new());
                }
                let branch_meta = meta
                    .entry(key.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                if replaced || !branch_meta.is_object() {
                    *branch_meta = Value::Object(Map::new());
                }
                let emptied = {
                    let state = target
                        .get_mut(key)
                        .and_then(Value::as_object_mut)
                        .expect("the branch was just made an object");
                    let state_meta = meta
                        .get_mut(key)
                        .and_then(Value::as_object_mut)
                        .expect("the branch metadata was just made an object");
                    merge_object(state, state_meta, sub, timestamp_secs);
                    state.is_empty()
                };
                if emptied {
                    // Every key under this node was deleted, so the node goes too.
                    target.remove(key);
                    meta.remove(key);
                }
            }
            other => {
                target.insert(key.clone(), other.clone());
                meta.insert(key.clone(), metadata_for(other, timestamp_secs));
            }
        }
    }
}

/// The metadata mirror of one written value: the same shape, with each leaf replaced
/// by its write timestamp.
fn metadata_for(value: &Value, timestamp_secs: i64) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(key, member)| (key.clone(), metadata_for(member, timestamp_secs)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| metadata_for(item, timestamp_secs))
                .collect(),
        ),
        _ => {
            let mut leaf = Map::new();
            leaf.insert("timestamp".to_string(), Value::from(timestamp_secs));
            Value::Object(leaf)
        }
    }
}

/// The retained message of one topic, as the last `Publish` with `retain=true` left
/// it. The payload is kept verbatim: `GetRetainedMessage` hands back exactly the bytes
/// that were published.
#[derive(Debug, Clone)]
pub struct RetainedMessage {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: i32,
    /// When the message was stored, in epoch MILLISECONDS — the unit
    /// `GetRetainedMessageResponse$lastModifiedTime` documents.
    pub last_modified_time_millis: i64,
    /// The publisher's `userProperties` JSON string, as bytes, or `None` when the
    /// message carried none.
    pub user_properties: Option<Vec<u8>>,
}

/// The service's fixed answer when the addressed MQTT client has no session the broker
/// holds — the one condition every connection operation shares. The requirement is the
/// 404 and the `ResourceNotFoundException` identity; this is the service's constant
/// wording for it, which carries no request data.
pub const CLIENT_NOT_FOUND: &str =
    "The client ID is not found. Verify that the client is connected and try again";

/// The service's answer when a `SendDirectMessage` asked for delivery confirmation.
///
/// `confirmation=true` means the service delivers at QoS 1 and answers only once the
/// RECEIVING CLIENT has sent a PUBACK; this emulator serves no MQTT transport, so no
/// client process can ever send one, and the modelled 504 is the honest answer rather
/// than a 200 that claims a confirmation nothing produced.
pub const NO_DELIVERY_CONFIRMATION: &str =
    "this emulator holds the client's session but serves no MQTT connection it could \
     acknowledge the delivery over, so no delivery confirmation can be received";

/// The lifecycle disconnect reason a disconnect the SERVICE performed carries — which is
/// what `DeleteConnection` is, as against a client closing its own connection
/// (`CLIENT_INITIATED_DISCONNECT`). One of the reasons AWS IoT Core's lifecycle-event
/// `disconnectReason` takes.
pub const SERVER_INITIATED_DISCONNECT: &str = "SERVER_INITIATED_DISCONNECT";

/// How long a disconnected client's connection information is still reported, in
/// MILLISECONDS.
///
/// Source: `GetConnectionResponse$disconnectedSince`'s own documentation — "This
/// information is available for 30 minutes after the client disconnects"
/// (`smithy_query(query="doc", shape="GetConnectionResponse", member="disconnectedSince")`).
/// Past it the session record is no longer something the service reports, so the
/// connection operations answer [`CLIENT_NOT_FOUND`] and the record is released.
pub const DISCONNECT_INFORMATION_RETENTION_MILLIS: i64 = 30 * 60 * 1000;

/// How many messages one session queues for a client that is offline. The IoT Core
/// message-broker quota "Maximum number of queued messages per persistent session" is
/// 100, and a 101st delivery pushes the oldest out, as a broker's bounded queue does.
pub const MAX_QUEUED_SESSION_MESSAGES: usize = 100;

/// One message `SendDirectMessage` delivered to a client, kept verbatim: the message a
/// session queues for a client that is offline is exactly the bytes and MQTT5 properties
/// that were sent.
#[derive(Debug, Clone)]
pub struct DirectMessage {
    pub topic: String,
    pub payload: Vec<u8>,
    /// The QoS the delivery used: 1 when the sender asked for confirmation, else 0 —
    /// what `SendDirectMessageRequest$confirmation` documents.
    pub qos: i32,
    pub content_type: Option<String>,
    pub response_topic: Option<String>,
    pub user_properties: Option<String>,
    pub payload_format_indicator: Option<String>,
    pub correlation_data: Option<String>,
    /// When the message was delivered, in epoch MILLISECONDS.
    pub delivered_at_millis: i64,
}

/// One MQTT client session the broker is holding: what the connection operations report
/// on, and what `DeleteConnection` disconnects or discards.
///
/// A session is opened by an MQTT CONNECT, which is a transport this emulator does not
/// serve, so nothing in the HTTP data plane puts a session in the table: a client id that
/// has never connected has no session, which is why every connection operation answers
/// it with [`CLIENT_NOT_FOUND`] — the 404 AWS answers for one. Every field is a fact
/// about the session, so each operation's response is computed from the record and never
/// from the request that asked for it.
#[derive(Debug, Clone, Default)]
pub struct MqttSession {
    /// The client id the session was opened under, which `GetConnection` reports back.
    pub client_id: String,
    /// Whether the client's network connection is open. A session outlives it when the
    /// client asked for a persistent session.
    pub connected: bool,
    /// Whether the client connected with a clean session.
    pub clean_session: bool,
    /// The thing associated with the client's principal, when there is one.
    pub thing_name: Option<String>,
    /// The keep-alive interval the client asked for, in SECONDS.
    pub keep_alive_seconds: i32,
    /// The session expiry interval the client configured, in SECONDS.
    pub session_expiry_seconds: i64,
    /// When the client connected, in epoch MILLISECONDS — the unit
    /// `GetConnectionResponse$connectedSince` documents.
    pub connected_since_millis: i64,
    /// When the client disconnected, in epoch MILLISECONDS, or 0 while it is connected.
    pub disconnected_since_millis: i64,
    /// Why the client last disconnected, when it is disconnected.
    pub disconnect_reason: Option<String>,
    /// The socket information, which `GetConnection` reports only to a request that
    /// asked for it (`includeSocketInformation`).
    pub source_ip: Option<String>,
    pub source_port: i32,
    pub target_ip: Option<String>,
    pub target_port: i32,
    /// The VPC endpoint the client connected through, for a client that connected
    /// through one.
    pub vpc_endpoint_id: Option<String>,
    /// The topic filters this session subscribes to, each with its QoS, in stable
    /// filter order — the order `ListSubscriptions` pages over.
    pub subscriptions: BTreeMap<String, i32>,
    /// What `SendDirectMessage` delivered to this session while the client was offline,
    /// oldest first and bounded by [`MAX_QUEUED_SESSION_MESSAGES`]. A message to a
    /// CONNECTED client is handed to the connection and queues nothing, exactly as a
    /// publish to a live subscriber leaves no trace.
    pub queued_messages: VecDeque<DirectMessage>,
}

impl MqttSession {
    /// Whether the service still reports this session at `now_millis`: always while the
    /// client is connected, and for [`DISCONNECT_INFORMATION_RETENTION_MILLIS`] after it
    /// disconnected.
    pub fn is_reported_at(&self, now_millis: i64) -> bool {
        self.connected
            || now_millis.saturating_sub(self.disconnected_since_millis)
                < DISCONNECT_INFORMATION_RETENTION_MILLIS
    }

    /// Close this session's network connection at `at_millis`, recording why. The
    /// session state (its subscriptions and queued messages) is kept: that is what makes
    /// it a persistent session.
    pub fn disconnect(&mut self, reason: &str, at_millis: i64) {
        self.connected = false;
        self.disconnected_since_millis = at_millis;
        self.disconnect_reason = Some(reason.to_string());
    }

    /// Queue `message` for this offline session, dropping the oldest once the queue is
    /// at [`MAX_QUEUED_SESSION_MESSAGES`].
    pub fn queue(&mut self, message: DirectMessage) {
        self.queued_messages.push_back(message);
        while self.queued_messages.len() > MAX_QUEUED_SESSION_MESSAGES {
            self.queued_messages.pop_front();
        }
    }
}

/// Per-service state for com-amazonaws-iotdataplane-iotmoonrakerservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotdataplaneIotmoonrakerserviceState {
    /// Shadow slots keyed by `(thingName, shadowName)`; [`CLASSIC_SHADOW`] is the
    /// unnamed shadow's key.
    pub shadows: BTreeMap<(String, String), ShadowSlot>,
    /// Retained messages keyed by topic.
    pub retained_messages: BTreeMap<String, RetainedMessage>,
    /// The MQTT session the broker holds per client id — see [`MqttSession`].
    pub connections: BTreeMap<String, MqttSession>,
}

impl ComAmazonawsIotdataplaneIotmoonrakerserviceState {
    /// Whether `thing_name` has any shadow DOCUMENT at all — named or classic. A slot
    /// whose document was deleted is not a shadow: it is only the counter that outlived
    /// one.
    pub fn has_any_shadow(&self, thing_name: &str) -> bool {
        self.shadows
            .iter()
            .any(|((thing, _), slot)| thing == thing_name && slot.document.is_some())
    }

    /// The NAMED shadows of `thing_name` that hold a document, in stable key order. The
    /// classic shadow is not a named shadow, so it is never listed.
    pub fn named_shadows_for_thing(&self, thing_name: &str) -> Vec<String> {
        self.shadows
            .iter()
            .filter(|((thing, name), slot)| {
                thing == thing_name && name != CLASSIC_SHADOW && slot.document.is_some()
            })
            .map(|((_, name), _)| name.clone())
            .collect()
    }

    /// The document stored under `key` and the version its slot stands at, or `None`
    /// when no document is stored there — whether the shadow never existed or its
    /// document was deleted.
    pub fn stored_shadow(&self, key: &(String, String)) -> Option<(&Shadow, i64)> {
        let slot = self.shadows.get(key)?;
        slot.document.as_ref().map(|document| (document, slot.version))
    }

    /// The version `key` stands at: the last version it consumed, which is what a
    /// conditional update must state and 0 for a shadow name nothing has written yet.
    /// A delete consumes one too, so this does not fall back after a delete.
    pub fn shadow_version(&self, key: &(String, String)) -> i64 {
        self.shadows.get(key).map_or(0, |slot| slot.version)
    }

    /// Release every session whose record the service no longer reports — see
    /// [`MqttSession::is_reported_at`]. A release the CLOCK already made due: it was owed
    /// at this instant whether or not anybody called, so a reporting operation may
    /// materialise it before it answers.
    pub fn release_retired_sessions(&mut self, now_millis: i64) {
        self.connections
            .retain(|_, session| session.is_reported_at(now_millis));
    }

    /// The session the broker holds for `client_id`, after the sessions the clock has
    /// already retired are released. `None` is the condition every connection operation
    /// answers with [`CLIENT_NOT_FOUND`].
    pub fn session(&mut self, client_id: &str, now_millis: i64) -> Option<&mut MqttSession> {
        self.release_retired_sessions(now_millis);
        self.connections.get_mut(client_id)
    }
}

/// The shadow-name key a request addresses: the request's `name`, or
/// [`CLASSIC_SHADOW`] when it names none.
pub fn shadow_key(thing_name: &str, shadow_name: Option<&str>) -> (String, String) {
    (
        thing_name.to_string(),
        shadow_name.unwrap_or(CLASSIC_SHADOW).to_string(),
    )
}

/// Now, in epoch seconds — the unit a shadow document's `timestamp` carries.
pub fn now_secs() -> i64 {
    duration_since_epoch().as_secs() as i64
}

/// Now, in epoch milliseconds — the unit `lastModifiedTime` carries.
pub fn now_millis() -> i64 {
    duration_since_epoch().as_millis() as i64
}

fn duration_since_epoch() -> std::time::Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotdataplaneIotmoonrakerserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotdataplaneIotmoonrakerserviceState>)
}
