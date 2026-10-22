// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralchannel-partnercentralchannel service.
//!
//! This module owns everything the handlers in `src/ops/` share: the stored form of a
//! program management account and of a relationship (a relationship is modelled as a
//! CHILD of a program management account, so its ARN hangs off the parent's), the
//! client-token (idempotency) ledger, the opaque page token `ListRelationships` hands
//! out, and the identifier resolution every operation needs — each identifier member
//! accepts either a bare id (`pma-…` / `rs-…`) or the full ARN.
//!
//! Ids are minted here from a counter held in this store, so every id and ARN the
//! service hands out is also stored here and can be resolved again. Handlers build
//! their own output shapes from what they read back out of here, never from the
//! request that created the record.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    RelationshipDetail, RelationshipSummary, SupportPlan, Tag, ValidationException,
    ValidationExceptionField,
};

/// The ARN namespace this service stamps on every ARN, from the model's
/// `aws.api#service` trait (`arnNamespace: partnercentral`).
const ARN_NAMESPACE: &str = "partnercentral";

/// The region segment to use when a request carries no SigV4 credential scope to
/// read one from (an unsigned request to the emulator).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The revision a freshly created resource carries. `revision` is this service's
/// optimistic-concurrency token ("The current revision number of the
/// relationship"), so the first stored revision is 1 and the first successful
/// update makes it 2.
pub const INITIAL_REVISION: u32 = 1;

/// The `ProgramManagementAccountId` prefix, from the pattern `^pma-[a-z0-9]{13}$`.
pub const PROGRAM_MANAGEMENT_ACCOUNT_ID_PREFIX: &str = "pma-";

/// The `RelationshipId` prefix, from the pattern `^rs-[a-z0-9]{13}$`.
pub const RELATIONSHIP_ID_PREFIX: &str = "rs-";

/// How many `[a-z0-9]` characters follow an id prefix, from both id patterns.
const ID_SUFFIX_LEN: usize = 13;

/// The `ProgramManagementAccountStatus` a freshly created account reports, and the
/// one it settles in. The model's value set is `PENDING | ACTIVE | INACTIVE`, with
/// `PENDING` the non-terminal state a create leaves the account in.
pub const STATUS_PENDING: &str = "PENDING";
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// How long a newly created program management account stays `PENDING` before the
/// clock owes it `ACTIVE`.
///
/// CHOSEN span: neither the model nor the operation docs give an activation time.
/// The project's emulator contract requires a clock-owed transition to fall due
/// 250–750 ms after the call that starts it, so a caller acting immediately after
/// the create still observes the transient state. 300 ms sits inside that band.
pub const ACTIVATION_SPAN: Duration = Duration::from_millis(300);

/// `ListRelationships`' page size when the request declares none. The model gives
/// `maxResults` `@default: 20` with `@range(min: 1, max: 100)`, so the protocol
/// boundary normally fills this in; it is named here so the handler has a value
/// even for a request that somehow carries none.
pub const DEFAULT_MAX_RESULTS: i32 = 20;

/// How many program management accounts this store holds before
/// `CreateProgramManagementAccount` answers the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// CHOSEN bound: the real per-account quota is in neither the model nor the
/// operation docs, and this store lives in memory, so the count is capped rather
/// than left to grow with every request.
pub const MAX_PROGRAM_MANAGEMENT_ACCOUNTS: usize = 100;

/// How many relationships one program management account holds before
/// `CreateRelationship` answers `ServiceQuotaExceededException`. CHOSEN for the
/// same reason as [`MAX_PROGRAM_MANAGEMENT_ACCOUNTS`].
pub const MAX_RELATIONSHIPS_PER_ACCOUNT: usize = 100;

/// The `quotaCode` the modelled `ServiceQuotaExceededException` carries. The model
/// types it as a plain required `String` and documents no value set, so the quota
/// is named after the resource it bounds.
pub const PROGRAM_MANAGEMENT_ACCOUNT_QUOTA_CODE: &str = "ProgramManagementAccountsPerAccount";
pub const RELATIONSHIP_QUOTA_CODE: &str = "RelationshipsPerProgramManagementAccount";

/// The `resourceType` the modelled errors carry. The model types it as a plain
/// required `String`, so these are the model's own shape names.
pub const PROGRAM_MANAGEMENT_ACCOUNT_RESOURCE_TYPE: &str = "ProgramManagementAccount";
pub const RELATIONSHIP_RESOURCE_TYPE: &str = "Relationship";

/// The `ValidationExceptionReason` a malformed request carries, from the model's
/// value set `BUSINESS_VALIDATION_FAILED | REQUEST_VALIDATION_FAILED`.
pub const REQUEST_VALIDATION_FAILED: &str = "REQUEST_VALIDATION_FAILED";

/// How many client tokens the idempotency ledger remembers. Reached only by
/// distinct caller-supplied tokens; the oldest entry is evicted to make room, so
/// the ledger cannot grow without bound.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 1024;

/// How long a client token stays replayable. CHOSEN bound: the model gives no
/// lifetime for `ClientToken`, and an entry no retry will ever ask about must not
/// be kept for the life of the process.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The ARN of the program management account `id` in `catalog`, in the form the
/// model's `ProgramManagementAccountIdentifier` pattern requires
/// (`arn:aws:partnercentral:<region>:<account>:catalog/<catalog>/program-management-account/<id>`).
/// The account segment is the emulator's canonical placeholder account, built
/// through the engine's accessor rather than a literal.
pub fn program_management_account_arn(region: &str, catalog: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("catalog/{catalog}/program-management-account/{id}"),
    )
}

/// The ARN of relationship `id` under the account whose ARN is `parent_arn`: the
/// `RelationshipIdentifier` pattern makes a relationship ARN the parent's ARN with
/// `/relationship/<id>` appended.
pub fn relationship_arn(parent_arn: &str, id: &str) -> String {
    format!("{parent_arn}/relationship/{id}")
}

/// Now, as the wire timestamp type. The ONE clock the handlers read, so a stored
/// timestamp is always the real store time.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
}

/// A stable digest of `value`'s debug rendering, used both as a request
/// fingerprint for the client-token ledger and as the filter fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its
/// members in declaration order, so the same request always digests to the same
/// string and a different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    hex::encode(Sha256::digest(format!("{value:?}").as_bytes()))
}

/// The bare resource id an identifier member names.
///
/// Every identifier member in this service is modelled as either the bare id or
/// the full ARN (`^(arn:…/program-management-account/)?pma-[a-z0-9]{13}$`), so the
/// id is the last `/`-separated segment. `None` when the segment is not an id of
/// `prefix`'s kind — which is how a caller that passed a well-formed ARN of the
/// WRONG kind is told apart from one that passed the right one.
pub fn resolve_identifier<'a>(identifier: &'a str, prefix: &str) -> Option<&'a str> {
    let last = identifier.split('/').next_back()?;
    let suffix = last.strip_prefix(prefix)?;
    (suffix.len() == ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
    .then_some(last)
}

/// A `ValidationException` (HTTP 400) naming the member that was refused and why.
/// Built here so every handler refuses a request this service's own rules reject
/// in the same shape: the message, the modelled `reason`, and a one-entry
/// `fieldList`.
pub fn validation_exception(member: &str, message: impl Into<String>) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: message.clone(),
        reason: REQUEST_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            code: "InvalidValue".to_string(),
            message,
        }]),
    }
}

/// A program management account as stored. Every response that reports one is
/// built from this, never from the request that created it.
#[derive(Debug, Clone)]
pub struct ProgramManagementAccount {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    pub program: String,
    pub display_name: String,
    pub account_id: String,
    pub status: String,
    pub revision: u32,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub start_date: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
    /// When the clock owes this account its `PENDING` -> `ACTIVE` transition.
    /// Stored rather than slept for: [`PartnerCentralChannelState::advance_clock`]
    /// materialises it the next time any handler touches the store.
    pub active_at: SystemTime,
}

/// A relationship as stored, the child of exactly one program management account.
#[derive(Debug, Clone)]
pub struct Relationship {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    pub program_management_account_id: String,
    pub association_type: String,
    pub associated_account_id: String,
    pub display_name: String,
    pub resale_account_model: Option<String>,
    pub sector: String,
    pub requested_support_plan: Option<SupportPlan>,
    pub revision: u32,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub start_date: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl Relationship {
    /// This relationship as the `RelationshipDetail` `GetRelationship` returns.
    pub fn to_detail(&self) -> RelationshipDetail {
        RelationshipDetail {
            arn: Some(self.arn.clone()),
            id: Some(self.id.clone()),
            revision: Some(self.revision.to_string()),
            catalog: Some(self.catalog.clone()),
            association_type: Some(self.association_type.clone()),
            program_management_account_id: Some(self.program_management_account_id.clone()),
            associated_account_id: Some(self.associated_account_id.clone()),
            display_name: Some(self.display_name.clone()),
            resale_account_model: self.resale_account_model.clone(),
            sector: Some(self.sector.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            start_date: Some(self.start_date),
        }
    }

    /// This relationship as the `RelationshipSummary` `ListRelationships` returns.
    /// The summary shape carries neither `resaleAccountModel` nor the support plan
    /// — the model does not declare them on it — so those are reachable only
    /// through `GetRelationship`.
    pub fn to_summary(&self) -> RelationshipSummary {
        RelationshipSummary {
            arn: Some(self.arn.clone()),
            id: Some(self.id.clone()),
            revision: Some(self.revision.to_string()),
            catalog: Some(self.catalog.clone()),
            association_type: Some(self.association_type.clone()),
            program_management_account_id: Some(self.program_management_account_id.clone()),
            associated_account_id: Some(self.associated_account_id.clone()),
            display_name: Some(self.display_name.clone()),
            sector: Some(self.sector.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            start_date: Some(self.start_date),
        }
    }
}

/// The program management accounts of this service, keyed by id and kept in
/// creation order so a paginated list walks a stable sequence. `insert` and
/// `remove` are the only writers, which is what keeps the order and the map
/// consistent.
#[derive(Debug, Default)]
pub struct ProgramManagementAccountStore {
    order: Vec<String>,
    by_id: HashMap<String, ProgramManagementAccount>,
}

impl ProgramManagementAccountStore {
    pub fn insert(&mut self, account: ProgramManagementAccount) {
        self.order.push(account.id.clone());
        self.by_id.insert(account.id.clone(), account);
    }

    pub fn get(&self, id: &str) -> Option<&ProgramManagementAccount> {
        self.by_id.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ProgramManagementAccount> {
        self.by_id.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) -> Option<ProgramManagementAccount> {
        let account = self.by_id.remove(id)?;
        self.order.retain(|stored| stored != id);
        Some(account)
    }

    /// Every stored account, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &ProgramManagementAccount> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The account whose (catalog, program, accountId) triple is already taken, if
    /// any. That triple is what makes a second create a `ConflictException`: one
    /// AWS account can hold only one program management account per program in a
    /// catalog.
    pub fn find_duplicate(
        &self,
        catalog: &str,
        program: &str,
        account_id: &str,
    ) -> Option<&ProgramManagementAccount> {
        self.iter_in_order().find(|account| {
            account.catalog == catalog
                && account.program == program
                && account.account_id == account_id
        })
    }
}

/// The relationships of this service, keyed by id and kept in creation order.
#[derive(Debug, Default)]
pub struct RelationshipStore {
    order: Vec<String>,
    by_id: HashMap<String, Relationship>,
}

impl RelationshipStore {
    pub fn insert(&mut self, relationship: Relationship) {
        self.order.push(relationship.id.clone());
        self.by_id.insert(relationship.id.clone(), relationship);
    }

    pub fn get(&self, id: &str) -> Option<&Relationship> {
        self.by_id.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Relationship> {
        self.by_id.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) -> Option<Relationship> {
        let relationship = self.by_id.remove(id)?;
        self.order.retain(|stored| stored != id);
        Some(relationship)
    }

    /// Every stored relationship, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &Relationship> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// How many relationships hang off one program management account, for the
    /// [`MAX_RELATIONSHIPS_PER_ACCOUNT`] quota.
    pub fn count_for_parent(&self, program_management_account_id: &str) -> usize {
        self.by_id
            .values()
            .filter(|relationship| {
                relationship.program_management_account_id == program_management_account_id
            })
            .count()
    }
}

/// What a client token already in the ledger says about the request carrying it.
#[derive(Debug)]
pub enum TokenOutcome {
    /// The token has not been seen: do the work.
    Fresh,
    /// The same token with the same parameters for the same operation: replay the
    /// earlier result, recomputed from the store.
    Replay(IdempotencyRecord),
    /// The same token and operation with DIFFERENT parameters.
    ParameterMismatch,
    /// The same token already used by a different operation.
    TokenInUse,
}

/// One entry of the client-token ledger. It records WHICH resource the call acted
/// on rather than a serialized body, so a replay is recomputed from the store and
/// can never report a resource that is no longer there.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub operation: &'static str,
    pub request_fingerprint: String,
    pub resource_id: Option<String>,
    /// When the token was first used, for the [`IDEMPOTENCY_TTL`] window.
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-partnercentralchannel-partnercentralchannel. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPartnercentralchannelPartnercentralchannelState {
    /// Every program management account, in creation order.
    pub program_management_accounts: ProgramManagementAccountStore,
    /// Every relationship, in creation order.
    pub relationships: RelationshipStore,
    /// `clientToken` -> what the call that first used it did. Bounded by
    /// [`MAX_IDEMPOTENCY_ENTRIES`] and [`IDEMPOTENCY_TTL`].
    idempotency: HashMap<String, IdempotencyRecord>,
    /// The tokens in `idempotency`, oldest first, so the ledger can evict.
    idempotency_order: VecDeque<String>,
    /// Monotonic counter mixed into a minted id, so two resources created in the
    /// same nanosecond still get different ids.
    id_sequence: u64,
}

impl ComAmazonawsPartnercentralchannelPartnercentralchannelState {
    /// Materialise every transition the clock has already made due: a program
    /// management account past its [`ACTIVATION_SPAN`] leaves `PENDING` for
    /// `ACTIVE`. Called by every handler before it reads or writes, so a
    /// reporting operation reports exactly what was owed at the same instant even
    /// if nobody had called.
    pub fn advance_clock(&mut self) {
        let now = SystemTime::now();
        let due: Vec<String> = self
            .program_management_accounts
            .iter_in_order()
            .filter(|account| account.status == STATUS_PENDING && account.active_at <= now)
            .map(|account| account.id.clone())
            .collect();
        for id in due {
            if let Some(account) = self.program_management_accounts.get_mut(&id) {
                account.status = STATUS_ACTIVE.to_string();
            }
        }
    }

    /// A fresh id of `prefix`'s kind, matching the model's `^<prefix>[a-z0-9]{13}$`:
    /// the leading hex of a digest over this store's counter and the current clock
    /// (hex digits are `[0-9a-f]`, a subset of `[a-z0-9]`), retried in the
    /// astronomically unlikely event of a collision with a live resource.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        loop {
            self.id_sequence += 1;
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let digest = Sha256::digest(format!("{prefix}{}:{nanos}", self.id_sequence).as_bytes());
            let suffix: String = hex::encode(digest).chars().take(ID_SUFFIX_LEN).collect();
            let candidate = format!("{prefix}{suffix}");
            let taken = self.program_management_accounts.contains(&candidate)
                || self.relationships.contains(&candidate);
            if !taken {
                return candidate;
            }
        }
    }

    /// What the ledger says about `token` for `operation` with
    /// `request_fingerprint`. An absent token is always [`TokenOutcome::Fresh`]: a
    /// caller that sends no token asks for no replay.
    pub fn classify_token(
        &self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
    ) -> TokenOutcome {
        let Some(token) = token else {
            return TokenOutcome::Fresh;
        };
        match self.idempotency.get(token) {
            None => TokenOutcome::Fresh,
            // Past its replay window: the token is as good as unseen.
            Some(record)
                if record
                    .recorded_at
                    .elapsed()
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL) =>
            {
                TokenOutcome::Fresh
            }
            Some(record) if record.operation != operation => TokenOutcome::TokenInUse,
            Some(record) if record.request_fingerprint != request_fingerprint => {
                TokenOutcome::ParameterMismatch
            }
            Some(record) => TokenOutcome::Replay(record.clone()),
        }
    }

    /// Record that `token` was used by `operation` for `request_fingerprint`. A
    /// no-op when the caller sent no token.
    pub fn record_token(
        &mut self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
        resource_id: Option<String>,
    ) {
        let Some(token) = token else { return };
        let now = SystemTime::now();
        // Drop what no retry can still ask about, then make room for this entry.
        self.evict_expired_tokens(now);
        if self
            .idempotency
            .insert(
                token.to_string(),
                IdempotencyRecord {
                    operation,
                    request_fingerprint: request_fingerprint.to_string(),
                    resource_id,
                    recorded_at: now,
                },
            )
            .is_none()
        {
            self.idempotency_order.push_back(token.to_string());
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Forget every token past its [`IDEMPOTENCY_TTL`] window.
    fn evict_expired_tokens(&mut self, now: SystemTime) {
        while let Some(oldest) = self.idempotency_order.front() {
            let expired = self.idempotency.get(oldest).is_none_or(|record| {
                now.duration_since(record.recorded_at)
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL)
            });
            if !expired {
                return;
            }
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }
}

/// The equality filters `ListRelationships` accepts, already resolved: each is
/// `None` when the request omitted it (no constraint) and `Some(values)`
/// otherwise. Built by the handler and carried here so the matching rule and the
/// token fingerprint read the same thing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RelationshipFilters {
    pub catalog: String,
    pub associated_account_ids: Option<Vec<String>>,
    pub association_types: Option<Vec<String>>,
    pub display_names: Option<Vec<String>>,
    /// Already resolved from identifiers (bare id or ARN) to bare `pma-…` ids.
    pub program_management_account_ids: Option<Vec<String>>,
}

impl RelationshipFilters {
    /// Does `relationship` satisfy every filter?
    ///
    /// The catalog scopes the list (a relationship lives in exactly one catalog),
    /// and each remaining filter matches when the relationship's corresponding
    /// field equals any of that filter's values; several filters are ANDed. An
    /// omitted filter constrains nothing.
    pub fn matches(&self, relationship: &Relationship) -> bool {
        fn any_equals(values: &Option<Vec<String>>, field: &str) -> bool {
            values
                .as_ref()
                .is_none_or(|values| values.iter().any(|value| value == field))
        }

        relationship.catalog == self.catalog
            && any_equals(
                &self.associated_account_ids,
                &relationship.associated_account_id,
            )
            && any_equals(&self.association_types, &relationship.association_type)
            && any_equals(&self.display_names, &relationship.display_name)
            && any_equals(
                &self.program_management_account_ids,
                &relationship.program_management_account_id,
            )
    }
}

/// Where a page of a paginated list resumed from: how far into the filtered
/// sequence the previous page stopped, and a fingerprint of the filters it was
/// taken with, so resuming with different filters is refused instead of being
/// answered with a page of a different result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub offset: usize,
    pub filter_fingerprint: String,
}

/// Encode a cursor as the opaque `nextToken` the model describes (`NextToken`,
/// len 1..2048, pattern `^[^\x00-\x1F\x7F\x20]*$`). Hex of
/// `<offset>:<fingerprint>` satisfies that pattern and carries the cursor itself,
/// so a page token needs no server-side entry and a read never writes.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    hex::encode(format!("{}:{}", cursor.offset, cursor.filter_fingerprint))
}

/// Decode a `nextToken` produced by [`encode_page_token`]. `None` when the token
/// was not produced by this service.
pub fn decode_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (offset, fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        filter_fingerprint: fingerprint.to_string(),
    })
}

/// The region a request was signed for, from its SigV4 credential scope.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, and for a scope whose
/// region segment could not appear in an ARN this service mints (the modelled ARN
/// patterns allow `[a-z0-9-]+` there).
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralchannelPartnercentralchannelState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralchannelPartnercentralchannelState>)
}
