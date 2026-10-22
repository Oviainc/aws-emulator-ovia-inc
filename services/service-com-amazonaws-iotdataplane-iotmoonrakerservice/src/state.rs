// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotdataplane-iotmoonrakerservice service.
//!
//! Two stores, and every handler computes its whole response from them:
//!
//! * `shadows` — the Device Shadow documents, keyed by `(thingName, shadowName)` with
//!   the empty shadow name standing for the unnamed ("classic") shadow. The stored
//!   document is the merge of every `UpdateThingShadow` that has been applied to it;
//!   `state.delta` is NOT stored, it is derived from `desired` vs `reported` whenever
//!   a document is rendered.
//! * `retained_messages` — the MQTT retained message of each topic, as the last
//!   `Publish` with `retain=true` left it.
//!
//! Both are `BTreeMap`s so a list operation walks them in a stable key order and an
//! offset-style `nextToken` keeps meaning across calls.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

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

/// The largest stored shadow document this service keeps, in bytes, counting the
/// `metadata` mirror and the version/timestamp alongside the `state` node that
/// [`MAX_SHADOW_DOCUMENT_BYTES`] bounds. Emulator-chosen: the metadata mirror has one
/// `{"timestamp": …}` leaf per state leaf, so it grows in proportion to a state node
/// that is already bounded, and this is the belt-and-braces cap that makes the whole
/// stored record — and therefore a `GetThingShadow` response — bounded outright.
pub const MAX_STORED_SHADOW_BYTES: usize = 64 * 1024;

/// How many shadow documents this emulator stores, across every thing and shadow
/// name. Emulator-chosen, for the same reason as [`MAX_RETAINED_TOPICS`]: both the
/// thing name and the shadow name come straight from the request, so without a cap a
/// caller could name a new shadow per request forever. An update of a shadow that is
/// already stored is never refused by this cap, and `DeleteThingShadow` frees a slot.
pub const MAX_SHADOWS: usize = 1024;

/// The largest `Publish` payload this service accepts, in bytes. The AWS IoT Core
/// message-broker quota "Maximum size of a message payload" is 128 KB.
pub const MAX_PUBLISH_PAYLOAD_BYTES: usize = 128 * 1024;

/// How many topics may hold a retained message at once. Emulator-chosen: the real
/// service's quota is far larger than an in-memory emulator should be asked to hold,
/// and a bound is what keeps `Publish` from growing the store without limit. Clearing
/// a retained message (an empty retained payload) always frees a slot.
pub const MAX_RETAINED_TOPICS: usize = 1024;

/// The page size `ListNamedShadowsForThing` uses when the caller gives no `pageSize`,
/// so one unparameterised request cannot return a page that grows with the number of
/// stored shadows. It is the top of the modelled `pageSize` range (1..100).
pub const DEFAULT_SHADOW_PAGE_SIZE: usize = 100;

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
#[derive(Debug, Clone, Default)]
pub struct Shadow {
    pub desired: Option<ShadowSection>,
    pub reported: Option<ShadowSection>,
    /// The document version, 1 after the first update and incremented by every
    /// accepted update.
    pub version: i64,
    /// When the document was last accepted, in epoch SECONDS — the unit shadow
    /// documents use for `timestamp` and for every metadata leaf.
    pub timestamp_secs: i64,
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

    /// The shadow document as a client reads it: `state` (with `delta` derived now),
    /// `metadata`, `version` and `timestamp`.
    pub fn document(&self) -> Value {
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
        document.insert("version".to_string(), Value::from(self.version));
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

/// Per-service state for com-amazonaws-iotdataplane-iotmoonrakerservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotdataplaneIotmoonrakerserviceState {
    /// Shadow documents keyed by `(thingName, shadowName)`; [`CLASSIC_SHADOW`] is the
    /// unnamed shadow's key.
    pub shadows: BTreeMap<(String, String), Shadow>,
    /// Retained messages keyed by topic.
    pub retained_messages: BTreeMap<String, RetainedMessage>,
}

impl ComAmazonawsIotdataplaneIotmoonrakerserviceState {
    /// Whether `thing_name` has any shadow at all — named or classic.
    pub fn has_any_shadow(&self, thing_name: &str) -> bool {
        self.shadows.keys().any(|(thing, _)| thing == thing_name)
    }

    /// The NAMED shadows of `thing_name`, in stable key order. The classic shadow is
    /// not a named shadow, so it is never listed.
    pub fn named_shadows_for_thing(&self, thing_name: &str) -> Vec<String> {
        self.shadows
            .keys()
            .filter(|(thing, name)| thing == thing_name && name != CLASSIC_SHADOW)
            .map(|(_, name)| name.clone())
            .collect()
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
