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
        self.id_counter = self.id_counter.saturating_add(1);
        uuid_v4_shaped(self.id_counter)
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

/// The sentence SNS states for a topic no account of this emulator holds.
///
/// Fixed, and the SAME for every operation that looks a topic up: SNS names the
/// resource kind that is missing and does NOT echo the identifier the caller sent
/// back at it. Recorded evidence: `GetTopicAttributes` on a deleted topic answers
/// `404 NotFound` with exactly `Topic does not exist`.
pub const TOPIC_DOES_NOT_EXIST: &str = "Topic does not exist";

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
