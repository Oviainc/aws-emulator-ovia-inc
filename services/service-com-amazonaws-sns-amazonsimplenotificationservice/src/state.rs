// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sns-amazonsimplenotificationservice service.
//!
//! The records here are the SNS control plane: topics, their subscriptions, and the
//! messages `Publish` delivered to each subscription. They are deliberately named
//! `*Record` so a handler cannot confuse them with the wire shapes of the same name
//! in [`crate::ops::types`] (`Topic`, `Subscription`).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

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

/// The refusal reason for an attribute map that would cross
/// [`MAX_ATTRIBUTE_BYTES_PER_RECORD`], or `None` when it fits.
///
/// `holder` names the record the map belongs to ("a topic", "a subscription"), so ONE
/// ceiling check serves every record this service stores and each caller lifts the
/// reason into its own modelled error.
pub fn attribute_bytes_refusal(
    holder: &str,
    attributes: &HashMap<String, String>,
) -> Option<String> {
    let held = attribute_bytes(attributes);
    (held > MAX_ATTRIBUTE_BYTES_PER_RECORD).then(|| {
        format!(
            "Invalid parameter: Attributes - {holder} holds at most \
             {MAX_ATTRIBUTE_BYTES_PER_RECORD} bytes of Attributes: {held}"
        )
    })
}

/// The same check for a write of ONE attribute: measured on the map the write WOULD
/// produce (`value` of `None` clears the attribute), so a caller cannot grow one
/// record past the ceiling by repeating the call with ever larger values.
pub fn attribute_write_refusal(
    holder: &str,
    attributes: &HashMap<String, String>,
    name: &str,
    value: Option<&str>,
) -> Option<String> {
    let mut would_hold = attributes.clone();
    match value {
        Some(value) => {
            would_hold.insert(name.to_string(), value.to_string());
        }
        None => {
            would_hold.remove(name);
        }
    }
    attribute_bytes_refusal(holder, &would_hold)
}

/// The name a refusal gives a topic record.
pub const TOPIC_RECORD: &str = "a topic";

/// The name a refusal gives a subscription record.
pub const SUBSCRIPTION_RECORD: &str = "a subscription";

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

// =============================================================================
// Topic attributes SNS gives a meaning of its own
// =============================================================================

/// The topic attribute a caller-set access control policy lives under.
pub const POLICY_ATTRIBUTE: &str = "Policy";

/// The topic attribute a caller-set delivery policy lives under.
pub const DELIVERY_POLICY_ATTRIBUTE: &str = "DeliveryPolicy";

/// The topic attribute a caller-set display name lives under.
pub const DISPLAY_NAME_ATTRIBUTE: &str = "DisplayName";

/// The topic attribute the server-side-encryption key id lives under:
/// "The following attribute applies only to server-side-encryption: KmsMasterKeyId"
/// (`SetTopicAttributesInput$AttributeName` doc).
pub const KMS_MASTER_KEY_ID_ATTRIBUTE: &str = "KmsMasterKeyId";

/// The attribute that asks `CreateTopic` for a FIFO topic. It is fixed for the life of
/// the topic: a topic's kind is decided when it is created, so `SetTopicAttributes`
/// refuses it.
pub const FIFO_TOPIC_ATTRIBUTE: &str = "FifoTopic";

/// The attributes SNS COMPUTES for a topic rather than storing: the account that owns
/// it, its ARN, its subscription counts and the delivery policy it is effectively run
/// under. None of them is settable, so none is in [`TOPIC_ATTRIBUTE_NAMES`].
pub const OWNER_ATTRIBUTE: &str = "Owner";
pub const TOPIC_ARN_ATTRIBUTE: &str = "TopicArn";
pub const SUBSCRIPTIONS_PENDING_ATTRIBUTE: &str = "SubscriptionsPending";
pub const SUBSCRIPTIONS_CONFIRMED_ATTRIBUTE: &str = "SubscriptionsConfirmed";
pub const SUBSCRIPTIONS_DELETED_ATTRIBUTE: &str = "SubscriptionsDeleted";
pub const EFFECTIVE_DELIVERY_POLICY_ATTRIBUTE: &str = "EffectiveDeliveryPolicy";

/// The order SNS reports a topic's attributes in, as recorded `GetTopicAttributes`
/// responses carry it: the access policy, the owner, the pending-subscription count,
/// the encryption key id of an encrypted topic, the ARN, the effective delivery
/// policy, the confirmed count, the display name, the deleted count. Every other
/// attribute a topic holds follows these, ordered by name.
///
/// SNS answers the same attribute set with the same entry order every time, so a
/// client that reads the map positionally, or diffs two reads of one topic, sees one
/// stable document — which is why the attribute map is keyed by
/// [`TopicAttributeName`], whose ordering IS this order, rather than by a `String` in
/// a `HashMap` (whose entries come out in a different order on every call).
pub const REPORTED_TOPIC_ATTRIBUTE_ORDER: &[&str] = &[
    POLICY_ATTRIBUTE,
    OWNER_ATTRIBUTE,
    SUBSCRIPTIONS_PENDING_ATTRIBUTE,
    KMS_MASTER_KEY_ID_ATTRIBUTE,
    TOPIC_ARN_ATTRIBUTE,
    EFFECTIVE_DELIVERY_POLICY_ATTRIBUTE,
    SUBSCRIPTIONS_CONFIRMED_ATTRIBUTE,
    DISPLAY_NAME_ATTRIBUTE,
    SUBSCRIPTIONS_DELETED_ATTRIBUTE,
];

/// A topic attribute name as the key of a reported attribute map: a string on the
/// wire, ordered by [`REPORTED_TOPIC_ATTRIBUTE_ORDER`] rather than alphabetically, so
/// a `BTreeMap` keyed by it serializes its entries in the order SNS reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicAttributeName(String);

impl TopicAttributeName {
    /// Where SNS reports this attribute: its place in
    /// [`REPORTED_TOPIC_ATTRIBUTE_ORDER`], or past the end for a name that order does
    /// not list (which then sorts by name).
    fn reported_position(&self) -> usize {
        REPORTED_TOPIC_ATTRIBUTE_ORDER
            .iter()
            .position(|name| *name == self.0)
            .unwrap_or(REPORTED_TOPIC_ATTRIBUTE_ORDER.len())
    }
}

impl Ord for TopicAttributeName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.reported_position()
            .cmp(&other.reported_position())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for TopicAttributeName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<str> for TopicAttributeName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TopicAttributeName {
    fn from(name: &str) -> Self {
        Self(name.to_string())
    }
}

impl From<String> for TopicAttributeName {
    fn from(name: String) -> Self {
        Self(name)
    }
}

impl std::str::FromStr for TopicAttributeName {
    type Err = std::convert::Infallible;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(Self(name.to_string()))
    }
}

/// The maximum-message-size bounds SNS documents: "Valid values are 1024 to 1048576
/// (1 MiB). The default is 262144 (256 KiB)"
/// (`SetTopicAttributesInput$AttributeName` doc).
pub const MAXIMUM_MESSAGE_SIZE_ATTRIBUTE: &str = "MaximumMessageSize";
pub const MIN_MAXIMUM_MESSAGE_SIZE: u64 = 1_024;
pub const MAX_MAXIMUM_MESSAGE_SIZE: u64 = 1_048_576;

// =============================================================================
// The topic access policy document
// =============================================================================

/// One statement of a topic's access control policy.
///
/// The document is MODELLED rather than carried as opaque JSON for two reasons:
/// `AddPermission` and `RemovePermission` edit it statement by statement, and the
/// members of a document SNS itself produces come back in SNS's own order (`Sid`,
/// `Effect`, `Principal`, `Action`, `Resource`, `Condition`), which a
/// [`serde_json::Value`] object — sorted by key — cannot reproduce. Members this
/// emulator gives no meaning to (`NotAction`, `NotPrincipal`, …) are kept in `other`
/// so editing a caller's policy never drops part of it, and every member but `Effect`
/// is optional: `Effect` is the one member a policy statement cannot be read without.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TopicPolicyStatement {
    #[serde(rename = "Sid", skip_serializing_if = "Option::is_none", default)]
    pub sid: Option<String>,
    #[serde(rename = "Effect")]
    pub effect: String,
    #[serde(rename = "Principal", skip_serializing_if = "Option::is_none", default)]
    pub principal: Option<serde_json::Value>,
    #[serde(rename = "Action", skip_serializing_if = "Option::is_none", default)]
    pub action: Option<serde_json::Value>,
    #[serde(rename = "Resource", skip_serializing_if = "Option::is_none", default)]
    pub resource: Option<serde_json::Value>,
    #[serde(rename = "Condition", skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<serde_json::Value>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

/// A topic's access control policy: the JSON document the `Policy` attribute carries.
///
/// Top-level members this emulator gives no meaning to are kept in `other`, so editing
/// a caller's document (a grant, a revoke) rewrites it without dropping part of it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TopicPolicyDocument {
    #[serde(rename = "Version", skip_serializing_if = "Option::is_none", default)]
    pub version: Option<String>,
    #[serde(rename = "Id", skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    #[serde(rename = "Statement", deserialize_with = "statements")]
    pub statement: Vec<TopicPolicyStatement>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Read a document's `Statement`: ONE statement or a list of them, both of which the
/// policy grammar accepts. It is written back as a list, the form SNS answers with.
fn statements<'de, D>(deserializer: D) -> Result<Vec<TopicPolicyStatement>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        // Boxed: one statement is a far larger value than the vector of them, and an
        // enum is as big as its largest variant.
        One(Box<TopicPolicyStatement>),
        Many(Vec<TopicPolicyStatement>),
    }

    Ok(match <OneOrMany as serde::Deserialize>::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![*one],
        OneOrMany::Many(many) => many,
    })
}

impl TopicPolicyDocument {
    /// This document as the `Policy` attribute carries it.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The statement labelled `label`, if the document holds one. A statement's `Sid`
    /// IS the `Label` `AddPermission` gave it, which is what makes a label removable.
    pub fn position_of(&self, label: &str) -> Option<usize> {
        self.statement
            .iter()
            .position(|statement| statement.sid.as_deref() == Some(label))
    }
}

/// One value as a bare string, several as an array — the shape SNS writes a policy
/// statement's `Principal` and `Action` in.
pub fn one_or_many(values: Vec<String>) -> serde_json::Value {
    match <[String; 1]>::try_from(values) {
        Ok([only]) => serde_json::Value::String(only),
        Err(values) => serde_json::Value::Array(
            values.into_iter().map(serde_json::Value::String).collect(),
        ),
    }
}

/// The access policy of a topic whose owner has set none: the one SNS generates, which
/// allows the topic's own account every topic action.
pub fn default_topic_policy(arn: &str, owner: &str) -> TopicPolicyDocument {
    TopicPolicyDocument {
        version: Some("2008-10-17".to_string()),
        id: Some("__default_policy_ID".to_string()),
        statement: vec![TopicPolicyStatement {
            sid: Some("__default_statement_ID".to_string()),
            effect: "Allow".to_string(),
            principal: Some(serde_json::json!({ "AWS": "*" })),
            action: Some(serde_json::json!([
                "SNS:GetTopicAttributes",
                "SNS:SetTopicAttributes",
                "SNS:AddPermission",
                "SNS:RemovePermission",
                "SNS:DeleteTopic",
                "SNS:Subscribe",
                "SNS:ListSubscriptionsByTopic",
                "SNS:Publish"
            ])),
            resource: Some(serde_json::Value::String(arn.to_string())),
            condition: Some(serde_json::json!({
                "StringEquals": { "AWS:SourceOwner": owner }
            })),
            other: std::collections::BTreeMap::new(),
        }],
        other: std::collections::BTreeMap::new(),
    }
}

/// The statement `AddPermission` adds: an `Allow` of `SNS:<action>` for each named
/// action, to the root principal of each named account, on this topic alone.
///
/// An account id is written as its root user's ARN, the principal form SNS stores a
/// delegated account as.
pub fn permission_statement(
    label: &str,
    account_ids: &[String],
    action_names: &[String],
    topic_arn: &str,
) -> TopicPolicyStatement {
    TopicPolicyStatement {
        sid: Some(label.to_string()),
        effect: "Allow".to_string(),
        principal: Some(serde_json::json!({
            "AWS": one_or_many(
                account_ids
                    .iter()
                    .map(|account| format!("arn:aws:iam::{account}:root"))
                    .collect(),
            )
        })),
        action: Some(one_or_many(
            action_names
                .iter()
                .map(|action| format!("SNS:{action}"))
                .collect(),
        )),
        resource: Some(serde_json::Value::String(topic_arn.to_string())),
        condition: None,
        other: std::collections::BTreeMap::new(),
    }
}

impl TopicRecord {
    /// This topic's access control policy document: the one its owner set, or the
    /// default SNS generates for a topic with none. `None` when the stored value is not
    /// a policy document at all, which the write paths refuse to create.
    pub fn policy_document(&self) -> Option<TopicPolicyDocument> {
        match self.attributes.get(POLICY_ATTRIBUTE) {
            Some(stored) => serde_json::from_str(stored).ok(),
            None => Some(default_topic_policy(&self.arn, &self.owner)),
        }
    }

    /// The `Policy` attribute value this topic reports: what its owner set, verbatim,
    /// or the default document.
    pub fn policy_attribute(&self) -> String {
        match self.attributes.get(POLICY_ATTRIBUTE) {
            Some(stored) => stored.clone(),
            None => default_topic_policy(&self.arn, &self.owner).to_json(),
        }
    }

    /// Write `policy` back as this topic's access control policy.
    pub fn store_policy(&mut self, policy: &TopicPolicyDocument) {
        self.attributes
            .insert(POLICY_ATTRIBUTE.to_string(), policy.to_json());
    }
}

// =============================================================================
// The delivery policy document
// =============================================================================

/// A delivery policy document: how SNS retries a failed delivery to an HTTP/S
/// endpoint.
///
/// Modelled as a struct rather than carried as a [`serde_json::Value`] so the
/// document's members serialize in SNS's own order — a `Value` object is sorted by
/// key, which no SNS response is. Members this emulator gives no meaning to (a
/// throttle policy, a protocol SNS has added since) are carried in the `other` map of
/// each level, so a document the topic holds is reported WITH them rather than
/// silently shortened; they serialize after the modelled members of their level.
///
/// It lives here, beside [`TopicPolicyDocument`], because both the read that reports
/// `EffectiveDeliveryPolicy` and the write that accepts a `DeliveryPolicy` are stated
/// by it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeliveryPolicy {
    pub http: HttpDeliveryPolicy,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpDeliveryPolicy {
    pub default_healthy_retry_policy: HealthyRetryPolicy,
    pub disable_subscription_overrides: bool,
    pub default_request_policy: RequestPolicy,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthyRetryPolicy {
    pub min_delay_target: i64,
    pub max_delay_target: i64,
    pub num_retries: i64,
    pub num_max_delay_retries: i64,
    pub num_no_delay_retries: i64,
    pub num_min_delay_retries: i64,
    pub backoff_function: String,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPolicy {
    pub header_content_type: String,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl DeliveryPolicy {
    /// SNS's own delivery policy — the "system defaults" an `EffectiveDeliveryPolicy`
    /// is taken over (`GetTopicAttributesResponse$Attributes` doc). The numbers are
    /// SNS's documented defaults for a topic no caller has given a `DeliveryPolicy`:
    /// "the default delivery policy ... numRetries 3 ... minDelayTarget 20 ...
    /// maxDelayTarget 20 ... backoffFunction linear" (SNS Developer Guide, "Amazon SNS
    /// message delivery retries"), with the plain-text `defaultRequestPolicy` recorded
    /// AWS responses carry.
    pub fn system_default() -> Self {
        Self {
            http: HttpDeliveryPolicy {
                default_healthy_retry_policy: HealthyRetryPolicy {
                    min_delay_target: 20,
                    max_delay_target: 20,
                    num_retries: 3,
                    num_max_delay_retries: 0,
                    num_no_delay_retries: 0,
                    num_min_delay_retries: 0,
                    backoff_function: "linear".to_string(),
                    other: std::collections::BTreeMap::new(),
                },
                disable_subscription_overrides: false,
                default_request_policy: RequestPolicy {
                    header_content_type: "text/plain; charset=UTF-8".to_string(),
                    other: std::collections::BTreeMap::new(),
                },
                other: std::collections::BTreeMap::new(),
            },
            other: std::collections::BTreeMap::new(),
        }
    }
}

/// A delivery policy as a CALLER wrote it: every member optional, because a caller
/// sets only the ones it wants to override. Laying one of these over
/// [`DeliveryPolicy::system_default`] is what "taking system defaults into account"
/// means.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredDeliveryPolicy {
    pub http: Option<ConfiguredHttpPolicy>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredHttpPolicy {
    pub default_healthy_retry_policy: Option<ConfiguredRetryPolicy>,
    pub disable_subscription_overrides: Option<bool>,
    pub default_request_policy: Option<ConfiguredRequestPolicy>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredRetryPolicy {
    pub min_delay_target: Option<i64>,
    pub max_delay_target: Option<i64>,
    pub num_retries: Option<i64>,
    pub num_max_delay_retries: Option<i64>,
    pub num_no_delay_retries: Option<i64>,
    pub num_min_delay_retries: Option<i64>,
    pub backoff_function: Option<String>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredRequestPolicy {
    pub header_content_type: Option<String>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ConfiguredDeliveryPolicy {
    /// Overlay what the caller set onto `defaults`, member by member: a member the
    /// caller left out keeps SNS's default.
    pub fn overlay(self, defaults: &mut DeliveryPolicy) {
        defaults.other.extend(self.other);
        let Some(http) = self.http else { return };
        defaults.http.other.extend(http.other);
        if let Some(disable) = http.disable_subscription_overrides {
            defaults.http.disable_subscription_overrides = disable;
        }
        if let Some(retry) = http.default_healthy_retry_policy {
            let held = &mut defaults.http.default_healthy_retry_policy;
            if let Some(value) = retry.min_delay_target {
                held.min_delay_target = value;
            }
            if let Some(value) = retry.max_delay_target {
                held.max_delay_target = value;
            }
            if let Some(value) = retry.num_retries {
                held.num_retries = value;
            }
            if let Some(value) = retry.num_max_delay_retries {
                held.num_max_delay_retries = value;
            }
            if let Some(value) = retry.num_no_delay_retries {
                held.num_no_delay_retries = value;
            }
            if let Some(value) = retry.num_min_delay_retries {
                held.num_min_delay_retries = value;
            }
            if let Some(value) = retry.backoff_function {
                held.backoff_function = value;
            }
            held.other.extend(retry.other);
        }
        if let Some(request) = http.default_request_policy {
            if let Some(value) = request.header_content_type {
                defaults.http.default_request_policy.header_content_type = value;
            }
            defaults
                .http
                .default_request_policy
                .other
                .extend(request.other);
        }
    }
}

/// Why `value` is not a delivery policy document this emulator can lay over the system
/// defaults, or `None` when it is one.
///
/// It is the SAME parse [`effective_delivery_policy`] performs, so the write path
/// refuses exactly what the read could not honour: a document whose members are of the
/// wrong type would otherwise be stored and then be left out of the effective policy.
pub fn delivery_policy_refusal(value: &str) -> Option<String> {
    serde_json::from_str::<ConfiguredDeliveryPolicy>(value)
        .err()
        .map(|error| format!("the value must be a delivery policy document: {error}"))
}

/// The effective delivery policy of a topic: the system defaults with the topic's own
/// `DeliveryPolicy` laid over them, as the `EffectiveDeliveryPolicy` attribute carries
/// it.
pub fn effective_delivery_policy(topic: &TopicRecord) -> String {
    let mut effective = DeliveryPolicy::system_default();
    if let Some(configured) = topic
        .attributes
        .get(DELIVERY_POLICY_ATTRIBUTE)
        .and_then(|raw| serde_json::from_str::<ConfiguredDeliveryPolicy>(raw).ok())
    {
        configured.overlay(&mut effective);
    }
    serde_json::to_string(&effective).unwrap_or_default()
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

/// The subscription attribute a caller-set message filter lives under.
pub const FILTER_POLICY_ATTRIBUTE: &str = "FilterPolicy";

/// The subscription attribute that says WHICH part of a message the filter policy is
/// applied to.
pub const FILTER_POLICY_SCOPE_ATTRIBUTE: &str = "FilterPolicyScope";

/// The filtering scope a subscription with a filter policy and no explicit scope is
/// filtered under: "FilterPolicyScope – ... MessageAttributes (default) – The filter
/// is applied on the message attributes"
/// (`GetSubscriptionAttributesResponse$Attributes` doc).
pub const DEFAULT_FILTER_POLICY_SCOPE: &str = "MessageAttributes";

/// The subscription attribute that says whether SNS delivers the message body alone.
pub const RAW_MESSAGE_DELIVERY_ATTRIBUTE: &str = "RawMessageDelivery";

/// What `RawMessageDelivery` is for a subscription whose owner has not set it: SNS
/// wraps a notification in its JSON envelope unless raw delivery is asked for
/// ("RawMessageDelivery – true if raw message delivery is enabled for the
/// subscription", `GetSubscriptionAttributesResponse$Attributes` doc).
pub const RAW_MESSAGE_DELIVERY_DISABLED: &str = "false";

/// The attributes SNS COMPUTES for a subscription rather than storing: its own ARN,
/// the topic it is a subscription of, how and where it delivers, the account that owns
/// it, and its confirmation state. None of them is settable, so none is in
/// [`SUBSCRIPTION_ATTRIBUTE_NAMES`].
pub const SUBSCRIPTION_ARN_ATTRIBUTE: &str = "SubscriptionArn";
pub const PROTOCOL_ATTRIBUTE: &str = "Protocol";
pub const ENDPOINT_ATTRIBUTE: &str = "Endpoint";
pub const PENDING_CONFIRMATION_ATTRIBUTE: &str = "PendingConfirmation";
pub const CONFIRMATION_WAS_AUTHENTICATED_ATTRIBUTE: &str = "ConfirmationWasAuthenticated";

/// How a boolean-valued attribute is written on the wire: SNS carries every attribute
/// value as a string, so a flag is the word, not a JSON literal.
pub fn attribute_flag(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// The protocols a DELIVERY POLICY governs: "DeliveryPolicy – The policy that defines
/// how Amazon SNS retries failed deliveries to HTTP/S endpoints"
/// (`SetSubscriptionAttributesInput$AttributeName` doc). A subscription over any other
/// protocol has no retry policy to report, so none is reported for it.
pub const DELIVERY_POLICY_PROTOCOLS: &[&str] = &["http", "https"];

/// Whether a delivery policy governs delivery to `protocol`'s endpoints.
pub fn protocol_has_delivery_policy(protocol: &str) -> bool {
    DELIVERY_POLICY_PROTOCOLS.contains(&protocol)
}

// =============================================================================
// What a subscription attribute may be SET to
// =============================================================================

/// The subscription attribute a dead-letter queue is named under: "RedrivePolicy –
/// When specified, sends undeliverable messages to the specified Amazon SQS
/// dead-letter queue" (`SetSubscriptionAttributesInput$AttributeName` doc).
pub const REDRIVE_POLICY_ATTRIBUTE: &str = "RedrivePolicy";

/// The subscription attribute a message replay is asked for under
/// (`SubscribeInput$Attributes` doc).
pub const REPLAY_POLICY_ATTRIBUTE: &str = "ReplayPolicy";

/// The subscription attribute the role a Firehose delivery is written under lives
/// under: "SubscriptionRoleArn – The ARN of the IAM role that has … Permission to write
/// to the Firehose delivery stream" (`SetSubscriptionAttributesInput$AttributeName`
/// doc).
pub const SUBSCRIPTION_ROLE_ARN_ATTRIBUTE: &str = "SubscriptionRoleArn";

/// The member of a redrive policy that names the dead-letter queue.
pub const DEAD_LETTER_TARGET_ARN: &str = "deadLetterTargetArn";

/// What `RawMessageDelivery` is when raw delivery is asked for — the counterpart of
/// [`RAW_MESSAGE_DELIVERY_DISABLED`], and the only other value the flag takes.
pub const RAW_MESSAGE_DELIVERY_ENABLED: &str = "true";

/// The filtering scopes SNS defines: "FilterPolicyScope – This attribute lets you
/// choose the filtering scope by using one of the following string value types:
/// MessageAttributes (default) … MessageBody"
/// (`SetSubscriptionAttributesInput$AttributeName` doc).
pub const FILTER_POLICY_SCOPES: &[&str] = &[DEFAULT_FILTER_POLICY_SCOPE, "MessageBody"];

/// The protocols raw message delivery may be ENABLED for: "RawMessageDelivery – When
/// set to true, enables raw message delivery to Amazon SQS or HTTP/S endpoints"
/// (`SetSubscriptionAttributesInput$AttributeName` doc). A subscription over any other
/// protocol always carries SNS's JSON envelope, so there is no raw delivery to switch
/// on for it.
pub const RAW_MESSAGE_DELIVERY_PROTOCOLS: &[&str] = &["sqs", "http", "https"];

/// The protocols a `SubscriptionRoleArn` applies to: "The following attribute applies
/// only to Amazon Data Firehose delivery stream subscriptions: SubscriptionRoleArn"
/// (`SetSubscriptionAttributesInput$AttributeName` doc).
pub const SUBSCRIPTION_ROLE_ARN_PROTOCOLS: &[&str] = &["firehose"];

/// The members of `value` when it is a JSON OBJECT, else `None` — the form every policy
/// document a subscription attribute carries has.
fn json_object(value: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// The sentence SNS refuses a `FilterPolicy` it could not parse with: the attribute
/// and the parse that failed. (The service appends its parser's own location detail,
/// which is an implementation diagnostic of that parser rather than a rule.)
pub const FILTER_POLICY_PARSE_REFUSAL: &str =
    "Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON.";

/// The sentence SNS refuses a NESTED filter policy scoped to the message attributes
/// with: a message attribute is a flat name and value, so only a policy scoped to the
/// message BODY matches a property inside a property.
pub const NESTED_FILTER_POLICY_REFUSAL: &str =
    "Invalid parameter: Filter policy scope MessageAttributes does not support nested \
     filter policy";

/// Whether this filter policy NESTS: a member whose value is an object of further
/// property names, rather than the list of values a property is matched against.
fn filter_policy_is_nested(policy: &str) -> bool {
    json_object(policy).is_some_and(|members| members.values().any(serde_json::Value::is_object))
}

/// Why the subscription attributes `attributes` would leave a record SNS cannot match
/// messages against, or `None` when they would not.
///
/// The `FilterPolicy` and the `FilterPolicyScope` constrain EACH OTHER, so neither can
/// be judged on its own: "MessageAttributes (default) – The filter is applied on the
/// message attributes" (`SetSubscriptionAttributesInput$AttributeName` doc), and a
/// message attribute is a flat name and value — a policy that matches a property
/// INSIDE a property is only meaningful against the message body. The pair is
/// therefore judged on the map the write WOULD leave, whichever of the two members the
/// request carried.
pub fn filter_policy_pair_refusal(attributes: &HashMap<String, String>) -> Option<String> {
    let policy = attributes.get(FILTER_POLICY_ATTRIBUTE)?;
    let scope = attributes
        .get(FILTER_POLICY_SCOPE_ATTRIBUTE)
        .map_or(DEFAULT_FILTER_POLICY_SCOPE, String::as_str);
    (scope == DEFAULT_FILTER_POLICY_SCOPE && filter_policy_is_nested(policy))
        .then(|| NESTED_FILTER_POLICY_REFUSAL.to_string())
}

/// The sentence SNS refuses a `FilterPolicyScope` outside its value set with: the
/// attribute, the value the caller sent in brackets, and the two values it accepts.
fn filter_policy_scope_refusal(value: &str) -> String {
    format!(
        "Invalid parameter: {FILTER_POLICY_SCOPE_ATTRIBUTE}: Invalid value [{value}]. \
         Please use either MessageBody or MessageAttributes"
    )
}

/// The sentence SNS refuses a `RedrivePolicy` that names no dead-letter target with:
/// the required member of the document is the value that is missing.
pub const REDRIVE_POLICY_MISSING_VALUE_REFUSAL: &str =
    "Invalid parameter: RedrivePolicy: missing required value";

/// The sentence SNS refuses a `RedrivePolicy` whose dead-letter target is not a queue
/// with: "sends undeliverable messages to the specified Amazon SQS dead-letter queue"
/// (`SetSubscriptionAttributesInput$AttributeName` doc), so only an SQS queue can be
/// one.
pub const REDRIVE_POLICY_NOT_A_QUEUE_REFUSAL: &str =
    "Invalid parameter: RedrivePolicy: deadLetterTargetArn must be a SQS queue";

/// The message a write of the subscription attribute `name` to `value` is refused
/// with, or `None` when the value is one the attribute accepts — including every
/// `name` SNS documents no constraint on the value of.
///
/// The message is the whole sentence SNS states for that condition, not a clause a
/// caller wraps: the same condition reads the same whichever operation carried the
/// write. It never echoes a policy document back, and names the value only where the
/// sentence SNS states itself names it.
///
/// This is ARGUMENT validation — it judges the value alone — so it runs before any
/// record is looked up, and it is SHARED by every operation that writes a subscription
/// attribute: a value `Subscribe` accepts cannot be one `SetSubscriptionAttributes`
/// refuses.
pub fn subscription_attribute_refusal(name: &str, value: &str) -> Option<String> {
    // The clause form the attributes SNS states no fixed sentence for keep: the
    // attribute and the constraint its value failed.
    let attribute_reason = |reason: String| format!("Invalid parameter: Attributes Reason: {reason}");
    match name {
        // A flag, carried as a word because SNS carries every attribute value as a
        // string (see [`attribute_flag`]).
        RAW_MESSAGE_DELIVERY_ATTRIBUTE => (value != RAW_MESSAGE_DELIVERY_ENABLED
            && value != RAW_MESSAGE_DELIVERY_DISABLED)
            .then(|| {
                attribute_reason(format!(
                    "{name} - the value must be {RAW_MESSAGE_DELIVERY_ENABLED} or \
                     {RAW_MESSAGE_DELIVERY_DISABLED}"
                ))
            }),
        FILTER_POLICY_SCOPE_ATTRIBUTE => {
            (!FILTER_POLICY_SCOPES.contains(&value)).then(|| filter_policy_scope_refusal(value))
        }
        // "The simple JSON object that lets your subscriber receive only a subset of
        // messages" (`SetSubscriptionAttributesInput$AttributeName` doc): the policy is
        // matched property by property, so it has to be an object to match anything.
        FILTER_POLICY_ATTRIBUTE => match serde_json::from_str::<serde_json::Value>(value) {
            // What SNS reports when its parse of the document failed, naming the
            // attribute and the parse rather than the document the caller sent.
            Err(_) => Some(FILTER_POLICY_PARSE_REFUSAL.to_string()),
            Ok(serde_json::Value::Object(_)) => None,
            Ok(_) => Some(attribute_reason(format!(
                "{name} - the value must be a filter policy document: a JSON object of \
                 the message properties to match"
            ))),
        },
        // The SAME parse the effective policy is computed with, so a document the read
        // path could not honour is never stored.
        DELIVERY_POLICY_ATTRIBUTE => delivery_policy_refusal(value)
            .map(|reason| attribute_reason(format!("{name} - {reason}"))),
        REDRIVE_POLICY_ATTRIBUTE => match json_object(value) {
            // A value that is not a policy document at all carries no dead-letter
            // target either, which is the member the document is refused for.
            None => Some(REDRIVE_POLICY_MISSING_VALUE_REFUSAL.to_string()),
            Some(members) => match members
                .get(DEAD_LETTER_TARGET_ARN)
                .and_then(serde_json::Value::as_str)
            {
                None => Some(REDRIVE_POLICY_MISSING_VALUE_REFUSAL.to_string()),
                // "sends undeliverable messages to the specified Amazon SQS dead-letter
                // queue" — a redrive policy that names no SQS queue redrives nowhere.
                Some(arn) if arn_resource(arn, "sqs").is_none() => {
                    Some(REDRIVE_POLICY_NOT_A_QUEUE_REFUSAL.to_string())
                }
                Some(_) => None,
            },
        },
        REPLAY_POLICY_ATTRIBUTE => json_object(value).is_none().then(|| {
            attribute_reason(format!(
                "{name} - the value must be a replay policy document: a JSON object"
            ))
        }),
        // "The ARN of the IAM role that has …" — only the ARN's shape is judged here;
        // whether that role exists is not this service's record to check.
        SUBSCRIPTION_ROLE_ARN_ATTRIBUTE => arn_shape_reason(value)
            .map(|reason| attribute_reason(format!("{name} - {reason}"))),
        _ => None,
    }
}

/// The WHOLE `InvalidParameter` message SNS refuses a subscription attribute with when
/// the subscription's delivery protocol is not one SNS documents the attribute for, or
/// `None` when the attribute applies to that protocol.
///
/// SNS documents some subscription attributes for the delivery protocols they have a
/// meaning for alone, and refuses one set on a subscription of any other protocol:
/// storing it would leave the subscription reporting a setting nothing it does is
/// governed by. Shared by every operation that writes a subscription attribute, so the
/// scope does not depend on which call wrote it.
///
/// The message is the complete sentence SNS states for this condition, directly after
/// `Invalid parameter: ` — this refusal is about the SUBSCRIPTION the attribute was set
/// on rather than about the value of one request member, so it is not nested in the
/// `<member> Reason: ` frame this crate frames a member-level constraint failure in.
/// Recorded evidence: enabling raw message delivery on a `lambda` subscription answers
/// `400 InvalidParameter` with exactly `Invalid parameter: Delivery protocol [lambda]
/// does not support raw message delivery.`
pub fn subscription_attribute_protocol_refusal(
    name: &str,
    value: &str,
    protocol: &str,
) -> Option<String> {
    let (scoped, feature): (&[&str], &str) = match name {
        // Raw delivery is a property of the envelope SNS puts a notification in, so
        // only switching it ON is scoped: every protocol carries the JSON envelope by
        // default, which is what the flag's other value asks for.
        RAW_MESSAGE_DELIVERY_ATTRIBUTE if value == RAW_MESSAGE_DELIVERY_ENABLED => {
            (RAW_MESSAGE_DELIVERY_PROTOCOLS, "raw message delivery")
        }
        DELIVERY_POLICY_ATTRIBUTE => (DELIVERY_POLICY_PROTOCOLS, "a delivery policy"),
        SUBSCRIPTION_ROLE_ARN_ATTRIBUTE => {
            (SUBSCRIPTION_ROLE_ARN_PROTOCOLS, "a subscription role")
        }
        _ => return None,
    };
    (!scoped.contains(&protocol)).then(|| {
        format!(
            "Invalid parameter: Delivery protocol [{protocol}] does not support \
             {feature}."
        )
    })
}

/// The effective delivery policy of one SUBSCRIPTION: the system defaults, the
/// `DeliveryPolicy` of the topic laid over them, and then the subscription's own
/// `DeliveryPolicy` — "the effective delivery policy that takes into account the topic
/// delivery policy and account system defaults"
/// (`GetSubscriptionAttributesResponse$Attributes` doc).
///
/// A topic whose policy sets `disableSubscriptionOverrides` keeps its own policy: that
/// is what the member disables, so the subscription's policy is not laid over it.
pub fn effective_subscription_delivery_policy(
    topic: Option<&TopicRecord>,
    subscription: &SubscriptionRecord,
) -> String {
    let mut effective = DeliveryPolicy::system_default();
    if let Some(configured) = topic
        .and_then(|topic| topic.attributes.get(DELIVERY_POLICY_ATTRIBUTE))
        .and_then(|raw| serde_json::from_str::<ConfiguredDeliveryPolicy>(raw).ok())
    {
        configured.overlay(&mut effective);
    }
    if !effective.http.disable_subscription_overrides {
        if let Some(configured) = subscription
            .attributes
            .get(DELIVERY_POLICY_ATTRIBUTE)
            .and_then(|raw| serde_json::from_str::<ConfiguredDeliveryPolicy>(raw).ok())
        {
            configured.overlay(&mut effective);
        }
    }
    serde_json::to_string(&effective).unwrap_or_default()
}

/// The fields a SUBSCRIPTION's ARN holds: an ARN whose resource is a topic name
/// followed by the subscription's own id segment
/// (`arn:aws:sns:<region>:<account>:<topic>:<id>`, as `Subscribe` mints it).
pub const SUBSCRIPTION_ARN_FIELDS: usize = ARN_ELEMENTS + 1;

/// Why `arn` is not a subscription ARN, or `None` when it is one.
///
/// The reason names the member's constraint and is built from the request, the way
/// SNS's own argument validation reports one ("An ARN must have at least 6 elements,
/// not 1"); nothing of the recorded wire text is required beyond that. The generic
/// ARN shape comes from [`arn_shape_reason`], the one place this crate judges it, so
/// every ARN-typed member of this service states that part alike; what is added here
/// is what makes an ARN a SUBSCRIPTION's.
///
/// Shared, because every operation that takes a `SubscriptionArn` asks the same
/// question of it — a subscription ARN one call accepts cannot be one another refuses.
pub fn malformed_subscription_arn(arn: &str) -> Option<String> {
    if let Some(reason) = arn_shape_reason(arn) {
        return Some(reason);
    }
    let fields: Vec<&str> = arn.split(':').collect();
    let well_formed = fields.len() == SUBSCRIPTION_ARN_FIELDS
        && fields[2] == "sns"
        && fields[1..].iter().all(|field| !field.is_empty())
        && topic_name_is_valid(fields[5]);
    (!well_formed).then(|| {
        format!(
            "A subscription ARN names a topic and the subscription's id, so it has \
             {SUBSCRIPTION_ARN_FIELDS} elements: {arn}"
        )
    })
}

// =============================================================================
// The per-message rules every publishing operation shares
// =============================================================================

/// The only value `MessageStructure` accepts ("Valid value: json" —
/// `PublishInput$MessageStructure` doc).
pub const JSON_MESSAGE_STRUCTURE: &str = "json";

/// The key a JSON-structured message must carry: "contain at least a top-level JSON
/// key of \"default\" with a value that is a string" (`PublishInput$MessageStructure`
/// doc).
pub const DEFAULT_JSON_KEY: &str = "default";

/// "By default, a message can be at most 256 KiB in size (262,144 bytes)"
/// (`PublishInput$Message` doc); a topic's [`MAXIMUM_MESSAGE_SIZE_ATTRIBUTE`]
/// attribute overrides it.
pub const DEFAULT_MAXIMUM_MESSAGE_SIZE: usize = 262_144;

/// The FIFO attribute that makes SNS derive the `MessageDeduplicationId` from the
/// message body instead of requiring the caller to supply one
/// (`GetTopicAttributesResponse$Attributes` doc).
pub const CONTENT_BASED_DEDUPLICATION_ATTRIBUTE: &str = "ContentBasedDeduplication";

/// The longest `Subject` SNS accepts, in CHARACTERS: "Subjects must be UTF-8 text
/// with no line breaks or control characters, and less than 100 characters long"
/// (`PublishInput$Subject` doc). The limit counts the subject's CHARACTERS, not its
/// UTF-8 bytes, and the recorded service accepts a subject of exactly this many and
/// refuses the next one — so it is an inclusive bound here.
pub const MAX_SUBJECT_CHARS: usize = 100;

/// "The MessageGroupId can contain up to 128 alphanumeric characters (a-z, A-Z, 0-9)
/// and punctuation" — the same rule the doc states for `MessageDeduplicationId`
/// ("When provided, the same validation rules apply").
pub const MAX_ORDERING_ID_CHARS: usize = 128;

/// Whether an ordering id (`MessageGroupId`, `MessageDeduplicationId`) satisfies the
/// rule above: 1 to 128 characters of ASCII alphanumerics and ASCII punctuation,
/// which is exactly the set the doc lists.
fn ordering_id_is_valid(value: &str) -> bool {
    let length = value.chars().count();
    length > 0
        && length <= MAX_ORDERING_ID_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c.is_ascii_punctuation())
}

/// The Unicode LINE SEPARATOR and PARAGRAPH SEPARATOR: the two line breaks that are
/// not control characters, so a rule that reads "no line breaks or control
/// characters" has to name them beside the control category.
const LINE_SEPARATORS: [char; 2] = ['\u{2028}', '\u{2029}'];

/// Whether `subject` satisfies the rule SNS documents for it: at most
/// [`MAX_SUBJECT_CHARS`] characters (CHARACTERS, not UTF-8 bytes — any script is
/// accepted), none of them a line break or a control character.
fn subject_is_valid(subject: &str) -> bool {
    subject.chars().count() <= MAX_SUBJECT_CHARS
        && !subject
            .chars()
            .any(|c| c.is_control() || LINE_SEPARATORS.contains(&c))
}

/// The sentence SNS refuses a publish that names no destination with: the members one
/// of which a publish has to carry, and the reason it is refused — "If you don't
/// specify a value for the TopicArn parameter, you must specify a value for the
/// PhoneNumber or TargetArn parameters" (`PublishInput$TopicArn` doc).
pub const NO_DESTINATION_REFUSAL: &str =
    "Invalid parameter: TopicArn or TargetArn Reason: no value for required parameter";

/// The sentence SNS refuses a publish that names MORE than one destination with: a
/// message goes to one destination, so the destination members cannot be combined.
pub const DESTINATIONS_NOT_EXCLUSIVE_REFUSAL: &str =
    "Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn cannot be \
     specified at the same time.";

/// The sentence SNS refuses a publish with no message body with.
///
/// SNS answers one FIXED sentence per refused condition — the parameter as the
/// service names it, and the value or the bound only where that sentence itself
/// carries them — so a client that reads `Error.Message` sees the same text for the
/// same condition on every call. Recorded evidence: `Publish` with `Message=""`
/// answers `400 InvalidParameter` with exactly this.
pub const EMPTY_MESSAGE_REFUSAL: &str = "Invalid parameter: Empty message";

/// The sentence SNS refuses a body past the topic's ceiling with: the condition, with
/// neither the body's size nor the ceiling in it.
pub const MESSAGE_TOO_LONG_REFUSAL: &str = "Invalid parameter: Message too long";

/// The sentence SNS refuses an unacceptable `Subject` with: `Invalid parameter:` and
/// the parameter, whichever of the member's rules the value broke (its length or a
/// character it may not carry).
pub const INVALID_SUBJECT_REFUSAL: &str = "Invalid parameter: Subject";

/// The sentence SNS refuses a `MessageDeduplicationId` on a STANDARD topic with:
/// deduplication is a FIFO property, so the parameter is not one this topic type
/// takes.
pub const DEDUPLICATION_ID_WRONG_TOPIC_TYPE_REFUSAL: &str =
    "Invalid parameter: MessageDeduplicationId Reason: The request includes \
     MessageDeduplicationId parameter that is not valid for this topic type";

/// The sentence SNS refuses an ordering id (`MessageGroupId`,
/// `MessageDeduplicationId`) that is not 1 to [`MAX_ORDERING_ID_CHARS`] alphanumeric
/// or punctuation characters with: it names the parameter and states the rule, and
/// never the value the caller sent.
pub fn ordering_id_refusal(member: &str) -> String {
    format!(
        "Invalid parameter: The {member} parameter can only include alphanumeric and \
         punctuation characters. 1 to {MAX_ORDERING_ID_CHARS} in length"
    )
}

/// Check a `MessageStructure: json` body: a JSON object with a string `default`
/// member, every other member a string too. Returns the reason it is not.
fn json_message_error(message: &str) -> Option<&'static str> {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(message) else {
        return Some("message must be a syntactically valid JSON object");
    };
    let Some(members) = parsed.as_object() else {
        return Some("message must be a syntactically valid JSON object");
    };
    if !members.get(DEFAULT_JSON_KEY).is_some_and(|v| v.is_string()) {
        return Some("message must carry a top-level \"default\" key whose value is a string");
    }
    if members.values().any(|value| !value.is_string()) {
        return Some("every top-level value of the message must be a string");
    }
    None
}

/// One message as a caller supplied it: the members `PublishInput` and
/// `PublishBatchRequestEntry` have in common, which is every member either shape
/// carries a documented per-message rule for. Borrowed, so judging a message costs no
/// clone.
#[derive(Debug, Clone, Copy)]
pub struct MessageRequest<'a> {
    pub message: &'a str,
    pub subject: Option<&'a str>,
    pub message_structure: Option<&'a str>,
    pub message_group_id: Option<&'a str>,
    pub message_deduplication_id: Option<&'a str>,
}

/// Why SNS refuses this message on its own members alone, or `None` when every
/// documented per-member rule holds.
///
/// This is ARGUMENT validation — it judges the message without looking at any record —
/// so it runs before the topic is read, and it is SHARED by every operation that
/// publishes a message: a message `Publish` accepts cannot be one `PublishBatch`
/// refuses. The reason names the member and the constraint it failed, built from the
/// request.
pub fn message_refusal(request: MessageRequest<'_>) -> Option<String> {
    if let Some(structure) = request.message_structure {
        if structure != JSON_MESSAGE_STRUCTURE {
            return Some(format!(
                "Invalid parameter: MessageStructure Reason: the only valid value is \
                 {JSON_MESSAGE_STRUCTURE}"
            ));
        }
        if let Some(reason) = json_message_error(request.message) {
            return Some(format!("Invalid parameter: Message - {reason}"));
        }
    }

    if request.message.is_empty() {
        return Some(EMPTY_MESSAGE_REFUSAL.to_string());
    }

    if request.subject.is_some_and(|subject| !subject_is_valid(subject)) {
        return Some(INVALID_SUBJECT_REFUSAL.to_string());
    }

    // The ordering ids follow one rule, whatever the topic type, and it is also what
    // keeps a retained delivery's own size bounded.
    for (member, value) in [
        ("MessageGroupId", request.message_group_id),
        ("MessageDeduplicationId", request.message_deduplication_id),
    ] {
        if value.is_some_and(|value| !ordering_id_is_valid(value)) {
            return Some(ordering_id_refusal(member));
        }
    }

    None
}

/// The ceiling, in bytes, on what `topic` accepts: [`DEFAULT_MAXIMUM_MESSAGE_SIZE`],
/// which the topic's [`MAXIMUM_MESSAGE_SIZE_ATTRIBUTE`] may only LOWER.
///
/// The attribute reaches 1 MiB at AWS, but a caller-raised ceiling would let one
/// request buy more memory per subscription than the documented default, so the
/// emulator keeps the default as its hard limit.
pub fn max_message_size(topic: &TopicRecord) -> usize {
    topic
        .attributes
        .get(MAXIMUM_MESSAGE_SIZE_ATTRIBUTE)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(DEFAULT_MAXIMUM_MESSAGE_SIZE, |configured| {
            configured.min(DEFAULT_MAXIMUM_MESSAGE_SIZE)
        })
}

/// What `topic` makes of this message: the `MessageDeduplicationId` the stored
/// delivery carries (`None` for a standard topic, which SNS does not sequence or
/// deduplicate), or why this topic refuses the message.
///
/// These are the rules that need the TOPIC as well as the message — its size ceiling
/// and the FIFO ordering parameters — so they run after the topic record is read, and
/// they too are shared by every publishing operation.
pub fn topic_message_refusal(
    topic: &TopicRecord,
    request: MessageRequest<'_>,
) -> Result<Option<String>, String> {
    let ceiling = max_message_size(topic);
    if request.message.len() > ceiling {
        return Err(MESSAGE_TOO_LONG_REFUSAL.to_string());
    }

    if !topic.is_fifo {
        if request.message_deduplication_id.is_some() {
            return Err(DEDUPLICATION_ID_WRONG_TOPIC_TYPE_REFUSAL.to_string());
        }
        // A MessageGroupId is NOT refused here: for a standard topic "the
        // MessageGroupId is optional and is forwarded only to Amazon SQS standard
        // subscriptions to activate fair queues" (`PublishInput$MessageGroupId` doc),
        // so it is stored with the delivery rather than rejected.
        return Ok(None);
    }

    if request.message_group_id.is_none() {
        // The sentence SNS states for the ordering parameter a FIFO publish must
        // carry.
        return Err(
            "Invalid parameter: The MessageGroupId parameter is required for FIFO topics"
                .to_string(),
        );
    }

    let content_based = topic
        .attributes
        .get(CONTENT_BASED_DEDUPLICATION_ATTRIBUTE)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    match request.message_deduplication_id {
        Some(id) => Ok(Some(id.to_string())),
        // "Amazon SNS uses a SHA-256 hash to generate the MessageDeduplicationId using
        // the body of the message (but not the attributes of the message)"
        // (`GetTopicAttributesResponse$Attributes` doc).
        None if content_based => Ok(Some(hex::encode(Sha256::digest(
            request.message.as_bytes(),
        )))),
        None => Err(
            "Invalid parameter: MessageDeduplicationId - required for a FIFO topic whose \
             ContentBasedDeduplication is not enabled"
                .to_string(),
        ),
    }
}

/// The bytes one message's attribute map holds: every name, type and value, which is
/// what SNS counts beside the body when it measures a message against a topic's
/// `MaximumMessageSize` ("including each message's body and attributes" — the
/// `PublishBatch` doc).
pub fn message_attribute_bytes(attributes: &HashMap<String, MessageAttributeValue>) -> usize {
    attributes
        .iter()
        .map(|(name, value)| {
            name.len()
                + value.data_type.len()
                + value.string_value.as_ref().map_or(0, String::len)
                + value.binary_value.as_ref().map_or(0, Vec::len)
        })
        .sum()
}

/// One message `Publish` FANNED OUT to one subscription.
///
/// What a publish handed to the subscription, not what the subscription's endpoint received:
/// the record is written for every confirmed subscription of the topic, including the
/// protocols this emulator has no delivery path for and the ones whose endpoint refuses or
/// never grants the delivery. What an endpoint really holds is observed on the endpoint.
///
/// The message body and its attributes are held behind an `Arc`, because one publish is
/// recorded against EVERY subscription of the topic: sharing them keeps one publish's cost
/// one body and N pointers rather than N copies of a body that may be up to the topic's
/// `MaximumMessageSize`, which is what bounds the (subscriptions x body bytes) product of a
/// single request. The body is immutable once published, so there is nothing to clone it for.
#[derive(Debug, Clone)]
pub struct PublishedMessage {
    pub message_id: String,
    pub topic_arn: String,
    pub message: ::std::sync::Arc<str>,
    pub subject: Option<String>,
    pub message_structure: Option<String>,
    pub message_attributes: ::std::sync::Arc<HashMap<String, MessageAttributeValue>>,
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
    ///
    /// The shared body is counted in FULL for every record that holds a pointer to it, which
    /// over-counts what is actually held: the ceiling is a bound, so counting the sharing
    /// would weaken it, and a bound that errs high evicts sooner rather than later.
    pub fn retained_bytes(&self) -> usize {
        /// The fixed-size parts of the record (the timestamp, the `Option`/`String`
        /// headers, the map's own allocation) that no caller varies.
        const FIXED_OVERHEAD: usize = 256;

        let attributes = message_attribute_bytes(&self.message_attributes);
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

// =============================================================================
// The SMS sandbox
// =============================================================================

/// The verification status a destination phone number in the SMS sandbox carries
/// until its one-time password has been verified.
///
/// One of the two values the modelled `SMSSandboxPhoneNumberVerificationStatus` enum
/// accepts; the other is [`SANDBOX_NUMBER_VERIFIED`]. Both are spelled exactly as
/// [`crate::ops::types::S_M_S_SANDBOX_PHONE_NUMBER_VERIFICATION_STATUS_VALUES`]
/// lists them, because that is what the enum lets a listing report.
pub const SANDBOX_NUMBER_PENDING: &str = "Pending";

/// The verification status a destination phone number in the SMS sandbox carries
/// once its one-time password has been verified — the other value of the enum named
/// on [`SANDBOX_NUMBER_PENDING`].
pub const SANDBOX_NUMBER_VERIFIED: &str = "Verified";

/// The name a refusal gives a sandbox destination phone number record.
pub const SANDBOX_NUMBER_RECORD: &str = "SMS sandbox destination phone numbers";

/// One destination phone number in the calling account's SMS sandbox, as
/// `CreateSMSSandboxPhoneNumber` added it.
#[derive(Debug, Clone)]
pub struct SandboxPhoneNumberRecord {
    /// The destination phone number, as the request that added it spelled it. The
    /// model marks this member `@sensitive`, so it belongs in the modelled listing
    /// and in no error string.
    pub phone_number: String,
    /// [`SANDBOX_NUMBER_PENDING`] or [`SANDBOX_NUMBER_VERIFIED`].
    pub status: String,
    pub created_at: SystemTime,
}

// =============================================================================
// Phone numbers: the E.164 rule and the SMS opt-out list
// =============================================================================

/// The parameter name SNS states in a refusal of a phone number, whatever case the
/// operation spells the member in on the wire (`phoneNumber` on the opt-out pair,
/// `PhoneNumber` on the sandbox and publish ones).
///
/// Recorded evidence: `OptInPhoneNumber` with a value that is not an E.164 number
/// answers `400 InvalidParameter` naming `PhoneNumber`.
pub const PHONE_NUMBER_PARAMETER: &str = "PhoneNumber";

/// The most digits a phone number in E.164 format carries, country code included:
/// ITU-T Recommendation E.164 caps an international public telecommunication number
/// at fifteen digits.
pub const MAX_E164_DIGITS: usize = 15;

/// The fewest digits this service accepts after the `+` of a phone number in E.164
/// format, taken from SNS's own published constraint on a destination phone number:
/// `smithy_query(query="shape", shape="PhoneNumberString")` reports the pattern
/// `^(\+[0-9]{8,}|[0-9]{0,9})$`, whose E.164 branch takes eight digits at least.
pub const MIN_E164_DIGITS: usize = 8;

/// Whether `value` is a phone number in E.164 format: a `+`, then a country code
/// that does not begin with `0`, then digits only — between [`MIN_E164_DIGITS`] and
/// [`MAX_E164_DIGITS`] digits in all.
///
/// Every SNS operation whose phone-number member the model leaves an unconstrained
/// string documents it as "Use E.164 format", so this is the rule those members are
/// held to, and it is ARGUMENT validation — the phone-number counterpart of
/// [`arn_shape_reason`]: a value that could not be a phone number anywhere is bad
/// input, judged BEFORE the store is consulted.
///
/// Deliberately STRICTER than the SMS subscription endpoint's rule
/// (`is_phone_number` in `crate::ops::subscribe`), which accepts a national form
/// written with separators and no `+`: `Subscribe` documents its `sms` endpoint as "a
/// phone number of an SMS-enabled device", while these members say E.164 and nothing
/// else. The two floors differ for the same reason — this one is the digit count SNS
/// publishes for an E.164 destination number.
pub fn is_e164(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('+') else {
        return false;
    };
    (MIN_E164_DIGITS..=MAX_E164_DIGITS).contains(&digits.len())
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// The opening every refusal of a phone-number parameter carries: `Invalid
/// parameter:`, the parameter as SNS names it, and the `Reason:` the clauses below
/// complete — so the parameter name has ONE source for every such sentence.
fn phone_number_refusal_opening() -> String {
    format!("Invalid parameter: {PHONE_NUMBER_PARAMETER} Reason: ")
}

/// The sentence SNS refuses a phone number that is not in E.164 format with.
///
/// Fixed, whichever clause of [`is_e164`] the value broke — SNS states the parameter
/// and that its input was formatted wrongly, and does NOT reproduce the value the
/// caller sent (the model marks every phone-number member `@sensitive`, so echoing it
/// would put a sensitive value in an error string). Recorded evidence:
/// `OptInPhoneNumber` with a value that is not an E.164 number answers `400
/// InvalidParameter` with exactly this sentence.
pub fn invalid_phone_number_message() -> String {
    format!("{}input incorrectly formatted", phone_number_refusal_opening())
}

/// The message a phone-number member holding `value` is refused with, or `None` when
/// `value` is a phone number in E.164 format — the phone-number counterpart of
/// [`arn_refusal`].
pub fn phone_number_refusal(value: &str) -> Option<String> {
    (!is_e164(value)).then(invalid_phone_number_message)
}

/// How long after an opt-in the next one on the same phone number may happen: "You
/// can opt in a phone number only once every 30 days" (`OptInPhoneNumber` doc).
pub const OPT_IN_INTERVAL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The sentence an opt-in is refused with while the number's previous opt-in is
/// younger than [`OPT_IN_INTERVAL`]: the parameter and the documented interval it
/// failed, with none of the (sensitive) number echoed back.
///
/// The interval is documented; the error SHAPE is this emulator's reading of the
/// operation's modelled error set — `InvalidParameterException` is the one client
/// error it declares for a parameter that does not comply, and no recorded response
/// of a too-soon opt-in is available to fix either the shape or this wording. A
/// recording that shows otherwise corrects both here.
pub fn opt_in_too_soon_message() -> String {
    let days = OPT_IN_INTERVAL.as_secs() / (24 * 60 * 60);
    format!(
        "{}A phone number can be opted in only once every {days} days",
        phone_number_refusal_opening()
    )
}

/// What this account knows about one phone number's SMS opt-out state.
///
/// A number opts OUT out of band — the recipient replies `STOP` to the message — and
/// no SNS API performs that, so nothing a caller sends creates a record here. What a
/// caller can do is opt a number back in, which is why the record outlives the
/// opt-out it started as: [`Self::last_opt_in`] is what the
/// once-every-[`OPT_IN_INTERVAL`] rule is measured from.
#[derive(Debug, Clone)]
pub struct PhoneNumberRecord {
    /// The phone number, as the opt-out that created the record spelled it. The model
    /// marks every phone-number member `@sensitive`, so it belongs in a modelled
    /// listing — `ListPhoneNumbersOptedOut`, still a scaffold stub, is what will read
    /// it — and in no error string.
    pub phone_number: String,
    /// When the number opted out of receiving SMS messages from this account —
    /// `None` once it has been opted back in.
    pub opted_out_at: Option<SystemTime>,
    /// When `OptInPhoneNumber` last opted this number back in, or `None` when it
    /// never has been.
    pub last_opt_in: Option<SystemTime>,
}

impl PhoneNumberRecord {
    /// A number that has opted out of receiving SMS messages from this account and
    /// has not been opted back in: what `CheckIfPhoneNumberIsOptedOut` reports and
    /// what `OptInPhoneNumber` has something to do about.
    pub fn is_opted_out(&self) -> bool {
        self.opted_out_at.is_some()
    }

    /// The instant from which this number may be opted in again, or `None` when it
    /// has never been opted in and so may be opted in now.
    pub fn opt_in_available_at(&self) -> Option<SystemTime> {
        self.last_opt_in.map(|at| at + OPT_IN_INTERVAL)
    }
}

// =============================================================================
// Mobile push: platform applications and their device endpoints
// =============================================================================

/// The prefix the resource part of a platform application's ARN carries:
/// `arn:aws:sns:<region>:<account>:app/<platform>/<name>`, from the
/// `CreatePlatformApplicationResponse$PlatformApplicationArn` doc.
pub const PLATFORM_APPLICATION_PREFIX: &str = "app/";

/// The prefix the resource part of a platform ENDPOINT's ARN carries:
/// `arn:aws:sns:<region>:<account>:endpoint/<platform>/<application>/<id>`, from the
/// `CreatePlatformEndpointResponse$EndpointArn` doc.
///
/// Shared, because "is this string the ARN of a platform endpoint?" is asked both by
/// `Subscribe`'s `application` protocol and by the mobile-push operations that act on
/// an endpoint directly.
pub const PLATFORM_ENDPOINT_PREFIX: &str = "endpoint/";

/// A platform application's ARN, for the mobile push platform `platform` (`ADM`,
/// `APNS`, `APNS_SANDBOX`, `GCM`) and the application's `name`.
pub fn platform_application_arn(region: &str, platform: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        "sns",
        region,
        &format!("{PLATFORM_APPLICATION_PREFIX}{platform}/{name}"),
    )
}

/// A device endpoint's ARN: the endpoint resource of the platform application named by
/// `platform` and `application`, plus the id that tells one of its devices from
/// another.
pub fn platform_endpoint_arn(
    region: &str,
    platform: &str,
    application: &str,
    endpoint_id: &str,
) -> String {
    ::emulator_server::emulator_arn(
        "sns",
        region,
        &format!("{PLATFORM_ENDPOINT_PREFIX}{platform}/{application}/{endpoint_id}"),
    )
}

/// The attribute names SNS defines for a device endpoint, from the
/// `GetEndpointAttributesResponse$Attributes` doc: "CustomUserData – arbitrary user
/// data to associate with the endpoint … Enabled – flag that enables/disables delivery
/// to the endpoint … Token – device token, also referred to as a registration id".
pub const ENDPOINT_ATTRIBUTE_NAMES: &[&str] = &[
    ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE,
    ENDPOINT_ENABLED_ATTRIBUTE,
    ENDPOINT_TOKEN_ATTRIBUTE,
];

/// The arbitrary user data an endpoint's owner associated with it.
pub const ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE: &str = "CustomUserData";

/// Whether SNS delivers a notification to the endpoint. SNS sets it to `false` when
/// the notification service reports the endpoint invalid, so an endpoint nobody has
/// disabled is enabled.
pub const ENDPOINT_ENABLED_ATTRIBUTE: &str = "Enabled";

/// The device token the notification service issued for the endpoint's device: the
/// value [`PlatformEndpointRecord::token`] holds.
pub const ENDPOINT_TOKEN_ATTRIBUTE: &str = "Token";

/// One device endpoint of a platform application: the device a mobile push
/// notification is addressed to, as a registration call recorded it.
#[derive(Debug, Clone)]
pub struct PlatformEndpointRecord {
    /// The endpoint's own ARN, in the form [`platform_endpoint_arn`] builds — also
    /// this record's key in
    /// [`ComAmazonawsSnsAmazonsimplenotificationserviceState::platform_endpoints`].
    pub arn: String,
    /// The ARN of the platform application the endpoint belongs to, in the form
    /// [`platform_application_arn`] builds.
    pub platform_application_arn: String,
    /// The device token the notification service issued for the device, which is what
    /// identifies the device to that service.
    pub token: String,
    /// The endpoint's attributes, as `GetEndpointAttributes` reports them. Bounded by
    /// [`MAX_ATTRIBUTE_BYTES_PER_RECORD`], like every other caller-sized attribute map
    /// here.
    pub attributes: HashMap<String, String>,
    pub created_at: SystemTime,
}

/// Per-service state for com-amazonaws-sns-amazonsimplenotificationservice. A handler reaches it through [`service_state`].
///
/// Every CALLER-DRIVEN store here is therefore BOUNDED: `topics`,
/// `subscriptions` and `sandbox_phone_numbers` at [`MAX_RECORDS_PER_STORE`] records
/// each (a request that would cross it is refused with a modelled error), and
/// `delivered` at
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
    /// The calling account's SMS sandbox destination phone numbers, keyed by the
    /// phone number itself: the sandbox holds one record per destination number. At
    /// most [`MAX_RECORDS_PER_STORE`].
    pub sandbox_phone_numbers: HashMap<String, SandboxPhoneNumberRecord>,
    /// The SMS opt-out state this account knows per phone number, keyed by the number
    /// itself. NOT caller-grown, so it needs no ceiling of its own: a record appears
    /// when a number opts OUT, which the recipient does out of band and no SNS API
    /// performs — `OptInPhoneNumber` only clears and annotates a record already here,
    /// and `CheckIfPhoneNumberIsOptedOut` only reads one.
    pub phone_numbers: HashMap<String, PhoneNumberRecord>,
    /// The device endpoints of this account's platform applications, keyed by the
    /// endpoint's own ARN: what `DeleteEndpoint` removes one of. A caller-grown store,
    /// so the registration that adds a record is held to [`MAX_RECORDS_PER_STORE`]
    /// records like every other store here.
    pub platform_endpoints: HashMap<String, PlatformEndpointRecord>,
    /// Delivered messages by subscription ARN, oldest first. Bounded: write through
    /// [`Self::record_delivery`], never by hand.
    ///
    /// ONE published message is held ONCE however many subscriptions received it: the
    /// record is SHARED, so a publish to a topic with many subscriptions does not
    /// allocate its body per subscription (the same sharing the queue deliveries of
    /// `crate::ops::publish` use for the envelope they hand out).
    pub delivered: HashMap<String, Vec<::std::sync::Arc<PublishedMessage>>>,
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

    /// Every destination phone number in the SMS sandbox, oldest first (the number
    /// itself breaking a tie), so a listing and its pagination token are stable
    /// across calls — see [`Self::subscriptions_of`].
    pub fn sandbox_phone_numbers_in_order(&self) -> Vec<&SandboxPhoneNumberRecord> {
        let mut found: Vec<&SandboxPhoneNumberRecord> =
            self.sandbox_phone_numbers.values().collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.phone_number.cmp(&b.phone_number))
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
        self.id_counter = self.id_counter.saturating_add(1);
        uuid_v4_shaped(self.id_counter)
    }

    /// Record one delivery of `message` to `subscription_arn`, then bring the log back
    /// within [`MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION`] and
    /// [`MAX_RETAINED_DELIVERY_BYTES`] by dropping the oldest deliveries. This is the
    /// ONLY way to write `delivered`: writing the map directly loses the bound.
    ///
    /// The message arrives SHARED, so recording one publish to N subscriptions costs
    /// N handles and one body. The retained-byte count still charges every recording
    /// its whole [`PublishedMessage::retained_bytes`], which over-states what a shared
    /// body costs and therefore evicts sooner than the ceiling demands — the bound
    /// stays an upper bound on the bytes one caller can make the store hold.
    pub fn record_delivery(
        &mut self,
        subscription_arn: &str,
        message: ::std::sync::Arc<PublishedMessage>,
    ) {
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
            let released: usize = held.iter().map(|message| message.retained_bytes()).sum();
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

/// An id in the 8-4-4-4-12 hexadecimal form of a version-4 UUID, derived from
/// `counter` plus the clock and the process's randomly seeded hasher.
///
/// The ONE place this crate shapes such an id, reached by
/// [`ComAmazonawsSnsAmazonsimplenotificationserviceState::next_unique_id`] (whose
/// counter is the store's) and by [`new_request_id`] (whose counter is this
/// module's). Unique because a monotone counter goes into it; unpredictable
/// because the hasher's random seed does too.
fn uuid_v4_shaped(counter: u64) -> String {
    use std::hash::{BuildHasher, Hasher};

    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let digest = Sha256::digest(format!("{counter}:{nanos}:{entropy}"));
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

/// The id one response reports as its own: the value of the `<RequestId>` element
/// every SNS response document carries.
///
/// A correlation id, not a security token, and per-response rather than
/// per-resource: two responses are distinguishable, which is what a client logs
/// and a proxy correlates on. It needs no state, so it is a free function rather
/// than a method on the store — a request that fails before any store is reached
/// still answers with one.
pub fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The monotone part of a request id, so two ids minted inside one clock tick
    /// still differ.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    uuid_v4_shaped(MINTED.fetch_add(1, Ordering::Relaxed))
}

/// How many colon-separated elements an ARN has:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`.
pub const ARN_ELEMENTS: usize = 6;

/// Why `value` is not an ARN at all, worded as the `Reason:` clause of an
/// `InvalidParameterException` states it — or `None` when it has an ARN's shape.
///
/// This is ARGUMENT validation, not a lookup: a value that could not name any
/// resource in any account is bad input, so an ARN-typed member is checked for
/// this BEFORE the store is consulted. Reporting it as a resource that does not
/// exist tells a caller the opposite of what happened — "it is gone" instead of
/// "you sent me something that is not an ARN" — and an idempotent delete then
/// swallows the defect entirely.
///
/// Only the SHAPE is judged here. An ARN-shaped value that names no resource of
/// this account is a lookup miss, which each operation reports as its own
/// modelled `NotFoundException`.
pub fn arn_shape_reason(value: &str) -> Option<String> {
    let elements = value.split(':').count();
    if elements < ARN_ELEMENTS {
        return Some(format!(
            "An ARN must have at least {ARN_ELEMENTS} elements, not {elements}"
        ));
    }
    if !value.starts_with("arn:") {
        return Some("An ARN must begin with 'arn:'".to_string());
    }
    None
}

/// The message an ARN-typed `member` holding `value` is refused with, or `None`
/// when `value` has an ARN's shape: it names the member and the constraint the
/// value failed, and never echoes the value itself.
pub fn arn_refusal(member: &str, value: &str) -> Option<String> {
    arn_shape_reason(value).map(|reason| format!("Invalid parameter: {member} Reason: {reason}"))
}

/// The most tags one topic holds: "Can't add more than 50 tags to a topic"
/// ([`TagLimitExceededException`](crate::ops::types::TagLimitExceededException) doc).
///
/// Shared by every operation that writes a topic's tags, so a topic cannot hold a
/// different number depending on which call put them there.
pub const MAX_TAGS_PER_TOPIC: usize = 50;

/// The sentence SNS refuses a write that would take a resource past
/// [`MAX_TAGS_PER_TOPIC`] with: the quota that is exhausted, with neither the count
/// held nor the count asked for in it. Shared by every operation that writes a
/// topic's tags, so one condition reads the same whichever call carried the write.
pub const TAG_QUOTA_EXCEEDED_REFUSAL: &str =
    "Could not complete request: tag quota of per resource exceeded";

/// The message a `ResourceArn` of one of SNS's TAG operations is refused with, or
/// `None` when the value is a topic ARN.
///
/// Two rules, in order, and both are ARGUMENT validation rather than a lookup — a
/// value that could not name a topic in ANY account is bad input, so it is judged
/// before the store is consulted:
///
/// 1. it has to be an ARN at all ([`arn_refusal`]);
/// 2. it has to be an ARN of an Amazon SNS TOPIC. Every tag operation documents its
///    `ResourceArn` as "The ARN of the topic …", and SNS holds tags on topics alone,
///    so an ARN of another service, or an SNS ARN whose resource part is not a topic
///    name (a platform endpoint's `endpoint/GCM/app/…`, a subscription's
///    `<topic>:<uuid>`), names nothing a tag operation could ever act on.
///
/// Neither clause reproduces the value the caller sent: both name the member and the
/// constraint it failed, as every other refusal of this service does.
pub fn tagged_topic_arn_refusal(member: &str, value: &str) -> Option<String> {
    if let Some(message) = arn_refusal(member, value) {
        return Some(message);
    }
    topic_name_from_arn(value).is_none().then(|| {
        format!(
            "Invalid parameter: {member} Reason: An SNS tag is held by a topic, so the \
             ARN must name an Amazon SNS topic"
        )
    })
}

/// The refusal a topic's tag map is held to: `None` when `tags` is a map the topic may
/// hold, a message when it is not.
///
/// Both ceilings are checked, in the order a caller fixes them: the documented
/// [`MAX_TAGS_PER_TOPIC`] entry COUNT, then this emulator's own
/// [`MAX_TAG_BYTES_PER_TOPIC`] byte ceiling on the one caller-sized map a topic record
/// carries. The first is the modelled `TagLimitExceededException`'s sentence; the
/// second is an `InvalidParameterException`'s — so the caller is told which it crossed.
pub enum TagRefusal {
    /// Too many entries: the documented per-topic tag limit.
    TooMany(String),
    /// Too many bytes: this emulator's per-record ceiling.
    TooLarge(String),
}

/// The refusal for the tag map a write WOULD leave a topic holding, or `None` when
/// the topic may hold it. Measured on the resulting map, so a caller cannot grow one
/// record past either ceiling by repeating a call with ever larger values.
pub fn tag_map_refusal(tags: &HashMap<String, String>) -> Option<TagRefusal> {
    if tags.len() > MAX_TAGS_PER_TOPIC {
        return Some(TagRefusal::TooMany(TAG_QUOTA_EXCEEDED_REFUSAL.to_string()));
    }
    let held = attribute_bytes(tags);
    (held > MAX_TAG_BYTES_PER_TOPIC).then(|| {
        TagRefusal::TooLarge(format!(
            "Invalid parameter: Tags - a topic holds at most {MAX_TAG_BYTES_PER_TOPIC} \
             bytes of Tags: {held}"
        ))
    })
}

/// The sentence SNS states for a topic no account of this emulator holds.
///
/// Fixed, and the SAME for every operation that looks a topic up: SNS names the
/// resource kind that is missing and does NOT echo the identifier the caller sent
/// back at it. Recorded evidence: `GetTopicAttributes` on a deleted topic answers
/// `404 NotFound` with exactly `Topic does not exist`.
pub const TOPIC_DOES_NOT_EXIST: &str = "Topic does not exist";

/// The sentence SNS states for a subscription this emulator holds no record of — the
/// subscription counterpart of [`TOPIC_DOES_NOT_EXIST`], worded the same way: the
/// resource kind that is missing, with none of the caller's identifier echoed back.
/// The HTTP 404 and the `NotFound` code are the requirement; this is SNS's wording of
/// the reason.
pub const SUBSCRIPTION_DOES_NOT_EXIST: &str = "Subscription does not exist";

/// The sentence SNS states for an ENDPOINT of a platform application this emulator
/// holds no record of — the endpoint counterpart of [`TOPIC_DOES_NOT_EXIST`], worded
/// the same way: the resource kind that is missing, with none of the caller's
/// identifier echoed back. The HTTP 404 and the `NotFound` code are the requirement;
/// this is SNS's wording of the reason.
pub const ENDPOINT_DOES_NOT_EXIST: &str = "Endpoint does not exist";

/// The sentence SNS states for a RESOURCE a call names and the account does not hold:
/// the operations whose miss is the resource-scoped `ResourceNotFoundException` (the
/// tag operations, the SMS sandbox) say "resource", where the topic-scoped
/// `NotFoundException` names the topic ([`TOPIC_DOES_NOT_EXIST`]). Neither echoes the
/// caller's identifier back — the sandbox's is a phone number, which the model marks
/// `@sensitive`.
pub const RESOURCE_DOES_NOT_EXIST: &str = "Resource does not exist";

/// The sentence SNS refuses an attribute name it does not write with: `Invalid
/// parameter:` and the parameter, whether the name is none SNS defines for that
/// resource, one only the create call sets, or one the service computes. Shared by
/// every operation that writes an attribute by name, so the sentence does not depend
/// on which call carried it.
pub const INVALID_ATTRIBUTE_NAME_REFUSAL: &str = "Invalid parameter: AttributeName";

/// The sentence SNS refuses an unacceptable topic name with.
///
/// `Invalid parameter:` plus the parameter as SNS names it (`Topic Name`, not the
/// Smithy member `Name`), and nothing else: the sentence is the same whichever
/// name rule the value broke — the character set, the length, or the
/// [`FIFO_SUFFIX`] a FIFO topic's name needs — and it does not reproduce the name
/// the caller sent. Recorded evidence: `CreateTopic` with `FifoTopic=true` and a
/// name that does not end in `.fifo` answers `400 InvalidParameter` with exactly
/// `Invalid parameter: Topic Name`.
pub const INVALID_TOPIC_NAME: &str = "Invalid parameter: Topic Name";

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
