// SPDX-License-Identifier: CC0-1.0

//! The REGION slice of this service's state: the store the engine holds, the
//! per-region partitions of resources inside it, the handle a handler reads one
//! partition through, and the decode that says which region a request is for.
//!
//! One file for the whole slice — the store, its ceiling, the partition handle
//! and the accessors — so the slice can be read, and merged, on its own, exactly
//! as the [`super::account_configuration`] slice is. The records themselves, and
//! the mints the partitions share, stay in the parent module.
//!
//! MediaLive is a REGIONAL service: every resource it mints belongs to the
//! region whose endpoint created it, which is the region its ARN names. So the
//! resources of one region live in a partition of their own and
//! [`service_state`] hands a handler the partition of the region ITS OWN request
//! was signed for, and no other. The isolation is therefore STRUCTURAL rather
//! than a filter each operation has to remember: an operation reaches one
//! region's resources because that is the only store it is given, so every
//! listing, lookup, reference and mutation answers for that region alone and an
//! identifier of another region's resource resolves to nothing.
//!
//! Two things the partitioning does NOT change, each held here:
//!
//! * the store's SIZE is bounded, by a ceiling on the partitions themselves
//!   ([`MAX_RESOURCE_REGIONS`]) as well as by each resource store's own ceiling
//!   inside a partition, because the partition key is request metadata the
//!   caller chooses;
//! * a minted id stays the ACCOUNT's: every partition mints out of the one
//!   shared [`super::IdMints`], so two regions never hand out the same id.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{AccountConfigurationRecord, ComAmazonawsMedialiveMedialiveState, IdMints};

/// Everything this service holds in the shared engine: one PARTITION of
/// resources per region, beside the account's own per-region settings.
///
/// A handler on a resource reaches one partition ([`service_state`]); a handler
/// whose subject is the account rather than a region's resources reaches the
/// store itself ([`service_store`]).
#[derive(Debug, Default)]
pub struct ComAmazonawsMedialiveMedialiveStore {
    /// The resources of each region this service has been reached in, keyed by
    /// that region. A `BTreeMap` so a debug dump of the store is in region
    /// order rather than in first-touch order.
    ///
    /// PRIVATE, so the ceiling on how many partitions the store keeps stays an
    /// invariant of [`Self::region`] rather than of each handler.
    regions: BTreeMap<String, RegionState>,
    /// The id mints every one of those partitions hands its resources out of.
    ///
    /// SHARED rather than per partition: an id is what a caller addresses a
    /// resource by and what it records in its own books, so one this account
    /// holds in one region must not name another resource in a second region.
    /// Every partition therefore mints out of the same counters, and the whole
    /// account's ids stay distinct however many regions it is reached in.
    mints: Arc<IdMints>,
    /// The account's MediaLive configuration in each region an
    /// `UpdateAccountConfiguration` has written one in, keyed by the region the
    /// writing request was signed for. A `BTreeMap` so a debug dump of the store
    /// is in region order rather than in write order.
    ///
    /// It sits beside the region partitions rather than inside one because it is
    /// not a resource a caller creates — it is the account's own settings, so a
    /// region nothing has configured yet reads as the unset configuration rather
    /// than as missing, and its bound is on the configurations the whole store
    /// keeps.
    ///
    /// Reached only from the [`super::account_configuration`] slice, so the
    /// partial-update rule, the blank-clears-the-key rule and that bound stay
    /// invariants of its methods rather than of each handler.
    pub(super) account_configurations: BTreeMap<String, AccountConfigurationRecord>,
}

impl ComAmazonawsMedialiveMedialiveStore {
    /// The partition holding `region`'s resources, created empty the first time
    /// a request reaches this service in that region.
    ///
    /// `None` once the store already holds [`MAX_RESOURCE_REGIONS`] partitions
    /// and `region` is not one of them — the ONE growth site this slice owns,
    /// and the bound that caps it, checked BEFORE the partition is allocated:
    /// the key is request metadata the caller chooses, so without a ceiling a
    /// loop over made-up region names would grow the emulator's heap without
    /// limit. A region the store already holds is never refused, and nothing is
    /// evicted: a full store refuses the new region instead, and the caller
    /// answers that with the modelled server error it answers a missing store
    /// with.
    fn region(&mut self, region: &str) -> Option<RegionState> {
        if let Some(held) = self.regions.get(region) {
            return Some(held.clone());
        }
        if self.regions.len() >= MAX_RESOURCE_REGIONS {
            return None;
        }
        let mints = Arc::clone(&self.mints);
        Some(
            self.regions
                .entry(region.to_string())
                .or_insert_with(|| RegionState::new(region, mints))
                .clone(),
        )
    }
}

/// The most regions this emulator keeps a partition of resources for.
///
/// A ceiling on the whole store, not a quota of AWS's, for the same reason as
/// [`super::MAX_ACCOUNT_CONFIGURATION_REGIONS`]: the partition key is the region
/// a request was signed for, which the caller chooses rather than this service.
/// It bounds the PRODUCT the partitioning introduces — what the store can hold
/// is this ceiling times each per-region resource ceiling (`MAX_CHANNELS`,
/// `MAX_INPUTS`, … , each enforced inside a partition), both factors being
/// capped — rather than leaving the number of partitions open. It is set well
/// above the number of regions MediaLive is offered in, so no sequence of real
/// regional endpoints can reach it.
pub const MAX_RESOURCE_REGIONS: usize = 64;

/// A handle to ONE region's resources. Clone-cheap (an `Arc` inside), so the
/// store hands a handler its own handle and the handler reads and writes that
/// region through the guard [`Self::lock`] gives back.
#[derive(Clone, Debug)]
pub struct RegionState(Arc<Mutex<ComAmazonawsMedialiveMedialiveState>>);

impl RegionState {
    /// An empty partition, minting its resources out of `mints` — the store's
    /// own set, which every partition shares.
    ///
    /// `region` is what the partition is for: a resource belongs to the region
    /// whose endpoint created it, and the monitor template resources the SERVICE
    /// publishes ([`super::managed_monitor_templates`]) are read through every
    /// regional endpoint, so the partition comes up carrying them with that
    /// region's ARNs.
    ///
    /// There is no `Default`: a partition that minted out of counters of its own
    /// could hand out an id another region had already handed out.
    fn new(region: &str, mints: Arc<IdMints>) -> Self {
        Self(Arc::new(Mutex::new(
            ComAmazonawsMedialiveMedialiveState::with_mints(region, mints),
        )))
    }

    /// Lock this region's resources for exclusive access.
    ///
    /// Recovers from a poisoned lock rather than propagating the panic, exactly
    /// as the engine's own handle does: a long-lived emulator must stay
    /// available to every later request, so one panicking handler must not wedge
    /// a region into failing until the process restarts.
    pub fn lock(&self) -> MutexGuard<'_, ComAmazonawsMedialiveMedialiveState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// This service's whole store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// What an operation whose subject is the ACCOUNT rather than a region's
/// resources reaches (the account configuration, which is region-keyed on the
/// store itself). An operation on a resource takes [`service_state`] instead.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMedialiveMedialiveStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMedialiveMedialiveStore>)
}

/// The resources of the region `parts` was signed for: call it with a handler's
/// `&*self.engine` and its `ctx.parts()`.
///
/// This is the ONLY store an operation on a resource is given, which is what
/// makes a resource reachable through the region that created it and through no
/// other: a request signed for another region is served from that region's own
/// (empty, until it mints something) partition, so it lists nothing of this
/// one's and resolves none of its ids.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
) -> Option<RegionState> {
    service_state_in(engine, &request_region(parts))
}

/// The same partition, named by its region rather than read off a request.
///
/// The seam for a caller that holds the region itself rather than the request it
/// came from — a test that seeds state no operation of this service writes (a
/// channel's or a cluster's alert log, a device's thumbnail).
pub fn service_state_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Option<RegionState> {
    service_store(engine)?.lock().region(region)
}

/// The region this request was signed for, which is the region every ARN this
/// request mints names and the partition of resources it is served from.
///
/// TOTAL: whatever the request's headers hold, this answers with a region.
/// The credential scope is read by the core's own parser, and a request that
/// carries no SigV4 credential it can read — or one whose scope names an empty
/// region — is treated as being in [`DEFAULT_REGION`] rather than refused: there
/// is no indexing, no slicing and no unwrapping of header text here, so a
/// malformed or absent `Authorization` header cannot fail this decode, and the
/// request goes on to be served out of the default region's partition.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region a request that names none is treated as being in (`emulator_arn`'s
/// documented default).
const DEFAULT_REGION: &str = "us-east-1";
