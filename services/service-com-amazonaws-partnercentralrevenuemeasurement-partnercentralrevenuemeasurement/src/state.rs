// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement service.
//!
//! The records here are the ONLY place a marketplace revenue share or one of its
//! allocations exists: every implemented handler in `src/ops/` mints into this
//! store or reads out of it, so a create is observable by the next get/list and
//! an update by the next reader.
//!
//! # Revisions are held, not faked
//!
//! `ListMarketplaceRevenueShareAllocations` documents "Supports historical reads
//! at a specific share revision", and `Get…`/`Get…Allocation` both take an
//! optional revision that reads the resource "as it existed at this revision".
//! A share therefore carries a revision counter plus the instant each revision
//! was written ([`MarketplaceRevenueShare::revision_dates`]), and every
//! allocation keeps the ordered list of its own versions
//! ([`MarketplaceRevenueShareAllocation::versions`]), each stamped with the share
//! revision at which it became current. A read at revision `r` is then answered
//! by [`MarketplaceRevenueShareAllocation::version_at`] — reconstructed from
//! state, never stored as a response literal.
//!
//! Two things this slice deliberately does NOT emulate, so a reader does not
//! mistake a modelled error for reachable behaviour: authorization (every
//! operation models `AccessDeniedException`, and the emulator holds no principals
//! to evaluate, so no handler constructs it) and throttling
//! (`ThrottlingException`, likewise). The emulator verifies request signatures in
//! the framework, before any handler runs.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ConflictException, InternalServerException, ResourceNotFoundException,
    ServiceQuotaExceededException, Tag, ValidationException, ValidationExceptionField,
    CATALOG_NAME_VALUES,
};

/// The Region an ARN carries when the request is unsigned, so no SigV4
/// credential scope names one. AWS Partner Central's control plane is reached in
/// US East (N. Virginia).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace for every resource this service mints
/// (`OPERATIONS.md`: "ARN namespace: `partnercentral`").
pub const ARN_SERVICE: &str = "partnercentral";

/// The ARN resource prefix of a marketplace revenue share, as `ResourceARN`'s
/// own pattern spells it
/// (`…:catalog/[a-zA-Z]+/marketplace-revenue-share/prod-[a-z0-9]{13}`).
pub const MARKETPLACE_REVENUE_SHARE_RESOURCE: &str = "marketplace-revenue-share";

/// The ARN sub-resource prefix of one allocation under a revenue share.
pub const ALLOCATION_RESOURCE: &str = "allocation";

/// The leading characters of every allocation id this service mints
/// (`MarketplaceRevenueShareAllocationId`: `^mrsa-[A-Za-z0-9]{13}$`).
pub const ALLOCATION_ID_PREFIX: &str = "mrsa-";

/// The most marketplace revenue shares this emulator holds at once.
///
/// A bound on how far the store can grow under repeated creates: a create past
/// it is refused with the modelled `ServiceQuotaExceededException` (HTTP 402)
/// rather than accepted, so a caller cannot grow the process's memory without
/// limit. AWS publishes its own quota and it is not this number; this is the
/// emulator's own ceiling, chosen well above what a client script needs.
pub const MAX_REVENUE_SHARES: usize = 100;

/// The most allocations one revenue share holds at once, for the same reason.
/// `CreateMarketplaceRevenueShareAllocation` models no quota error, so a create
/// past this ceiling is refused with the modelled `ValidationException`.
pub const MAX_ALLOCATIONS_PER_SHARE: usize = 250;

/// The highest revision one revenue share reaches in this emulator.
///
/// Every allocation write appends a version and bumps the share's revision, and
/// the history is what a historical read is answered from — so the history is
/// also the store's growth vector. A write past this ceiling is refused with the
/// modelled `ValidationException`. `RevisionToken` admits up to 19 digits, so
/// this is the emulator's own ceiling and not a modelled one.
pub const MAX_SHARE_REVISION: i32 = 10_000;

/// The most items any `@paginated` list in this service returns in one page,
/// including when `MaxResults` is absent or larger.
///
/// This is the EMULATOR's own ceiling: the generated contract headers render
/// `MaxResults` as a plain `Integer` on both lists, so no modelled `@range`
/// bounds it and one call could otherwise be asked for the whole store in a
/// single response body. A caller that wants the rest follows `NextToken`. A
/// value AT OR BELOW zero is refused with the modelled `ValidationException`
/// rather than quietly clamped, because a page of "zero or fewer" items is not
/// a request this service can honour.
pub const MAX_PAGE_SIZE: i32 = 100;

/// `CatalogName` — which catalog a resource lives in (`AWS` | `Sandbox`).
///
/// Part of every resource's key AND of its ARN, so the two catalogs are separate
/// namespaces: the same `ProductId` in `AWS` and in `Sandbox` is two resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CatalogName {
    /// The production catalog.
    Aws,
    /// The sandbox catalog.
    Sandbox,
}

impl CatalogName {
    /// The value this catalog serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Aws => "AWS",
            Self::Sandbox => "Sandbox",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "AWS" => Some(Self::Aws),
            "Sandbox" => Some(Self::Sandbox),
            _ => None,
        }
    }
}

/// `AllocationStatus` — whether an allocation is in force (`ACTIVE`) or has been
/// soft-deleted (`INACTIVE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationStatus {
    /// In force; counted by `TotalActiveMarketplaceRevenueShareAllocationCount`.
    Active,
    /// Soft-deleted.
    Inactive,
}

impl AllocationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ACTIVE" => Some(Self::Active),
            "INACTIVE" => Some(Self::Inactive),
            _ => None,
        }
    }
}

/// `SortOrder` — the direction a list sorts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Smallest sort key first.
    Ascending,
    /// Largest sort key first.
    Descending,
}

impl SortOrder {
    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ASCENDING" => Some(Self::Ascending),
            "DESCENDING" => Some(Self::Descending),
            _ => None,
        }
    }
}

/// One version of an allocation: the mutable half of the record, plus the share
/// revision at which this version became the current one.
#[derive(Debug, Clone)]
pub struct AllocationVersion {
    /// The parent share revision this version was written at. A read at share
    /// revision `r` sees the last version whose `revision` is `<= r`.
    pub revision: i32,
    /// `EffectiveFrom` — the first day of a month.
    pub effective_from: String,
    /// `EffectiveUntil` — the last day of a month, or `None` for open-ended.
    pub effective_until: Option<String>,
    /// `RevenueSharePercent`, verbatim as the caller wrote it.
    pub revenue_share_percent: String,
    /// `Status`.
    pub status: AllocationStatus,
    /// `LastModifiedDate` for this version.
    pub modified_date: SystemTime,
}

/// A marketplace revenue share allocation, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct MarketplaceRevenueShareAllocation {
    /// `MarketplaceRevenueShareAllocationId` — minted by this service.
    pub allocation_id: String,
    /// `Arn`.
    pub arn: String,
    /// `ProductId` of the parent revenue share.
    pub product_id: String,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// The version this allocation was created with.
    ///
    /// Held SEPARATELY from the later ones so "an allocation always has at
    /// least one version" is a fact of the type rather than an assertion a
    /// reader has to trust: every accessor below is total, and no path through
    /// this module can unwrap an empty history.
    pub created_version: AllocationVersion,
    /// Every version written after the create, oldest first.
    pub later_versions: Vec<AllocationVersion>,
}

impl MarketplaceRevenueShareAllocation {
    /// Every version of this allocation, oldest first.
    pub fn versions(&self) -> impl DoubleEndedIterator<Item = &AllocationVersion> {
        ::std::iter::once(&self.created_version).chain(self.later_versions.iter())
    }

    /// This allocation's current version.
    pub fn current(&self) -> &AllocationVersion {
        self.later_versions.last().unwrap_or(&self.created_version)
    }

    /// The version this allocation was created with.
    pub fn created(&self) -> &AllocationVersion {
        &self.created_version
    }

    /// Record a new current version.
    pub fn push_version(&mut self, version: AllocationVersion) {
        self.later_versions.push(version);
    }

    /// This allocation as it existed at share revision `revision`: the last
    /// version written at or before it, or `None` when the allocation did not
    /// exist yet.
    pub fn version_at(&self, revision: i32) -> Option<&AllocationVersion> {
        self.versions()
            .rev()
            .find(|version| version.revision <= revision)
    }

    /// The share revision this allocation was created at.
    pub fn created_revision(&self) -> i32 {
        self.created_version.revision
    }
}

/// A marketplace revenue share, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct MarketplaceRevenueShare {
    /// `ProductId` — the caller's AWS Marketplace product identifier.
    pub product_id: String,
    /// `Arn`.
    pub arn: String,
    /// `Catalog`.
    pub catalog: CatalogName,
    /// `ProductCode`.
    pub product_code: String,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// The current revision; `1` right after the create.
    pub revision: i32,
    /// When each revision was written: `revision_dates[r - 1]` is revision `r`.
    pub revision_dates: Vec<SystemTime>,
    /// The tags the create request carried.
    pub tags: Vec<Tag>,
    /// Every allocation under this share, keyed by id. A `BTreeMap` over
    /// zero-padded ids, so iteration order is creation order and a list page
    /// resumes deterministically.
    pub allocations: BTreeMap<String, MarketplaceRevenueShareAllocation>,
}

impl MarketplaceRevenueShare {
    /// `LastModifiedDate` — when the current revision was written.
    pub fn last_modified_date(&self) -> SystemTime {
        *self.revision_dates.last().unwrap_or(&self.created_date)
    }

    /// When `revision` was written, falling back to the creation instant for a
    /// revision this share never reached.
    pub fn modified_at(&self, revision: i32) -> SystemTime {
        usize::try_from(revision)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.revision_dates.get(index))
            .copied()
            .unwrap_or(self.created_date)
    }

    /// Record a new revision written at `now` and answer its number.
    pub fn bump_revision(&mut self, now: SystemTime) -> i32 {
        self.revision += 1;
        self.revision_dates.push(now);
        self.revision
    }

    /// Every allocation that existed at `revision`, paired with the version that
    /// was current then, in allocation-id (creation) order.
    pub fn allocations_at(
        &self,
        revision: i32,
    ) -> impl Iterator<Item = (&MarketplaceRevenueShareAllocation, &AllocationVersion)> {
        self.allocations
            .values()
            .filter_map(move |allocation| Some((allocation, allocation.version_at(revision)?)))
    }

    /// `TotalMarketplaceRevenueShareAllocationCount` and
    /// `TotalActiveMarketplaceRevenueShareAllocationCount` at `revision`, both
    /// counted from the stored allocations rather than held as a field.
    pub fn allocation_counts_at(&self, revision: i32) -> (i32, i32) {
        let mut total: i32 = 0;
        let mut active: i32 = 0;
        for (_, version) in self.allocations_at(revision) {
            total = total.saturating_add(1);
            if version.status == AllocationStatus::Active {
                active = active.saturating_add(1);
            }
        }
        (total, active)
    }

    /// Whether this record is the one `CreateMarketplaceRevenueShare` would mint
    /// for these parameters — the comparison an idempotent replay makes before
    /// answering with it, so a `ClientToken` reused with DIFFERENT parameters
    /// conflicts instead of quietly answering with the first call's share.
    pub fn matches_create(
        &self,
        catalog: CatalogName,
        product_id: &str,
        tags: Option<&Vec<Tag>>,
    ) -> bool {
        self.catalog == catalog
            && self.product_id == product_id
            && tag_pairs(Some(&self.tags)) == tag_pairs(tags)
    }
}

/// A tag list as a comparable value, and empty when the member is absent.
pub fn tag_pairs(tags: Option<&Vec<Tag>>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .map(|tags| {
            tags.iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// Per-service state for com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState {
    /// Every revenue share, keyed by `(Catalog, ProductId)` — a `BTreeMap`, so
    /// every list page is in a stable, resumable order and the two catalogs are
    /// contiguous ranges.
    pub marketplace_revenue_shares: BTreeMap<(CatalogName, String), MarketplaceRevenueShare>,
    /// `ClientToken` -> the share key it created, so an `@idempotent` create
    /// replayed with the same token answers with the share it already minted.
    pub share_idempotency: HashMap<String, (CatalogName, String)>,
    /// `ClientToken` -> `(Catalog, ProductId, allocation id)`, for the same
    /// reason. Kept separate from `share_idempotency` because the two creates are
    /// different operations: one token reused across both must not make a
    /// `CreateMarketplaceRevenueShareAllocation` answer with a share.
    pub allocation_idempotency: HashMap<String, (CatalogName, String, String)>,
    /// `ClientToken` -> `(Catalog, ProductId, allocation id, share revision the
    /// update produced)`, so an `@idempotent` update replayed with the same token
    /// answers with the version it wrote instead of failing the optimistic
    /// concurrency check against a revision it has itself already moved.
    pub allocation_update_idempotency: HashMap<String, (CatalogName, String, String, i32)>,
    /// Source of the sequence number behind every minted allocation id.
    /// Monotonic, so no two allocations in one store ever share an id.
    pub next_sequence: u64,
}

impl ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState {
    /// The next sequence number: unique within this store for the life of the
    /// process.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// The share `(catalog, product_id)` names, if the store holds it.
    pub fn share(
        &self,
        catalog: CatalogName,
        product_id: &str,
    ) -> Option<&MarketplaceRevenueShare> {
        self.marketplace_revenue_shares
            .get(&(catalog, product_id.to_string()))
    }

    /// The share `(catalog, product_id)` names, for mutation.
    pub fn share_mut(
        &mut self,
        catalog: CatalogName,
        product_id: &str,
    ) -> Option<&mut MarketplaceRevenueShare> {
        self.marketplace_revenue_shares
            .get_mut(&(catalog, product_id.to_string()))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState>)
}

/// The Region an ARN this request mints belongs in: the one the request's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
///
/// A scope region that is not an ARN region segment (`[a-z0-9-]+`, from
/// `ResourceARN`'s own pattern) is not used, so a crafted `Authorization` header
/// can never produce a stored `Arn` that violates that pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The tracing target every audit record this service emits carries.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the structured audit record a state-changing operation owes: the
/// operation, the resource it changed, how the call ended, the Region it was
/// signed for and the access key id the caller presented. The subscriber stamps
/// the time.
///
/// Only identifiers are recorded. The signature, the secret, the request body
/// and every other header stay out of it, so a record can never be the thing
/// that leaks a credential.
pub fn audit(parts: &::http::request::Parts, operation: &str, resource: &str, outcome: &str) {
    ::tracing::info!(
        target: AUDIT_TARGET,
        operation,
        resource,
        outcome,
        region = %request_region(parts),
        access_key_id = %::engine::extract_akid(&parts.headers),
        "PartnerCentral Revenue Measurement state change"
    );
}

/// The ARN of a marketplace revenue share, through the one ARN builder the
/// framework vends (so the account segment is the canonical emulator account).
pub fn share_arn(region: &str, catalog: CatalogName, product_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "catalog/{}/{MARKETPLACE_REVENUE_SHARE_RESOURCE}/{product_id}",
            catalog.as_wire()
        ),
    )
}

/// The ARN of one allocation: its parent share's ARN plus the allocation
/// sub-resource. `Arn` on the allocation shapes carries no `@pattern`, and the
/// model documents it as "the ARN of the allocation", so it is the share ARN
/// extended rather than the share ARN repeated.
///
/// Which of the two ARNs a response carries is per member, as the model words
/// it: `CreateMarketplaceRevenueShareAllocationOutput`,
/// `GetMarketplaceRevenueShareAllocationOutput` and the list summaries document
/// `Arn` as the allocation's, so they carry this one, while
/// `UpdateMarketplaceRevenueShareAllocationOutput` documents its `Arn` as "the
/// ARN of the PARENT marketplace revenue share" and so carries
/// [`AllocationView::share_arn`].
pub fn allocation_arn(
    region: &str,
    catalog: CatalogName,
    product_id: &str,
    allocation_id: &str,
) -> String {
    format!(
        "{}/{ALLOCATION_RESOURCE}/{allocation_id}",
        share_arn(region, catalog, product_id)
    )
}

/// An allocation id for sequence number `sequence`, matching
/// `MarketplaceRevenueShareAllocationId`'s `^mrsa-[A-Za-z0-9]{13}$`. Zero-padded
/// to exactly 13 digits, so id order is creation order and a list page resumes
/// deterministically.
///
/// The modulo keeps the id exactly 13 characters however far the sequence runs;
/// two ids could only collide past 10^13 writes, which
/// [`MAX_REVENUE_SHARES`] times [`MAX_ALLOCATIONS_PER_SHARE`] puts far out of
/// reach of any store this emulator holds.
pub fn allocation_id(sequence: u64) -> String {
    format!("{ALLOCATION_ID_PREFIX}{:013}", sequence % 10_000_000_000_000)
}

// A request that carries no `ClientToken` gets NO idempotency entry: an absent
// `@idempotencyToken` member means "no idempotency key", so this service mints
// no substitute for it. A server-minted token recorded in the same map a caller
// is matched against would make a value the client never sent replayable by a
// client that guessed it.

// `ProductName` is NOT held: the display name of an AWS Marketplace product
// lives in the Marketplace catalog, which this emulator does not have, and the
// member is optional on every output that carries it. Every handler therefore
// leaves it absent rather than answering with generated prose that would read as
// looked up. `ProductCode` IS answered, for the reason [`product_code`] states.

/// The AWS Marketplace product code of `product_id`: the identifier's own
/// suffix.
///
/// Why this member is answered while `ProductName` is not. `ProductCode` is an
/// opaque token — the model gives it neither a `@pattern` nor a `@length`, so
/// every string is a legal one — and the emulator has exactly one opaque token
/// that is unique per product, stable across every read, derived from a value
/// the caller itself supplied, and never regenerated per request: the product
/// identifier minus its `prod-` prefix. So the code is that token, and a
/// `ProductCodes` filter is a real filter over state. `ProductName`, by
/// contrast, is human-readable PROSE: there is no token to derive it from, only
/// text to invent, so that member is left absent (see the note above).
///
/// The divergence this leaves is stated plainly: on real AWS the product code is
/// a different string from the product id, assigned by AWS Marketplace, and this
/// emulator holds no Marketplace catalog to read it out of.
pub fn product_code(product_id: &str) -> String {
    product_id
        .strip_prefix("prod-")
        .unwrap_or(product_id)
        .to_string()
}

/// `Revision` / `MarketplaceRevenueShareRevision` as the wire carries it: a
/// `RevisionToken` matching `^[1-9][0-9]*$`.
pub fn revision_token(revision: i32) -> String {
    revision.to_string()
}

/// Parse a `RevisionToken`. The `@pattern` is checked at the boundary, so this
/// only has to refuse a value too large for the counter.
pub fn parse_revision_token(value: &str) -> Option<i32> {
    value.parse::<i32>().ok().filter(|revision| *revision >= 1)
}

// ---------------------------------------------------------------------------
// Calendar questions
//
// `AllocationEffectiveDateString` is `^\d{4}-\d{2}-\d{2}$`, which the boundary
// validator enforces — but the pattern admits `2024-13-45`, and the model
// documents that `EffectiveFrom` "must be the first day of a month" and
// `EffectiveUntil` "must be the last day of a month". Those are calendar
// questions, and a date-time type answers them: this module does no calendar
// arithmetic of its own. The type is `aws_smithy_types::DateTime`, the one this
// crate already carries every modelled `Timestamp` in (see [`wire_timestamp`])
// — so one date type across the service and no dependency added for it. Dates
// in `YYYY-MM-DD` also compare chronologically as strings, which is what every
// date filter and the "on or after today" rule below rely on.
// ---------------------------------------------------------------------------

/// The RFC 3339 instant an `AllocationEffectiveDateString` names: that day at
/// midnight UTC. It is the protocol's own timestamp format, which is what makes
/// the parse below the date type's rather than this module's.
const DATE_TIME_FORMAT: ::aws_smithy_types::date_time::Format =
    ::aws_smithy_types::date_time::Format::DateTime;

/// One day, in seconds.
const ONE_DAY: i64 = 86_400;

/// Parse an `AllocationEffectiveDateString`, or `None` when it names no day
/// this service can answer a calendar question about — including `2024-02-30`,
/// `2023-02-29` and `2024-13-01`, which the `@pattern` admits and the date type
/// refuses, and a year outside the range RFC 3339 represents (`0000-01-01`
/// parses to an instant the formatter will not render).
///
/// The render is part of the parse ON PURPOSE: the calendar questions below are
/// answered from the RENDERED date, so a value the formatter cannot render is
/// refused here, where the caller earns the modelled `ValidationException`,
/// rather than deeper down where only a panic would be left.
pub fn parse_date(value: &str) -> Option<::aws_smithy_types::DateTime> {
    // The member is a DATE, so it is read as that day at midnight UTC. The
    // length check is what keeps a longer string from reaching the parser
    // through the concatenation; the calendar itself is the parser's.
    if value.len() != 10 {
        return None;
    }
    let date =
        ::aws_smithy_types::DateTime::from_str(&format!("{value}T00:00:00Z"), DATE_TIME_FORMAT)
            .ok()?;
    format_date(date)?;
    Some(date)
}

/// `instant`'s UTC date as `YYYY-MM-DD`: the date type renders the instant and
/// this takes the date part — or `None` when the instant has no RFC 3339
/// rendering at all, which is every instant outside years `0001..=9999`.
///
/// Fallible rather than asserted: the instants this is asked about are derived
/// from request members (`EffectiveFrom`, `EffectiveUntil`), whose `@pattern`
/// admits a year the formatter refuses, so an unrenderable instant is a request
/// to refuse and never a reason to unwind a handler.
pub fn format_date(instant: ::aws_smithy_types::DateTime) -> Option<String> {
    let rendered = instant.fmt(DATE_TIME_FORMAT).ok()?;
    Some(rendered.chars().take(10).collect())
}

/// Whether `date` is the first day of its month. A date with no rendering is
/// not the first day of anything — [`parse_date`] has already refused it.
pub fn is_first_day_of_month(date: ::aws_smithy_types::DateTime) -> bool {
    format_date(date).is_some_and(|rendered| rendered.ends_with("-01"))
}

/// Whether `date` is the last day of its month: one day later falls in another
/// month. Month lengths and the leap rule are the date type's — this only adds
/// a day's worth of seconds and compares the rendered `YYYY-MM`.
pub fn is_last_day_of_month(date: ::aws_smithy_types::DateTime) -> bool {
    let Some(this) = format_date(date) else {
        return false;
    };
    let Some(next) = date
        .secs()
        .checked_add(ONE_DAY)
        .map(::aws_smithy_types::DateTime::from_secs)
    else {
        return false;
    };
    match format_date(next) {
        Some(next) => month_of(&next) != month_of(&this),
        // `date` renders and the day after it does not, so `date` is the last
        // day RFC 3339 represents (`9999-12-31`) — and the last day of its
        // month, since the range ends on a month boundary. Answered here
        // instead of unwinding: the value came from a request member.
        None => true,
    }
}

/// The `YYYY-MM` of a `YYYY-MM-DD` date string.
fn month_of(date: &str) -> &str {
    date.get(0..7).unwrap_or(date)
}

/// `instant`'s UTC date as `YYYY-MM-DD`, so it compares with an
/// `AllocationEffectiveDateString` directly — or `None` when the instant lies
/// outside the years RFC 3339 renders (see [`format_date`]). The one caller
/// asks about the emulator's own clock, so it answers the modelled
/// [`clock_unavailable`] 500 rather than guessing a date.
pub fn utc_date_string(instant: SystemTime) -> Option<String> {
    format_date(::aws_smithy_types::DateTime::from(instant))
}

/// `RevenueSharePercent` in hundredths of a percent, or `None` when the string
/// is not one. The `@pattern` (`^\d{1,3}(\.\d{1,2})?$`) is enforced at the
/// boundary; this is what lets the handler refuse a percentage above 100.
pub fn percent_hundredths(value: &str) -> Option<u32> {
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (value, ""),
    };
    if whole.is_empty() || whole.len() > 3 || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if fraction.len() > 2 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: u32 = whole.parse().ok()?;
    let fraction: u32 = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u32>().ok()? * 10,
        _ => fraction.parse().ok()?,
    };
    Some(whole * 100 + fraction)
}

/// One hundred percent, in the units [`percent_hundredths`] answers in: the
/// largest share of revenue there is.
pub const MAX_PERCENT_HUNDREDTHS: u32 = 10_000;

// ---------------------------------------------------------------------------
// Modelled failures
// ---------------------------------------------------------------------------

/// The modelled 500 for a request that could not reach this service's store.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the PartnerCentral Revenue Measurement state store is unavailable".to_string(),
    }
}

/// The modelled 500 for an emulator clock that names no date the calendar
/// rules in this module can compare a request's dates against.
pub fn clock_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the service clock is outside the range of dates this service compares against"
            .to_string(),
    }
}

/// The modelled 404 for an identifier that names nothing in the store.
pub fn not_found(message: impl Into<String>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: message.into(),
        reason: "RESOURCE_NOT_FOUND".to_string(),
    }
}

/// The modelled 404 for a revenue share this catalog does not hold.
pub fn share_not_found(catalog: CatalogName, product_id: &str) -> ResourceNotFoundException {
    not_found(format!(
        "no marketplace revenue share for ProductId {product_id} in catalog {}",
        catalog.as_wire()
    ))
}

/// The modelled 404 for an allocation this share does not hold.
pub fn allocation_not_found(product_id: &str, allocation_id: &str) -> ResourceNotFoundException {
    not_found(format!(
        "no marketplace revenue share allocation {allocation_id} under ProductId {product_id}"
    ))
}

/// The modelled 404 for a share revision the store never reached.
///
/// `requested` is the caller's token text, not a parsed number: `RevisionToken`
/// admits up to 19 digits, which no `i32` holds, so re-rendering a parse would
/// report a revision the caller never sent.
pub fn revision_not_found(
    member: &str,
    product_id: &str,
    requested: &str,
    latest: i32,
) -> ResourceNotFoundException {
    not_found(format!(
        "{member} {requested} does not exist for ProductId {product_id}; \
         its latest revision is {latest}"
    ))
}

/// The modelled 409 for a request that conflicts with the stored resource,
/// carrying one of `ConflictExceptionReason`'s values.
pub fn conflict(reason: &str, message: impl Into<String>) -> ConflictException {
    ConflictException {
        message: message.into(),
        reason: reason.to_string(),
    }
}

/// The modelled 409 for a create of a `ProductId` the catalog already holds.
pub fn already_exists(catalog: CatalogName, product_id: &str) -> ConflictException {
    conflict(
        "RESOURCE_ALREADY_EXISTS",
        format!(
            "catalog {} already holds a marketplace revenue share for ProductId {product_id}",
            catalog.as_wire()
        ),
    )
}

/// The modelled 409 for a `ClientToken` replayed with parameters that do not
/// match the resource it already wrote.
///
/// The message names the TOKEN's reuse and not the resource behind it: that
/// resource is the one the FIRST request wrote, which this request has not been
/// shown to be allowed to see, so its ARN is not disclosed here.
pub fn client_token_conflict() -> ConflictException {
    conflict(
        "CONFLICT_CLIENT_TOKEN",
        "the ClientToken of this request was already used for a request with different \
         parameters",
    )
}

/// The modelled 409 for the optimistic concurrency check
/// `MarketplaceRevenueShareRevision` carries: the caller's revision is not the
/// share's current one, so the share moved under it.
pub fn revision_mismatch(product_id: &str, supplied: &str, current: i32) -> ConflictException {
    conflict(
        "REVISION_MISMATCH",
        format!(
            "MarketplaceRevenueShareRevision {supplied} is not the current revision of \
             ProductId {product_id}, which is {current}"
        ),
    )
}

/// The modelled 400 for a member this service refuses, naming the member and the
/// rule it broke — the text is built from the request, so a caller can fix the
/// request from it alone.
pub fn field_validation(
    reason: &str,
    code: &str,
    member: &str,
    message: impl Into<String>,
) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: format!("{member}: {message}"),
        reason: reason.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
            code: code.to_string(),
        }]),
    }
}

/// The modelled 400 for a member whose value the request could not carry
/// correctly (`REQUEST_VALIDATION_FAILED`).
pub fn invalid_value(member: &str, message: impl Into<String>) -> ValidationException {
    field_validation(
        "REQUEST_VALIDATION_FAILED",
        "INVALID_VALUE",
        member,
        message,
    )
}

/// The modelled 400 for a request that is wrong as a WHOLE rather than in one
/// member — a combination of members that cannot hold together. It carries no
/// `fieldList`, because naming one member of the set would point a caller at a
/// member that is not at fault.
pub fn request_validation(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        reason: "REQUEST_VALIDATION_FAILED".to_string(),
        field_list: None,
    }
}

/// The modelled 400 for an enum member carrying a value the model does not
/// declare.
pub fn invalid_enum(member: &str, allowed: &[&str]) -> ValidationException {
    field_validation(
        "REQUEST_VALIDATION_FAILED",
        "INVALID_ENUM_VALUE",
        member,
        format!("must be one of {}", allowed.join(", ")),
    )
}

/// The modelled 400 for a `@required` enum member the request left empty. The
/// deserializer materializes an absent enum member as the empty string, so this
/// is the refusal the handler owes it.
pub fn required_member(member: &str) -> ValidationException {
    field_validation(
        "REQUEST_VALIDATION_FAILED",
        "REQUIRED_FIELD_MISSING",
        member,
        "must be present",
    )
}

/// The modelled 400 for a request that is well-formed but asks for something the
/// resource's own rules forbid (`BUSINESS_VALIDATION_FAILED`).
pub fn business_validation(member: &str, message: impl Into<String>) -> ValidationException {
    field_validation(
        "BUSINESS_VALIDATION_FAILED",
        "ACTION_NOT_PERMITTED",
        member,
        message,
    )
}

/// The modelled 402 for a create this store has no room for, naming the ceiling
/// it would pass.
///
/// `ServiceQuotaExceededExceptionReason` declares only
/// `ATTRIBUTION_LIMIT_EXCEEDED` and `TAG_LIMIT_EXCEEDED`; the former is the only
/// one that names a resource-count limit at all, so it is the reason a resource
/// ceiling is reported with even though the resource here is a revenue share
/// rather than a revenue attribution. The wire message states the ceiling and
/// the resource and nothing else.
pub fn quota_exceeded(resource: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!("this emulator holds at most {limit} {resource} resources"),
        reason: "ATTRIBUTION_LIMIT_EXCEEDED".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Reading an allocation out, and paging
// ---------------------------------------------------------------------------

/// One allocation as every allocation-shaped response carries it. The four
/// operations that answer with an allocation declare four distinct output shapes
/// with identical members, so they share this one view built from state and each
/// maps it into its own shape.
#[derive(Debug, Clone)]
pub struct AllocationView {
    /// `MarketplaceRevenueShareAllocationId`.
    pub allocation_id: String,
    /// `ProductId`.
    pub product_id: String,
    /// `Arn`, as the three outputs that document it as the ALLOCATION's ARN
    /// carry it.
    pub arn: String,
    /// The parent share's ARN, which is what
    /// `UpdateMarketplaceRevenueShareAllocationOutput` documents its own `Arn`
    /// member as.
    pub share_arn: String,
    /// `EffectiveFrom`.
    pub effective_from: String,
    /// `EffectiveUntil`.
    pub effective_until: Option<String>,
    /// `RevenueSharePercent`.
    pub revenue_share_percent: String,
    /// `Status`.
    pub status: String,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// `LastModifiedDate`.
    pub last_modified_date: SystemTime,
    /// `LatestMarketplaceRevenueShareRevision` — the parent share's LATEST
    /// revision, which is not the revision the allocation was read at.
    pub latest_marketplace_revenue_share_revision: String,
}

/// The view of `allocation` at `version`, under `share`.
pub fn allocation_view(
    share: &MarketplaceRevenueShare,
    allocation: &MarketplaceRevenueShareAllocation,
    version: &AllocationVersion,
) -> AllocationView {
    AllocationView {
        allocation_id: allocation.allocation_id.clone(),
        product_id: allocation.product_id.clone(),
        arn: allocation.arn.clone(),
        share_arn: share.arn.clone(),
        effective_from: version.effective_from.clone(),
        effective_until: version.effective_until.clone(),
        revenue_share_percent: version.revenue_share_percent.clone(),
        status: version.status.as_wire().to_string(),
        created_date: allocation.created_date,
        last_modified_date: version.modified_date,
        latest_marketplace_revenue_share_revision: revision_token(share.revision),
    }
}

/// A timestamp as the wire carries it.
pub fn wire_timestamp(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(instant)
}

/// `instant` as whole seconds and nanoseconds since the Unix epoch, which is a
/// total order a list can sort timestamps by.
pub fn epoch_parts(instant: SystemTime) -> (i64, u32) {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => (
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
            elapsed.subsec_nanos(),
        ),
        Err(_) => (0, 0),
    }
}

/// `instant` as whole seconds since the Unix epoch — the precision the two
/// creation-time list filters compare at.
///
/// A `Timestamp` travels this protocol as an epoch value whose fraction is a
/// float, so a sub-second bound a caller echoes back from a previous response
/// does not reconstruct bit-for-bit. Whole seconds do, so comparing at second
/// granularity is what makes a bound a caller READ OUT of a `CreatedDate` mean
/// exactly that instant rather than a nanosecond either side of it.
pub fn epoch_whole_seconds(instant: SystemTime) -> i64 {
    epoch_parts(instant).0
}

/// A modelled `Timestamp` in the same whole seconds [`epoch_whole_seconds`]
/// answers in.
pub fn wire_whole_seconds(at: ::aws_smithy_types::DateTime) -> i64 {
    at.secs()
}

/// One page of a `@paginated` list over `cursors`, which are the sorted
/// resource identifiers of every item the filters kept.
///
/// `NextToken` is the identifier the next page starts at: the only cursor this
/// store has that survives concurrent writes, and a non-whitespace string, so it
/// satisfies `NextToken`'s `^[\S]+$`. A token naming no kept item was never
/// issued for this query, so this answers `None` and the caller is told so rather
/// than silently served the whole collection from the start. `MaxResults` is
/// clamped to [`MAX_PAGE_SIZE`], and an absent one asks for a full page rather
/// than for everything. A `MaxResults` at or below zero is refused by the
/// handler, with the modelled `ValidationException`, before this is reached; the
/// lower end of the clamp here is defence in depth and never the refusal.
pub fn page(
    cursors: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<Page> {
    let start = match next_token {
        Some(token) => cursors.iter().position(|cursor| cursor == token)?,
        None => 0,
    };
    let limit = max_results.map_or(MAX_PAGE_SIZE, |value| value.clamp(1, MAX_PAGE_SIZE)) as usize;
    let end = start.saturating_add(limit).min(cursors.len());
    Some(Page {
        start,
        end,
        next_token: cursors.get(end).cloned(),
    })
}

/// Where one page of a list begins and ends in the sorted, filtered items, and
/// the `NextToken` that resumes after it.
#[derive(Debug, Clone)]
pub struct Page {
    /// Index of the first item on this page.
    pub start: usize,
    /// Index one past the last item on this page.
    pub end: usize,
    /// `NextToken`, absent on the last page.
    pub next_token: Option<String>,
}

// ---------------------------------------------------------------------------
// The allocation date and percentage rules the model documents
// ---------------------------------------------------------------------------

/// `EffectiveFrom` as a calendar date.
///
/// The model documents "Must be the first day of a month", and the `@pattern`
/// the boundary enforces still admits a day no calendar has (`2024-02-30`), so
/// both are checked here.
pub fn validate_effective_from(
    member: &str,
    value: &str,
) -> Result<::aws_smithy_types::DateTime, ValidationException> {
    let date = parse_date(value).ok_or_else(|| {
        invalid_value(
            member,
            format!("{value:?} is not a calendar date in YYYY-MM-DD between 0001 and 9999"),
        )
    })?;
    if !is_first_day_of_month(date) {
        return Err(business_validation(
            member,
            format!("{value:?} must be the first day of a month"),
        ));
    }
    Ok(date)
}

/// `EffectiveUntil` as a calendar date.
///
/// The model documents "Must be the last day of a month (YYYY-MM-DD)".
pub fn validate_effective_until(
    member: &str,
    value: &str,
) -> Result<::aws_smithy_types::DateTime, ValidationException> {
    let date = parse_date(value).ok_or_else(|| {
        invalid_value(
            member,
            format!("{value:?} is not a calendar date in YYYY-MM-DD between 0001 and 9999"),
        )
    })?;
    if !is_last_day_of_month(date) {
        return Err(business_validation(
            member,
            format!("{value:?} must be the last day of a month"),
        ));
    }
    Ok(date)
}

/// The allocation's effective interval must not end before it starts. The model
/// states no bound relating the two members, so this is the emulator's own
/// interval check, reported as a business validation.
pub fn validate_interval(from: &str, until: &str) -> Result<(), ValidationException> {
    // `YYYY-MM-DD` orders chronologically as a string, and both members have
    // already been parsed as calendar dates by the callers.
    if until < from {
        return Err(business_validation(
            "EffectiveUntil",
            format!("{until:?} is before EffectiveFrom {from:?}"),
        ));
    }
    Ok(())
}

/// `RevenueSharePercent` must be a share of revenue, so it cannot exceed 100.
/// The `@pattern` admits three integer digits, so the handler owns this bound.
pub fn validate_percent(member: &str, value: &str) -> Result<(), ValidationException> {
    let hundredths = percent_hundredths(value)
        .ok_or_else(|| invalid_value(member, format!("{value:?} is not a percentage")))?;
    if hundredths > MAX_PERCENT_HUNDREDTHS {
        return Err(business_validation(
            member,
            format!("{value:?} is more than 100 percent"),
        ));
    }
    Ok(())
}

/// `Catalog` as this service holds it, or the modelled 400 the wire value earns.
///
/// `CatalogName` is an enum SHAPE: the boundary validator does not check its
/// membership, and the deserializer materializes an absent enum member as the
/// empty string, so both cases are a handler's to refuse.
pub fn parse_catalog(value: &str) -> Result<CatalogName, ValidationException> {
    if value.is_empty() {
        return Err(required_member("Catalog"));
    }
    CatalogName::from_wire(value).ok_or_else(|| invalid_enum("Catalog", CATALOG_NAME_VALUES))
}

/// The modelled 400 for a `MaxResults` that asks for a page of no items.
pub fn invalid_max_results(requested: i32) -> ValidationException {
    invalid_value(
        "MaxResults",
        format!("{requested} is not a page size; it must be at least 1"),
    )
}

/// The modelled 400 for a `NextToken` this query never issued.
pub fn invalid_next_token() -> ValidationException {
    invalid_value(
        "NextToken",
        "is not a pagination token this query issued; restart the list without it",
    )
}
