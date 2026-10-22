// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connect-amazonconnectservice service.
//!
//! Holds Amazon Connect's primary resource loop — an instance and the queues that
//! live inside it — plus the small helpers every handler of that loop needs
//! (identifier minting, ARN construction, pagination tokens, and the record ->
//! modelled-shape projections). The helpers live here rather than in one operation's
//! file because `src/ops/<op>.rs` is per-operation: a projection that `DescribeQueue`
//! and `ListQueues` both need has to be reachable from both.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ContactFlow, Instance, InstanceSummary, OutboundCallerConfig, OutboundEmailConfig, Queue,
    QueueSummary,
};

/// The region an unsigned request is served in. The emulator's own shared AWS config
/// names `us-east-1` (`vendor/server/src/lib.rs`), so a client that never sets a
/// region still gets ARNs in the region it believes it is talking to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `InstanceStatus` values, from the `com.amazonaws.connect#InstanceStatus` enum.
pub const INSTANCE_STATUS_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";
/// See [`INSTANCE_STATUS_CREATION_IN_PROGRESS`].
pub const INSTANCE_STATUS_ACTIVE: &str = "ACTIVE";

/// `QueueStatus` value a freshly created queue carries, from
/// `com.amazonaws.connect#QueueStatus`.
pub const QUEUE_STATUS_ENABLED: &str = "ENABLED";

/// `QueueType` value `CreateQueue` produces, from `com.amazonaws.connect#QueueType`
/// (the other member, `AGENT`, is the implicit per-agent queue, not a created one).
pub const QUEUE_TYPE_STANDARD: &str = "STANDARD";

/// How long a new instance stays in `CREATION_IN_PROGRESS` before the clock owes it
/// `ACTIVE`. Connect returns a new instance in the non-terminal
/// `CREATION_IN_PROGRESS` status, so the transition has to be observable; the span
/// itself is chosen (the emulator does not model the real provisioning time) and is
/// deliberately non-zero so a caller polling straight after the create still sees the
/// transient status.
pub const INSTANCE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The default page size for a `List*` that sends no `MaxResults`: the modelled
/// maximum for that operation's page-size member (`MaxResult10` -> 10 for
/// `ListInstances`, `MaxResult1000` -> 1000 for `ListQueues`), so an unpaginated
/// caller sees as much as one page can legally hold.
pub const LIST_INSTANCES_DEFAULT_PAGE_SIZE: usize = 10;
/// See [`LIST_INSTANCES_DEFAULT_PAGE_SIZE`].
pub const LIST_QUEUES_DEFAULT_PAGE_SIZE: usize = 1000;

/// How many instances the registry holds before `CreateInstance` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// This is the EMULATOR's own bound on its store, not a claim about any AWS
/// account's instance limit (Connect publishes an adjustable per-account quota this
/// emulator does not model): the store lives for the life of the process and the
/// emulator authenticates nobody, so an unbounded create would let any caller that
/// reaches it grow the map until the host runs out of memory. Every record it holds
/// is released by `DeleteInstance`, and it sits far above any workflow an emulator
/// serves. The same bound caps [`ComAmazonawsConnectAmazonconnectserviceState::client_tokens`].
pub const INSTANCE_QUOTA: usize = 1_000;

/// How many queues one instance holds before `CreateQueue` refuses with the modelled
/// `LimitExceededException`. The emulator's own bound on the store, for the reason
/// [`INSTANCE_QUOTA`] gives; released by `DeleteQueue` (and by deleting the instance).
pub const QUEUES_PER_INSTANCE_QUOTA: usize = 1_000;

/// `ContactFlowStatus` values, from the `com.amazonaws.connect#ContactFlowStatus` enum.
pub const CONTACT_FLOW_STATUS_PUBLISHED: &str = "PUBLISHED";
/// See [`CONTACT_FLOW_STATUS_PUBLISHED`].
pub const CONTACT_FLOW_STATUS_SAVED: &str = "SAVED";

/// `ContactFlowState` value a created flow carries, from
/// `com.amazonaws.connect#ContactFlowState` (the other member, `ARCHIVED`, is what
/// `UpdateContactFlowMetadata` moves a flow to, not a state a create produces).
pub const CONTACT_FLOW_STATE_ACTIVE: &str = "ACTIVE";

/// The alias `DescribeContactFlow` documents for "the saved, not-yet-published
/// content of this flow": `arn:aws:.../contact-flow/{id}:$SAVED`.
pub const CONTACT_FLOW_SAVED_ALIAS: &str = "$SAVED";

/// How many flows one instance holds before `CreateContactFlow` refuses with the
/// modelled `LimitExceededException`. The emulator's own bound on the store, for the
/// reason [`INSTANCE_QUOTA`] gives; released by deleting the instance.
pub const CONTACT_FLOWS_PER_INSTANCE_QUOTA: usize = 1_000;

/// Per-service state for com-amazonaws-connect-amazonconnectservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectAmazonconnectserviceState {
    /// Every live instance, keyed by instance id. Queues hang off their instance, so
    /// deleting the instance drops its queues with it. Bounded by [`INSTANCE_QUOTA`].
    pub instances: BTreeMap<String, InstanceRecord>,
    /// `CreateInstance` idempotency: client token -> the instance it minted. One entry
    /// accompanies one created instance and `DeleteInstance` drops it with that
    /// instance, so this map is bounded by [`INSTANCE_QUOTA`] too — which
    /// `CreateInstance` also checks directly rather than relying on that argument.
    pub client_tokens: BTreeMap<String, String>,
    /// How many identifiers this store has minted. It only ever grows, which is what
    /// makes [`Self::mint_resource_id`] hand out a different id every time.
    pub minted_ids: u64,
}

impl ComAmazonawsConnectAmazonconnectserviceState {
    /// Mint a fresh resource identifier. Connect's instance and queue ids are v4
    /// UUIDs, so this digests the store's monotonic mint counter and lays the 128 bits
    /// out in the `8-4-4-4-12` lowercase-hex form with RFC 4122's version (4) and
    /// variant nibbles set — unique per call, and of the shape the model's `InstanceId`
    /// / `QueueId` carry.
    pub fn mint_resource_id(&mut self) -> String {
        self.minted_ids += 1;
        let digest = Sha256::digest(format!("connect-resource-{}", self.minted_ids));
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
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
}

/// One stored Connect instance.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub id: String,
    pub arn: String,
    pub region: String,
    pub identity_management_type: String,
    pub instance_alias: Option<String>,
    pub directory_id: Option<String>,
    pub inbound_calls_enabled: bool,
    pub outbound_calls_enabled: bool,
    pub service_role: String,
    pub instance_access_url: String,
    pub created_time: SystemTime,
    /// The instant the clock owes this instance `ACTIVE`; see
    /// [`INSTANCE_ACTIVATION_DELAY`].
    pub active_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// This instance's queues, keyed by queue id.
    pub queues: BTreeMap<String, QueueRecord>,
    /// This instance's flows, keyed by flow id.
    pub contact_flows: BTreeMap<String, ContactFlowRecord>,
}

impl InstanceRecord {
    /// The instance's status at `now` — derived from the stored activation instant, so
    /// reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.active_at {
            INSTANCE_STATUS_CREATION_IN_PROGRESS
        } else {
            INSTANCE_STATUS_ACTIVE
        }
    }

    /// The modelled `Instance` shape `DescribeInstance` returns.
    pub fn to_instance(&self, now: SystemTime) -> Instance {
        Instance {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            identity_management_type: Some(self.identity_management_type.clone()),
            instance_alias: self.instance_alias.clone(),
            created_time: Some(to_date_time(self.created_time)),
            service_role: Some(self.service_role.clone()),
            instance_status: Some(self.status(now).to_string()),
            status_reason: None,
            inbound_calls_enabled: Some(self.inbound_calls_enabled),
            outbound_calls_enabled: Some(self.outbound_calls_enabled),
            instance_access_url: Some(self.instance_access_url.clone()),
            tags: tags_out(&self.tags),
        }
    }

    /// The modelled `InstanceSummary` shape `ListInstances` returns.
    pub fn to_summary(&self, now: SystemTime) -> InstanceSummary {
        InstanceSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            identity_management_type: Some(self.identity_management_type.clone()),
            instance_alias: self.instance_alias.clone(),
            created_time: Some(to_date_time(self.created_time)),
            service_role: Some(self.service_role.clone()),
            instance_status: Some(self.status(now).to_string()),
            inbound_calls_enabled: Some(self.inbound_calls_enabled),
            outbound_calls_enabled: Some(self.outbound_calls_enabled),
            instance_access_url: Some(self.instance_access_url.clone()),
        }
    }

    /// The id of this instance's queue called `name`, if it has one. Queue names are
    /// unique within an instance, which is what makes a same-name create a duplicate.
    pub fn queue_id_named(&self, name: &str) -> Option<&str> {
        self.queues
            .values()
            .find(|queue| queue.name == name)
            .map(|queue| queue.id.as_str())
    }

    /// The id of this instance's flow called `name`, if it has one. Flow names are
    /// unique within an instance, which is what makes a same-name create a duplicate.
    pub fn contact_flow_id_named(&self, name: &str) -> Option<&str> {
        self.contact_flows
            .values()
            .find(|flow| flow.name == name)
            .map(|flow| flow.id.as_str())
    }

    /// This instance's flow named by a `ContactFlowId` path label.
    ///
    /// `DescribeContactFlow` documents three forms for that label: the bare flow id,
    /// the flow's ARN, and either of those with a `:$SAVED` (or `:<version>`) suffix.
    /// The emulator stores one content per flow — the saved content, which publishing
    /// does not change — so the `$SAVED` alias resolves to the same record, while a
    /// numbered version resolves to nothing until flow versions are stored.
    pub fn contact_flow_for_label(&self, label: &str) -> Option<&ContactFlowRecord> {
        // An ARN's own colons all precede its resource segment, so taking the last
        // `/`-separated segment first leaves at most the `:<qualifier>` suffix.
        let resource = label.rsplit_once('/').map_or(label, |(_, last)| last);
        let (id, qualifier) = match resource.split_once(':') {
            Some((id, qualifier)) => (id, Some(qualifier)),
            None => (resource, None),
        };
        if qualifier.is_some_and(|qualifier| qualifier != CONTACT_FLOW_SAVED_ALIAS) {
            return None;
        }
        self.contact_flows.get(id)
    }
}

/// One stored queue, inside its instance.
#[derive(Debug, Clone)]
pub struct QueueRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub hours_of_operation_id: String,
    pub max_contacts: Option<i32>,
    pub outbound_caller_config: Option<OutboundCallerConfig>,
    pub outbound_email_config: Option<OutboundEmailConfig>,
    /// Kept as given: no modelled `Queue`/`QueueSummary` member carries it, and the
    /// operations that read it (`ListQueueQuickConnects`,
    /// `AssociateQueueQuickConnects`) are still scaffolded.
    pub quick_connect_ids: Vec<String>,
    pub queue_type: String,
    pub status: String,
    pub tags: BTreeMap<String, String>,
    pub last_modified_time: SystemTime,
    pub last_modified_region: String,
}

impl QueueRecord {
    /// The modelled `Queue` shape `DescribeQueue` returns.
    pub fn to_queue(&self) -> Queue {
        Queue {
            name: Some(self.name.clone()),
            queue_arn: Some(self.arn.clone()),
            queue_id: Some(self.id.clone()),
            description: self.description.clone(),
            outbound_caller_config: self.outbound_caller_config.clone(),
            outbound_email_config: self.outbound_email_config.clone(),
            hours_of_operation_id: Some(self.hours_of_operation_id.clone()),
            max_contacts: self.max_contacts,
            status: Some(self.status.clone()),
            tags: tags_out(&self.tags),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }

    /// The modelled `QueueSummary` shape `ListQueues` returns.
    pub fn to_summary(&self) -> QueueSummary {
        QueueSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            queue_type: Some(self.queue_type.clone()),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }
}

/// One stored flow, inside its instance.
#[derive(Debug, Clone)]
pub struct ContactFlowRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub flow_type: String,
    pub state: String,
    pub status: String,
    pub content: String,
    pub tags: BTreeMap<String, String>,
    pub last_modified_time: SystemTime,
    pub last_modified_region: String,
}

impl ContactFlowRecord {
    /// The modelled `ContactFlow` shape `DescribeContactFlow` returns.
    ///
    /// `Version`/`VersionDescription` stay absent: a flow version is only minted by
    /// `CreateContactFlowVersion`, which this emulator does not implement yet, so there
    /// is no stored version to report.
    pub fn to_contact_flow(&self) -> ContactFlow {
        ContactFlow {
            arn: Some(self.arn.clone()),
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            r#type: Some(self.flow_type.clone()),
            state: Some(self.state.clone()),
            status: Some(self.status.clone()),
            description: self.description.clone(),
            content: Some(self.content.clone()),
            tags: tags_out(&self.tags),
            flow_content_sha256: Some(flow_content_sha256(&self.content)),
            version: None,
            version_description: None,
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }
}

/// The checksum Connect reports for a flow's content: the SHA-256 of the content
/// bytes, lowercase hex — 64 characters, which is the only form the modelled
/// `FlowContentSha256` (`^[a-zA-Z0-9]{64}$`) can take. Derived from the stored
/// content, so the same content always checksums the same and a changed content
/// always changes the checksum.
pub fn flow_content_sha256(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope (an unsigned call, or one the emulator serves with
/// authentication off).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:connect:<region>:<account>:instance/<id>`.
pub fn instance_arn(region: &str, instance_id: &str) -> String {
    ::engine::emulator_arn("connect", region, &format!("instance/{instance_id}"))
}

/// The service-linked role Connect reports for an instance, as an IAM ARN (IAM is
/// global, so the region segment is empty).
pub fn instance_service_role(instance_id: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!(
            "role/aws-service-role/connect.amazonaws.com/AWSServiceRoleForAmazonConnect_{instance_id}"
        ),
    )
}

/// The agent/admin console URL Connect reports for an instance. It is keyed on the
/// instance alias where there is one (`https://<alias>.my.connect.aws`), and on the
/// instance id otherwise, so the URL is always derived from stored state.
pub fn instance_access_url(alias: Option<&str>, instance_id: &str) -> String {
    let label = alias.unwrap_or(instance_id);
    format!("https://{label}.my.connect.aws")
}

/// Convert a stored instant into the modelled timestamp type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// Store a request's tags in deterministic order.
pub fn tags_in(tags: Option<HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.unwrap_or_default().into_iter().collect()
}

/// Project stored tags back onto the wire. An untagged resource reports no `Tags`
/// member at all rather than an empty map, which is also the only form the modelled
/// `TagMap` (length 1..50) can take.
pub fn tags_out(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(
            tags.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// Encode a pagination cursor: the key of the last item on the page, in a form a
/// caller cannot read or construct meaning from.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key.as_bytes())
}

/// Decode a cursor produced by [`encode_page_token`]. `None` for anything else, which
/// the caller refuses as an invalid request rather than silently restarting the scan.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// Cut one page out of a key-ordered list.
///
/// `entries` is every matching item paired with its sort key, in ascending key order;
/// `after` is the decoded cursor (exclusive). The returned token is present only when
/// items remain beyond this page, so the last page carries none.
pub fn paginate<T>(
    entries: Vec<(String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
    default_page_size: usize,
) -> (Vec<T>, Option<String>) {
    let page_size = max_results
        .filter(|requested| *requested > 0)
        .map_or(default_page_size, |requested| requested as usize);

    let mut items = Vec::new();
    let mut last_key: Option<String> = None;
    let mut truncated = false;

    for (key, item) in entries {
        if after.is_some_and(|cursor| key.as_str() <= cursor) {
            continue;
        }
        if items.len() >= page_size {
            truncated = true;
            break;
        }
        last_key = Some(key);
        items.push(item);
    }

    let next_token = if truncated {
        last_key.as_deref().map(encode_page_token)
    } else {
        None
    };
    (items, next_token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectAmazonconnectserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectAmazonconnectserviceState>)
}
