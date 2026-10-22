// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codegurureviewer-awsgurufrontendservice service.
//!
//! The store holds the two primary resources of CodeGuru Reviewer's control plane —
//! repository associations and code reviews — plus the indexes that keep every operation's
//! work proportional to what it answers rather than to how much a caller has stored. Every
//! handler reads and writes THIS struct through [`service_state`]; nothing in `src/ops/`
//! keeps a second copy.
//!
//! # Bounds
//!
//! Both record maps are capacity-capped ([`MAX_ASSOCIATIONS`], [`MAX_CODE_REVIEWS`]): past
//! the cap a create is declined rather than accepted, so a caller cannot grow the store
//! without limit. Disassociation ends in the record's REMOVAL, so repeated
//! associate/disassociate churn does not consume the cap. Every token index entry belongs to
//! a stored record and is dropped with it, so the indexes are bounded by the record counts.
//!
//! Every lookup and every due-transition sweep is served from an index, so no operation
//! walks the whole store while holding the state lock; the only scan that remains is the
//! filtered listing, which stops after one page.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::{Duration, Instant, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    CodeReview, KMSKeyDetails, Metrics, RepositoryAssociation, RepositoryAssociationSummary,
    S3RepositoryDetails, SourceCodeType,
};

/// How long after the starting call a status this service reports as non-terminal
/// (`Associating`, `Disassociating`, `Pending`) becomes due to settle.
///
/// AWS documents no duration for these transitions, so the span is CHOSEN: it is inside
/// the emulator's 250–750 ms window, which is deliberately non-zero so a caller that
/// reads straight after the starting call still observes the transient status (and the
/// state errors that go with it) exactly as it would against the real service.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How many repository associations this emulator will hold at once.
///
/// AWS enforces an account-level service quota on repository associations but publishes no
/// figure in the model, so this capacity is CHOSEN. It is what makes the store bounded: past
/// it a create is declined instead of accepted, so a caller cannot grow the process's memory
/// without limit. Disassociation removes the record, so the cap limits LIVE associations,
/// not how many a caller may create over time.
pub const MAX_ASSOCIATIONS: usize = 1_000;

/// How many code reviews this emulator will hold at once.
///
/// Chosen for the same reason as [`MAX_ASSOCIATIONS`]. The service models no operation that
/// deletes a code review (and AWS keeps one after its repository is disassociated), so this
/// cap is the only bound on the code-review store.
pub const MAX_CODE_REVIEWS: usize = 10_000;

/// The signing name / ARN namespace of this service, used to build every ARN it mints.
pub const ARN_SERVICE: &str = "codeguru-reviewer";

/// The region the emulator reports in an ARN when a request carries no SigV4 scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The provider/owner/name triple that identifies the repository behind an association.
type RepositoryKey = (String, Option<String>, String);

/// A status change the CLOCK owes.
///
/// Stored rather than slept on: a reporting operation materialises it when it is already
/// due, so a change that was owed at a given instant is observed even if nobody polled.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The state to move to, or `None` when what the clock owes is the record's REMOVAL
    /// (the terminal step of a delete).
    pub to: Option<String>,
    /// Text for `StateReason` once the move happens.
    pub reason: Option<String>,
    pub due_at: Instant,
}

impl PendingTransition {
    /// A move to `to`, due [`SETTLE_AFTER`] from now.
    pub fn to_state(to: &str, reason: Option<String>) -> Self {
        Self {
            to: Some(to.to_string()),
            reason,
            due_at: Instant::now() + SETTLE_AFTER,
        }
    }

    /// The record's removal, due [`SETTLE_AFTER`] from now.
    pub fn removal() -> Self {
        Self {
            to: None,
            reason: None,
            due_at: Instant::now() + SETTLE_AFTER,
        }
    }
}

/// Which record a [`ComAmazonawsCodegurureviewerAwsgurufrontendserviceState::pending_due`]
/// entry points at.
#[derive(Debug, Clone)]
enum PendingKey {
    Association(String),
    CodeReview(String),
}

/// A stored repository association (`RepositoryAssociation` on the wire).
#[derive(Debug, Clone)]
pub struct Association {
    /// Creation order, assigned by the store, so pagination over it is stable.
    pub seq: u64,
    pub association_id: String,
    pub association_arn: String,
    pub name: String,
    pub owner: Option<String>,
    pub provider_type: String,
    pub connection_arn: Option<String>,
    pub state: String,
    pub state_reason: Option<String>,
    pub created_time_stamp: DateTime,
    pub last_updated_time_stamp: DateTime,
    pub kms_key_details: Option<KMSKeyDetails>,
    pub s3_repository_details: Option<S3RepositoryDetails>,
    pub tags: HashMap<String, String>,
    /// The caller's idempotency token, so the token index entry can be dropped with the
    /// record instead of being searched for.
    pub client_request_token: Option<String>,
    pub pending: Option<PendingTransition>,
}

impl Association {
    /// The repository this association holds.
    pub fn repository_key(&self) -> RepositoryKey {
        (
            self.provider_type.clone(),
            self.owner.clone(),
            self.name.clone(),
        )
    }

    /// The stored association as the modelled `RepositoryAssociation`.
    ///
    /// The single renderer: AssociateRepository, DescribeRepositoryAssociation and
    /// DisassociateRepository all answer through it, so no two of them can describe the
    /// same stored resource differently.
    pub fn to_wire(&self) -> RepositoryAssociation {
        RepositoryAssociation {
            association_id: Some(self.association_id.clone()),
            association_arn: Some(self.association_arn.clone()),
            connection_arn: self.connection_arn.clone(),
            name: Some(self.name.clone()),
            owner: self.owner.clone(),
            provider_type: Some(self.provider_type.clone()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            last_updated_time_stamp: Some(self.last_updated_time_stamp),
            created_time_stamp: Some(self.created_time_stamp),
            kms_key_details: self.kms_key_details.clone(),
            s3_repository_details: self.s3_repository_details.clone(),
        }
    }

    /// The stored association as a `RepositoryAssociationSummary` (the listing projection).
    pub fn to_summary(&self) -> RepositoryAssociationSummary {
        RepositoryAssociationSummary {
            association_arn: Some(self.association_arn.clone()),
            connection_arn: self.connection_arn.clone(),
            last_updated_time_stamp: Some(self.last_updated_time_stamp),
            association_id: Some(self.association_id.clone()),
            name: Some(self.name.clone()),
            owner: self.owner.clone(),
            provider_type: Some(self.provider_type.clone()),
            state: Some(self.state.clone()),
        }
    }

    /// The stored tags, or `None` when there are none: `TagMap` is modelled `len 1..50`, so
    /// an empty map is not a value this service may put on the wire.
    pub fn wire_tags(&self) -> Option<HashMap<String, String>> {
        (!self.tags.is_empty()).then(|| self.tags.clone())
    }
}

/// A stored code review (`CodeReview` on the wire).
#[derive(Debug, Clone)]
pub struct CodeReviewRecord {
    /// Creation order, assigned by the store.
    pub seq: u64,
    pub name: String,
    pub code_review_arn: String,
    pub association_arn: String,
    pub repository_name: String,
    pub owner: Option<String>,
    pub provider_type: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub r#type: String,
    pub pull_request_id: Option<String>,
    pub analysis_types: Vec<String>,
    pub source_code_type: Option<SourceCodeType>,
    pub metrics: Option<Metrics>,
    pub config_file_state: Option<String>,
    pub created_time_stamp: DateTime,
    pub last_updated_time_stamp: DateTime,
    pub client_request_token: Option<String>,
    pub pending: Option<PendingTransition>,
}

impl CodeReviewRecord {
    /// The stored code review as the modelled `CodeReview`.
    ///
    /// The single renderer, shared by CreateCodeReview and DescribeCodeReview, so a created
    /// review and the same review read back cannot differ.
    pub fn to_wire(&self) -> CodeReview {
        CodeReview {
            name: Some(self.name.clone()),
            code_review_arn: Some(self.code_review_arn.clone()),
            repository_name: Some(self.repository_name.clone()),
            owner: self.owner.clone(),
            provider_type: Some(self.provider_type.clone()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            created_time_stamp: Some(self.created_time_stamp),
            last_updated_time_stamp: Some(self.last_updated_time_stamp),
            r#type: Some(self.r#type.clone()),
            pull_request_id: self.pull_request_id.clone(),
            source_code_type: self.source_code_type.clone(),
            association_arn: Some(self.association_arn.clone()),
            metrics: self.metrics.clone(),
            analysis_types: (!self.analysis_types.is_empty()).then(|| self.analysis_types.clone()),
            config_file_state: self.config_file_state.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-codegurureviewer-awsgurufrontendservice. A handler reaches it through [`service_state`].
///
/// Every field is private: the record maps and the indexes over them have to stay in step,
/// so a handler reaches them only through the methods below.
#[derive(Debug, Default)]
pub struct ComAmazonawsCodegurureviewerAwsgurufrontendserviceState {
    /// Associations keyed by `AssociationArn`.
    associations: HashMap<String, Association>,
    /// Creation order -> `AssociationArn`, so a listing resumes at its cursor with a range
    /// scan instead of sorting the store.
    association_order: BTreeMap<u64, String>,
    /// The live association for a repository, so the conflict check is a lookup.
    active_repositories: HashMap<RepositoryKey, String>,
    /// `ClientRequestToken` -> `AssociationArn`, so a retried AssociateRepository is not a
    /// second association.
    association_tokens: HashMap<String, String>,
    /// Code reviews keyed by `CodeReviewArn`.
    code_reviews: HashMap<String, CodeReviewRecord>,
    /// `Name` -> `CodeReviewArn`, so the documented name-uniqueness check is a lookup.
    code_review_names: HashMap<String, String>,
    /// `ClientRequestToken` -> `CodeReviewArn`, so a retried CreateCodeReview is not a
    /// second code review.
    review_tokens: HashMap<String, String>,
    /// Records with a transition the clock owes, ordered by due instant, so a request
    /// settles only what is actually due instead of walking every record.
    pending_due: BTreeMap<(Instant, u64), PendingKey>,
    /// Monotonic creation counter shared by both resource kinds.
    next_seq: u64,
}

impl ComAmazonawsCodegurureviewerAwsgurufrontendserviceState {
    // ---- the clock ------------------------------------------------------------------

    /// Materialise every status change the clock has already made due.
    ///
    /// Call this FIRST in every handler, reporting ones included: it only applies a change
    /// that was owed at this instant whether or not anybody called, so a read never moves a
    /// status that was not already due. Cost is proportional to the number of transitions
    /// actually due, not to how many records are stored.
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        let due: Vec<(Instant, u64)> = self
            .pending_due
            .range((Bound::Unbounded, Bound::Included((now, u64::MAX))))
            .map(|(key, _)| *key)
            .collect();
        for key in due {
            match self.pending_due.remove(&key) {
                Some(PendingKey::Association(arn)) => self.settle_association(&arn, now),
                Some(PendingKey::CodeReview(arn)) => self.settle_code_review(&arn, now),
                None => {}
            }
        }
    }

    fn settle_association(&mut self, arn: &str, now: Instant) {
        let removes = match self.associations.get_mut(arn) {
            None => return,
            Some(association) => {
                if association.pending.as_ref().is_none_or(|p| p.due_at > now) {
                    return;
                }
                let pending = association.pending.take().expect("checked just above");
                match pending.to {
                    None => true,
                    Some(to) => {
                        association.state = to;
                        if let Some(reason) = pending.reason {
                            association.state_reason = Some(reason);
                        }
                        association.last_updated_time_stamp = DateTime::from(SystemTime::now());
                        false
                    }
                }
            }
        };
        if removes {
            self.remove_association(arn);
        }
    }

    fn settle_code_review(&mut self, arn: &str, now: Instant) {
        let Some(review) = self.code_reviews.get_mut(arn) else {
            return;
        };
        if review.pending.as_ref().is_none_or(|p| p.due_at > now) {
            return;
        }
        let pending = review.pending.take().expect("checked just above");
        if let Some(to) = pending.to {
            review.state = to;
            if let Some(reason) = pending.reason {
                review.state_reason = Some(reason);
            }
            review.last_updated_time_stamp = DateTime::from(SystemTime::now());
        }
    }

    /// Index `record`'s pending transition, dropping any entry its previous one left, so a
    /// superseded transition can never be applied by the clock.
    fn index_pending(&mut self, seq: u64, superseded: Option<Instant>, key: PendingKey) {
        if let Some(previous) = superseded {
            self.pending_due.remove(&(previous, seq));
        }
        let due_at = match &key {
            PendingKey::Association(arn) => self
                .associations
                .get(arn)
                .and_then(|a| a.pending.as_ref())
                .map(|p| p.due_at),
            PendingKey::CodeReview(arn) => self
                .code_reviews
                .get(arn)
                .and_then(|r| r.pending.as_ref())
                .map(|p| p.due_at),
        };
        if let Some(due_at) = due_at {
            self.pending_due.insert((due_at, seq), key);
        }
    }

    // ---- associations ---------------------------------------------------------------

    /// True when the association store is full and a create must be declined.
    pub fn associations_at_capacity(&self) -> bool {
        self.associations.len() >= MAX_ASSOCIATIONS
    }

    pub fn association(&self, arn: &str) -> Option<&Association> {
        self.associations.get(arn)
    }

    /// The association an earlier call minted under `token`, if it is still stored.
    pub fn association_for_token(&self, token: &str) -> Option<&Association> {
        self.associations.get(self.association_tokens.get(token)?)
    }

    /// The live association for a repository, i.e. the one a second association for the same
    /// provider/owner/name would conflict with.
    pub fn active_association_for(&self, key: &RepositoryKey) -> Option<&Association> {
        self.associations.get(self.active_repositories.get(key)?)
    }

    /// Store `association` (its `seq` is assigned here) and answer with the STORED record.
    pub fn insert_association(&mut self, mut association: Association) -> &Association {
        association.seq = self.next_seq();
        let seq = association.seq;
        let arn = association.association_arn.clone();
        self.association_order.insert(seq, arn.clone());
        self.active_repositories
            .insert(association.repository_key(), arn.clone());
        if let Some(token) = &association.client_request_token {
            self.association_tokens.insert(token.clone(), arn.clone());
        }
        self.associations.insert(arn.clone(), association);
        self.index_pending(seq, None, PendingKey::Association(arn.clone()));
        &self.associations[&arn]
    }

    /// Start the association's removal: it reports `transient_state` now, releases the
    /// repository immediately, and the clock owes the record's removal.
    pub fn begin_association_removal(
        &mut self,
        arn: &str,
        transient_state: &str,
    ) -> Option<&Association> {
        let (seq, superseded, key) = {
            let association = self.associations.get_mut(arn)?;
            let superseded = association.pending.as_ref().map(|p| p.due_at);
            association.state = transient_state.to_string();
            association.state_reason = None;
            association.last_updated_time_stamp = DateTime::from(SystemTime::now());
            association.pending = Some(PendingTransition::removal());
            (association.seq, superseded, association.repository_key())
        };
        self.active_repositories.remove(&key);
        self.index_pending(seq, superseded, PendingKey::Association(arn.to_string()));
        self.associations.get(arn)
    }

    /// Drop the association and every index entry that belonged to it.
    fn remove_association(&mut self, arn: &str) {
        let Some(association) = self.associations.remove(arn) else {
            return;
        };
        self.association_order.remove(&association.seq);
        if self.active_repositories.get(&association.repository_key()) == Some(&arn.to_string()) {
            self.active_repositories.remove(&association.repository_key());
        }
        if let Some(token) = &association.client_request_token {
            if self.association_tokens.get(token) == Some(&arn.to_string()) {
                self.association_tokens.remove(token);
            }
        }
    }

    /// Stored associations in creation order, starting after the cursor `after`.
    ///
    /// A range scan over the order index: a listing that stops after one page reads one
    /// page's worth of records, not the whole store.
    pub fn associations_after(&self, after: u64) -> impl Iterator<Item = &Association> + '_ {
        self.association_order
            .range((Bound::Excluded(after), Bound::Unbounded))
            .filter_map(|(_, arn)| self.associations.get(arn))
    }

    // ---- code reviews ---------------------------------------------------------------

    /// True when the code-review store is full and a create must be declined.
    pub fn code_reviews_at_capacity(&self) -> bool {
        self.code_reviews.len() >= MAX_CODE_REVIEWS
    }

    pub fn code_review(&self, arn: &str) -> Option<&CodeReviewRecord> {
        self.code_reviews.get(arn)
    }

    /// The code review an earlier call minted under `token`, if it is still stored.
    pub fn code_review_for_token(&self, token: &str) -> Option<&CodeReviewRecord> {
        self.code_reviews.get(self.review_tokens.get(token)?)
    }

    /// The code review already using `name`, if any.
    pub fn code_review_named(&self, name: &str) -> Option<&CodeReviewRecord> {
        self.code_reviews.get(self.code_review_names.get(name)?)
    }

    /// Store `review` (its `seq` is assigned here) and answer with the STORED record.
    pub fn insert_code_review(&mut self, mut review: CodeReviewRecord) -> &CodeReviewRecord {
        review.seq = self.next_seq();
        let seq = review.seq;
        let arn = review.code_review_arn.clone();
        self.code_review_names.insert(review.name.clone(), arn.clone());
        if let Some(token) = &review.client_request_token {
            self.review_tokens.insert(token.clone(), arn.clone());
        }
        self.code_reviews.insert(arn.clone(), review);
        self.index_pending(seq, None, PendingKey::CodeReview(arn.clone()));
        &self.code_reviews[&arn]
    }

    fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Mint a fresh resource id for a record this store is about to hold.
    ///
    /// The value is a v4 UUID: 32 lowercase hex digits laid out `8-4-4-4-12`, with the
    /// version nibble 4 and the RFC 4122 variant bits, which is the form the model's
    /// `AssociationArn` pattern (`…:association:[a-f0-9]{8}-…`) requires.
    ///
    /// Built from a value that is unique BY CONSTRUCTION — this store's monotonic sequence
    /// — mixed with per-process entropy (`RandomState`, seeded once by the OS) and the
    /// clock, then hashed with SHA-256. So two ids can never collide, an id is not guessable
    /// from the request that minted it, and no part of it is derived from caller input.
    pub fn mint_resource_id(&mut self) -> String {
        use ::sha2::{Digest, Sha256};
        use ::std::fmt::Write as _;
        use ::std::hash::{BuildHasher, Hash, Hasher};

        let unique = self.next_seq();
        let mut entropy = ::std::collections::hash_map::RandomState::new().build_hasher();
        unique.hash(&mut entropy);
        SystemTime::now().hash(&mut entropy);

        let digest = Sha256::new()
            .chain_update(unique.to_be_bytes())
            .chain_update(entropy.finish().to_be_bytes())
            .finalize();
        let mut raw = [0u8; 16];
        raw.copy_from_slice(&digest[..16]);
        // Version 4 and the RFC 4122 variant, the two fields the UUID form fixes.
        raw[6] = (raw[6] & 0x0f) | 0x40;
        raw[8] = (raw[8] & 0x3f) | 0x80;

        let mut id = String::with_capacity(36);
        for (index, byte) in raw.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                id.push('-');
            }
            let _ = write!(id, "{byte:02x}");
        }
        id
    }
}

/// The region to stamp into a minted ARN: the one the caller signed for, else
/// [`DEFAULT_REGION`].
///
/// Taken from the request's SigV4 credential scope, which is where an AWS SDK puts the
/// region it addressed, so an ARN this emulator mints names the caller's own region.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a repository association.
///
/// The resource portion is `association:<id>` — the colon form the model's `AssociationArn`
/// pattern requires (`…:[\d]{12}:association:[a-f0-9]{8}-…`), not a slash.
pub fn association_arn(region: &str, association_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("association:{association_id}"))
}

/// The ARN of a code review: `code-review:<id>`, matching the model's `Arn` pattern
/// (`…:[\d]{12}:[a-z-]+:[\w-]+$`).
pub fn code_review_arn(region: &str, review_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("code-review:{review_id}"))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodegurureviewerAwsgurufrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodegurureviewerAwsgurufrontendserviceState>)
}
