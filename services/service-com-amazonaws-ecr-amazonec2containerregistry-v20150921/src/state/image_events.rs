// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this registry publishes when an image ARRIVES IN or LEAVES a
//! repository.
//!
//! # The rule this module implements
//!
//! Amazon ECR is an event SOURCE, not only a control plane a client polls: every image
//! action on a repository — a push that stores a manifest, a delete that removes one — is
//! published onto the account's default event bus ([`DEFAULT_EVENT_BUS`]) under the
//! `source` this service owns ([`EVENT_SOURCE`]) and the `detail-type` of an image action
//! ([`IMAGE_ACTION_DETAIL_TYPE`]), carrying in the event's `detail` the outcome, the
//! repository, the image's digest, the action that was performed and — when the action
//! involved one — the image's tag. A rule on that bus whose `EventPattern` matches then
//! delivers it to that rule's targets, which is how a push starts a pipeline, a function
//! or a workflow without the client calling anything else.
//!
//! # How an action becomes an event
//!
//! Nothing here decides WHAT happened: the operation that changed the repository does, and
//! what it recorded is the fact an event reports — so each event is built from the stored
//! record the operation's own answer is built from, and an action nothing performed
//! publishes nothing. A refused call changes no image, and so publishes no event either:
//! only the actions an operation really carried out reach [`publish`].
//!
//! Publication is made with NO store lock held: EventBridge is reached the way every
//! emulated service reaches another, through the framework's in-process callback
//! (`service_common::eventbridge_client`), so this crate names no other service's crate.
//! It is best effort — an emulator with no EventBridge mounted has nowhere to publish — so
//! a failure is logged and the operation's own answer stands.

use ::std::time::{SystemTime, UNIX_EPOCH};

/// The `source` of every event Amazon ECR publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry (`ecr`).
pub const EVENT_SOURCE: &str = "aws.ecr";

/// The `detail-type` of the event published when an image is pushed to or deleted from a
/// repository.
pub const IMAGE_ACTION_DETAIL_TYPE: &str = "ECR Image Action";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The `detail.action-type` of a push that stored an image manifest.
pub const ACTION_TYPE_PUSH: &str = "PUSH";

/// The `detail.action-type` of a delete that removed an image, or the last tag of one.
pub const ACTION_TYPE_DELETE: &str = "DELETE";

/// The `detail.result` of an action the registry carried out. Only actions that were
/// carried out are published here, so this is the one result an event of this module
/// reports.
pub const RESULT_SUCCESS: &str = "SUCCESS";

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a call that performed more actions than that publishes
/// them in several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many image actions ONE call publishes.
///
/// THIS CEILING IS THE EMULATOR'S, NOT AWS'S: each published event fans out to the targets
/// of every rule that matches it, so the work one request can set off is bounded here. It
/// is the batch maximum of the operation that can perform the most actions at once
/// (`BatchDeleteImage`'s `imageIds` is `@length(min: 1, max: 100)`), so no realistic call
/// loses an event to it.
const MAX_PUBLISHED_ACTIONS: usize = 100;

/// One image action an operation really carried out, waiting to be published.
///
/// Every field is read off the record the operation wrote or removed, so the event reports
/// the image that a read of the same instant reports and never a second, invented copy of
/// it.
#[derive(Debug, Clone)]
pub struct ImageAction {
    /// The repository the action was performed on.
    pub repository_name: String,
    /// [`ACTION_TYPE_PUSH`] or [`ACTION_TYPE_DELETE`].
    pub action_type: &'static str,
    /// The content digest of the image manifest the action was performed on.
    pub image_digest: String,
    /// The tag the action involved, when it involved one: the tag a push attached, or the
    /// tag a delete took off the image. An untagged push or a delete by digest of an
    /// untagged image carries none, and the event then states no `image-tag` rather than
    /// an empty one.
    pub image_tag: Option<String>,
    /// WHEN the action was performed — the instant the operation recorded, not the instant
    /// the event happened to be published.
    pub at: SystemTime,
}

impl ImageAction {
    /// The event `detail` this action is reported in.
    ///
    /// Public because it IS the contract: the members a rule's `EventPattern` filters on,
    /// which is what a suite of this crate holds the published event to.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        let mut detail = ::serde_json::Map::new();
        detail.insert(
            "result".to_string(),
            ::serde_json::Value::String(RESULT_SUCCESS.to_string()),
        );
        detail.insert(
            "repository-name".to_string(),
            ::serde_json::Value::String(self.repository_name.clone()),
        );
        detail.insert(
            "image-digest".to_string(),
            ::serde_json::Value::String(self.image_digest.clone()),
        );
        detail.insert(
            "action-type".to_string(),
            ::serde_json::Value::String(self.action_type.to_string()),
        );
        if let Some(tag) = &self.image_tag {
            detail.insert(
                "image-tag".to_string(),
                ::serde_json::Value::String(tag.clone()),
            );
        }
        ::serde_json::Value::Object(detail)
    }
}

/// Publish the image actions `actions` reports onto the account's default EventBridge bus.
///
/// Called with NO store lock held, after the operation has recorded what it did: a
/// publication reaches EventBridge's own handler, which matches the event against the
/// account's rules and delivers it to their targets — and a target may call back into this
/// service, so holding the registry's lock across it would deadlock the very call the event
/// set off.
pub async fn publish(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    actions: &[ImageAction],
) {
    if actions.is_empty() {
        return;
    }
    let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
    let published = &actions[..actions.len().min(MAX_PUBLISHED_ACTIONS)];
    if published.len() < actions.len() {
        ::tracing::warn!(
            performed = actions.len(),
            published = published.len(),
            "more image actions were performed in one call than this registry publishes to \
             EventBridge; the rest were not published"
        );
    }
    for window in published.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for action in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(IMAGE_ACTION_DETAIL_TYPE)
                // The event's `time` is WHEN the action was performed:
                // `PutEventsRequestEntry$Time` is "the time stamp of the event, per
                // RFC3339. If no time stamp is provided, the time stamp of the PutEvents
                // call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(action.at),
                ))
                .detail(action.detail().to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Amazon ECR image action could not be published to EventBridge"
            );
        }
    }
}

/// `at` as the epoch milliseconds the event's `time` carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}
