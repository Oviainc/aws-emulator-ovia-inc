// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2 service.
//!
//! The origination-identity + send-text slice of AWS End User Messaging SMS
//! (`sms-voice`) is persisted here: configuration sets, requested phone numbers and
//! the messages that were actually sent. Every implemented handler computes its whole
//! response from these records, so a create is observable through the matching
//! describe/release and a release is observable as an absence.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{EventDestination, Tag};

/// This service's SigV4 signing name, which is also its ARN namespace.
///
/// From the model's `awsService` identity (`OPERATIONS.md` "Wire protocol": sigv4
/// signing name / ARN namespace `sms-voice`).
pub const SIGNING_NAME: &str = "sms-voice";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The opt-out list a phone number is associated with when the request names none.
///
/// Must satisfy `OptOutListName` (len 1..64, `^[A-Za-z0-9_-]+$`), which
/// `PhoneNumberInformation$OptOutListName` declares `@required`.
pub const DEFAULT_OPT_OUT_LIST_NAME: &str = "Default";

/// The lease price reported for every minted number. Real per-country, per-type
/// pricing is not emulatable, so one chosen value is reported for all of them; it is
/// a US-dollar amount, the unit `MonthlyLeasingPrice` documents.
pub const MONTHLY_LEASING_PRICE: &str = "0.00";

/// The `ClientToken` idempotency key is namespaced by operation, so the same token on
/// two different creates is two different requests. These are the operations that mint a
/// resource under a token, named here because a delete/release must also forget the
/// token that created the resource it removes.
pub const CREATE_CONFIGURATION_SET: &str = "CreateConfigurationSet";
/// See [`CREATE_CONFIGURATION_SET`].
pub const REQUEST_PHONE_NUMBER: &str = "RequestPhoneNumber";

// ---------------------------------------------------------------------------------
// Bounds. Every store below grows on a caller-driven path, so each one has a maximum:
// without one, a loop of requests grows the emulator's heap until the host is out of
// memory and only a restart releases it. The two resource maps are bounded by an
// account quota, refused with the modelled `ServiceQuotaExceededException`; the two
// bookkeeping stores are bounded by eviction of their oldest entry, because a caller
// cannot delete from them.
// ---------------------------------------------------------------------------------

/// The most configuration sets one account may hold. Reaching it is answered with
/// `ServiceQuotaExceededException` / `reason = CONFIGURATION_SETS_PER_ACCOUNT`, the
/// quota the model names for this resource. The number itself is this emulator's
/// choice: AWS publishes an adjustable per-account quota, not a fixed one.
pub const MAX_CONFIGURATION_SETS_PER_ACCOUNT: usize = 1_000;

/// The most phone numbers one account may lease, answered with
/// `ServiceQuotaExceededException` / `reason = PHONE_NUMBERS_PER_ACCOUNT`. Chosen for
/// the same reason as [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`].
pub const MAX_PHONE_NUMBERS_PER_ACCOUNT: usize = 1_000;

/// How many of the most recent sends the message log retains. The log is a diagnostic
/// trail with no operation that reads or trims it, and each record holds caller-sized
/// text, so it is a ring: the oldest record is dropped to make room for a new one.
pub const MAX_RETAINED_MESSAGES: usize = 1_000;

/// How many `ClientToken` entries are retained. An entry is dropped when the resource
/// it created is deleted or released; past this many live entries the oldest is dropped,
/// which can only make a replayed create act as a first create.
pub const MAX_RETAINED_CLIENT_TOKENS: usize = 1_000;

/// The region to stamp into ARNs for this request: the SigV4 credential scope's
/// region, or [`DEFAULT_REGION`] when the request carries no scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A wall-clock instant as the modelled `Timestamp` type.
pub fn to_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The resource name carried by a `<Thing>NameOrArn` / `<Thing>IdOrArn` member: the
/// last `/`-separated segment of an ARN, or the value itself when it is a bare name.
pub fn name_from_name_or_arn(value: &str) -> &str {
    if value.starts_with("arn:") {
        value.split('/').next_back().unwrap_or(value)
    } else {
        value
    }
}

/// A stored configuration set.
#[derive(Debug, Clone)]
pub struct ConfigurationSet {
    pub name: String,
    pub arn: String,
    pub tags: Vec<Tag>,
    pub created_timestamp: SystemTime,
    pub default_message_type: Option<String>,
    pub default_sender_id: Option<String>,
    pub default_message_feedback_enabled: bool,
    pub event_destinations: Vec<EventDestination>,
}

/// A stored origination phone number.
#[derive(Debug, Clone)]
pub struct PhoneNumberRecord {
    pub id: String,
    pub arn: String,
    /// E.164, as `PhoneNumber` (`^\+?[1-9][0-9]{1,18}$`) requires.
    pub phone_number: String,
    pub status: String,
    pub iso_country_code: String,
    pub message_type: String,
    pub number_capabilities: Vec<String>,
    pub number_type: String,
    pub monthly_leasing_price: String,
    pub two_way_enabled: bool,
    pub two_way_channel_arn: Option<String>,
    pub two_way_channel_role: Option<String>,
    pub self_managed_opt_outs_enabled: bool,
    pub opt_out_list_name: String,
    pub international_sending_enabled: bool,
    pub deletion_protection_enabled: bool,
    pub pool_id: Option<String>,
    pub registration_id: Option<String>,
    pub tags: Vec<Tag>,
    pub created_timestamp: SystemTime,
}

impl PhoneNumberRecord {
    /// The record's value for one `PhoneNumberFilterName`, or `None` when the record
    /// carries no value for that attribute (so no filter value can match it).
    ///
    /// The names are exactly the model's `PhoneNumberFilterName` enum, re-exported as
    /// [`crate::ops::types::PHONE_NUMBER_FILTER_NAME_VALUES`]; `number-capability` is
    /// multi-valued, which is why this answers a list.
    pub fn filterable(&self, name: &str) -> Option<Vec<String>> {
        let single = |value: String| Some(vec![value]);
        match name {
            "status" => single(self.status.clone()),
            "iso-country-code" => single(self.iso_country_code.clone()),
            "message-type" => single(self.message_type.clone()),
            "number-capability" => Some(self.number_capabilities.clone()),
            "number-type" => single(self.number_type.clone()),
            "two-way-enabled" => single(self.two_way_enabled.to_string()),
            "self-managed-opt-outs-enabled" => single(self.self_managed_opt_outs_enabled.to_string()),
            "opt-out-list-name" => single(self.opt_out_list_name.clone()),
            "deletion-protection-enabled" => single(self.deletion_protection_enabled.to_string()),
            "two-way-channel-arn" => self.two_way_channel_arn.clone().map(|arn| vec![arn]),
            _ => None,
        }
    }
}

/// A message this emulator accepted for sending.
#[derive(Debug, Clone)]
pub struct SentMessage {
    pub message_id: String,
    pub destination_phone_number: String,
    pub origination_identity: String,
    pub message_body: Option<String>,
    pub message_type: Option<String>,
    pub configuration_set_name: Option<String>,
    pub context: HashMap<String, String>,
    pub dry_run: bool,
    pub sent_at: SystemTime,
}

/// Per-service state for com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2. A handler reaches it through [`service_state`].
///
/// The two resource maps are `BTreeMap`s so a list/describe walks them in a stable
/// order; `DescribePhoneNumbers` pages through that order, and a page boundary that
/// moved between calls would skip or repeat records.
///
/// Every field that grows on a caller-driven path is bounded — see the `MAX_*`
/// constants above. The two bookkeeping stores are private so the only way to grow them
/// is through [`record_message`](Self::record_message) /
/// [`remember_client_token`](Self::remember_client_token), which enforce their caps.
#[derive(Debug, Default)]
pub struct ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State {
    /// Keyed by `ConfigurationSetName`, which AWS makes unique per account. Bounded by
    /// [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`], enforced by `CreateConfigurationSet`.
    pub configuration_sets: BTreeMap<String, ConfigurationSet>,
    /// Keyed by `PhoneNumberId`. Bounded by [`MAX_PHONE_NUMBERS_PER_ACCOUNT`], enforced
    /// by `RequestPhoneNumber`.
    pub phone_numbers: BTreeMap<String, PhoneNumberRecord>,
    /// The most recent accepted sends, oldest first, dry runs included (`dry_run` tells
    /// them apart). A ring bounded by [`MAX_RETAINED_MESSAGES`].
    messages: VecDeque<SentMessage>,
    /// `"<Operation>:<ClientToken>"` -> what that token already created, so a replayed
    /// request with the same token creates nothing further. Bounded by
    /// [`MAX_RETAINED_CLIENT_TOKENS`].
    client_tokens: HashMap<String, ClientTokenRecord>,
    /// The keys of [`Self::client_tokens`] in insertion order, so the oldest entry is the
    /// one evicted when the cap is reached.
    client_token_order: VecDeque<String>,
    /// Monotonic, and the only source of minted ids: phone number ids, synthetic E.164
    /// numbers and message ids all draw from it, so no two of them collide.
    pub sequence: u64,
}

impl ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State {
    /// The next value of the monotonic id counter.
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// Look a stored number up by any of the origination identities that name it: its
    /// `PhoneNumberId`, its `PhoneNumberArn`, or its E.164 `PhoneNumber` (with or
    /// without the leading `+`, both of which `PhoneNumber` accepts).
    pub fn find_phone_number(&self, identity: &str) -> Option<&PhoneNumberRecord> {
        if let Some(record) = self.phone_numbers.get(identity) {
            return Some(record);
        }
        self.phone_numbers.values().find(|record| {
            record.arn == identity
                || record.phone_number == identity
                || record.phone_number.strip_prefix('+') == Some(identity)
        })
    }

    /// The retained sends, oldest first.
    pub fn messages(&self) -> impl Iterator<Item = &SentMessage> + '_ {
        self.messages.iter()
    }

    /// Record one accepted send, dropping the oldest retained send when the ring is full.
    ///
    /// This is the ONLY way to grow the message log, and the cap dominates its growth:
    /// after [`MAX_RETAINED_MESSAGES`] sends the log's size stops changing.
    pub fn record_message(&mut self, message: SentMessage) {
        while self.messages.len() >= MAX_RETAINED_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(message);
    }

    /// What one `ClientToken` already created, if that entry is retained.
    pub fn client_token_record(&self, operation: &str, token: &str) -> Option<&ClientTokenRecord> {
        self.client_tokens.get(&client_token_key(operation, token))
    }

    /// Remember that `operation` created `resource` under `token` for the request
    /// `fingerprint`, dropping the oldest retained entry when the cap is reached.
    ///
    /// This is the ONLY way to grow the token store, and the cap dominates its growth.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        token: &str,
        resource: &str,
        fingerprint: u64,
    ) {
        let key = client_token_key(operation, token);
        let record = ClientTokenRecord {
            resource: resource.to_string(),
            fingerprint,
        };
        if self.client_tokens.insert(key.clone(), record).is_none() {
            self.client_token_order.push_back(key);
        }
        while self.client_token_order.len() > MAX_RETAINED_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
    }

    /// Drop the idempotency entries that point at a resource that no longer exists, so a
    /// delete/release releases the bookkeeping it created too.
    pub fn forget_client_tokens_for(&mut self, operation: &str, resource: &str) {
        let prefix = format!("{operation}:");
        let stale: Vec<String> = self
            .client_tokens
            .iter()
            .filter(|(key, record)| key.starts_with(&prefix) && record.resource == resource)
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            self.client_tokens.remove(&key);
            self.client_token_order.retain(|retained| *retained != key);
        }
    }
}

/// The idempotency store's key: a `ClientToken` namespaced by the operation that used
/// it, so the same token on two different creates is two different requests.
fn client_token_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// What one `ClientToken` already created, and the request it was first used with.
///
/// AWS replays an idempotency token only for an IDENTICAL request, so the fingerprint is
/// kept beside the resource: a replay that changes any of the request's members is a
/// different request and must be refused rather than answered with this resource.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The id (phone number) or name (configuration set) of what was created.
    pub resource: String,
    /// [`request_fingerprint`] of the request the token was first used with.
    pub fingerprint: u64,
}

/// A fingerprint of the request members a `ClientToken` is scoped to, so a replay can be
/// told apart from a different request reusing the token.
pub fn request_fingerprint(members: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    members.len().hash(&mut hasher);
    for member in members {
        member.hash(&mut hasher);
    }
    hasher.finish()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State>)
}
