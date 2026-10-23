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
//!
//! For the same reason a DELETE RETIRES an invoice unit rather than erasing it: the
//! model documents `AsOf` as "the state of an invoice unit at a specified time. You can
//! see legacy invoice units that are currently deleted if the `AsOf` time is set to
//! before it was deleted", so the instant a unit was deleted at is part of its history
//! and a read at an earlier instant still has to answer with it. A retired unit carries
//! a [`StoredInvoiceUnit::deleted_at`] tombstone, is reported by a point-in-time read
//! whose instant falls between its creation and its deletion, and is absent from every
//! read of the state NOW — so the live surface (a create of the same `Name`, a tag
//! write, an update, a second delete) sees it as gone.
//!
//! Beside the invoice units sits the INVOICE registry [`ComAmazonawsInvoicingInvoicingState::invoices`],
//! the one copy of the invoices this emulator holds, which `ListInvoiceSummaries`
//! reports. An invoice is ISSUED by AWS out of a billing period's activity: no
//! operation of this service's API creates one (`smithy_query(query="list_operations")`
//! lists one create, `CreateInvoiceUnit`), and this emulator runs no billing, so
//! nothing mints an invoice here either. The registry therefore starts out EMPTY —
//! which is the answer a reader owes for an account that has no invoices — and a
//! reader reports exactly what it holds rather than synthesising amounts for charges
//! nothing incurred.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AccessDeniedException, InternalServerException, InvoiceProfile, InvoiceSummary, InvoiceUnit,
    InvoiceUnitRule, ProcurementPortalPreference, ResourceNotFoundException, ResourceTag,
};

/// Per-service state for com-amazonaws-invoicing-invoicing. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsInvoicingInvoicingState {
    /// Every invoice unit this emulator holds, LIVE AND RETIRED, keyed by its
    /// `InvoiceUnitArn`. A `BTreeMap` so `ListInvoiceUnits` walks a stable,
    /// deterministic order and a pagination token (a position in that order) stays
    /// meaningful across calls.
    ///
    /// A retired unit (one [`StoredInvoiceUnit::deleted_at`] is set on) is history, not
    /// a resource: reach the live surface through [`Self::live_invoice_unit`],
    /// [`Self::live_invoice_unit_mut`] and [`Self::live_invoice_units`] rather than
    /// indexing this map, and read history through
    /// [`StoredInvoiceUnit::version_as_of`], which answers `None` for an instant the
    /// unit did not exist at — before it was created or at or after it was deleted.
    pub invoice_units: BTreeMap<String, StoredInvoiceUnit>,
    /// Every invoice this emulator holds, keyed by its `InvoiceId`, as the modelled
    /// `InvoiceSummary` a reader reports it with — one copy, so two readers of one
    /// invoice cannot disagree. A `BTreeMap` so `ListInvoiceSummaries` walks a stable,
    /// deterministic order and a pagination token (a position in that order) stays
    /// meaningful across calls.
    ///
    /// Empty until something puts an invoice here, because nothing in this service's
    /// API issues one — see the module docs.
    pub invoices: BTreeMap<String, InvoiceSummary>,
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
    /// Every procurement portal preference this emulator holds, keyed by its
    /// `ProcurementPortalPreferenceArn` — the one copy of a preference, so the
    /// operations of the portal-validation flow cannot disagree about whether one
    /// exists. A `BTreeMap` so a listing walks a stable, deterministic order.
    ///
    /// Empty until a create puts a preference here:
    /// `CreateProcurementPortalPreference` and `PutProcurementPortalPreference` are the
    /// operations that do so and are still scaffolded (their own stub issues own them),
    /// so for now every ARN naming a preference resolves to nothing — which is exactly
    /// what `SendProcurementPortalValidation` and `VerifyProcurementPortalValidation`
    /// report as the modelled `ResourceNotFoundException`, rather than claiming a
    /// validation state for a preference that does not exist.
    pub procurement_portal_preferences: BTreeMap<String, StoredProcurementPortalPreference>,
    /// Source of invoice-unit ids for ARN minting.
    next_invoice_unit_id: u64,
    /// Source of procurement-portal-preference ids for ARN minting.
    next_procurement_portal_preference_id: u64,
    /// Source of the validation codes `SendProcurementPortalValidation` issues.
    next_portal_validation_code: u64,
}

/// What an `@idempotent` replay record is keyed by: the operation, the request's target
/// (the invoice unit it names, or the name it creates) and the caller's `ClientToken`.
type IdempotencyKey = (String, String, String);

/// How many invoice units this emulator holds. The model's `ValidationExceptionReason`
/// declares `maxInvoiceUnitsExceeded`, so the real service refuses a create once its own
/// (account-adjustable) maximum is reached; this constant is the emulator's maximum. It
/// bounds the units that EXIST (the registry also holds the tombstones a delete leaves,
/// bounded by [`MAX_RETIRED_INVOICE_UNITS`]) and the per-request walks over them.
pub const MAX_INVOICE_UNITS: usize = 1000;

/// How many tags ONE RESOURCE holds — a quota on the resource, independent of how many
/// tags one call may carry.
///
/// `ResourceTagList` is modelled `length 0..200` as `TagResource`'s input
/// (`smithy_query(query="operation", operation="TagResource,ListTagsForResource")`), but
/// that is the bound on ONE CALL's array: a per-resource quota equal to it could never
/// be reached by any legal sequence of calls, because no single call may carry more than
/// the quota either. The per-resource quota is therefore the AWS-wide tagging quota —
/// "Maximum number of tags per resource – 50" (AWS General Reference, "Tagging your
/// resources") — strictly below the per-call maximum, so a caller tagging in batches
/// reaches it. A write that would push a resource past it is refused ATOMICALLY with the
/// modelled `ServiceQuotaExceededException` (HTTP 402); nothing of the refused batch
/// persists.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// The sentence the per-resource tag quota is refused with: a CONSTANT, the service's
/// own canned wording as the recorded 402 of this service states it — it names no ARN
/// and no count, so a client matching it matches every one of these refusals.
pub const TAG_QUOTA_EXCEEDED_MESSAGE: &str =
    "The request exceeds the maximum number of tags allowed per resource.";

/// How many RETIRED invoice units this emulator keeps for point-in-time reads. Deletes
/// are caller-driven, so the tombstones are bounded the same way the configuration
/// history is: past this many the unit deleted longest ago is dropped, and an `AsOf`
/// read can no longer answer with it.
const MAX_RETIRED_INVOICE_UNITS: usize = 1000;

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

    /// The invoice unit `arn` names RIGHT NOW: the stored unit, unless it has been
    /// retired, in which case there is no such resource any more and a reader of the
    /// current state answers `ResourceNotFoundException`.
    ///
    /// Every operation that acts on the live resource \u2014 a read with no `AsOf`, an
    /// update, a tag write, a second delete \u2014 resolves the ARN through here, so one
    /// retired unit cannot look present to one operation and absent to another.
    pub fn live_invoice_unit(&self, arn: &str) -> Option<&StoredInvoiceUnit> {
        self.invoice_units.get(arn).filter(|unit| unit.is_live())
    }

    /// [`Self::live_invoice_unit`], for a caller that goes on to change the unit.
    pub fn live_invoice_unit_mut(&mut self, arn: &str) -> Option<&mut StoredInvoiceUnit> {
        self.invoice_units
            .get_mut(arn)
            .filter(|unit| unit.is_live())
    }

    /// Every invoice unit that exists right now, in `InvoiceUnitArn` order. The retired
    /// ones are history and are reached only through a point-in-time read.
    pub fn live_invoice_units(&self) -> impl Iterator<Item = &StoredInvoiceUnit> {
        self.invoice_units.values().filter(|unit| unit.is_live())
    }

    /// How many invoice units exist right now \u2014 what [`MAX_INVOICE_UNITS`] bounds. The
    /// tombstones a delete leaves behind are history and do not consume the account's
    /// invoice-unit allowance.
    pub fn live_invoice_unit_count(&self) -> usize {
        self.live_invoice_units().count()
    }

    /// Retire the invoice unit `arn` names at `at`, keeping it as history: it stops
    /// being a resource (every live read now reports it missing) while a point-in-time
    /// read before `at` still answers with the configuration that was in effect then.
    ///
    /// `false` when `arn` names no LIVE invoice unit, which a delete reports as
    /// `ResourceNotFoundException` \u2014 an already-retired unit included, because the
    /// resource is already gone.
    pub fn retire_invoice_unit(&mut self, arn: &str, at: DateTime) -> bool {
        match self.live_invoice_unit_mut(arn) {
            Some(unit) => unit.deleted_at = Some(at),
            None => return false,
        }
        // Bounded history: deletes are caller-driven, so keep only the most recently
        // retired units rather than growing this map without limit.
        while self.retired_invoice_unit_count() > MAX_RETIRED_INVOICE_UNITS {
            let oldest = self
                .invoice_units
                .values()
                .filter_map(|unit| {
                    unit.deleted_at
                        .map(|deleted| (instant_key(&deleted), unit.arn.clone()))
                })
                .min();
            match oldest {
                Some((_, arn)) => self.invoice_units.remove(&arn),
                None => break,
            };
        }
        true
    }

    /// How many retired invoice units this emulator is holding as history.
    fn retired_invoice_unit_count(&self) -> usize {
        self.invoice_units
            .values()
            .filter(|unit| !unit.is_live())
            .count()
    }

    /// Mint the ARN of a new procurement portal preference:
    /// `arn:aws:invoicing::<account>:procurement-portal-preference/<id>`, the form
    /// `ProcurementPortalPreferenceArnString`'s `@pattern` spells
    /// (`smithy_query(query=\"shape\", shape=\"ProcurementPortalPreference\")`).
    pub fn mint_procurement_portal_preference_arn(&mut self) -> String {
        self.next_procurement_portal_preference_id =
            self.next_procurement_portal_preference_id.wrapping_add(1);
        let id = self.next_procurement_portal_preference_id;
        ::engine::emulator_arn(
            "invoicing",
            "",
            &format!("procurement-portal-preference/{id:012x}"),
        )
    }

    /// The validation code the next `SendProcurementPortalValidation` issues: six
    /// decimal digits, the form the portal-validation flow's `Code` is quoted in.
    ///
    /// Minted from a monotonic counter mixed with the nanosecond clock, so two
    /// validations issued in one run do not share a code. It is NOT a credential and is
    /// never returned on the wire: the real service delivers it out of band through the
    /// configured portal, and this emulator has no out-of-band channel, so the code it
    /// issued is kept here (see [`IssuedPortalValidation`]) for
    /// `VerifyProcurementPortalValidation` to check a submitted code against.
    pub fn issue_portal_validation_code(&mut self) -> String {
        self.next_portal_validation_code = self.next_portal_validation_code.wrapping_add(1);
        let mixed = u64::from(now().subsec_nanos())
            .wrapping_mul(1_000_003)
            .wrapping_add(self.next_portal_validation_code);
        format!("{:06}", mixed % 1_000_000)
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

    /// Every account the requester's organization contains, as this service's state
    /// names them.
    ///
    /// An operation documented over "linked accounts under the requester management
    /// account organization" (`BatchGetInvoiceProfile`) has to decide membership from
    /// what the emulator actually holds, and this service's state names an account of
    /// that organization in exactly two places: an invoice unit's `InvoiceReceiver`
    /// ("the Amazon Web Services account ID chosen to be the receiver of an invoice
    /// unit") and an invoice-unit rule's `LinkedAccounts` ("the list of
    /// `LINKED_ACCOUNT` IDs where charges are included within the invoice unit") —
    /// `smithy_query(query="shape", shape="InvoiceUnitRule")` and
    /// `smithy_query(query="operation", operation="CreateInvoiceUnit")`. The requester's
    /// own account ([`::engine::emulator_account_id`]) is the management account of that
    /// organization, so it is a member of it too.
    ///
    /// `BillSourceAccounts` is deliberately NOT a source: the model documents those as
    /// accounts that "can be payer accounts from other organizations that have
    /// authorized billing transfer to this account", so naming one does not place it in
    /// this organization.
    ///
    /// Every RETAINED configuration of a unit counts, not only its current one: an
    /// account an update dropped from a rule left that invoice unit, not the
    /// organization. A deleted invoice unit takes its accounts with it — state is the
    /// only record either way.
    pub fn organization_accounts(&self) -> BTreeSet<String> {
        let mut accounts = BTreeSet::new();
        accounts.insert(::engine::emulator_account_id().to_string());
        for unit in self.live_invoice_units() {
            accounts.insert(unit.invoice_receiver.clone());
            for version in std::iter::once(&unit.current).chain(unit.superseded.iter()) {
                if let Some(linked) = version.rule.linked_accounts.as_ref() {
                    accounts.extend(linked.iter().cloned());
                }
            }
        }
        accounts
    }
}

/// One procurement portal preference as it is stored: the modelled structure a reader
/// reports, plus the portal-validation state the validation flow keeps about it.
#[derive(Debug, Clone)]
pub struct StoredProcurementPortalPreference {
    /// The preference itself, as the modelled `ProcurementPortalPreference` a reader
    /// reports it with — one copy, so two readers of one preference cannot disagree.
    pub preference: ProcurementPortalPreference,
    /// The validation `SendProcurementPortalValidation` last issued for this
    /// preference, if any. `None` means no validation has been requested, so there is
    /// no code a `VerifyProcurementPortalValidation` could match.
    pub validation: Option<IssuedPortalValidation>,
}

/// One issued procurement-portal validation: the code the service issued, when it
/// issued it, and when (if ever) a caller submitted that same code back.
#[derive(Debug, Clone)]
pub struct IssuedPortalValidation {
    /// The code `SendProcurementPortalValidation` issued. Kept in state and never put
    /// on the wire: the modelled response carries only the preference's ARN, because
    /// the real service delivers the code out of band through the portal.
    pub code: String,
    /// When the code was issued.
    pub issued_at: DateTime,
    /// When a `VerifyProcurementPortalValidation` submitted this code back, which is
    /// what completes the validation. `None` while it is still outstanding.
    pub verified_at: Option<DateTime>,
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
    /// When the unit was DELETED, for a unit a delete has retired. The unit is no
    /// longer a resource from this instant on, while a point-in-time read before it
    /// still answers with the configuration that was in effect then — the model
    /// documents `AsOf` as showing "legacy invoice units that are currently deleted if
    /// the AsOf time is set to before it was deleted". `None` for a unit that still
    /// exists.
    pub deleted_at: Option<DateTime>,
}

impl StoredInvoiceUnit {
    /// Whether this unit still exists: a delete retires a unit rather than erasing it,
    /// and a retired one is history rather than a resource.
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }

    /// The configuration that was in effect at `as_of`, or the current one when the
    /// caller asked for no particular instant. `None` when the unit did not exist at
    /// `as_of`, which a reader reports as `ResourceNotFoundException`.
    ///
    /// "Did not exist" is both ends of the unit's life: an instant before it was
    /// created, and an instant at or after it was deleted. An instant in between
    /// answers with the configuration from then even for a unit that has since been
    /// deleted, and a caller that asked for no instant at all is asking about NOW, so a
    /// deleted unit is not part of that answer.
    ///
    /// A unit that existed at `as_of` but whose configuration from then has been evicted
    /// by [`MAX_RETAINED_VERSIONS`] reads the oldest configuration still retained.
    pub fn version_as_of(&self, as_of: Option<&DateTime>) -> Option<&InvoiceUnitVersion> {
        let Some(as_of) = as_of else {
            return self.is_live().then_some(&self.current);
        };
        let wanted = instant_key(as_of);
        if wanted < instant_key(&self.created_at) {
            return None;
        }
        if self
            .deleted_at
            .as_ref()
            .is_some_and(|deleted| instant_key(deleted) <= wanted)
        {
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

/// A total order on timestamps, so a stored instant can be compared with a requested
/// one — a `LastModified` against an `AsOf`, an `IssuedDate` against a `DateInterval`'s
/// bounds — without assuming `DateTime` is itself `Ord`.
///
/// The order is at MILLISECOND granularity, rounded to the nearest millisecond, because
/// that is the precision this service's instants survive a round trip through the wire
/// at: a timestamp travels on the awsJson protocols as epoch seconds in a JSON NUMBER,
/// which a client (and this emulator's own deserializer) reads as a double, and a double
/// cannot hold a nanosecond-precise instant of the current epoch — the value comes back
/// a few hundred nanoseconds off, in either direction. Comparing at a finer granularity
/// than the wire carries would make a read at an instant the SERVICE ITSELF reported —
/// `AsOf` set to a unit's own `LastModified`, which is how a caller asks for "the state
/// as of that write" — miss the very state it reported, which is not a difference any
/// caller can account for. Milliseconds is also the granularity [`now`] stamps with, so
/// a stored instant is exactly on a boundary and the rounding only absorbs the wire's
/// error.
///
/// The arithmetic is SATURATING because the instant can come straight off the wire: a
/// timestamp arrives as a JSON number and is built with `DateTime::from_secs_f64`, whose
/// cast saturates, so a caller can name a second count whose millisecond count no `i64`
/// holds. Saturating keeps the key total and order-preserving at those extremes — an
/// absurdly large instant is "at or after everything stored", an absurdly small one is
/// "before everything" — instead of panicking under overflow checks or wrapping to a
/// negative key and mis-ordering the comparison.
pub fn instant_key(at: &DateTime) -> i64 {
    const NANOS_PER_MILLI: i64 = 1_000_000;
    at.secs()
        .saturating_mul(1_000)
        .saturating_add((i64::from(at.subsec_nanos()) + NANOS_PER_MILLI / 2) / NANOS_PER_MILLI)
}

/// Now, as the timestamp an invoice unit records in `LastModified`.
///
/// Built from whole seconds and MILLISECONDS rather than through an `f64`, so two writes
/// a few milliseconds apart are ordered exactly by [`instant_key`] and a stamp this
/// service hands out survives the round trip back through an `AsOf` — see
/// [`instant_key`] for why a finer stamp would not.
pub fn now() -> DateTime {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_millis() * 1_000_000,
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

/// The invoice profile this service holds for an account of its organization.
///
/// Every other member of `InvoiceProfile` describes the invoice RECEIVER itself —
/// `ReceiverName`, `ReceiverAddress`, `ReceiverEmail`, `Issuer` and
/// `TaxRegistrationNumber` (`smithy_query(query="shape", shape="InvoiceProfile")`) —
/// and all five are set through the billing/tax-settings plane that this service does
/// not own: no operation of THIS model writes any of them, so there is nothing in state
/// to report them from. Each is an optional member, so they are reported ABSENT —
/// "not on file here" — rather than invented. `AccountId`, the member that says whose
/// profile this is, is the one this service can answer, and it is answered only for an
/// account [`ComAmazonawsInvoicingInvoicingState::organization_accounts`] holds.
pub fn invoice_profile(account_id: &str) -> InvoiceProfile {
    InvoiceProfile {
        account_id: Some(account_id.to_string()),
        ..Default::default()
    }
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

/// The 404 a reader answers for a resource named by something that is NOT an ARN — an
/// invoice id, say: the same constant message, with the ARN member left unset.
///
/// That member is modelled `InvoiceUnitArnString`, whose `@pattern` admits only an ARN
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`), so a
/// non-ARN identifier cannot be reported in it without answering a value the model
/// says that member never holds.
pub fn no_such_resource_unnamed() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        resource_id: None,
    }
}

/// The 403 an operation of this service answers when the requester may not read what it
/// asked about: the account it asked about is not one the requester's organization
/// contains, so the requester is not authorized for that account's resources.
///
/// The message is BUILT FROM THE REQUEST, as the recorded refusals of this service state
/// it: it names the requester ([`::engine::emulator_account_id`], the account this
/// emulator presents to every caller) and the account the requester was refused.
///
/// `resourceName` is left absent: the model types it as an `InvoiceUnitArnString`
/// (`smithy_query(query="shape", shape="AccessDeniedException")`), so it names an
/// invoice unit rather than an account.
pub fn not_authorized_for_account(account_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "Account {} is not authorized to access resources for account {account_id}",
            ::engine::emulator_account_id()
        )),
        resource_name: None,
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
