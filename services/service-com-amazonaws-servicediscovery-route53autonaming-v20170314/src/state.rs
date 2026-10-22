// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicediscovery-route53autonaming-v20170314 service.
//!
//! Cloud Map's resources form one tree — namespaces hold services, services hold
//! instances — plus the asynchronous `Operation` records every mutating call files
//! and `GetOperation` reads back. Everything the handlers in `src/ops/` answer with
//! is computed from the records below; nothing is canned.
//!
//! Two name indexes exist because the API resolves resources BOTH ways: the control
//! plane addresses a namespace/service by id, while `DiscoverInstances` addresses them
//! by name. They are maintained beside the record maps on every write, so a lookup by
//! either key reaches the one stored record.
//!
//! Every store here is BOUNDED, because it lives in memory for the life of the
//! process and grows on request: a create past a resource quota is refused with the
//! modelled `ResourceLimitExceeded` (`MAX_NAMESPACES`,
//! `MAX_SERVICES_PER_NAMESPACE`, `MAX_INSTANCES_PER_SERVICE`), and the two stores no
//! API can ever shrink — the operation records and the remembered idempotency tokens
//! — evict their oldest entry instead (`MAX_RETAINED_OPERATIONS`,
//! `MAX_RETAINED_IDEMPOTENCY_TOKENS`).

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, HttpProperties, Instance, Namespace,
    NamespaceProperties, Operation, Service, Tag, SOA,
};

/// The region an ARN carries when the request is unsigned, matching the region a
/// generated emulator defaults to (see `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN/`aws.api#service` namespace for this service, used for every ARN it mints.
pub const ARN_NAMESPACE: &str = "servicediscovery";

/// How long an asynchronous operation stays `SUBMITTED` before the clock owes it
/// `SUCCESS`.
///
/// AWS documents no duration for this, so the span is CHOSEN: it sits inside the
/// emulator's 250–750 ms window for a clock-owed transition, so a caller that polls
/// `GetOperation` immediately still observes the non-terminal `SUBMITTED` status the
/// real service reports. The due instant is stored and materialised on read — the
/// service never sleeps for it.
pub const OPERATION_SUCCESS_SPAN: Duration = Duration::from_millis(250);

/// The quota on tags per resource, from the `TooManyTagsException` documentation:
/// "The maximum number of tags that can be applied to a resource is 50."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many operation records this store keeps.
///
/// AWS does not publish a retention rule for `Operation` records, so this bound is
/// the EMULATOR's: the store is in memory for the life of the process, and every
/// mutating call files a record, so without a bound the map would grow without
/// limit. When the bound is passed the oldest record that has already reached a
/// terminal status is dropped first, so a caller can still resolve an operation it is
/// polling. `GetOperation` answers `OperationNotFound` for an evicted id, which is
/// the same answer it gives for an id the service never filed.
pub const MAX_RETAINED_OPERATIONS: usize = 1_000;

/// How many `CreatorRequestId` tokens this store remembers, for the same reason as
/// [`MAX_RETAINED_OPERATIONS`]. The oldest token is forgotten first; a retry whose
/// token has been forgotten is served as a fresh request.
pub const MAX_RETAINED_IDEMPOTENCY_TOKENS: usize = 1_000;

/// How many namespaces this emulator holds before refusing to create another with
/// the modelled `ResourceLimitExceeded` ("The resource can't be created because
/// you've reached the quota on the number of resources").
///
/// Real Cloud Map publishes per-account, adjustable quotas; this emulator does not
/// claim to reproduce their values, so the bound here is the EMULATOR's own: a
/// resource store that only ever grows has no other limit, since the records live in
/// the process for its lifetime. The bound is answered with the error the model
/// declares for exactly this case, never by dropping a resource a caller created.
pub const MAX_NAMESPACES: usize = 1_000;

/// How many services one namespace holds, bounded for the reason [`MAX_NAMESPACES`]
/// gives.
pub const MAX_SERVICES_PER_NAMESPACE: usize = 1_000;

/// How many instances one service holds, bounded for the reason [`MAX_NAMESPACES`]
/// gives. Re-registering an instance that already exists is a replace, so it is never
/// refused by this bound.
pub const MAX_INSTANCES_PER_SERVICE: usize = 1_000;

/// What a handler reports when this service was mounted on an engine that holds no
/// state at all.
///
/// A wiring failure, not a bad request — but this service's model declares no
/// server-fault shape (`InvalidInput` is its only service-wide error), and a 501 would
/// claim the operation is unwritten, which is a different and false claim. So the
/// handlers answer with `InvalidInput` carrying this message, which says plainly what
/// went wrong. Unreachable in the assembled emulator, where every service is mounted
/// on the one shared `InMemoryEngine`.
pub const UNREACHABLE_STATE_MESSAGE: &str =
    "the servicediscovery emulator is not mounted on the shared engine, so its stored \
     namespaces, services, instances and operations are not reachable";

/// `OperationStatus` values this service reports (see `OPERATION_STATUS_VALUES`).
pub const OPERATION_STATUS_SUBMITTED: &str = "SUBMITTED";
/// The terminal success status an operation reaches once its due instant passes.
pub const OPERATION_STATUS_SUCCESS: &str = "SUCCESS";

/// `HealthStatus` reported for an instance no health checker has reported on.
pub const HEALTH_STATUS_UNKNOWN: &str = "UNKNOWN";
/// `HealthStatus` of an instance a health checker reported healthy.
pub const HEALTH_STATUS_HEALTHY: &str = "HEALTHY";
/// `HealthStatus` of an instance a health checker reported unhealthy.
pub const HEALTH_STATUS_UNHEALTHY: &str = "UNHEALTHY";

/// A persisted namespace. `ServiceCount` is NOT a field: it is derived from
/// [`ComAmazonawsServicediscoveryRoute53autonamingV20170314State::service_count`] so
/// the namespace and the service map can never disagree.
#[derive(Debug, Clone)]
pub struct NamespaceRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// A `NamespaceType` value (`HTTP`, `DNS_PRIVATE`, `DNS_PUBLIC`).
    pub namespace_type: String,
    pub description: Option<String>,
    pub create_date: SystemTime,
    pub creator_request_id: Option<String>,
    /// `Properties.HttpProperties.HttpName` — the name instances are discovered under.
    pub http_name: Option<String>,
    /// `Properties.DnsProperties.HostedZoneId`, set only for a DNS namespace.
    pub hosted_zone_id: Option<String>,
    /// `Properties.DnsProperties.SOA.TTL` — the TTL "for purposes of negative
    /// caching" of the hosted zone's Start of Authority record. Held only for a DNS
    /// namespace whose creator stated it: `SOA` is optional on the reported
    /// `DnsProperties`, AWS publishes no default value for it, and the emulator
    /// reports no TTL it was never given rather than inventing one.
    pub soa_ttl: Option<i64>,
    pub tags: Vec<(String, String)>,
}

/// A persisted service. `InstanceCount` is derived from `instances`, and
/// `InstancesRevision` is bumped on every register/deregister.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub namespace_id: String,
    pub description: Option<String>,
    pub dns_config: Option<DnsConfig>,
    pub health_check_config: Option<HealthCheckConfig>,
    pub health_check_custom_config: Option<HealthCheckCustomConfig>,
    /// A `ServiceType` value (`HTTP`, `DNS`, `DNS_HTTP`).
    pub service_type: String,
    pub create_date: SystemTime,
    pub creator_request_id: Option<String>,
    pub tags: Vec<(String, String)>,
    /// Keyed by `InstanceId`; a `BTreeMap` so every read orders identically.
    pub instances: BTreeMap<String, InstanceRecord>,
    pub instances_revision: i64,
}

impl ServiceRecord {
    /// `InstanceCount`, derived from what is stored rather than counted by hand.
    pub fn instance_count(&self) -> i32 {
        i32::try_from(self.instances.len()).unwrap_or(i32::MAX)
    }

    /// Whether the service configures health checking at all. Documented on
    /// `DiscoverInstancesRequest$HealthStatus`: the filter "is ignored for services
    /// that don't have a health check configured, and all instances are returned".
    pub fn health_checked(&self) -> bool {
        self.health_check_config.is_some() || self.health_check_custom_config.is_some()
    }
}

/// A persisted service instance.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub instance_id: String,
    pub attributes: HashMap<String, String>,
    pub creator_request_id: Option<String>,
    /// A `HealthStatus` value. An instance starts `UNKNOWN`: nothing has reported on
    /// it yet, neither a Route 53 health checker nor an
    /// `UpdateInstanceCustomHealthStatus` call.
    pub health_status: String,
}

/// A persisted asynchronous operation, the record `GetOperation` reads.
#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub id: String,
    /// An `OperationType` value (`CREATE_NAMESPACE`, `REGISTER_INSTANCE`, …).
    pub operation_type: String,
    /// An `OperationStatus` value.
    pub status: String,
    pub create_date: SystemTime,
    pub update_date: SystemTime,
    /// When the clock owes this operation its terminal status.
    pub success_due_at: SystemTime,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    /// `OperationTargetType` -> resource id (`NAMESPACE`, `SERVICE`, `INSTANCE`).
    pub targets: HashMap<String, String>,
}

/// A resource one of this service's tagging operations addresses, identified by the
/// record that holds its tags.
///
/// Cloud Map's taggable resources are its namespaces and its services (`Tag` is "a
/// custom key-value pair that's associated with a resource", and both carry a `Tags`
/// member on creation), and `TagResource`, `UntagResource` and `ListTagsForResource`
/// all address them the same way — by `ResourceARN` — so one resolution serves all
/// three rather than each handler knowing the resource tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// The namespace with this `Id`.
    Namespace(String),
    /// The service with this `Id`.
    Service(String),
}

/// Which call an idempotency token was first seen on, so one client's
/// `CreatorRequestId` cannot collide across unrelated operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdempotencyScope {
    CreateHttpNamespace,
    CreatePublicDnsNamespace,
    CreateService,
    RegisterInstance,
    UpdateHttpNamespace,
}
impl IdempotencyScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::CreateHttpNamespace => "CreateHttpNamespace",
            Self::CreatePublicDnsNamespace => "CreatePublicDnsNamespace",
            Self::CreateService => "CreateService",
            Self::RegisterInstance => "RegisterInstance",
            Self::UpdateHttpNamespace => "UpdateHttpNamespace",
        }
    }
}

/// Per-service state for com-amazonaws-servicediscovery-route53autonaming-v20170314. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsServicediscoveryRoute53autonamingV20170314State {
    /// Namespaces by `Id`.
    pub namespaces: HashMap<String, NamespaceRecord>,
    /// `Name` -> namespace `Id`: what `DiscoverInstances` resolves and what makes
    /// `NamespaceAlreadyExists` detectable.
    pub namespace_ids_by_name: HashMap<String, String>,
    /// Services by `Id`.
    pub services: HashMap<String, ServiceRecord>,
    /// (namespace `Id`, service `Name`) -> service `Id`: resolves
    /// `DiscoverInstances`' `ServiceName` and enforces `ServiceAlreadyExists`.
    pub service_ids_by_namespace_and_name: HashMap<(String, String), String>,
    /// Operations by `OperationId`, bounded by [`MAX_RETAINED_OPERATIONS`].
    pub operations: HashMap<String, OperationRecord>,
    /// scoped `CreatorRequestId` -> the id the first call returned, bounded by
    /// [`MAX_RETAINED_IDEMPOTENCY_TOKENS`].
    pub idempotency: HashMap<String, String>,
    /// The idempotency keys in the order they were first remembered, so the oldest
    /// token is the one forgotten when the bound is reached.
    idempotency_order: VecDeque<String>,
    /// Monotonic counter that makes every minted id unique within this emulator.
    id_seq: u64,
}

impl ComAmazonawsServicediscoveryRoute53autonamingV20170314State {
    /// Mint an opaque id of the form `<prefix>-<digits lowercase alphanumerics>`
    /// (just the digits when `prefix` is empty), unique for the lifetime of this store.
    ///
    /// The digits come from a SHA-256 of the monotonic counter and the wall clock, so
    /// an id is neither guessable nor derived from any request value. `digits` is
    /// clamped to the digest width, so an id is at most 32 characters long.
    pub fn next_id(&mut self, prefix: &str, digits: usize) -> String {
        self.id_seq = self.id_seq.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(prefix.as_bytes());
        hasher.update(self.id_seq.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        let digest = hasher.finalize();
        // Cloud Map ids are lowercase alphanumeric, so the digest bytes are folded
        // into base 36 rather than rendered as hex.
        const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let body: String = digest
            .iter()
            .take(digits.min(digest.len()))
            .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
            .collect();
        if prefix.is_empty() {
            body
        } else {
            format!("{prefix}-{body}")
        }
    }

    /// Mint the id of the Route 53 hosted zone a DNS namespace carries —
    /// `DnsProperties$HostedZoneId` is "the ID for the Route 53 hosted zone that Cloud
    /// Map creates when you create a namespace".
    ///
    /// Route 53 hosted zone ids are an uppercase alphanumeric string that starts with
    /// `Z`, so the minted digits of [`Self::next_id`] are upper-cased behind that
    /// prefix. The id is opaque: it is derived from the store's counter and the clock,
    /// never from the namespace name.
    pub fn next_hosted_zone_id(&mut self) -> String {
        format!("Z{}", self.next_id("", 20).to_uppercase())
    }

    /// File an operation record and return its `OperationId`.
    ///
    /// The record starts `SUBMITTED` with its success due [`OPERATION_SUCCESS_SPAN`]
    /// later; [`Self::operation`] materialises the transition when the clock reaches it.
    pub fn record_operation(
        &mut self,
        operation_type: &str,
        now: SystemTime,
        targets: impl IntoIterator<Item = (String, String)>,
    ) -> String {
        // An `OperationId` carries no prefix; 32 digits is the digest width.
        let id = self.next_id("", 32);
        self.operations.insert(
            id.clone(),
            OperationRecord {
                id: id.clone(),
                operation_type: operation_type.to_string(),
                status: OPERATION_STATUS_SUBMITTED.to_string(),
                create_date: now,
                update_date: now,
                success_due_at: now + OPERATION_SUCCESS_SPAN,
                error_code: None,
                error_message: None,
                targets: targets.into_iter().collect(),
            },
        );
        self.evict_oldest_operations();
        id
    }

    /// Keep [`Self::operations`] within [`MAX_RETAINED_OPERATIONS`], dropping the
    /// oldest record that has already reached a terminal status first so a caller
    /// polling a fresh operation can still resolve it.
    ///
    /// A token remembered for an evicted operation is forgotten with it, so a replay
    /// never hands back an id `GetOperation` can no longer resolve.
    fn evict_oldest_operations(&mut self) {
        while self.operations.len() > MAX_RETAINED_OPERATIONS {
            let victim = self
                .operations
                .values()
                .min_by_key(|record| {
                    (
                        record.status == OPERATION_STATUS_SUBMITTED,
                        record.create_date,
                    )
                })
                .map(|record| record.id.clone());
            match victim {
                Some(id) => {
                    self.operations.remove(&id);
                    // Forget every token that produced this id, and keep the order
                    // queue in step with the map so no dead key is ever popped in
                    // place of a live one.
                    let tokens = &mut self.idempotency;
                    let order = &mut self.idempotency_order;
                    tokens.retain(|_, produced| *produced != id);
                    order.retain(|key| tokens.contains_key(key));
                }
                None => break,
            }
        }
    }

    /// Apply the status transition the clock already owes `record`.
    ///
    /// Writing here is what a reporting call may do: the operation was due to succeed
    /// at `success_due_at` whether or not anybody asked, and `update_date` is the
    /// instant it became due rather than the instant somebody noticed, so two readers
    /// of the same record report the same dates.
    fn materialise(record: &mut OperationRecord, now: SystemTime) {
        if record.status == OPERATION_STATUS_SUBMITTED && now >= record.success_due_at {
            record.status = OPERATION_STATUS_SUCCESS.to_string();
            record.update_date = record.success_due_at;
        }
    }

    /// The operation `id`, with any status transition the clock already owes it
    /// applied first.
    pub fn operation(&mut self, id: &str, now: SystemTime) -> Option<&OperationRecord> {
        let record = self.operations.get_mut(id)?;
        Self::materialise(record, now);
        Some(record)
    }

    /// Every stored operation in one stable total order — oldest first, `OperationId`
    /// breaking a tie — each with the status transition the clock already owes it
    /// applied first, exactly as [`Self::operation`] applies it to a single record.
    ///
    /// The order is a property of the records themselves (create instant, then id), not
    /// of the map's iteration order, so the page boundary a `ListOperations`
    /// `NextToken` names stays put as later operations are filed.
    pub fn operations_in_order(&mut self, now: SystemTime) -> Vec<OperationRecord> {
        for record in self.operations.values_mut() {
            Self::materialise(record, now);
        }
        let mut ordered: Vec<OperationRecord> = self.operations.values().cloned().collect();
        ordered
            .sort_by(|left, right| (left.create_date, &left.id).cmp(&(right.create_date, &right.id)));
        ordered
    }

    /// `ServiceCount` for a namespace, derived from the service map.
    pub fn service_count(&self, namespace_id: &str) -> i32 {
        let count = self
            .services
            .values()
            .filter(|service| service.namespace_id == namespace_id)
            .count();
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// Resolve a namespace reference — the members that name one are modelled as
    /// `Arn`, and the docs say a namespace shared with the account is named by its
    /// ARN — to a stored namespace id.
    pub fn resolve_namespace_id(&self, reference: &str) -> Option<String> {
        if self.namespaces.contains_key(reference) {
            return Some(reference.to_string());
        }
        self.namespaces
            .values()
            .find(|namespace| namespace.arn == reference)
            .map(|namespace| namespace.id.clone())
    }

    /// Resolve a service reference (its id or its ARN) to a stored service id.
    pub fn resolve_service_id(&self, reference: &str) -> Option<String> {
        if self.services.contains_key(reference) {
            return Some(reference.to_string());
        }
        self.services
            .values()
            .find(|service| service.arn == reference)
            .map(|service| service.id.clone())
    }

    /// The stored resource an `AmazonResourceName` names, or `None` when no resource
    /// in this store carries that ARN.
    ///
    /// The tagging operations are documented against a `ResourceARN`, so the match is
    /// on the ARN the create minted — a resource id is not an ARN and names nothing
    /// here.
    pub fn resolve_tagged_resource(&self, resource_arn: &str) -> Option<TaggedResource> {
        if let Some(namespace) = self
            .namespaces
            .values()
            .find(|namespace| namespace.arn == resource_arn)
        {
            return Some(TaggedResource::Namespace(namespace.id.clone()));
        }
        self.services
            .values()
            .find(|service| service.arn == resource_arn)
            .map(|service| TaggedResource::Service(service.id.clone()))
    }

    /// The tags stored on a resolved resource, in the order they were first set.
    pub fn tags(&self, resource: &TaggedResource) -> Option<&[(String, String)]> {
        match resource {
            TaggedResource::Namespace(id) => {
                self.namespaces.get(id).map(|record| record.tags.as_slice())
            }
            TaggedResource::Service(id) => {
                self.services.get(id).map(|record| record.tags.as_slice())
            }
        }
    }

    /// The tags stored on a resolved resource, for a tagging call to write through.
    pub fn tags_mut(&mut self, resource: &TaggedResource) -> Option<&mut Vec<(String, String)>> {
        match resource {
            TaggedResource::Namespace(id) => {
                self.namespaces.get_mut(id).map(|record| &mut record.tags)
            }
            TaggedResource::Service(id) => self.services.get_mut(id).map(|record| &mut record.tags),
        }
    }

    /// The namespace `DiscoverInstances`' `NamespaceName` names.
    ///
    /// That member is documented as the namespace's `HttpName`, which "in most cases"
    /// equals its `Name`, so the `HttpName` is matched first and the `Name` index is
    /// the fallback.
    pub fn namespace_by_http_name(&self, http_name: &str) -> Option<&NamespaceRecord> {
        if let Some(found) = self
            .namespaces
            .values()
            .find(|namespace| namespace.http_name.as_deref() == Some(http_name))
        {
            return Some(found);
        }
        self.namespace_ids_by_name
            .get(http_name)
            .and_then(|id| self.namespaces.get(id))
    }

    /// The id a previous call with this `CreatorRequestId` returned, if any.
    ///
    /// `request` identifies WHAT was asked for (the namespace name, the service's
    /// namespace and name, the service and instance id, the namespace and the change
    /// asked of it), so a token reused for a
    /// different resource is not treated as a retry of the first one — a retry that
    /// silently stored nothing while answering 200 would be indistinguishable from a
    /// stored write.
    pub fn replayed(
        &self,
        scope: IdempotencyScope,
        token: &str,
        request: &str,
    ) -> Option<String> {
        self.idempotency
            .get(&Self::idempotency_key(scope, token, request))
            .cloned()
    }

    /// The `OperationId` a previous create of the namespace `name` with this
    /// `CreatorRequestId` answered with — but ONLY while that namespace is still
    /// stored.
    ///
    /// Every namespace create is idempotent on its `CreatorRequestId`, so a retry of
    /// the SAME request replays the operation the first call filed instead of creating
    /// a second namespace. Once `DeleteNamespace` has removed the namespace the token
    /// is stale: replaying it would answer success for a create this call never made,
    /// which is a discarded write indistinguishable from a stored one, so the retry is
    /// served as a fresh create. The rule is the same for every namespace type, so it
    /// lives here rather than in one handler.
    pub fn replayed_namespace_create(
        &self,
        scope: IdempotencyScope,
        token: Option<&str>,
        name: &str,
    ) -> Option<String> {
        if !self.namespace_ids_by_name.contains_key(name) {
            return None;
        }
        self.replayed(scope, token?, name)
    }

    /// Remember what a `CreatorRequestId` produced for this request, so a retry of the
    /// SAME request replays it. Keeps the token store within
    /// [`MAX_RETAINED_IDEMPOTENCY_TOKENS`], forgetting the oldest token first.
    pub fn remember(
        &mut self,
        scope: IdempotencyScope,
        token: &str,
        request: &str,
        produced: &str,
    ) {
        let key = Self::idempotency_key(scope, token, request);
        if self
            .idempotency
            .insert(key.clone(), produced.to_string())
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_RETAINED_IDEMPOTENCY_TOKENS {
            match self.idempotency_order.pop_front() {
                Some(forgotten) => {
                    self.idempotency.remove(&forgotten);
                }
                None => break,
            }
        }
    }

    fn idempotency_key(scope: IdempotencyScope, token: &str, request: &str) -> String {
        format!("{}:{token}:{request}", scope.as_str())
    }
}

/// The region the request was signed for, which every ARN this service mints carries.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// An ARN for one of this service's resources, e.g. `namespace/ns-…`.
pub fn service_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, resource)
}

/// A per-call sort key for one instance id.
///
/// `DiscoverInstances` is documented to return "a randomized list of instances
/// allowing customers to distribute traffic evenly": sorting by this key permutes the
/// list from `nonce` (the caller's wall clock), so the permutation changes between
/// calls without any stored counter being bumped by a read.
pub fn shuffle_key(nonce: u128, instance_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(nonce.to_be_bytes());
    hasher.update(instance_id.as_bytes());
    hasher.finalize().into()
}

/// The separator between a `NextToken`'s scope and its cursor.
///
/// A unit separator cannot appear in a resource id or in an operation name, so splitting
/// a decoded token on it recovers exactly the two fields that were encoded.
const NEXT_TOKEN_SEPARATOR: char = '\u{1f}';

/// An opaque `NextToken` that resumes a paginated read after `cursor`.
///
/// `scope` is the operation that issued it, so a token minted by one paginated read is
/// not honoured as a page boundary by another. The payload is hex-encoded because the
/// member is modelled as an opaque string the caller only ever echoes back.
pub fn encode_next_token(scope: &str, cursor: &str) -> String {
    ::hex::encode(format!("{scope}{NEXT_TOKEN_SEPARATOR}{cursor}"))
}

/// The cursor a `NextToken` carries for `scope`, or `None` when the token is not one
/// this operation issued — which covers a token that is not hex, is not UTF-8, carries
/// no separator, or names another operation's scope. A forged token is therefore
/// refused rather than silently honoured as "start from the beginning".
pub fn decode_next_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (encoded_scope, cursor) = decoded.split_once(NEXT_TOKEN_SEPARATOR)?;
    (encoded_scope == scope).then(|| cursor.to_string())
}

/// A stored instant as the wire `timestamp` the modelled shapes carry.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64()),
        Err(_) => ::aws_smithy_types::DateTime::from_secs(0),
    }
}

/// The modelled `Namespace` for a stored namespace, with `ServiceCount` passed in
/// because it is derived from the service map rather than held on the record.
pub fn namespace_view(record: &NamespaceRecord, service_count: i32) -> Namespace {
    Namespace {
        id: Some(record.id.clone()),
        arn: Some(record.arn.clone()),
        resource_owner: None,
        name: Some(record.name.clone()),
        r#type: Some(record.namespace_type.clone()),
        description: record.description.clone(),
        service_count: Some(service_count),
        properties: Some(NamespaceProperties {
            // A DNS namespace's hosted zone; absent for an HTTP namespace, which has none.
            dns_properties: record.hosted_zone_id.as_ref().map(|hosted_zone_id| {
                crate::ops::types::DnsProperties {
                    hosted_zone_id: Some(hosted_zone_id.clone()),
                    // The SOA record of that hosted zone, reported only when the
                    // namespace carries a TTL for it.
                    soa: record.soa_ttl.map(|ttl| SOA { ttl }),
                }
            }),
            http_properties: record
                .http_name
                .as_ref()
                .map(|http_name| HttpProperties {
                    http_name: Some(http_name.clone()),
                }),
        }),
        create_date: Some(timestamp(record.create_date)),
        creator_request_id: record.creator_request_id.clone(),
    }
}

/// The modelled `Service` for a stored service.
pub fn service_view(record: &ServiceRecord) -> Service {
    Service {
        id: Some(record.id.clone()),
        arn: Some(record.arn.clone()),
        resource_owner: None,
        name: Some(record.name.clone()),
        namespace_id: Some(record.namespace_id.clone()),
        description: record.description.clone(),
        instance_count: Some(record.instance_count()),
        dns_config: record.dns_config.clone(),
        r#type: Some(record.service_type.clone()),
        health_check_config: record.health_check_config.clone(),
        health_check_custom_config: record.health_check_custom_config.clone(),
        create_date: Some(timestamp(record.create_date)),
        creator_request_id: record.creator_request_id.clone(),
        created_by_account: None,
    }
}

/// The modelled `Tag` list for a stored tag list, in the stored order.
pub fn tag_views(tags: &[(String, String)]) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Apply a `TagList` onto a resource's stored tags, returning the result.
///
/// A tag is "a custom key-value pair", so a key identifies at most one tag: setting a
/// key that is already stored REPLACES its value in place (leaving every other tag
/// alone and keeping the stored order stable for a reader), and a key that is not
/// stored yet is appended. The incoming list is applied in order, so a key repeated
/// within one request ends up carrying the last value given for it.
///
/// Returns the would-be tag set WITHOUT storing it, so a caller can refuse the whole
/// call — with the modelled `TooManyTagsException` — before any of it is persisted.
/// Both sides are bounded: the stored set by [`MAX_TAGS_PER_RESOURCE`] and the
/// incoming list by the model's `TagList` maximum of 200 entries, which the request
/// walk refuses before a handler runs.
pub fn tags_after_applying(
    stored: &[(String, String)],
    incoming: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    let mut merged = stored.to_vec();
    for (key, value) in incoming {
        match merged.iter_mut().find(|(stored_key, _)| *stored_key == key) {
            Some(existing) => existing.1 = value,
            None => merged.push((key, value)),
        }
    }
    merged
}

/// The modelled `Instance` for a stored instance.
pub fn instance_view(record: &InstanceRecord) -> Instance {
    Instance {
        id: record.instance_id.clone(),
        creator_request_id: record.creator_request_id.clone(),
        attributes: Some(record.attributes.clone()),
        created_by_account: None,
    }
}

/// The modelled `Operation` for a stored operation record.
pub fn operation_view(record: &OperationRecord) -> Operation {
    Operation {
        id: Some(record.id.clone()),
        owner_account: None,
        r#type: Some(record.operation_type.clone()),
        status: Some(record.status.clone()),
        error_message: record.error_message.clone(),
        error_code: record.error_code.clone(),
        create_date: Some(timestamp(record.create_date)),
        update_date: Some(timestamp(record.update_date)),
        targets: Some(record.targets.clone()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicediscoveryRoute53autonamingV20170314State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicediscoveryRoute53autonamingV20170314State>)
}
