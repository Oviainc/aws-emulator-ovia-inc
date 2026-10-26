// SPDX-License-Identifier: CC0-1.0

//! The Region partitioning of this service's store.
//!
//! Strategy Recommendations is a REGIONAL service: the endpoint and the caller's
//! credential scope select ONE Region's portfolio, and a resource created through one
//! Region's endpoint is reported only by reads signed for that same Region. So the store
//! this module defines holds one [`RegionState`] per Region — every handler resolves its
//! request's Region with [`request_region`] and then reaches that Region's portfolio
//! alone — while [`RegionState`] itself, in the parent module, is what one Region holds.
//!
//! Everything a Region's partition costs is bounded here: [`MAX_REGIONS`] caps how many
//! partitions the store keeps (the Region is caller-chosen, so this is the ceiling that
//! stops made-up Regions growing the store), only a WRITE creates one, and the per
//! resource quotas of the parent module cap what each one holds.

use std::collections::BTreeMap;

use super::RegionState;

/// How many Regions this store keeps a portfolio for.
///
/// The store is partitioned per Region and the Region comes from the caller's own SigV4
/// credential scope, so without a ceiling a caller could mint a partition per made-up
/// Region. It is comfortably more than the Regions AWS operates, so no real multi-Region
/// workflow meets it; with the per-resource quotas of the parent module it is what bounds
/// the whole store. A write that would be the FIRST thing stored in a further Region is
/// refused with the operation's own quota refusal
/// ([`ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState::region_capacity_reached`]);
/// a read mints no partition, so only Regions something was written in count against it.
pub const MAX_REGIONS: usize = 64;

/// The region an unsigned request (one carrying no SigV4 credential scope) is served in.
///
/// It is also the region an in-process call this service makes into another emulated
/// service is addressed to when the caller named none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region the caller signed for, or [`DEFAULT_REGION`] for an unsigned request.
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the framework's
/// single source of SigV4-scope reading: it answers `None` for a header it cannot parse
/// rather than failing, so a malformed authorization header is served in the default
/// region instead of refusing or panicking. Only the default lives here.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Per-service state for com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation. A handler reaches it through [`super::service_state`].
///
/// One [`RegionState`] per Region rather than one flat portfolio. The partitions are a
/// `BTreeMap` so the store has one stable order whatever order Regions were first
/// written in.
#[derive(Debug, Default)]
pub struct ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState {
    /// One portfolio per Region that holds something. Written only through
    /// [`Self::region_entry`], so a READ never mints one; bounded by [`MAX_REGIONS`].
    regions: BTreeMap<String, RegionState>,
    /// The counter every id of this service is minted from. It lives HERE, above the
    /// partitions, so a resource of one Region never shares an id with a resource of
    /// another: an id a caller holds names a resource in at most one Region.
    next_id: u64,
}

impl ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState {
    /// What a request signed for `region` may observe: that Region's portfolio, and
    /// nothing else.
    ///
    /// A Region nothing was ever stored in reads as an EMPTY portfolio, so a read
    /// answers honestly for any Region a caller names WITHOUT minting a partition for
    /// it.
    pub fn region(&self, region: &str) -> &RegionState {
        static EMPTY: ::std::sync::OnceLock<RegionState> = ::std::sync::OnceLock::new();
        self.regions
            .get(region)
            .unwrap_or_else(|| EMPTY.get_or_init(RegionState::default))
    }

    /// `region`'s portfolio for a read that materialises what the clock already owes it,
    /// or `None` when nothing was ever stored there — so a read mints no partition and a
    /// Region holding nothing answers as the empty account it is.
    pub fn region_mut(&mut self, region: &str) -> Option<&mut RegionState> {
        self.regions.get_mut(region)
    }

    /// `region`'s portfolio for WRITING, created when this is the first thing stored
    /// there. A caller that already found a resource in `region` through [`Self::region`]
    /// creates nothing by reaching for it.
    ///
    /// This is the store's only growth site in the number of Regions, and
    /// [`Self::region_capacity_reached`] is what each write checks before reaching it.
    pub fn region_entry(&mut self, region: &str) -> &mut RegionState {
        self.regions.entry(region.to_string()).or_default()
    }

    /// Whether the store already holds a portfolio for `region`.
    pub fn holds_region(&self, region: &str) -> bool {
        self.regions.contains_key(region)
    }

    /// How many Region portfolios the store holds, which [`MAX_REGIONS`] bounds.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Whether a write that would store its FIRST resource in `region` is past the
    /// [`MAX_REGIONS`] ceiling, so the operation must refuse it rather than mint a
    /// further partition. A Region the store already holds is never past it.
    pub fn region_capacity_reached(&self, region: &str) -> bool {
        !self.holds_region(region) && self.region_count() >= MAX_REGIONS
    }

    /// The next value of the shared id counter.
    ///
    /// It wraps rather than overflowing, and [`Self::mint_id`] skips an id already taken,
    /// so a wrap cannot hand out an id twice.
    fn next_sequence(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    /// Mint an id `taken` does not already report as `region`'s, rendering each candidate
    /// from the shared counter with `render`.
    ///
    /// The ONE mint loop of this store: every resource of this service gets its id this
    /// way, so the counter is shared across them (an id names a resource of at most one
    /// kind in at most one Region) and the skip that makes the counter's wrap safe is
    /// written once. `render` is what decides the id's shape, which each resource's own
    /// `mint_*` states from the member that reports it.
    fn mint_id(
        &mut self,
        region: &str,
        render: impl Fn(u64) -> String,
        taken: impl Fn(&RegionState, &str) -> bool,
    ) -> String {
        loop {
            let id = render(self.next_sequence());
            if !taken(self.region(region), &id) {
                return id;
            }
        }
    }

    /// Mint an import file task id unused in `region`.
    ///
    /// `StartImportFileTaskResponse$id` is a `String` bounded to 1024 with pattern
    /// `.*\S.*`, so the shape of the id is the emulator's choice.
    pub fn mint_import_file_task_id(&mut self, region: &str) -> String {
        self.mint_id(
            region,
            |sequence| format!("import-file-task-{sequence:012x}"),
            |portfolio, id| portfolio.import_tasks.contains_key(id),
        )
    }

    /// Mint an assessment id unused in `region`, matching `AsyncTaskId`'s `[0-9a-z-:]+`
    /// within its 52-character bound.
    pub fn mint_assessment_id(&mut self, region: &str) -> String {
        self.mint_id(
            region,
            |sequence| format!("assessment-{sequence:012x}"),
            |portfolio, id| portfolio.assessments.contains_key(id),
        )
    }

    /// Mint a recommendation report task id unused in `region`, matching
    /// `RecommendationTaskId`'s `[0-9a-z-:]+` within its 52-character bound (this one is
    /// 34 characters).
    pub fn mint_recommendation_report_id(&mut self, region: &str) -> String {
        self.mint_id(
            region,
            |sequence| format!("recommendation-report-{sequence:012x}"),
            |portfolio, id| portfolio.recommendation_reports.contains_key(id),
        )
    }

    /// Mint a server id unused in `region`, for a server an import file task ingested.
    ///
    /// `ServerDetail$id` is a `ResourceId`, whose `^[0-9a-b]+` and `@length(max: 44)`
    /// admit only those twelve characters at the start of the name: the id is an `a`
    /// (servers) and the counter in decimal, so every character of it is one the pattern
    /// admits and it stays far inside the bound.
    pub fn mint_server_id(&mut self, region: &str) -> String {
        self.mint_id(
            region,
            |sequence| format!("a{sequence:011}"),
            |portfolio, id| portfolio.servers.contains_key(id),
        )
    }

    /// Mint an application component id unused in `region`, under the same `ResourceId`
    /// rule as a server's and with `b` for a component, so one id never reads as the
    /// other's.
    pub fn mint_application_component_id(&mut self, region: &str) -> String {
        self.mint_id(
            region,
            |sequence| format!("b{sequence:011}"),
            |portfolio, id| portfolio.application_components.contains_key(id),
        )
    }
}
