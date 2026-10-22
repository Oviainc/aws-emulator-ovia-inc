// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ram-amazonresourcesharing service.
//!
//! The service's primary resource is the **resource share**. Every share is stored
//! exactly once, keyed by its ARN (`arn:aws:ram:<region>:<account>:resource-share/<id>`),
//! and every association between a share and an entity — a resource, a principal or
//! a source — is stored exactly once beside it, keyed by
//! `(share arn, association type, associated entity)`. Each operation reads and
//! writes those two maps, so what a create or an update stored is exactly what a
//! later read reports.
//!
//! Both maps are `BTreeMap`s: the key order is total and stable, so a paginated read
//! can name the next entry in an opaque `nextToken` and a page boundary survives
//! later writes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ResourceShare, ResourceShareAssociation, ResourceShareConfiguration, Tag, TagFilter,
};

/// This service's ARN namespace and sigv4 signing name (`awsService.arnNamespace`:
/// `smithy_query(query="service_info")` reports `ram`).
pub const ARN_SERVICE: &str = "ram";

/// The resource portion a resource-share ARN carries before its id.
pub const SHARE_ARN_RESOURCE_PREFIX: &str = "resource-share/";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `allowExternalPrincipals` when `CreateResourceShare` omits it.
///
/// From that member's own documentation: "The default value is true."
pub const DEFAULT_ALLOW_EXTERNAL_PRINCIPALS: bool = true;

/// `ResourceShareConfiguration$retainSharingOnAccountLeaveOrganization` when the
/// create request names no configuration, or names one that leaves the member out.
///
/// That member "specifies whether the consumer account retains access to the
/// resource share after leaving the organization" (its own documentation); a share
/// gets no such retention unless it is asked for, so the service default is false.
/// A share's configuration is part of its record either way: RAM reports the
/// structure on every `ResourceShare` it returns, with the defaults for a share
/// created without an explicit one.
pub const DEFAULT_RETAIN_SHARING_ON_ACCOUNT_LEAVE_ORGANIZATION: bool = false;

/// `ResourceShareConfiguration$exclusiveAccountAccess` when the create request does
/// not name it.
///
/// Access to a share is not exclusive to the accounts it is shared with unless that
/// is asked for, so the service default is false — the value RAM reports for a share
/// created without an explicit configuration.
pub const DEFAULT_EXCLUSIVE_ACCOUNT_ACCESS: bool = false;

/// The `featureSet` of a share created through the RAM API.
///
/// From the modelled `ResourceShare$featureSet` documentation: `STANDARD` is "a
/// resource share that supports all functionality … might have been created by RAM";
/// the other two values (`CREATED_FROM_POLICY`, `PROMOTING_TO_STANDARD`) describe a
/// share the customer created by attaching a resource-based policy, which no
/// operation in this crate can produce.
pub const FEATURE_SET_STANDARD: &str = "STANDARD";

/// `ResourceShareStatus` values this slice stores, from
/// `smithy_query(query="shape",shape="ResourceShareStatus")`.
pub const SHARE_STATUS_ACTIVE: &str = "ACTIVE";
/// A share `DeleteResourceShare` has removed: it keeps its record as a tombstone so
/// the ARN can never be reused and so a read that asks for `DELETED` can see it.
pub const SHARE_STATUS_DELETED: &str = "DELETED";

/// `ResourceShareAssociationStatus` values this slice stores, from
/// `smithy_query(query="shape",shape="ResourceShareAssociationStatus")`.
pub const ASSOCIATION_STATUS_ASSOCIATING: &str = "ASSOCIATING";
/// The status an association reaches once the move RAM owes it falls due.
pub const ASSOCIATION_STATUS_ASSOCIATED: &str = "ASSOCIATED";
/// The status an association that is being torn down carries while the teardown is
/// still owed.
pub const ASSOCIATION_STATUS_DISASSOCIATING: &str = "DISASSOCIATING";
/// The terminal status of an association RAM has torn down. The record survives in
/// it, so a read still reports the association and a caller polling for the teardown
/// to finish sees it complete.
pub const ASSOCIATION_STATUS_DISASSOCIATED: &str = "DISASSOCIATED";

/// The message RAM reports on a principal association that has not settled yet.
///
/// RAM invites the principal to the share, and the principal accepts or rejects the
/// invitation, so what a pending principal association is waiting for is that
/// answer. The member that reports it is the modelled
/// `ResourceShareAssociation$statusMessage` ("A message about the status of the
/// association"); AWS publishes no wording, so this sentence is this emulator's and
/// the RULE — a principal association that has not completed says what it is waiting
/// for, and a settled one reports nothing — is what is pinned.
pub const ASSOCIATING_PRINCIPAL_STATUS_MESSAGE: &str =
    "Waiting for the principal to either accept or reject the resource share invitation.";

/// `ResourceShareAssociationType` values, from
/// `smithy_query(query="shape",shape="ResourceShareAssociationType")`.
pub const ASSOCIATION_TYPE_RESOURCE: &str = "RESOURCE";
/// The association type of an entry that came from `principals`.
pub const ASSOCIATION_TYPE_PRINCIPAL: &str = "PRINCIPAL";
/// The association type of an entry that came from `sources`.
pub const ASSOCIATION_TYPE_SOURCE: &str = "SOURCE";

/// `ResourceOwner` values, from `smithy_query(query="shape",shape="ResourceOwner")`.
pub const RESOURCE_OWNER_SELF: &str = "SELF";
/// Shares another account shares with this one — none, in this emulator, because
/// every share it holds was created locally by the account that owns it.
pub const RESOURCE_OWNER_OTHER_ACCOUNTS: &str = "OTHER-ACCOUNTS";

/// How long after the call that creates an association RAM takes to move it from
/// `ASSOCIATING` to `ASSOCIATED`.
///
/// RAM answers `AssociateResourceShare` with the new associations in the
/// non-terminal `ASSOCIATING` status (the status the modelled
/// `ResourceShareAssociationStatus` enum carries for an association still being
/// made), so reaching `ASSOCIATED` is a transition the clock owes rather than
/// something a caller asks for. AWS publishes no duration for it, so this span is
/// this emulator's own choice; it is deliberately not zero, so a caller that reads
/// straight back still observes `ASSOCIATING`. Nothing sleeps for it: the due
/// instant is stored and the move is materialised by the first call at or after it.
pub const ASSOCIATION_READY_AFTER_MILLIS: i64 = 500;

/// How long after the call that tears an association down RAM takes to move it from
/// `DISASSOCIATING` to `DISASSOCIATED`.
///
/// The mirror of [`ASSOCIATION_READY_AFTER_MILLIS`], and owed on the same terms: a
/// teardown that has something to revoke is answered in the non-terminal
/// `DISASSOCIATING` status, so reaching `DISASSOCIATED` is a move the clock owes
/// rather than something a caller asks for. AWS publishes no duration for it either,
/// so this span is this emulator's own choice; it is deliberately not zero, and
/// nothing sleeps for it.
pub const DISASSOCIATION_READY_AFTER_MILLIS: i64 = 500;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteResourceShare`, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. A cap is RAM's own published limit
// where it publishes one; where it publishes none the constant says so and the
// value is this emulator's resource guard, set well above any realistic use.
// ---------------------------------------------------------------------------

/// The most resource shares this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. A create that would exceed it is
/// refused with the modelled `ResourceShareLimitExceededException`.
pub const MAX_RESOURCE_SHARES: usize = 500;

/// The most entities one call may associate.
///
/// From the `ResourceShareLimitExceededException` documentation: "You can associate
/// up to 100 resources per call."
pub const MAX_ENTITIES_PER_CALL: usize = 100;

/// The most associations one share may hold, across all three association types.
///
/// Emulator resource guard, NOT an AWS quota.
pub const MAX_ASSOCIATIONS_PER_SHARE: usize = 1000;

/// The most tags one resource share may carry.
///
/// RAM follows the Amazon Web Services tagging limit of 50 tags per resource; a
/// create that would exceed it is refused with the modelled
/// `TagLimitExceededException`.
pub const MAX_TAGS_PER_SHARE: usize = 50;

/// The longest `name`, `clientToken` or tag key/value this emulator stores. The
/// model publishes no length for any of them, so this is the emulator's guard; a
/// longer value is refused rather than stored.
pub const MAX_NAME_LEN: usize = 256;

/// The longest ARN or principal/source identifier this emulator stores: the AWS ARN
/// length limit.
pub const MAX_ARN_LEN: usize = 2048;

/// The most `(operation, clientToken)` pairs this emulator remembers for
/// idempotency. Emulator resource guard: the oldest entry is forgotten when a new
/// one arrives at the cap, so a long-running emulator cannot grow without bound.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// The largest page any paginated read here builds, and the size it uses when the
/// request omits `maxResults`: the maximum of the modelled `MaxResults` range
/// (1..500).
pub const MAX_PAGE_SIZE: usize = 500;

/// The tag-key prefix AWS reserves for its own tags; a caller may not set one.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Separates the fields of a composite page cursor or fingerprint input. A control
/// character, so it cannot occur inside an ARN, a name or a principal id.
const FIELD_SEPARATOR: char = '\u{1}';

/// How an association is keyed: `(resource share ARN, association type, associated
/// entity)`. One entity is associated with one share under one type at most once, so
/// this key is the association's identity as well as its page order.
pub type AssociationKey = (String, String, String);

/// One stored resource share: every member `ResourceShare` reports, plus the
/// permission ARNs the create request named.
#[derive(Debug, Clone, Default)]
pub struct ShareRecord {
    /// The share's ARN: this record's store key.
    pub arn: String,
    pub name: String,
    pub owning_account_id: String,
    pub allow_external_principals: bool,
    pub status: String,
    pub status_message: Option<String>,
    /// Tags keyed by tag key, so a key is carried at most once and `tags` reports a
    /// stable order.
    pub tags: BTreeMap<String, String>,
    /// Epoch milliseconds; rendered as the modelled `DateTime` timestamps.
    pub creation_time: i64,
    pub last_updated_time: i64,
    pub feature_set: String,
    /// The managed-permission ARNs the create request named, which
    /// `GetResourceShares`' `permissionArn` filter reads.
    pub permission_arns: Vec<String>,
    /// The share's configuration, always stored: a create that names none stores
    /// the service defaults, so every read reports the structure.
    pub resource_share_configuration: ResourceShareConfiguration,
}

impl ShareRecord {
    /// This record as the modelled `ResourceShare` wire shape.
    pub fn view(&self) -> ResourceShare {
        ResourceShare {
            resource_share_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            owning_account_id: Some(self.owning_account_id.clone()),
            allow_external_principals: Some(self.allow_external_principals),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            // `tags` reports the share's tag set, which is a list that may be
            // empty: a share carrying no tags reports `[]`, so a caller can tell
            // "no tags" from "tags not reported".
            tags: Some(tag_list(&self.tags)),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated_time: Some(to_date_time(self.last_updated_time)),
            feature_set: Some(self.feature_set.clone()),
            resource_share_configuration: Some(self.resource_share_configuration.clone()),
        }
    }

    /// Whether `DeleteResourceShare` has already removed this share.
    pub fn is_deleted(&self) -> bool {
        self.status == SHARE_STATUS_DELETED
    }

    /// Attach `tags` to this share, replacing the value of a key it already
    /// carries, and answer whether the stored tag set changed.
    ///
    /// Call it only after [`tags_rejection`] has passed, which is what guarantees
    /// every pair has a key.
    pub fn merge_tags(&mut self, tags: &[Tag]) -> bool {
        let mut changed = false;
        for (key, value) in tag_map(tags) {
            if self.tags.insert(key, value.clone()).as_ref() != Some(&value) {
                changed = true;
            }
        }
        changed
    }

    /// Detach every key in `keys` from this share, and answer the keys that were
    /// actually carried and so were removed, in key order. A key the share does not
    /// carry is not an error: removing it leaves the share exactly as it was, and it
    /// is not reported as removed — which is what lets a caller tell an untag that
    /// was a no-op from one that detached a tag.
    pub fn remove_tags(&mut self, keys: &[String]) -> Vec<String> {
        // Every key is removed, not just up to the first that was present, so this
        // collects the removals rather than short-circuiting on one. A key named
        // twice is removed once and reported once.
        keys.iter()
            .filter(|key| self.tags.remove(*key).is_some())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// A status move the clock owes a record: the status it reaches, and the instant it
/// falls due.
///
/// RAM answers the call that starts the move with the record in a NON-TERMINAL status
/// (`ASSOCIATING` while an association is being made, `DISASSOCIATING` while one is
/// being torn down), so reaching the terminal status is something the clock owes and
/// not something a caller asks for. The due instant is stored and nothing sleeps for
/// it: [`ComAmazonawsRamAmazonresourcesharingState::settle_due_associations`]
/// materialises the move on the first call at or after it.
#[derive(Debug, Clone)]
pub struct OwedMove {
    /// The status the record reaches when the move falls due.
    pub status: String,
    /// When it falls due, in epoch milliseconds.
    pub due_at: i64,
}

/// One stored association between a resource share and an entity.
#[derive(Debug, Clone, Default)]
pub struct AssociationRecord {
    pub resource_share_arn: String,
    pub resource_share_name: String,
    pub associated_entity: String,
    pub association_type: String,
    pub status: String,
    pub status_message: Option<String>,
    /// Epoch milliseconds, as on [`ShareRecord`].
    pub creation_time: i64,
    pub last_updated_time: i64,
    pub external: bool,
    /// The move the clock still owes this record, or `None` when its status is
    /// terminal and nothing further is owed.
    pub owed_move: Option<OwedMove>,
}

impl AssociationRecord {
    /// This record as the modelled `ResourceShareAssociation` wire shape.
    pub fn view(&self) -> ResourceShareAssociation {
        ResourceShareAssociation {
            resource_share_arn: Some(self.resource_share_arn.clone()),
            resource_share_name: Some(self.resource_share_name.clone()),
            associated_entity: Some(self.associated_entity.clone()),
            association_type: Some(self.association_type.clone()),
            status: Some(self.status.clone()),
            // A message about the status only belongs on a record whose status is
            // still moving: a terminal association is waiting for nothing, so
            // whatever it was waiting for no longer applies and the member is
            // dropped. The store clears the message when it makes the move, and this
            // is the same rule at the serializer, so no read can report a stale one.
            status_message: self.status_message.clone().filter(|_| !self.is_terminal()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated_time: Some(to_date_time(self.last_updated_time)),
            external: Some(self.external),
        }
    }

    /// Whether this record's status is one no further move is owed from.
    pub fn is_terminal(&self) -> bool {
        association_status_is_terminal(&self.status)
    }
}

/// What one `(operation, clientToken)` pair already did, so repeating the request
/// repeats the answer instead of acting twice.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// A digest of every other parameter of the first call that used this token.
    /// A later call with the same token and a different digest is the
    /// `IdempotentParameterMismatchException` case.
    pub fingerprint: String,
    /// The resource share the first call acted on, so the repeat can answer from
    /// the store rather than from a cached body.
    pub resource_share_arn: String,
}

/// Per-service state for com-amazonaws-ram-amazonresourcesharing. A handler reaches
/// it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRamAmazonresourcesharingState {
    /// Every resource share, live or tombstoned, keyed by its ARN.
    pub resource_shares: BTreeMap<String, ShareRecord>,
    /// Every association, keyed by [`AssociationKey`].
    pub associations: BTreeMap<AssociationKey, AssociationRecord>,
    /// `(operation, clientToken)` -> what that token already did.
    client_tokens: HashMap<(String, String), ClientTokenRecord>,
    /// The order tokens were remembered in, so the oldest can be forgotten at the
    /// cap.
    client_token_order: VecDeque<(String, String)>,
    /// Mixed into each generated id so two calls in the same nanosecond differ.
    id_counter: u64,
    /// The last instant handed out by [`Self::tick`].
    clock: i64,
}

impl ComAmazonawsRamAmazonresourcesharingState {
    /// This request's instant, in epoch milliseconds.
    ///
    /// The wall clock, nudged forward by a millisecond when two calls land inside
    /// the same one, so the `lastUpdatedTime` an update stores is always strictly
    /// after the one it replaced and the order of two writes is observable.
    pub fn tick(&mut self) -> i64 {
        self.clock = now_millis().max(self.clock + 1);
        self.clock
    }

    /// Make every association move the clock owed at or before `now`.
    ///
    /// This is the only write a reporting operation performs, and it only
    /// materialises what was already due: an association RAM accepted in
    /// `ASSOCIATING` reaches `ASSOCIATED` [`ASSOCIATION_READY_AFTER_MILLIS`] after
    /// the call that created it, and one it accepted in `DISASSOCIATING` reaches
    /// `DISASSOCIATED` [`DISASSOCIATION_READY_AFTER_MILLIS`] after the call that
    /// started the teardown — whether or not anybody asks.
    pub fn settle_due_associations(&mut self, now: i64) {
        for record in self.associations.values_mut() {
            let Some(owed) = record.owed_move.clone() else {
                continue;
            };
            if owed.due_at > now {
                continue;
            }
            record.status = owed.status;
            // A record that has reached its terminal status is waiting for nothing,
            // so it reports no status message.
            record.status_message = None;
            // The move happened when it fell due, not when somebody noticed.
            record.last_updated_time = owed.due_at;
            record.owed_move = None;
        }
    }

    /// The share `arn` names, whether live or tombstoned.
    pub fn share(&self, arn: &str) -> Option<&ShareRecord> {
        self.resource_shares.get(arn)
    }

    /// The share `arn` names, for an operation the spec says acts on it. A share
    /// `DeleteResourceShare` has removed is gone as far as those operations are
    /// concerned, so this answers `None` for one — the `UnknownResourceException`
    /// case.
    pub fn live_share(&self, arn: &str) -> Option<&ShareRecord> {
        self.resource_shares
            .get(arn)
            .filter(|share| !share.is_deleted())
    }

    /// [`Self::live_share`] for a caller that needs to change the record.
    pub fn live_share_mut(&mut self, arn: &str) -> Option<&mut ShareRecord> {
        self.resource_shares
            .get_mut(arn)
            .filter(|share| !share.is_deleted())
    }

    /// Store a newly created share.
    pub fn insert_share(&mut self, share: ShareRecord) {
        self.resource_shares.insert(share.arn.clone(), share);
    }

    /// How many associations the share `arn` holds, which the per-share cap counts.
    pub fn association_count(&self, arn: &str) -> usize {
        self.associations
            .keys()
            .filter(|(share_arn, _, _)| share_arn == arn)
            .count()
    }

    /// Associate `entity` with the share under `association_type`, returning the
    /// record as it now stands.
    ///
    /// A new association starts in the non-terminal `ASSOCIATING` status, and a
    /// principal association says in its `statusMessage` that it is waiting for the
    /// invitation to be answered ([`associating_status_message`]). The move to
    /// `ASSOCIATED` is one the clock owes, [`ASSOCIATION_READY_AFTER_MILLIS`] after
    /// this call; [`Self::settle_due_associations`] materialises it and drops the
    /// message with it.
    ///
    /// An entity already on the attach path — `ASSOCIATING` or `ASSOCIATED` — is left
    /// exactly as it is: re-associating it changes nothing, so neither its status nor
    /// its timestamps move, except for the share's name, which an update may have
    /// changed since. An entity whose association was torn down (or whose teardown is
    /// still in flight) is associated AGAIN: the record restarts the attach, so the
    /// status leaves `DISASSOCIATED`/`DISASSOCIATING`, the owed teardown is dropped
    /// and the record is dated from this call.
    pub fn associate(
        &mut self,
        share: &ShareRecord,
        association_type: &str,
        entity: &str,
        now: i64,
    ) -> AssociationRecord {
        let key = (
            share.arn.clone(),
            association_type.to_string(),
            entity.to_string(),
        );
        let attaching = |record: &AssociationRecord| {
            record.status == ASSOCIATION_STATUS_ASSOCIATING
                || record.status == ASSOCIATION_STATUS_ASSOCIATED
        };
        let record = self
            .associations
            .entry(key)
            .or_insert_with(|| AssociationRecord {
                resource_share_arn: share.arn.clone(),
                resource_share_name: share.name.clone(),
                associated_entity: entity.to_string(),
                association_type: association_type.to_string(),
                status: String::new(),
                status_message: None,
                creation_time: now,
                last_updated_time: now,
                external: entity_is_external(&share.owning_account_id, entity),
                owed_move: None,
            });
        if !attaching(record) {
            // Either the record was just created, or it had left the attach path; in
            // both cases this call is what associates the entity, so the attach
            // starts here.
            record.status = ASSOCIATION_STATUS_ASSOCIATING.to_string();
            record.status_message = associating_status_message(association_type).map(str::to_string);
            record.creation_time = now;
            record.last_updated_time = now;
            record.external = entity_is_external(&share.owning_account_id, entity);
            record.owed_move = Some(OwedMove {
                status: ASSOCIATION_STATUS_ASSOCIATED.to_string(),
                due_at: now + ASSOCIATION_READY_AFTER_MILLIS,
            });
        }
        record.resource_share_name = share.name.clone();
        record.clone()
    }

    /// Tear the association between the share `arn` and `entity` under
    /// `association_type` down, returning the record as it now stands, or `None` when
    /// the share holds no such association.
    ///
    /// The record SURVIVES the teardown in a terminal status, so a read still reports
    /// the association and a caller polling for the teardown sees it complete. Which
    /// status the call answers with follows from what there was to revoke:
    ///
    /// * an association that had reached `ASSOCIATED` was granting access, so
    ///   revoking it is work the service has to do: the record goes to the
    ///   non-terminal `DISASSOCIATING` and the clock owes `DISASSOCIATED`
    ///   [`DISASSOCIATION_READY_AFTER_MILLIS`] later;
    /// * an association still on the way in (`ASSOCIATING` — a principal that never
    ///   answered its invitation, say) was granting nothing, so there is nothing to
    ///   revoke and the record is terminal at once, in `DISASSOCIATED`;
    /// * a record already torn down, or already tearing down, is left exactly as it
    ///   is, so repeating the call neither restarts the move nor moves the stamps.
    ///
    /// A terminal record reports no `statusMessage`: whatever it was waiting for no
    /// longer applies.
    pub fn disassociate(
        &mut self,
        arn: &str,
        association_type: &str,
        entity: &str,
        now: i64,
    ) -> Option<AssociationRecord> {
        let key = (
            arn.to_string(),
            association_type.to_string(),
            entity.to_string(),
        );
        let record = self.associations.get_mut(&key)?;
        if record.status == ASSOCIATION_STATUS_DISASSOCIATING
            || record.status == ASSOCIATION_STATUS_DISASSOCIATED
        {
            return Some(record.clone());
        }
        let granted = record.status == ASSOCIATION_STATUS_ASSOCIATED;
        record.status_message = None;
        record.last_updated_time = now;
        if granted {
            record.status = ASSOCIATION_STATUS_DISASSOCIATING.to_string();
            record.owed_move = Some(OwedMove {
                status: ASSOCIATION_STATUS_DISASSOCIATED.to_string(),
                due_at: now + DISASSOCIATION_READY_AFTER_MILLIS,
            });
        } else {
            record.status = ASSOCIATION_STATUS_DISASSOCIATED.to_string();
            record.owed_move = None;
        }
        Some(record.clone())
    }

    /// Carry a share's new name onto its associations, which each report
    /// `resourceShareName`.
    pub fn rename_associations(&mut self, arn: &str, name: &str) {
        for (key, record) in &mut self.associations {
            if key.0 == arn {
                record.resource_share_name = name.to_string();
            }
        }
    }

    /// Tombstone the share `arn` names and drop its associations: the share stops
    /// sharing anything, and no later read reports an association of a share that
    /// no longer exists.
    pub fn delete_share(&mut self, arn: &str, now: i64) {
        if let Some(share) = self.resource_shares.get_mut(arn) {
            share.status = SHARE_STATUS_DELETED.to_string();
            share.status_message = None;
            share.last_updated_time = now;
        }
        self.associations.retain(|key, _| key.0 != arn);
    }

    /// What this `(operation, clientToken)` pair already did, if anything.
    pub fn client_token_record(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&ClientTokenRecord> {
        self.client_tokens
            .get(&(operation.to_string(), token.to_string()))
    }

    /// Remember what this `(operation, clientToken)` pair did, forgetting the
    /// oldest remembered pair once the cap is reached.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        token: &str,
        fingerprint: String,
        resource_share_arn: String,
    ) {
        let key = (operation.to_string(), token.to_string());
        if self.client_tokens.contains_key(&key) {
            return;
        }
        while self.client_token_order.len() >= MAX_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
        self.client_token_order.push_back(key.clone());
        self.client_tokens.insert(
            key,
            ClientTokenRecord {
                fingerprint,
                resource_share_arn,
            },
        );
    }

    /// A resource-share ARN in `region` that no stored share is using.
    pub fn fresh_share_arn(&mut self, region: &str) -> String {
        for _ in 0..1024 {
            let candidate = share_arn(region, &self.fresh_uuid());
            if !self.resource_shares.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: there are
        // always more of these than there are stored shares.
        (0u64..=self.resource_shares.len() as u64)
            .map(|n| share_arn(region, &format!("00000000-0000-4000-8000-{n:012x}")))
            .find(|arn| !self.resource_shares.contains_key(arn))
            .unwrap_or_else(|| share_arn(region, "00000000-0000-4000-8000-ffffffffffff"))
    }

    /// A fresh UUID-shaped identifier, used for a share id and for the
    /// `clientToken` RAM generates when the request omits one.
    pub fn fresh_uuid(&mut self) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.id_counter.rotate_left(17);
        let digits: String = (0..4)
            .map(|_| format!("{:08x}", splitmix64(&mut seed) as u32))
            .collect();
        format!(
            "{}-{}-{}-{}-{}",
            &digits[0..8],
            &digits[8..12],
            &digits[12..16],
            &digits[16..20],
            &digits[20..32]
        )
    }
}

/// The configuration to store for a share created with `requested`.
///
/// Every member the request leaves out takes this service's default, so the stored
/// record is complete and a read never has to invent one.
pub fn share_configuration(
    requested: Option<&ResourceShareConfiguration>,
) -> ResourceShareConfiguration {
    ResourceShareConfiguration {
        exclusive_account_access: Some(
            requested
                .and_then(|configuration| configuration.exclusive_account_access)
                .unwrap_or(DEFAULT_EXCLUSIVE_ACCOUNT_ACCESS),
        ),
        retain_sharing_on_account_leave_organization: Some(
            requested
                .and_then(|configuration| {
                    configuration.retain_sharing_on_account_leave_organization
                })
                .unwrap_or(DEFAULT_RETAIN_SHARING_ON_ACCOUNT_LEAVE_ORGANIZATION),
        ),
    }
}

/// The message of the `UnknownResourceException` every operation that takes a
/// `resourceShareArn` renders when no share has it.
///
/// One template across every such operation, built from the ARN the request named,
/// so a caller can read the ARN back out of the message it gets.
pub fn unknown_share_message(arn: &str) -> String {
    format!("ResourceShare {arn} could not be found.")
}

/// The `statusMessage` an association of this type carries while it has not settled
/// yet, or `None` when there is nothing to report.
///
/// RAM shares with a principal by INVITATION: the invited principal accepts or
/// rejects it (the modelled `AcceptResourceShareInvitation` /
/// `RejectResourceShareInvitation` operations, and the invitations
/// `GetResourceShareInvitations` reports), so what a principal association that has
/// not completed is waiting for is that answer, and
/// `ResourceShareAssociation$statusMessage` ("A message about the status of the
/// association") is the member that says so. A resource or a source association
/// names no principal, waits for nobody, and reports no message. A settled
/// association waits for nothing either, so the member is dropped when it reaches
/// `ASSOCIATED`.
pub fn associating_status_message(association_type: &str) -> Option<&'static str> {
    (association_type == ASSOCIATION_TYPE_PRINCIPAL)
        .then_some(ASSOCIATING_PRINCIPAL_STATUS_MESSAGE)
}

/// Whether an association status is TERMINAL: one no further move is owed from.
///
/// Of the modelled `ResourceShareAssociationStatus` values
/// (`smithy_query(query="shape",shape="ResourceShareAssociationStatus")`), the ones
/// that name a settled outcome are `ASSOCIATED`, `DISASSOCIATED` and `FAILED`; the
/// `-ING` ones (`ASSOCIATING`, `DISASSOCIATING`, `SUSPENDING`, `RESTORING`) each name
/// a move still under way. This slice only ever stores the first four of those, but
/// the predicate is written over the enum so it stays right if another is.
pub fn association_status_is_terminal(status: &str) -> bool {
    matches!(
        status,
        ASSOCIATION_STATUS_ASSOCIATED | ASSOCIATION_STATUS_DISASSOCIATED | "FAILED"
    )
}

/// Tags as the wire shape wants them, ordered by key.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// The tags of a create request, keyed by tag key. Call it only after
/// [`tags_rejection`] has passed, which is what guarantees every tag has a key.
pub fn tag_map(tags: &[Tag]) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|tag| {
            let key = tag.key.clone()?;
            Some((key, tag.value.clone().unwrap_or_default()))
        })
        .collect()
}

/// Why a create request's `tags` are unacceptable, or `None` when they are fine.
///
/// Each variant names the modelled error the operation renders it as, because RAM
/// words the three refusals differently.
#[derive(Debug, Clone)]
pub enum TagRejection {
    /// A tag the model admits but the service cannot store —
    /// `InvalidParameterException`.
    Invalid(String),
    /// More tags than one resource may carry — `TagLimitExceededException`.
    LimitExceeded(String),
    /// A tag key AWS reserves — `TagPolicyViolationException`.
    PolicyViolation(String),
}

/// Why a resource share cannot carry `total` tags, or `None` when it can.
///
/// The cap is on the share, not on one request, so an operation that ADDS tags to a
/// share that already has some passes the total it would end up with.
pub fn tag_count_rejection(total: usize) -> Option<String> {
    (total > MAX_TAGS_PER_SHARE).then(|| {
        format!(
            "the resource share would carry {total} tags, more than the \
             {MAX_TAGS_PER_SHARE} a resource share accepts"
        )
    })
}

/// Why `tags` cannot be attached to a resource share, or `None` when they can.
pub fn tags_rejection(tags: &[Tag]) -> Option<TagRejection> {
    if let Some(why) = tag_count_rejection(tags.len()) {
        return Some(TagRejection::LimitExceeded(why));
    }
    for tag in tags {
        // "Every tag must have a key" (modelled `Tag$key` documentation).
        let Some(key) = tag.key.as_deref().filter(|key| !key.is_empty()) else {
            return Some(TagRejection::Invalid(
                "every entry in tags must carry a non-empty key".to_string(),
            ));
        };
        if key.len() > MAX_NAME_LEN {
            return Some(TagRejection::Invalid(format!(
                "tag key '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
                truncate(key)
            )));
        }
        if tag.value.as_deref().unwrap_or_default().len() > MAX_NAME_LEN {
            return Some(TagRejection::Invalid(format!(
                "the value of tag key '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
                truncate(key)
            )));
        }
        if key
            .to_ascii_lowercase()
            .starts_with(RESERVED_TAG_KEY_PREFIX)
        {
            return Some(TagRejection::PolicyViolation(format!(
                "tag key '{}' uses the reserved prefix '{RESERVED_TAG_KEY_PREFIX}'",
                truncate(key)
            )));
        }
    }
    None
}

/// Why `tagKeys` cannot be detached from a resource share, or `None` when they can.
///
/// Rendered as `InvalidParameterException`: the same shape rules the keys of a
/// `TagList` must satisfy, applied to the bare keys an untag names.
pub fn tag_keys_rejection(keys: &[String]) -> Option<String> {
    keys.iter()
        .find(|key| key.is_empty() || key.len() > MAX_NAME_LEN)
        .map(|key| {
            if key.is_empty() {
                "every entry in tagKeys must be a non-empty key".to_string()
            } else {
                format!(
                    "tag key '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
                    truncate(key)
                )
            }
        })
}

/// Whether `tags` satisfies `filters`.
///
/// A filter matches a share that carries its key; when the filter also lists
/// values, the share's value for that key must be one of them. Several filters are
/// evaluated as an OR condition, which the modelled `TagFilter` documentation
/// states; no filters at all match everything.
pub fn tags_match_filters(tags: &BTreeMap<String, String>, filters: &[TagFilter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    filters.iter().any(|filter| {
        let Some(key) = filter.tag_key.as_deref() else {
            return false;
        };
        let Some(value) = tags.get(key) else {
            return false;
        };
        // "If no values are provided, then the filter matches any tag with the
        // specified key, regardless of its value" (modelled `TagFilter$tagValues`).
        filter.tag_values.as_deref().is_none_or(|candidates| {
            candidates.is_empty() || candidates.iter().any(|candidate| candidate == value)
        })
    })
}

/// Whether `value` is a syntactically valid ARN: the six colon-separated fields
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, with a partition, a
/// service and a resource.
pub fn is_arn(value: &str) -> bool {
    if value.len() > MAX_ARN_LEN {
        return false;
    }
    let mut fields = value.splitn(6, ':');
    let Some("arn") = fields.next() else {
        return false;
    };
    let (Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The account field of an ARN, or `None` when `value` is not one.
pub fn arn_account(value: &str) -> Option<&str> {
    if !is_arn(value) {
        return None;
    }
    value.split(':').nth(4)
}

/// Whether `value` is an ARN of THIS service's resource-share resource.
pub fn is_share_arn(value: &str) -> bool {
    if !is_arn(value) {
        return false;
    }
    let mut fields = value.splitn(6, ':');
    let (service, resource) = (fields.nth(2), fields.nth(2));
    service == Some(ARN_SERVICE)
        && resource.and_then(|resource| resource.strip_prefix(SHARE_ARN_RESOURCE_PREFIX))
            .is_some_and(|id| !id.is_empty())
}

/// The ARN of the resource share `id` in `region`.
pub fn share_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{SHARE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// Whether an associated entity belongs to an account other than `owner`.
///
/// `ResourceShareAssociation$external` reports whether the entity sits outside the
/// account that owns the share, so an entity that names that account — a bare
/// account id equal to it, or an ARN whose account field is it — is internal and
/// everything else is external.
pub fn entity_is_external(owner: &str, entity: &str) -> bool {
    match arn_account(entity) {
        Some(account) => account != owner,
        None => entity != owner,
    }
}

/// Why `arn` is not usable as a `resourceShareArn`, or `None` when it is.
///
/// A value that is not an ARN of this service's resource-share resource is
/// malformed, whether or not any share has it — the
/// `MalformedArnException` case, as distinct from the `UnknownResourceException` one.
pub fn share_arn_rejection(member: &str, arn: &str) -> Option<String> {
    (!is_share_arn(arn)).then(|| {
        format!(
            "{member} '{}' is not a resource share ARN of the form \
             arn:<partition>:{ARN_SERVICE}:<region>:<account>:{SHARE_ARN_RESOURCE_PREFIX}<id>",
            truncate(arn)
        )
    })
}

/// Why an entry of `resourceArns` (or a `resourceArn` filter) is not usable, or
/// `None` when every entry is an ARN.
pub fn resource_arn_rejection(member: &str, arns: &[String]) -> Option<String> {
    arns.iter().find(|arn| !is_arn(arn)).map(|arn| {
        format!(
            "{member} '{}' is not an ARN of the form arn:<partition>:<service>:<region>:<account>:<resource>",
            truncate(arn)
        )
    })
}

/// Why a list of entities to associate is too long for one call, or `None`.
///
/// Rendered as `ResourceShareLimitExceededException`.
pub fn entity_count_rejection(member: &str, count: usize) -> Option<String> {
    (count > MAX_ENTITIES_PER_CALL).then(|| {
        format!(
            "{member} names {count} entities, more than the {MAX_ENTITIES_PER_CALL} this service associates in one call"
        )
    })
}

/// Why an entry of a principal or source list cannot be stored, or `None`.
///
/// RAM accepts several forms here — an account id, an ARN, an organization id or
/// path, a service principal name — so the only rule applied is that an entry is
/// present and fits the store.
pub fn entity_value_rejection(member: &str, entities: &[String]) -> Option<String> {
    entities
        .iter()
        .find(|entity| entity.is_empty() || entity.len() > MAX_ARN_LEN)
        .map(|entity| {
            if entity.is_empty() {
                format!("{member} must not contain an empty value")
            } else {
                format!(
                    "{member} entry '{}' is longer than the {MAX_ARN_LEN} characters this service accepts",
                    truncate(entity)
                )
            }
        })
}

/// Why a resource share `name` cannot be stored, or `None` when it can.
pub fn name_rejection(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("name must not be empty".to_string());
    }
    (name.len() > MAX_NAME_LEN).then(|| {
        format!(
            "name '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
            truncate(name)
        )
    })
}

/// Why a `clientToken` is not a valid idempotency identifier, or `None`.
///
/// Rendered as `InvalidClientTokenException`. The model publishes no shape for the
/// token, so the rule is the weakest one that keeps it storable: present, and
/// within the store's bound.
pub fn client_token_rejection(token: &str) -> Option<String> {
    if token.is_empty() {
        return Some("clientToken must not be empty".to_string());
    }
    (token.len() > MAX_NAME_LEN).then(|| {
        format!(
            "clientToken is longer than the {MAX_NAME_LEN} characters this service accepts"
        )
    })
}

/// The page size this read uses: whatever the request asked for, clamped into
/// 1..=[`MAX_PAGE_SIZE`], so the page a handler allocates is sized by the server.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.map_or(MAX_PAGE_SIZE, |requested| {
        requested.clamp(1, MAX_PAGE_SIZE as i32) as usize
    })
}

/// A `nextToken` naming the resource share the next page starts at.
///
/// `nextToken` is modelled opaque, so the ARN is hex-encoded rather than handed
/// back in the clear — opaque to a caller, not secret.
pub fn encode_share_cursor(arn: &str) -> String {
    hex::encode(arn)
}

/// The share ARN a `nextToken` names, or `None` when the token is not one this
/// service issued — the `InvalidNextTokenException` case.
pub fn decode_share_cursor(token: &str) -> Option<String> {
    decode_cursor(token).filter(|arn| is_share_arn(arn))
}

/// A `nextToken` naming the association the next page starts at.
pub fn encode_association_cursor(key: &AssociationKey) -> String {
    hex::encode(format!(
        "{}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}{}",
        key.0, key.1, key.2
    ))
}

/// The association key a `nextToken` names, or `None` when the token is not one
/// this service issued.
pub fn decode_association_cursor(token: &str) -> Option<AssociationKey> {
    let decoded = decode_cursor(token)?;
    let mut fields = decoded.split(FIELD_SEPARATOR);
    let (Some(arn), Some(association_type), Some(entity), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if !is_share_arn(arn) || !crate::ops::types::RESOURCE_SHARE_ASSOCIATION_TYPE_VALUES
        .contains(&association_type)
    {
        return None;
    }
    Some((
        arn.to_string(),
        association_type.to_string(),
        entity.to_string(),
    ))
}

fn decode_cursor(token: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// A digest of the parameters of one idempotent call, so a later call with the same
/// `clientToken` can be told apart from a repeat of the same request.
///
/// Each field is length-prefixed, so no two different parameter lists can produce
/// the same input to the hash.
pub fn fingerprint(fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// One list parameter, rendered for [`fingerprint`]: sorted, so the same entities
/// in another order are the same request, as RAM's own set semantics imply.
pub fn fingerprint_list(values: &[String]) -> String {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.join(&FIELD_SEPARATOR.to_string())
}

/// One map parameter (a share's tags), rendered for [`fingerprint`] in key order.
pub fn fingerprint_map(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{key}{FIELD_SEPARATOR}{value}"))
        .collect::<Vec<_>>()
        .join(&FIELD_SEPARATOR.to_string())
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not a plain region token is refused here and the default is
/// used instead.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Epoch milliseconds as the modelled `DateTime` timestamps carry them.
pub fn to_date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Now, in milliseconds since the Unix epoch.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Caller text, shortened for an error message: a refusal names the value it
/// refused, but never a whole 2 KiB ARN.
fn truncate(value: &str) -> String {
    const LIMIT: usize = 64;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }
    let head: String = value.chars().take(LIMIT).collect();
    format!("{head}…")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRamAmazonresourcesharingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRamAmazonresourcesharingState>)
}
