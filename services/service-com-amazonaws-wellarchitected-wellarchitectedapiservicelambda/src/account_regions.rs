// SPDX-License-Identifier: CC0-1.0

//! How this service PARTITIONS its state by Region, and what bounds that partitioning.
//!
//! The Well-Architected Tool is a REGIONAL service: a workload, custom lens, review
//! template, profile or agent profile belongs to the Region whose endpoint created it,
//! its ARN carries that Region, and the documented uniqueness rules (`WorkloadName`
//! "must be unique within an account within an Amazon Web Services Region") and the
//! quotas are counted inside ONE Region. So the engine holds one
//! [`super::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState`] per Region
//! ([`RegionalStores`]) rather than one store for the account, and a handler resolves the
//! store of the Region its own request was signed for ([`service_state`]) and can reach
//! no other.
//!
//! This module is what the engine holds this service's state AS, so it sits beside
//! `state.rs` and is a module OF it (the `#[path]` there): the records and their
//! partitioning read as one unit, and `super` re-exports everything here, so a handler
//! names `crate::state::…` for its Region's store exactly as it does for a workload.
//!
//! ## Where the Region comes from, and what bounds it
//!
//! From the request itself, and from nothing else: [`request_region`] reads the SigV4
//! credential scope of the INBOUND `authorization` header — the same Region every ARN
//! this service mints is composed from — through the server's own total parser
//! (`::emulator_server::sigv4_credential_scope` answers `Option`, so a malformed or
//! absent header is a `None` here, never a panic). No environment variable, no file and
//! no metadata endpoint is read for it, here or anywhere else in this crate, and the
//! Region reaches only a map key and the modelled messages.
//!
//! A request's credential scope is the CALLER's own, so the string in it is caller-chosen
//! — which would make the Regions reaching a store keyed by it unbounded. It does not:
//! [`partition_region`] answers from the PUBLISHED Region list and nothing else
//! ([`super::AWS_REGIONS`]), attributing a scope that names no Region of the partition to
//! [`super::DEFAULT_REGION`], exactly as a request carrying no scope at all is attributed.
//! It is the one producer of a partition key — [`request_region`] and [`region_state`]
//! both answer through it — so the key space is a fixed set of [`MAX_REGIONS`] values, and
//! the store holds at most that many stores however many Region strings a caller signs
//! for. Each is bounded in turn by the per-Region resource quotas
//! ([`super::MAX_WORKLOADS_PER_ACCOUNT_REGION`], [`super::MAX_REVIEW_TEMPLATES`],
//! [`super::MAX_PROFILES`], [`super::MAX_CUSTOM_LENSES_PER_ACCOUNT_REGION`],
//! [`super::MAX_AGENT_PROFILES`]).
//!
//! A READ adds nothing even inside that space: [`RegionalStores::region`] hands back one
//! shared empty store for a Region the account holds nothing in, so a reporting operation
//! answers for such a Region without materialising a partition.

use std::collections::HashMap;

use super::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState;

/// The most stores [`RegionalStores`] can hold: one per Region the published list names,
/// which is the whole key space [`partition_region`] answers from.
///
/// It is the bound on the Region dimension of this store, and it is structural rather
/// than enforced: a caller cannot name a key outside that list, so nothing has to refuse
/// a further Region.
pub const MAX_REGIONS: usize = super::AWS_REGIONS.len();

/// The Region a store is keyed by for a request signed for `region`: `region` itself when
/// the published list names it, else [`super::DEFAULT_REGION`].
///
/// A Region the partition does not publish ([`super::AWS_REGIONS`]) has no endpoint of
/// this service, so a scope naming one names no Region a request could have reached, and
/// is attributed to the default Region exactly as a request carrying no scope is. That is
/// also what keeps this store's key space finite: the answer comes from a static list, so
/// a caller-chosen string cannot add a key to it — see this module's documentation.
pub fn partition_region(region: &str) -> &str {
    if super::AWS_REGIONS.contains(&region) {
        region
    } else {
        super::DEFAULT_REGION
    }
}

/// The region this request was signed for, from its SigV4 credential scope, as a Region
/// of the published list ([`partition_region`]); a request that carries no scope, or one
/// naming no published Region, is attributed to [`super::DEFAULT_REGION`].
///
/// Total: the scope is parsed by the server's own `Option`-answering parser, so a header
/// that is absent, empty or malformed resolves to the constant rather than failing.
pub fn request_region(parts: &::http::request::Parts) -> String {
    let signed = ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_default();
    partition_region(&signed).to_string()
}

/// Every Region's store, keyed by the Region name its requests are signed for — this
/// service's whole state in the shared engine.
///
/// One store per Region (see this module's documentation): a resource exists only in the
/// Region whose endpoint created it, every listing reports just that Region's resources,
/// each per-Region quota counts only them, and an id or ARN minted in one Region names
/// nothing in another. A handler picks its store with [`Self::region`] (to report) or
/// [`Self::region_mut`] (to write), passing the Region of the request's own credential
/// scope ([`request_region`]) — never the Region recorded on some other resource.
#[derive(Debug, Default)]
pub struct RegionalStores {
    /// Region name -> that Region's own store, at most [`MAX_REGIONS`] of them.
    ///
    /// Private: a Region's store is reached through the accessors below, so no handler
    /// can read one Region's resources while serving another Region's request, and this
    /// map has ONE growth site, keyed only by what [`partition_region`] answers.
    partitions: HashMap<String, ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>,
}

/// The one shared empty store every read of a Region holding nothing resolves to, so
/// such a read never has to create a partition to find nothing in it. Never mutated — it
/// is handed out behind a shared reference only.
fn empty_store() -> &'static ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
    static EMPTY: ::std::sync::OnceLock<
        ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState,
    > = ::std::sync::OnceLock::new();
    EMPTY.get_or_init(ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::default)
}

impl RegionalStores {
    /// One Region's store as it stands — what a request that only reads resolves to.
    ///
    /// Materialises nothing: a Region the account holds nothing in reads the shared
    /// [`empty_store`], which answers every read exactly as its own empty store would, so
    /// no request can grow this map by reading.
    pub fn region(
        &self,
        region: &str,
    ) -> &ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
        self.partitions.get(region).unwrap_or_else(|| empty_store())
    }

    /// One Region's store for a request that WRITES: the first write in a Region is what
    /// creates that Region's partition, and this is the ONE place the map grows.
    ///
    /// What bounds that growth is the key: `region` comes from [`partition_region`], whose
    /// answers are the published Region list, so this map cannot exceed [`MAX_REGIONS`]
    /// stores whatever a caller signs for.
    pub fn region_mut(
        &mut self,
        region: &str,
    ) -> &mut ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
        self.partitions.entry(region.to_string()).or_default()
    }

    /// How many Regions this store holds a partition for — at most [`MAX_REGIONS`].
    pub fn regions_held(&self) -> usize {
        self.partitions.len()
    }
}

/// A handle to ONE Region's store: what [`service_state`] hands a handler, so the
/// handler reads and writes the Region its request was signed for and no other.
pub struct RegionalServiceState {
    /// Every Region's store, as the shared engine holds it.
    stores: ::engine::ServiceState<RegionalStores>,
    /// The Region this handle resolves inside that store — one [`partition_region`]
    /// answered, so it is a Region of the published list.
    region: String,
}

impl RegionalServiceState {
    /// Lock the engine's store and resolve this handle's Region inside it. The guard
    /// hands out that Region's store, so a handler reads and writes it exactly as it
    /// would an unpartitioned one.
    ///
    /// The lock is the engine's own (`::engine::ServiceState::lock`), which RECOVERS a
    /// poisoned lock instead of propagating the panic, so one request that panics while
    /// holding it cannot wedge the service for the Regions it never touched.
    pub fn lock(&self) -> RegionalStateGuard<'_> {
        RegionalStateGuard {
            stores: self.stores.lock(),
            region: &self.region,
        }
    }

    /// The Region this handle resolves — the one the request was signed for.
    pub fn region(&self) -> &str {
        &self.region
    }
}

/// The locked store of one Region.
///
/// It derefs to that Region's own
/// [`super::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState`] — exactly
/// as a `MutexGuard` derefs to the value it guards — which is what lets every handler
/// reach its Region's resources through the same field and method names an unpartitioned
/// store would offer. A handler needing several fields of it at once takes `&mut *guard`
/// once, as it would with any other deref guard.
pub struct RegionalStateGuard<'a> {
    /// The locked store of every Region.
    stores: ::std::sync::MutexGuard<'a, RegionalStores>,
    /// The Region resolved inside it.
    region: &'a str,
}

impl RegionalStateGuard<'_> {
    /// The Region this guard resolves — the one the request was signed for.
    pub fn region(&self) -> &str {
        self.region
    }

    /// How many Regions the whole store holds a partition for
    /// ([`RegionalStores::regions_held`]) — at most [`MAX_REGIONS`].
    pub fn regions_held(&self) -> usize {
        self.stores.regions_held()
    }
}

impl ::std::ops::Deref for RegionalStateGuard<'_> {
    type Target = ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState;

    fn deref(&self) -> &Self::Target {
        self.stores.region(self.region)
    }
}

impl ::std::ops::DerefMut for RegionalStateGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.stores.region_mut(self.region)
    }
}

/// The store of the Region THIS request was signed for: call it with a handler's
/// `&*self.engine` and its `ctx.parts()`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
) -> Option<RegionalServiceState> {
    region_state(engine, &request_region(parts))
}

/// The store of one named Region, for a caller that already knows the Region rather than
/// holding the request it was signed for. The Region is resolved through
/// [`partition_region`], as a request's is, so this path cannot key a store by anything
/// the published list does not name either.
pub fn region_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Option<RegionalServiceState> {
    let region = partition_region(region).to_string();
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<RegionalStores>)
        .map(|stores| RegionalServiceState { stores, region })
}
