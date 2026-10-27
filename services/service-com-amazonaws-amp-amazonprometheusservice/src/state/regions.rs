// SPDX-License-Identifier: CC0-1.0

//! How this service PARTITIONS its state by Region, and what bounds that partitioning.
//!
//! Amazon Managed Service for Prometheus is a REGIONAL service — `CreateWorkspace` states
//! "You can have one or more workspaces in each Region in your account" — so the store
//! holds one [`RegionalState`] per Region rather than one set of records for the account.
//! A handler picks the store of the Region its request was signed for
//! ([`crate::state::request_region`]) and can reach no other.
//!
//! `crate::state` re-exports everything here, so a handler names `crate::state::…` for a
//! Region's store exactly as it does for a workspace.
//!
//! ## What bounds the partitioning
//!
//! The Region of a request is the caller's own SigV4 credential scope, so the number of
//! DISTINCT Region strings reaching this module is caller-chosen. Three rules, all in this
//! file, cap what that can grow:
//!
//! * Only a CREATE can bring a Region's store into being. Every other operation takes
//!   [`ComAmazonawsAmpAmazonprometheusserviceState::held_region_mut`], which materialises
//!   nothing and answers `None` for a Region the account holds nothing in — the handler's
//!   own modelled not-found, since a Region with no store holds no resource either.
//! * [`MAX_REGIONS`] caps the Regions that hold records at all: `CreateWorkspace` and
//!   `CreateScraper` — the only two callers of the one accessor that materialises a store
//!   ([`ComAmazonawsAmpAmazonprometheusserviceState::region_mut_for_create`]) — compare
//!   [`ComAmazonawsAmpAmazonprometheusserviceState::regions_holding_records`] against it
//!   first (unless the Region is one the store already holds records in, which needs no
//!   further room: [`ComAmazonawsAmpAmazonprometheusserviceState::holds_records_in`]) and
//!   refuse with the modelled quota error instead of growing past it.
//! * [`store_of`] — that accessor's one growth site — drops the stores that hold nothing
//!   before materialising a further one, and
//!   [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`], which every handler
//!   calls first, drops them too. A store holding nothing answers every read exactly as
//!   the absence of one does, so a Region whose records have all gone leaves nothing behind.
//!
//! Together they hold this map to at most `MAX_REGIONS` stores, each bounded in turn by the
//! per-Region resource quotas ([`crate::state::WORKSPACE_QUOTA`],
//! [`crate::state::SCRAPERS_PER_ACCOUNT_QUOTA`]).

use crate::state::{
    QueryLoggingConfiguration, Scraper, ScraperLoggingConfiguration, Workspace,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::SystemTime;

/// The most Regions this store holds records for at once, counted per Region and not per
/// resource: past it a create that would give a further Region its FIRST record is refused
/// as a full Region is, and nothing is written.
///
/// A request's Region is its own SigV4 credential scope, which the caller chooses, so
/// without a ceiling a caller could repeat a create under fresh Region names until the
/// process ran out of memory, each name carrying its own [`RegionalState`] and its own
/// per-Region room for resources. A partition's Regions are a fixed, published set; AWS
/// publishes no limit of this kind, so the figure is this emulator's own, set well above
/// the largest partition.
pub const MAX_REGIONS: usize = 64;

/// The `message` a create refused for want of room for a further Region carries.
///
/// It names the ceiling that was reached and says whose it is: AWS publishes no quota on
/// the Regions an account may hold resources in, so this bound is THIS EMULATOR's, and the
/// sentence says so rather than claiming a limit the service does not have.
pub fn region_ceiling_message() -> String {
    format!("this emulator holds resources in at most {MAX_REGIONS} Regions at once")
}

/// The read view of a Region that holds nothing: ONE empty store for the whole process, so
/// a REPORTING operation answers for such a Region without materialising anything a later
/// caller could observe. Never mutated — it is handed out behind a shared reference only.
static EMPTY_REGION: LazyLock<RegionalState> = LazyLock::new(RegionalState::default);

/// Per-service state for com-amazonaws-amp-amazonprometheusservice. A handler reaches it
/// through [`crate::state::service_state`].
///
/// One store per Region (see this module's documentation): a resource exists only in the
/// Region whose endpoint created it, every listing reports just that Region's resources,
/// each per-Region quota counts only them, and an id minted in one Region names nothing in
/// another. A handler picks its store with [`region`](Self::region) (to report),
/// [`held_region_mut`](Self::held_region_mut) (to write a Region the account already holds)
/// or — in a create only — [`region_mut_for_create`](Self::region_mut_for_create), passing
/// the Region of the request's own credential scope ([`crate::state::request_region`]) —
/// never the Region recorded on some other resource.
#[derive(Debug, Default)]
pub struct ComAmazonawsAmpAmazonprometheusserviceState {
    /// The Regions this account holds something in, keyed by Region.
    ///
    /// Private: a Region's store is reached through the accessors below, so no handler can
    /// read one Region's resources while serving another Region's request, and only a
    /// create can add an entry here.
    regions: HashMap<String, RegionalState>,
    /// Counts the identifiers this store has minted — workspace ids and resource-policy
    /// revision ids both; see [`crate::state::mint_workspace_id`] and
    /// [`crate::state::mint_revision_id`].
    ///
    /// One counter for the whole store rather than one per Region, so no two Regions mint
    /// the same id and an id names at most one resource in the account.
    pub next_id: u64,
}

impl ComAmazonawsAmpAmazonprometheusserviceState {
    /// The store of `region`, for a handler that REPORTS: that Region's own store, or the
    /// shared empty one when the account holds nothing there. Writes nothing.
    pub fn region(&self, region: &str) -> &RegionalState {
        self.regions.get(region).unwrap_or(&EMPTY_REGION)
    }

    /// The store of `region` to WRITE, when the account already HOLDS one there — `None`
    /// when it holds nothing in that Region.
    ///
    /// This is the accessor every operation but a create uses, and it NEVER materialises a
    /// store: such an operation acts on a resource the Region must already hold, and a
    /// Region with no store holds no resource, so `None` is the handler's own modelled
    /// not-found and nothing is written. That is what keeps a request's Region — a
    /// caller-chosen string — from growing this map at all outside the two bounded creates.
    pub fn held_region_mut(&mut self, region: &str) -> Option<&mut RegionalState> {
        self.regions.get_mut(region)
    }

    /// [`held_region_mut`](Self::held_region_mut) with the store's id counter beside it, for
    /// a handler that mints an identifier while writing a Region the account already holds:
    /// the counter is shared by every Region ([`next_id`](Self::next_id)). Materialises
    /// nothing either.
    pub fn held_region_and_ids_mut(
        &mut self,
        region: &str,
    ) -> Option<(&mut RegionalState, &mut u64)> {
        Some((self.regions.get_mut(region)?, &mut self.next_id))
    }

    /// The store of `region` for a CREATE, materialised when the account holds none there.
    ///
    /// The ONE accessor that can bring a Region's store into being, and so the ONE growth
    /// site of this map (through [`store_of`]): `CreateWorkspace` and `CreateScraper` are
    /// its only callers, and each compares
    /// [`regions_holding_records`](Self::regions_holding_records) against [`MAX_REGIONS`]
    /// first and refuses with the modelled quota error rather than calling this past it.
    /// Every other operation takes [`held_region_mut`](Self::held_region_mut), which
    /// materialises nothing.
    pub fn region_mut_for_create(&mut self, region: &str) -> &mut RegionalState {
        store_of(&mut self.regions, region)
    }

    /// [`region_mut_for_create`](Self::region_mut_for_create) with the store's id counter
    /// beside it — what a create that mints an identifier takes, under the same ceiling.
    pub fn region_and_ids_mut_for_create(
        &mut self,
        region: &str,
    ) -> (&mut RegionalState, &mut u64) {
        (store_of(&mut self.regions, region), &mut self.next_id)
    }

    /// How many Regions this store holds records for: the Regions whose store is not
    /// empty, counted per Region and not per resource.
    ///
    /// What a create compares against [`MAX_REGIONS`] before taking
    /// [`region_mut_for_create`](Self::region_mut_for_create) — see either create for the
    /// comparison itself.
    pub fn regions_holding_records(&self) -> usize {
        self.regions
            .values()
            .filter(|store| !store.is_empty())
            .count()
    }

    /// Whether this store holds any record in `region`.
    ///
    /// A create in such a Region needs no room for a further one, so it is never refused
    /// for the Region ceiling however many Regions the store holds.
    pub fn holds_records_in(&self, region: &str) -> bool {
        self.regions
            .get(region)
            .is_some_and(|store| !store.is_empty())
    }

    /// Apply every lifecycle transition the clock already owes at `now`, in every Region.
    ///
    /// Each pending transition was due at an instant recorded when the call that started it
    /// ran, so it is owed whether or not anybody calls again; applying it here is
    /// materialising a change the clock made, not a side effect of the read. Call it FIRST
    /// in every handler, so each one sees the same state a caller would after any other —
    /// and in every Region, because the clock owes a transition wherever it was recorded,
    /// not only where the next call happens to be signed for.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for store in self.regions.values_mut() {
            store.materialize_due(now);
        }
        // A Region whose last resource has just gone answers every read as a Region nobody
        // touched does, so its store is dropped rather than kept.
        self.regions.retain(|_, store| !store.is_empty());
    }
}

/// One Region's store: everything this service holds for the account in a single Region.
///
/// The Region itself is not a field: it is the key this store is filed under in
/// [`ComAmazonawsAmpAmazonprometheusserviceState`], so a record in here cannot be reached
/// while serving another Region's request. Every ARN and Prometheus endpoint on these
/// records names that same Region, because the call that created them composed it from its
/// own credential scope.
#[derive(Debug, Default)]
pub struct RegionalState {
    /// Every workspace this Region holds, keyed by `workspaceId`.
    pub workspaces: HashMap<String, Workspace>,
    /// Every scraper this Region holds, keyed by `scraperId` — what `ListScrapers` reports.
    /// Ordered, so a page boundary means the same thing on every call.
    pub scrapers: BTreeMap<String, Scraper>,
    /// Each workspace's query logging configuration, keyed by `workspaceId`. A workspace
    /// holds at most one, and it is dropped with the workspace.
    pub query_logging: HashMap<String, QueryLoggingConfiguration>,
    /// Each scraper's logging configuration, keyed by `scraperId`. A scraper holds at most
    /// one, and it is dropped with the scraper.
    pub scraper_logging: HashMap<String, ScraperLoggingConfiguration>,
    /// `clientToken -> workspaceId`, which is what makes `CreateWorkspace` idempotent per
    /// its `@idempotencyToken`. Per Region, like the workspace it names: the same token
    /// sent to two Regions is two requests, each creating its own workspace.
    pub client_tokens: HashMap<String, String>,
    /// `clientToken -> scraperId`, which is what makes `CreateScraper` idempotent per its
    /// `@idempotencyToken` — the role [`RegionalState::client_tokens`] plays for a
    /// workspace. Kept apart from that map because a token is scoped to the request that
    /// carried it: the same string may create a workspace and a scraper, and neither create
    /// may then answer with the other's resource. An entry is forgotten with the scraper it
    /// names, so the map cannot outgrow the scrapers it indexes.
    pub scraper_tokens: HashMap<String, String>,
}

impl RegionalState {
    /// Whether this Region holds nothing a caller put there.
    ///
    /// Such a store answers every read exactly as the absence of one does, so it is the
    /// store [`ComAmazonawsAmpAmazonprometheusserviceState`] may drop and the one that
    /// counts against no Region ceiling. The token maps are not consulted: an entry in
    /// either is forgotten with the resource it names, so a store holding no resource holds
    /// no token either.
    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
            && self.scrapers.is_empty()
            && self.query_logging.is_empty()
            && self.scraper_logging.is_empty()
    }
}

/// The store `region` writes to, materialised when `regions` holds none for it — the ONE
/// place a Region's store comes into being, and so the one place the growth of `regions` is
/// bounded.
///
/// The stores that hold nothing are dropped before a further Region is given one, which is
/// what makes naming a fresh Region cost nothing durable: such a store answers every read
/// exactly as the absence of one does, so this map carries at most one of them at a time.
/// Takes the map rather than the whole store, so a caller can hold the id counter beside
/// the store it hands back
/// ([`ComAmazonawsAmpAmazonprometheusserviceState::region_and_ids_mut_for_create`]).
fn store_of<'regions>(
    regions: &'regions mut HashMap<String, RegionalState>,
    region: &str,
) -> &'regions mut RegionalState {
    if !regions.contains_key(region) {
        regions.retain(|_, store| !store.is_empty());
    }
    regions.entry(region.to_string()).or_default()
}
