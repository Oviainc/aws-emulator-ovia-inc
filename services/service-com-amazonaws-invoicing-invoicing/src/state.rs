// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-invoicing-invoicing service.
//!
//! The invoice-unit lifecycle (`CreateInvoiceUnit` / `GetInvoiceUnit` /
//! `ListInvoiceUnits` / `UpdateInvoiceUnit` / `DeleteInvoiceUnit`) and the tag plane
//! (`TagResource` / `UntagResource` / `ListTagsForResource`) are served entirely from
//! the maps below: every response those operations return is computed from what an
//! earlier request stored here.
//!
//! An invoice unit keeps its superseded configurations as well as its current one,
//! because `GetInvoiceUnit` and `ListInvoiceUnits` both accept `AsOf` and the model
//! documents `LastModified` as "the version of invoice unit configuration used to
//! create the invoices" — so a read at an instant must answer with the configuration
//! that was in effect then, not with the latest one.

use std::collections::{BTreeMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    InternalServerException, InvoiceUnit, InvoiceUnitRule, ResourceNotFoundException, ResourceTag,
};

/// Per-service state for com-amazonaws-invoicing-invoicing. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsInvoicingInvoicingState {
    /// Every invoice unit that currently exists, keyed by its `InvoiceUnitArn`. A
    /// `BTreeMap` so `ListInvoiceUnits` walks a stable, deterministic order and a
    /// pagination token (a position in that order) stays meaningful across calls.
    pub invoice_units: BTreeMap<String, StoredInvoiceUnit>,
    /// `ResourceTags` per resource ARN, as `CreateInvoiceUnit` and `TagResource`
    /// persisted them and `ListTagsForResource` reports them. One tag per key: a
    /// later write of a key already set replaces that key's value.
    pub resource_tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The result an `@idempotent` operation already produced for a `ClientToken`,
    /// keyed by `(operation, target, token)`: one token replayed against another
    /// operation, or against another invoice unit, is a different request and must not
    /// replay this one's result. Bounded by [`MAX_IDEMPOTENT_RECORDS`].
    idempotent_results: BTreeMap<IdempotencyKey, String>,
    /// The keys of [`Self::idempotent_results`] in the order they were recorded, so the
    /// oldest record is the one evicted when the bound is reached.
    idempotent_record_order: VecDeque<IdempotencyKey>,
    /// Source of invoice-unit ids for ARN minting.
    next_invoice_unit_id: u64,
}

/// What an `@idempotent` replay record is keyed by: the operation, the request's target
/// (the invoice unit it names, or the name it creates) and the caller's `ClientToken`.
type IdempotencyKey = (String, String, String);

/// How many invoice units this emulator holds. The model's `ValidationExceptionReason`
/// declares `maxInvoiceUnitsExceeded`, so the real service refuses a create once its own
/// (account-adjustable) maximum is reached; this constant is the emulator's maximum, and
/// it also bounds the registry and the per-request walks over it.
pub const MAX_INVOICE_UNITS: usize = 1000;

/// How many tags one resource holds. `ResourceTagList` is modelled `length 0..200`
/// both as `TagResource`'s input and as `ListTagsForResource`'s output
/// (`smithy_query(query="operation", operation="TagResource,ListTagsForResource")`),
/// so a resource cannot hold more tags than that and still be reportable; a write
/// past it is the modelled `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_RESOURCE_TAGS: usize = 200;

/// How many `@idempotent` replay records this service keeps, across every operation and
/// `ClientToken`. The key is caller-supplied, so without a bound a long-lived emulator
/// would accumulate one entry per distinct token for the life of the process; past this
/// many the oldest record is dropped, and a replay of a token that old mints a new
/// result instead of replaying.
const MAX_IDEMPOTENT_RECORDS: usize = 1024;

/// How many superseded configurations one invoice unit keeps for `AsOf` reads. Updates
/// are caller-driven, so the history is bounded the same way: past this many the oldest
/// configuration is dropped, and an `AsOf` older than the oldest retained one reads
/// that oldest retained configuration.
const MAX_RETAINED_VERSIONS: usize = 32;

impl ComAmazonawsInvoicingInvoicingState {
    /// Mint the ARN of a new invoice unit: `arn:aws:invoicing::<account>:invoice-unit/<id>`.
    ///
    /// The id is zero-padded hex of a monotonic counter, so an ARN is never reused and
    /// the `BTreeMap` order of `invoice_units` is creation order.
    pub fn mint_invoice_unit_arn(&mut self) -> String {
        self.next_invoice_unit_id = self.next_invoice_unit_id.wrapping_add(1);
        let id = self.next_invoice_unit_id;
        ::engine::emulator_arn("invoicing", "", &format!("invoice-unit/{id:012x}"))
    }

    /// The ARN an earlier call of `operation` already returned for this `target` and
    /// `client_token`, if this is a replay of that request. `None` when the request
    /// carries no token, or carries one this operation has not seen against this target.
    pub fn replayed_result(
        &self,
        operation: &str,
        target: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let token = client_token?;
        self.idempotent_results
            .get(&(
                operation.to_string(),
                target.to_string(),
                token.to_string(),
            ))
            .cloned()
    }

    /// Record the ARN `operation` returned for this `target` and `client_token`, so a
    /// replay of that same request answers with this same ARN instead of changing state
    /// a second time.
    pub fn record_result(
        &mut self,
        operation: &str,
        target: &str,
        client_token: Option<&str>,
        arn: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        let key = (
            operation.to_string(),
            target.to_string(),
            token.to_string(),
        );
        if self
            .idempotent_results
            .insert(key.clone(), arn.to_string())
            .is_none()
        {
            self.idempotent_record_order.push_back(key);
        }
        // Bounded store: evict the oldest records once the cap is reached, so a stream
        // of distinct caller-supplied tokens cannot grow this map without limit.
        while self.idempotent_record_order.len() > MAX_IDEMPOTENT_RECORDS {
            if let Some(oldest) = self.idempotent_record_order.pop_front() {
                self.idempotent_results.remove(&oldest);
            }
        }
    }
}

/// One invoice unit as it is stored: the members that can never change, plus the
/// configuration history that `AsOf` reads.
#[derive(Debug, Clone)]
pub struct StoredInvoiceUnit {
    /// The minted `InvoiceUnitArn`. Immutable.
    pub arn: String,
    /// `Name`, which the model documents as unchangeable once set.
    pub name: String,
    /// `InvoiceReceiver`, set at creation and not updatable.
    pub invoice_receiver: String,
    /// When the unit was created. An `AsOf` before this instant reads no configuration
    /// at all, because the unit did not exist then.
    pub created_at: DateTime,
    /// The configuration in effect now.
    pub current: InvoiceUnitVersion,
    /// Configurations an update replaced, oldest first, at most
    /// [`MAX_RETAINED_VERSIONS`] of them.
    pub superseded: Vec<InvoiceUnitVersion>,
}

impl StoredInvoiceUnit {
    /// The configuration that was in effect at `as_of`, or the current one when the
    /// caller asked for no particular instant. `None` when the unit did not yet exist
    /// at `as_of`, which a reader reports as `ResourceNotFoundException`.
    ///
    /// A unit that existed at `as_of` but whose configuration from then has been evicted
    /// by [`MAX_RETAINED_VERSIONS`] reads the oldest configuration still retained.
    pub fn version_as_of(&self, as_of: Option<&DateTime>) -> Option<&InvoiceUnitVersion> {
        let Some(as_of) = as_of else {
            return Some(&self.current);
        };
        let wanted = instant_key(as_of);
        if wanted < instant_key(&self.created_at) {
            return None;
        }
        std::iter::once(&self.current)
            .chain(self.superseded.iter().rev())
            .find(|version| instant_key(&version.last_modified) <= wanted)
            .or_else(|| self.superseded.first())
    }

    /// Make `next` the current configuration, keeping the one it replaces as history.
    pub fn apply(&mut self, next: InvoiceUnitVersion) {
        let previous = std::mem::replace(&mut self.current, next);
        self.superseded.push(previous);
        // Bounded history: updates are caller-driven, so keep only the most recent
        // configurations rather than growing this vector without limit.
        if self.superseded.len() > MAX_RETAINED_VERSIONS {
            let excess = self.superseded.len() - MAX_RETAINED_VERSIONS;
            self.superseded.drain(..excess);
        }
    }

    /// This unit as the modelled `InvoiceUnit` structure, reporting `version`.
    pub fn to_invoice_unit(&self, version: &InvoiceUnitVersion) -> InvoiceUnit {
        InvoiceUnit {
            invoice_unit_arn: Some(self.arn.clone()),
            invoice_receiver: Some(self.invoice_receiver.clone()),
            name: Some(self.name.clone()),
            description: version.description.clone(),
            tax_inheritance_disabled: Some(version.tax_inheritance_disabled),
            rule: Some(version.rule.clone()),
            last_modified: Some(version.last_modified),
        }
    }
}

/// The updatable half of an invoice unit, as of the `LastModified` instant it was
/// written at.
#[derive(Debug, Clone)]
pub struct InvoiceUnitVersion {
    pub description: Option<String>,
    pub tax_inheritance_disabled: bool,
    pub rule: InvoiceUnitRule,
    pub last_modified: DateTime,
}

/// A total order on timestamps, so a stored `LastModified` can be compared with a
/// requested `AsOf` without assuming `DateTime` is itself `Ord`.
fn instant_key(at: &DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// Now, as the timestamp an invoice unit records in `LastModified`.
///
/// Built from whole seconds and nanoseconds rather than through an `f64`, so
/// [`instant_key`] orders two writes a few microseconds apart exactly.
pub fn now() -> DateTime {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// A tag map as the modelled `ResourceTagList`, in a deterministic key order.
pub fn resource_tag_list(tags: &BTreeMap<String, String>) -> Vec<ResourceTag> {
    tags.iter()
        .map(|(key, value)| ResourceTag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The message a `ResourceNotFoundException` of this service carries: a CONSTANT, the
/// same sentence whatever was asked for.
///
/// The shape's own documentation gives it (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")` -> "The resource could not be found."), and the
/// recorded 404s answer with exactly that sentence — the resource that was not found
/// travels in its own member instead of being spliced into the prose, so a client can
/// read it mechanically and a client matching the documented sentence still matches.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The resource could not be found.";

/// The 404 any operation of this service answers for an ARN that is not in state: the
/// constant message, plus `arn` in the shape's own ARN member.
pub fn no_such_resource(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        resource_id: Some(arn.to_string()),
    }
}

// The 400 `ValidationException` that states a member constraint the request failed
// lives in `crate::service::validation`, with the rest of how this service WORDS a
// refusal: the `message` envelope, each `fieldList` entry's own clause and the `reason`
// they are stated under are one rendering, reached by every refusal this service
// answers.

/// The 500 a handler answers when the engine it was constructed with is not the shared
/// in-memory engine, so this service has no store to read. Never construct an engine in
/// a handler instead: that store is private to the request and dropped when it returns.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some("the invoicing emulator's state store is unavailable".to_string()),
        retry_after_seconds: None,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInvoicingInvoicingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInvoicingInvoicingState>)
}
