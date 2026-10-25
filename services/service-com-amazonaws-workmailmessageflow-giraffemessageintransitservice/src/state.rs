// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workmailmessageflow-giraffemessageintransitservice service.
//!
//! WorkMail Message Flow has no create or delete operation of its own: an
//! in-transit message exists because WorkMail delivery put it in transit, and the
//! MIME content `PutRawMessageContent` installs comes out of an S3 object that S3
//! holds. Both of those producers live outside this API, so this module owns the
//! two stores the two modeled operations act on, plus the seed helpers that stand
//! in for those producers ([`ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState::seed_in_transit_message`]
//! and [`ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState::seed_s3_object`]).
//! Nothing here fabricates content: every byte a handler returns was written into
//! the store by a seed or by a successful `PutRawMessageContent`.
//!
//! Both stores are regional. A message is in transit in the message flow of one
//! region and an S3 object sits in one bucket region, so each is keyed or tagged
//! with its region and a handler resolves it against the region the request was
//! made in ([`request_region`]).

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::S3Reference;

/// The region a request is treated as having been made in when its SigV4
/// credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `message` of the `ResourceNotFoundException` every operation of this
/// service raises for a `messageId` that is not in transit.
///
/// The model documents the error only as "The requested email message is not
/// found." (`smithy_query(query="shape", shape="ResourceNotFoundException")`) and
/// states no text, but the real service builds the text from the request: it is
/// the fixed prefix `Unknown messageId: ` followed verbatim by the `messageId`
/// the request asked for, so the caller is told which id was rejected. Both
/// modeled operations raise this error on the same condition, so both render it
/// through this one function rather than each inventing wording.
pub fn unknown_message_id(message_id: &str) -> String {
    format!("Unknown messageId: {message_id}")
}

/// A reference to an S3 object, as `PutRawMessageContent` supplies it.
///
/// `object_version` is part of the identity of what is referenced — the model
/// documents it as "If you enable versioning for the bucket, you can specify the
/// object version" (`smithy_query(query="shape", shape="S3Reference")`) — so it is
/// retained on the key and on the record of the last accepted reference rather
/// than dropped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3Ref {
    pub bucket: String,
    pub key: String,
    pub object_version: Option<String>,
}

impl S3Ref {
    /// A reference to `bucket`/`key`, at `object_version` when one is given.
    pub fn new(
        bucket: impl Into<String>,
        key: impl Into<String>,
        object_version: Option<String>,
    ) -> Self {
        Self {
            bucket: bucket.into(),
            key: key.into(),
            object_version,
        }
    }

    /// The reference a request carried, as stored.
    pub fn from_modeled(reference: &S3Reference) -> Self {
        Self {
            bucket: reference.bucket.clone(),
            key: reference.key.clone(),
            object_version: reference.object_version.clone(),
        }
    }

    /// `s3://bucket/key` (plus `?versionId=` when the reference is versioned), for
    /// an error message that tells the caller which reference failed.
    pub fn display(&self) -> String {
        match &self.object_version {
            Some(version) => format!("s3://{}/{}?versionId={version}", self.bucket, self.key),
            None => format!("s3://{}/{}", self.bucket, self.key),
        }
    }
}

/// An S3 object this emulator can resolve a reference to.
///
/// Beside the bytes it carries the two facts the model says WorkMail must be able
/// to check before it reads an object, both from the `InvalidContentLocation`
/// documentation (`smithy_query(query="doc", shape="InvalidContentLocation")`):
/// the bucket's region, which must match the region the request was made in, and
/// the bucket owner, which must be the calling account.
#[derive(Debug, Clone)]
pub struct S3Object {
    pub content: Vec<u8>,
    pub region: String,
    pub owner_account_id: String,
}

impl S3Object {
    /// An object holding `content`, in [`DEFAULT_REGION`], owned by this
    /// emulator's account.
    pub fn new(content: impl Into<Vec<u8>>) -> Self {
        Self {
            content: content.into(),
            region: DEFAULT_REGION.to_string(),
            owner_account_id: ::engine::emulator_account_id().to_string(),
        }
    }

    /// The same object in `region`.
    #[must_use]
    pub fn in_region(mut self, region: impl Into<String>) -> Self {
        self.region = region.into();
        self
    }

    /// The same object owned by `account_id`.
    #[must_use]
    pub fn owned_by(mut self, account_id: impl Into<String>) -> Self {
        self.owner_account_id = account_id.into();
        self
    }
}

/// The key an in-transit message is stored under.
///
/// This service is a regional data plane: its endpoint carries a region
/// (`endpointPrefix` `workmailmessageflow` with `sigv4` and an endpoint rule set —
/// `smithy_query(query="service_info")`), and an in-transit message belongs to the
/// message flow of exactly one region. The id alone is therefore not an address:
/// the region a request was made in is part of it, exactly as it is part of the
/// identity of an [`S3Object`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageKey {
    pub region: String,
    pub message_id: String,
}

impl MessageKey {
    /// The address of `message_id` in `region`'s message flow.
    pub fn new(region: impl Into<String>, message_id: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            message_id: message_id.into(),
        }
    }
}

/// One email message currently in transit through WorkMail's message flow.
#[derive(Debug, Clone)]
pub struct InTransitMessage {
    /// The id the URI label names this message by.
    pub message_id: String,
    /// The region whose message flow this message is in transit in. Only a request
    /// made in this region addresses it; every other region's endpoint answers as
    /// though the message did not exist.
    pub region: String,
    /// The raw MIME bytes `GetRawMessageContent` returns verbatim as
    /// `messageContent`.
    pub content: Vec<u8>,
    /// Whether the message is still eligible for update. A frozen message answers
    /// `PutRawMessageContent` with `MessageFrozen` and still serves
    /// `GetRawMessageContent`.
    pub frozen: bool,
    /// The reference the most recent successful `PutRawMessageContent` resolved,
    /// so the update that produced the current content is observable.
    pub last_s3_reference: Option<S3Ref>,
    /// When the content last changed.
    pub updated_at: SystemTime,
}

impl InTransitMessage {
    fn new(region: String, message_id: String, content: Vec<u8>) -> Self {
        Self {
            message_id,
            region,
            content,
            frozen: false,
            last_s3_reference: None,
            updated_at: SystemTime::now(),
        }
    }
}

/// Per-service state for com-amazonaws-workmailmessageflow-giraffemessageintransitservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState {
    /// Every in-transit message, keyed by the region its message flow is in and
    /// the `messageId` that region's endpoint addresses it by, so one region's
    /// endpoint never resolves another region's message.
    pub messages: HashMap<MessageKey, InTransitMessage>,
    /// The S3 objects a `PutRawMessageContent` reference can resolve to, keyed by
    /// the whole reference (bucket, key and object version) so a versioned
    /// reference resolves only to the version it names.
    pub s3_objects: HashMap<S3Ref, S3Object>,
}

impl ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState {
    /// Put a message into transit in [`DEFAULT_REGION`] with `content` as its MIME
    /// body, as that region's WorkMail delivery does, and hand back the stored
    /// record so a caller can mark it frozen.
    ///
    /// This is the producer this API does not model; it is NOT an AWS operation.
    pub fn seed_in_transit_message(
        &mut self,
        message_id: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> &mut InTransitMessage {
        self.seed_in_transit_message_in_region(DEFAULT_REGION, message_id, content)
    }

    /// Put a message into transit in `region`, as that region's WorkMail delivery
    /// does. The same id may be in transit in another region: the two are distinct
    /// messages, each addressable only through its own region's endpoint.
    ///
    /// This is the producer this API does not model; it is NOT an AWS operation.
    pub fn seed_in_transit_message_in_region(
        &mut self,
        region: impl Into<String>,
        message_id: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> &mut InTransitMessage {
        let key = MessageKey::new(region, message_id);
        let message = InTransitMessage::new(
            key.region.clone(),
            key.message_id.clone(),
            content.into(),
        );
        self.messages.entry(key).insert_entry(message).into_mut()
    }

    /// The message `message_id` names in `region`'s message flow, or `None` when no
    /// message is in transit under that id in that region — including when one is
    /// in transit under the same id in a different region.
    pub fn message(&self, region: &str, message_id: &str) -> Option<&InTransitMessage> {
        self.messages
            .get(&MessageKey::new(region, message_id))
    }

    /// Make `object` resolvable at `reference`, as an S3 `PutObject` does.
    pub fn seed_s3_object(&mut self, reference: S3Ref, object: S3Object) {
        self.s3_objects.insert(reference, object);
    }

    /// The object `reference` names, or `None` when nothing is stored there.
    pub fn resolve_s3_object(&self, reference: &S3Ref) -> Option<&S3Object> {
        self.s3_objects.get(reference)
    }

    /// Replace the MIME content of `message_id` in `region`'s message flow with
    /// `content`, recording the reference it came from, and answer whether such a
    /// message existed. A message in transit in another region is left untouched.
    pub fn replace_message_content(
        &mut self,
        region: &str,
        message_id: &str,
        content: Vec<u8>,
        reference: S3Ref,
    ) -> bool {
        match self.messages.get_mut(&MessageKey::new(region, message_id)) {
            Some(message) => {
                message.content = content;
                message.last_s3_reference = Some(reference);
                message.updated_at = SystemTime::now();
                true
            }
            None => false,
        }
    }
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is not
/// signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState>)
}
