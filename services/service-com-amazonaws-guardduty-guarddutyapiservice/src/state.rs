// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-guardduty-guarddutyapiservice service.
//!
//! GuardDuty's control plane is rooted at the *detector*: one per account per
//! Region. Everything the detector owns — its findings, its filters and its threat
//! lists — hangs off the [`Detector`] record, so `DeleteDetector` cascades by
//! dropping one map entry.
//!
//! A Malware Protection plan is NOT one of those: it is created against an S3
//! bucket rather than against a detector, so [`MalwareProtectionPlan`] hangs off
//! the service's own map and outlives any detector.
//!
//! This module holds only plain records plus the helpers more than one operation
//! needs (id minting, the monotonic clock, finding attribute lookup, filtering and
//! sorting). Each operation builds its own modelled output in its own
//! `src/ops/<op>.rs`.

use std::collections::{BTreeMap, HashMap};

/// Per-service state for com-amazonaws-guardduty-guarddutyapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// Every persisted detector, keyed by its minted `DetectorId`.
    ///
    /// BOUNDED by [`MAX_DETECTOR_REGIONS`]: the store holds at most one detector
    /// per Region ("you can have only one detector per account per Region"), and
    /// the Region comes from the request's credential scope, so the number of
    /// PARTITIONS needs its own ceiling — see
    /// [`Self::detector_regions_exhausted`], which `CreateDetector` refuses past.
    /// Everything a detector owns is bounded in turn
    /// ([`MAX_MEMBERS_PER_DETECTOR`], [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`],
    /// [`IP_SETS_PER_DETECTOR`] plus [`DELETED_IP_SETS_RETAINED`],
    /// [`THREAT_INTEL_SETS_PER_DETECTOR`],
    /// [`THREAT_ENTITY_SETS_PER_DETECTOR`]), so the whole map is finite whatever a
    /// caller sends.
    pub detectors: HashMap<String, Detector>,
    /// `ClientToken` -> the detector a `CreateDetector` minted under it, so a
    /// replayed create returns that detector instead of refusing or minting a second
    /// one. Keyed by `(region, token)`: an idempotency token is scoped to the
    /// endpoint it was replayed against.
    ///
    /// BOUNDED by [`MAX_CLIENT_TOKENS`]. The token is REQUEST-chosen, so a caller
    /// replaying creates with fresh tokens could otherwise grow this map without
    /// limit — the same hazard [`MAX_MEMBERS_PER_DETECTOR`] guards for
    /// request-chosen account ids. Write it through
    /// [`Self::record_client_token`], which drops the oldest mappings and the ones
    /// whose detector is gone, and read it through
    /// [`Self::detector_for_client_token`]; never insert into it directly.
    pub client_tokens: HashMap<(String, String), ClientTokenRecord>,
    /// Every persisted Malware Protection plan, keyed by its minted
    /// `MalwareProtectionPlanId`.
    ///
    /// A plan is NOT owned by a detector: Malware Protection for S3 is created
    /// against an S3 bucket ("Creates a new Malware Protection plan for the
    /// protected resource", `smithy_query(query="doc",shape="CreateMalwareProtectionPlan")`),
    /// so it hangs off the service rather than off [`Detector`].
    ///
    /// BOUNDED at [`MAX_MALWARE_PROTECTION_PLANS_TOTAL`] records: the per-Region
    /// quota bounds one Region's plans and
    /// [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`] bounds how many Regions the store
    /// partitions into, both of which `CreateMalwareProtectionPlan` enforces before
    /// it writes.
    pub malware_protection_plans: HashMap<String, MalwareProtectionPlan>,
    /// `ClientToken` -> `MalwareProtectionPlanId`, the plan counterpart of
    /// [`Self::client_tokens`], keyed by `(region, token)` for the same reason.
    ///
    /// Separate from the detector index on purpose: a token resolves to a resource
    /// of the type the create that carried it made, so one token replayed against
    /// two different creates must never hand back the other one's id.
    ///
    /// The token is a pure REPLAY key: it maps to the plan it minted and nothing
    /// about the replaying request is compared against it, so a replay carrying
    /// different members answers with that plan unchanged rather than minting a
    /// second one or mutating the first. Creating is not updating —
    /// `UpdateMalwareProtectionPlan` is the operation that changes a plan — and a
    /// replay that did mutate would make a retried create a hidden update.
    ///
    /// It is BOUNDED by [`MAX_CLIENT_TOKENS`], the ceiling the detector index
    /// carries and for the same reason: the key is request-chosen, so an entry per
    /// create-then-delete cycle would otherwise accumulate for ever (the plan quota
    /// a delete frees bounds the PLANS, not the mappings that named them). Write it
    /// through [`Self::record_malware_protection_plan_client_token`], which drops
    /// the mappings whose plan is gone and then the oldest, read it through
    /// [`Self::malware_protection_plan_for_client_token`], which resolves the plan
    /// so a stale mapping yields nothing, and drop a deleted plan's mappings with
    /// [`Self::forget_malware_protection_plan_client_tokens_for`]; never insert
    /// into it directly.
    pub malware_protection_plan_client_tokens:
        HashMap<(String, String), MalwareProtectionPlanTokenRecord>,
    /// Every malware scan this service holds, keyed by its minted `ScanId`.
    ///
    /// A scan is NOT owned by a detector: `StartMalwareScan` names the resource to
    /// scan by ARN and carries no `DetectorId`
    /// (`smithy_query(operation="StartMalwareScan")`), so a scan hangs off the
    /// service and survives a `DeleteDetector` the way a
    /// [`MalwareProtectionPlan`] does. It is still REGIONAL — the Region the start
    /// call was signed for is the only endpoint that reports it, which
    /// [`Self::malware_scan_in`] enforces.
    ///
    /// BOUNDED at [`MAX_MALWARE_SCANS_PER_REGION`] scans per Region across at most
    /// [`MAX_MALWARE_SCAN_REGIONS`] Regions, both of which `StartMalwareScan`
    /// enforces before it writes ([`Self::malware_scan_count_in`],
    /// [`Self::malware_scan_regions_exhausted`]). The `ClientToken` a start carried
    /// lives ON the scan record ([`MalwareScan::client_token`]) rather than in an
    /// index of its own, so the idempotency mappings are bounded by the scan store
    /// itself and no mapping can outlive the scan it names.
    pub malware_scans: HashMap<String, MalwareScan>,
    /// GuardDuty's own catalogue of custom detection rules, materialised when this
    /// service's store is created.
    ///
    /// A rule is the SERVICE's, not an account's: the API models no operation that
    /// creates, updates or deletes one — `ListCustomDetectionRules` "Returns all
    /// available custom detection rules in GuardDuty"
    /// (`smithy_query(query="doc",shape="ListCustomDetectionRules")`), and the only
    /// writes it models against a rule are the ASSOCIATIONS an account makes
    /// (`CreateCustomDetectionRuleAssociation`). So the catalogue is seeded once,
    /// reads the same for every account and every Region, and is FINITE by
    /// construction: it is the fixed table behind
    /// [`CustomDetectionRuleCatalog::default`].
    pub custom_detection_rules: CustomDetectionRuleCatalog,
    /// Last instant handed out by [`Self::tick_millis`], so two mutations in the
    /// same millisecond still get strictly increasing timestamps.
    last_millis: i64,
    /// Monotonic counter folded into every minted id.
    seq: u64,
}

impl ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// The current instant in epoch milliseconds, strictly greater than every
    /// instant this state handed out before.
    ///
    /// Call it only from a MUTATING operation: it is the "this resource changed
    /// now" clock, and a reporting operation must not advance it. Monotonicity is
    /// what makes `UpdatedAt > CreatedAt` observable for two changes that land in
    /// the same wall-clock millisecond.
    pub fn tick_millis(&mut self) -> i64 {
        let wall = now_millis();
        let next = if wall > self.last_millis {
            wall
        } else {
            self.last_millis.saturating_add(1)
        };
        self.last_millis = next;
        next
    }

    /// Mint a fresh 32-hex-character id, the shape GuardDuty uses for both
    /// `DetectorId` and a finding `Id`.
    ///
    /// The low 16 bits carry this state's own sequence counter, so two ids minted
    /// by the same state differ even if the hash above them repeats.
    pub fn mint_id(&mut self) -> String {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        let nanos = now_nanos();
        let hi = scramble(seq, nanos);
        let lo = scramble(nanos ^ GOLDEN_RATIO_64, hi ^ seq);
        let lo = (lo & !0xFFFF) | (seq & 0xFFFF);
        format!("{hi:016x}{lo:016x}")
    }

    /// The detector `region`'s endpoint serves for `id`, if any.
    ///
    /// A detector is a REGIONAL resource — "you must create a detector in each
    /// Region where you enable the service"
    /// (`smithy_query(query="doc",shape="CreateDetector")`) — so an id created
    /// through another Region's endpoint is not visible here.
    pub fn detector_in(&self, region: &str, id: &str) -> Option<&Detector> {
        self.detectors
            .get(id)
            .filter(|detector| detector.region == region)
    }

    /// [`Self::detector_in`], for a mutating operation.
    pub fn detector_in_mut(&mut self, region: &str, id: &str) -> Option<&mut Detector> {
        self.detectors
            .get_mut(id)
            .filter(|detector| detector.region == region)
    }

    /// The detector `region`'s endpoint minted under `token`, if a create recorded
    /// one and that detector is still held.
    ///
    /// A mapping whose detector was deleted resolves to nothing, so a replayed
    /// create never answers with an id the store no longer holds.
    pub fn detector_for_client_token(&self, region: &str, token: &str) -> Option<&Detector> {
        let record = self
            .client_tokens
            .get(&(region.to_string(), token.to_string()))?;
        self.detector_in(region, &record.detector_id)
    }

    /// Record that `token` minted `detector_id` for `region`, keeping the store
    /// bounded by [`MAX_CLIENT_TOKENS`].
    ///
    /// Two kinds of mapping are dropped to make room, in this order: one whose
    /// detector the store no longer holds (it can never resolve again), then the
    /// OLDEST remaining mapping — the token a caller is least likely to replay.
    /// Evicting a mapping costs a replay its idempotency, never a resource a caller
    /// can still read: the detector itself stays exactly where it was.
    pub fn record_client_token(
        &mut self,
        region: &str,
        token: &str,
        detector_id: &str,
        now: i64,
    ) {
        // Taken out of `self` so the retain below can consult `self.detectors`.
        let mut tokens = std::mem::take(&mut self.client_tokens);
        tokens.retain(|(token_region, _), record| {
            self.detector_in(token_region, &record.detector_id).is_some()
        });
        while tokens.len() >= MAX_CLIENT_TOKENS {
            let oldest = tokens
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    left.recorded_at_millis
                        .cmp(&right.recorded_at_millis)
                        // The instants are strictly increasing, so this only orders
                        // mappings a coarser clock could tie.
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            tokens.remove(&oldest);
        }
        tokens.insert(
            (region.to_string(), token.to_string()),
            ClientTokenRecord {
                detector_id: detector_id.to_string(),
                recorded_at_millis: now,
            },
        );
        self.client_tokens = tokens;
    }

    /// Drop every idempotency mapping naming `detector_id`, so a replayed create
    /// stops resolving to a detector a delete removed.
    pub fn forget_client_tokens_for(&mut self, detector_id: &str) {
        self.client_tokens
            .retain(|_, record| record.detector_id != detector_id);
    }

    /// The Malware Protection plan `region`'s endpoint minted under `token`, if a
    /// create recorded one and that plan is still held.
    ///
    /// Resolved THROUGH [`Self::malware_protection_plan_in`] for the same reason
    /// [`Self::detector_for_client_token`] resolves through [`Self::detector_in`]:
    /// a mapping whose plan was deleted resolves to nothing, so a replayed create
    /// never answers with an id the store no longer holds.
    pub fn malware_protection_plan_for_client_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&MalwareProtectionPlan> {
        let record = self
            .malware_protection_plan_client_tokens
            .get(&(region.to_string(), token.to_string()))?;
        self.malware_protection_plan_in(region, &record.plan_id)
    }

    /// Record that `token` minted `plan_id` for `region`, keeping the plan
    /// idempotency index bounded by [`MAX_CLIENT_TOKENS`] — the detector index's
    /// ceiling, for the same reason: the key is request-chosen.
    ///
    /// Two kinds of mapping are dropped to make room, in this order: one whose plan
    /// the store no longer holds (it can never resolve again), then the OLDEST
    /// remaining mapping — the token a caller is least likely to replay. Evicting a
    /// mapping costs a replay its idempotency, never a resource a caller can still
    /// read: the plan itself stays exactly where it was.
    pub fn record_malware_protection_plan_client_token(
        &mut self,
        region: &str,
        token: &str,
        plan_id: &str,
        now: i64,
    ) {
        // Taken out of `self` so the retain below can consult the plan store.
        let mut tokens = std::mem::take(&mut self.malware_protection_plan_client_tokens);
        tokens.retain(|(token_region, _), record| {
            self.malware_protection_plan_in(token_region, &record.plan_id)
                .is_some()
        });
        while tokens.len() >= MAX_CLIENT_TOKENS {
            let oldest = tokens
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    left.recorded_at_millis
                        .cmp(&right.recorded_at_millis)
                        // The instants are strictly increasing, so this only orders
                        // mappings a coarser clock could tie.
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            tokens.remove(&oldest);
        }
        tokens.insert(
            (region.to_string(), token.to_string()),
            MalwareProtectionPlanTokenRecord {
                plan_id: plan_id.to_string(),
                recorded_at_millis: now,
            },
        );
        self.malware_protection_plan_client_tokens = tokens;
    }

    /// Drop every plan idempotency mapping naming `plan_id`, so a replayed create
    /// stops resolving to a plan a delete removed — the hook a plan delete calls, as
    /// `DeleteDetector` calls [`Self::forget_client_tokens_for`].
    pub fn forget_malware_protection_plan_client_tokens_for(&mut self, plan_id: &str) {
        self.malware_protection_plan_client_tokens
            .retain(|_, record| record.plan_id != plan_id);
    }

    /// Mint an `InvestigationId`: the 32 hexadecimal characters [`Self::mint_id`]
    /// mints, grouped in the 8-4-4-4-12 form.
    ///
    /// `InvestigationId` is `len 1..64, pattern ^[a-fA-F0-9\-]+$`
    /// (`smithy_query(operation="CreateInvestigation")`) — hexadecimal plus
    /// hyphens — so the 36-character grouped form is a value of the member, and it
    /// is distinguishable at a glance from the bare-hex `DetectorId` / finding id.
    pub fn mint_investigation_id(&mut self) -> String {
        let hex = self.mint_id();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Set the archive state of the named findings of the detector `region`'s
    /// endpoint serves for `detector_id`, recording the instant of the write as the
    /// `UpdatedAt` of each finding it moves.
    ///
    /// `None` means that endpoint does not hold the detector, which is the caller's
    /// cue to refuse the request: nothing is written and the clock is not advanced,
    /// because the detector is resolved BEFORE [`Self::tick_millis`] runs.
    pub fn set_findings_archived_in(
        &mut self,
        region: &str,
        detector_id: &str,
        finding_ids: &[String],
        archived: bool,
    ) -> Option<()> {
        self.detector_in(region, detector_id)?;
        let now = self.tick_millis();
        self.detector_in_mut(region, detector_id)?
            .set_findings_archived(finding_ids, archived, now);
        Some(())
    }

    /// The ids of the detectors `region`'s endpoint serves, in a total order.
    pub fn detector_ids_in(&self, region: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .detectors
            .values()
            .filter(|detector| detector.region == region)
            .map(|detector| detector.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The Malware Protection plan `region`'s endpoint serves for `id`, if any.
    ///
    /// Regional for the same reason a detector is: a plan is created through one
    /// Region's endpoint and only that endpoint reports it.
    pub fn malware_protection_plan_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&MalwareProtectionPlan> {
        self.malware_protection_plans
            .get(id)
            .filter(|plan| plan.region == region)
    }

    /// The plan `region` already holds for the S3 bucket named `bucket_name`, if
    /// any — the plan a second create for that same protected resource conflicts
    /// with.
    pub fn malware_protection_plan_for_bucket(
        &self,
        region: &str,
        bucket_name: &str,
    ) -> Option<&MalwareProtectionPlan> {
        self.malware_protection_plans
            .values()
            .filter(|plan| plan.region == region)
            .find(|plan| plan.bucket_name == bucket_name)
    }

    /// How many Malware Protection plans `region`'s endpoint holds — the count the
    /// per-account-per-Region quota
    /// ([`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`]) is measured
    /// against.
    pub fn malware_protection_plan_count_in(&self, region: &str) -> usize {
        self.malware_protection_plans
            .values()
            .filter(|plan| plan.region == region)
            .count()
    }

    /// Would a detector created through `region`'s endpoint partition the detector
    /// store into one Region more than it will hold
    /// ([`MAX_DETECTOR_REGIONS`])?
    ///
    /// "One detector per account per Region" bounds each PARTITION to a single
    /// detector, but the Region is read from the request's credential scope
    /// ([`request_region`]), so the caller chooses the partition KEY too: without
    /// this, a caller signing for invented Region names could make the store retain
    /// one detector — and everything it owns — per distinct Region string. A Region
    /// the store already holds a detector for is never refused by it, so this bound
    /// is invisible to a caller using the Regions AWS runs.
    pub fn detector_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for detector in self.detectors.values() {
            regions.insert(detector.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_DETECTOR_REGIONS
    }

    /// Would a plan created through `region`'s endpoint partition this store into
    /// one Region more than it will hold
    /// ([`MAX_MALWARE_PROTECTION_PLAN_REGIONS`])?
    ///
    /// The Region is request-chosen, so this is the bound on the number of
    /// PARTITIONS that makes the per-Region plan quota a bound on the whole plan
    /// map. A Region the store already holds a plan for is never refused by it.
    pub fn malware_protection_plan_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for plan in self.malware_protection_plans.values() {
            regions.insert(plan.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_MALWARE_PROTECTION_PLAN_REGIONS
    }

    /// The malware scan `region`'s endpoint serves for `scan_id`, if any.
    ///
    /// Regional for the same reason a detector and a plan are: a scan is started
    /// through one Region's endpoint and only that endpoint reports it.
    pub fn malware_scan_in(&self, region: &str, scan_id: &str) -> Option<&MalwareScan> {
        self.malware_scans
            .get(scan_id)
            .filter(|scan| scan.region == region)
    }

    /// [`Self::malware_scan_in`], for a caller that materialises the status change
    /// the clock already owes the scan ([`MalwareScan::settle`]).
    pub fn malware_scan_in_mut(&mut self, region: &str, scan_id: &str) -> Option<&mut MalwareScan> {
        self.malware_scans
            .get_mut(scan_id)
            .filter(|scan| scan.region == region)
    }

    /// The scan `region`'s endpoint started under `token`, if a start recorded one.
    ///
    /// The token is a pure REPLAY key: it resolves to the scan the first start
    /// minted and nothing about the replaying request is compared against it, so a
    /// replay answers with that scan rather than starting a second one.
    pub fn malware_scan_for_client_token(&self, region: &str, token: &str) -> Option<&MalwareScan> {
        self.malware_scans
            .values()
            .filter(|scan| scan.region == region)
            .find(|scan| scan.client_token.as_deref() == Some(token))
    }

    /// How many malware scans `region`'s endpoint holds — the count
    /// [`MAX_MALWARE_SCANS_PER_REGION`] is measured against.
    pub fn malware_scan_count_in(&self, region: &str) -> usize {
        self.malware_scans
            .values()
            .filter(|scan| scan.region == region)
            .count()
    }

    /// Would a scan started through `region`'s endpoint partition the scan store
    /// into one Region more than it will hold ([`MAX_MALWARE_SCAN_REGIONS`])?
    ///
    /// The Region is request-chosen, so this is the bound on the number of
    /// PARTITIONS that makes the per-Region scan ceiling a bound on the whole scan
    /// map. A Region the store already holds a scan for is never refused by it.
    pub fn malware_scan_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for scan in self.malware_scans.values() {
            regions.insert(scan.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_MALWARE_SCAN_REGIONS
    }
}

/// The most `(region, ClientToken)` -> detector mappings the store retains. The
/// plan idempotency index
/// ([`ComAmazonawsGuarddutyGuarddutyapiserviceState::malware_protection_plan_client_tokens`])
/// is bounded by the same number, for the same reason.
///
/// A STORE GUARD, not a modelled quota: AWS publishes no limit on how many
/// idempotency tokens an account may use, and nothing a caller can read reports this
/// number. It exists because the key is request-chosen, so the map needs a ceiling
/// the way [`MAX_MEMBERS_PER_DETECTOR`] gives the request-keyed member store one.
/// Past it, [`ComAmazonawsGuarddutyGuarddutyapiserviceState::record_client_token`]
/// evicts the oldest mapping.
pub const MAX_CLIENT_TOKENS: usize = 1_024;

/// One idempotency mapping: the detector a `ClientToken` minted, and when the create
/// that minted it was recorded.
///
/// The instant is what makes eviction ORDERED rather than arbitrary: the oldest
/// mapping goes first, so the token a caller just used is the last to be dropped.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The `DetectorId` the create minted.
    pub detector_id: String,
    /// When the mapping was recorded, in epoch milliseconds from
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis`].
    pub recorded_at_millis: i64,
}

/// [`ClientTokenRecord`] for the Malware Protection plan index: the plan a
/// `ClientToken` minted, and when the create that minted it was recorded.
///
/// A separate record rather than a reused one because the two indexes resolve
/// different resource types, and a token must never hand back the other store's id.
#[derive(Debug, Clone)]
pub struct MalwareProtectionPlanTokenRecord {
    /// The `MalwareProtectionPlanId` the create minted.
    pub plan_id: String,
    /// When the mapping was recorded, in epoch milliseconds from
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis`].
    pub recorded_at_millis: i64,
}

/// The 64-bit golden-ratio odd constant used to decorrelate the two hash inputs.
const GOLDEN_RATIO_64: u64 = 0x9E37_79B9_7F4A_7C15;

/// Hash two words into one, seeded by a freshly built [`std::collections::hash_map::RandomState`]
/// so ids are not predictable across runs.
fn scramble(a: u64, b: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(a);
    hasher.write_u64(b);
    hasher.finish()
}

/// The current instant in epoch milliseconds, straight off the wall clock.
///
/// A REPORTING operation reads the clock through this rather than through
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis`]: it needs to know
/// what the clock has already made due without advancing the "this resource
/// changed now" counter.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0))
}

/// Render an epoch-millisecond instant the way GuardDuty renders `CreatedAt` /
/// `UpdatedAt`: an ISO-8601 instant in UTC with a fixed three-digit millisecond
/// field.
///
/// The width is fixed deliberately. RFC 3339 allows trimming trailing zeros from
/// the subsecond field, but then `…:21.62Z` and `…:21.621Z` no longer compare in
/// time order as text, and `CreatedAt`/`UpdatedAt` are modelled as `String` — text
/// is all a client has to order them by.
pub fn iso8601(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let subsec_millis = millis.rem_euclid(1000);
    let whole = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    match whole.strip_suffix('Z') {
        Some(prefix) => format!("{prefix}.{subsec_millis:03}Z"),
        None => whole,
    }
}

/// `DetectorStatus` / `FeatureStatus` / `DataSourceStatus` — every one of the three
/// enums is `ENABLED | DISABLED` (`smithy_query(shape="DetectorStatus")`).
pub const ENABLED: &str = "ENABLED";
/// See [`ENABLED`].
pub const DISABLED: &str = "DISABLED";

/// Every member of the `DetectorFeature` enum, in model order
/// (`smithy_query(shape="DetectorFeature")`). These are the features a request may
/// configure: `DetectorFeatureConfiguration.Name` targets this enum.
pub const DETECTOR_FEATURES: [&str; 9] = [
    "S3_DATA_EVENTS",
    "EKS_AUDIT_LOGS",
    "EBS_MALWARE_PROTECTION",
    "RDS_LOGIN_EVENTS",
    "LAMBDA_NETWORK_LOGS",
    "EKS_RUNTIME_MONITORING",
    "RUNTIME_MONITORING",
    "AI_PROTECTION",
    "AI_ANALYST",
];

/// Runtime Monitoring: the feature `CreateDetector` documents as the exception to
/// its "all the optional features are enabled by default" default
/// (`smithy_query(query="doc",shape="CreateDetector")`).
pub const FEATURE_RUNTIME_MONITORING: &str = "RUNTIME_MONITORING";

/// Runtime Monitoring already covers Amazon EKS, so the two cannot both be
/// requested: "Specifying both EKS Runtime Monitoring (EKS_RUNTIME_MONITORING) and
/// Runtime Monitoring (RUNTIME_MONITORING) will cause an error"
/// (`smithy_query(query="doc",shape="CreateDetector")`).
pub const FEATURE_EKS_RUNTIME_MONITORING: &str = "EKS_RUNTIME_MONITORING";

/// The feature that governs S3 data-event ingestion — the `DataSources.S3Logs`
/// view of the same switch.
pub const FEATURE_S3_DATA_EVENTS: &str = "S3_DATA_EVENTS";
/// The feature behind `DataSources.Kubernetes.AuditLogs`.
pub const FEATURE_EKS_AUDIT_LOGS: &str = "EKS_AUDIT_LOGS";
/// The feature behind `DataSources.MalwareProtection.ScanEc2InstanceWithFindings`.
pub const FEATURE_EBS_MALWARE_PROTECTION: &str = "EBS_MALWARE_PROTECTION";
/// The feature behind `DataSources.Database.Rds.DatabaseEvents`: RDS Protection
/// watches Amazon RDS login activity.
pub const FEATURE_RDS_LOGIN_EVENTS: &str = "RDS_LOGIN_EVENTS";

/// GuardDuty Investigation's own feature: "To use this operation, the AI_ANALYST
/// feature must be enabled on your detector"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const FEATURE_AI_ANALYST: &str = "AI_ANALYST";

/// The feature behind `DataSources.CloudTrail`: GuardDuty's analysis of CloudTrail
/// management events.
pub const FEATURE_CLOUD_TRAIL: &str = "CLOUD_TRAIL";
/// The feature behind `DataSources.DNSLogs`.
pub const FEATURE_DNS_LOGS: &str = "DNS_LOGS";
/// The feature behind `DataSources.FlowLogs`.
pub const FEATURE_FLOW_LOGS: &str = "FLOW_LOGS";

/// The `EbsSnapshotPreservation` member that retains nothing: Malware Protection
/// deletes the EBS volume snapshots it took for a scan once the scan finishes, which
/// is the setting a detector has until a request asks for the other member
/// (retention when the scan produced a malware finding).
///
/// Named, not taken by position out of the generated enum list: the const assertion
/// below fails the BUILD if the model stops carrying this member, where
/// `EBS_SNAPSHOT_PRESERVATION_VALUES[0]` would instead have silently made some other
/// member the detector's default. That list is also what the request boundary
/// refuses a value outside the enum against, before any handler runs.
pub const NO_RETENTION: &str = "NO_RETENTION";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::EBS_SNAPSHOT_PRESERVATION_VALUES,
    NO_RETENTION
));

/// Does the generated enum list `values` carry `value`?
///
/// A `const fn` over bytes because `==` on `str` is not usable in a const assertion;
/// it exists only for the compile-time check above.
const fn enum_carries(values: &[&str], value: &str) -> bool {
    let mut index = 0;
    while index < values.len() {
        if bytes_equal(values[index].as_bytes(), value.as_bytes()) {
            return true;
        }
        index += 1;
    }
    false
}

/// Byte-wise equality, for [`enum_carries`]'s const context.
const fn bytes_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// The reserved tag-key prefix no `ScanConditionPair.Key` may use, as the member's
/// own pattern states it — `^(?!aws:)[a-zA-Z+-=._:/]+$`
/// (`smithy_query(shape="ScanConditionPair")`).
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The optional features a bare `CreateDetector` leaves DISABLED — the ones a
/// detector reports only once a request asks for them.
///
/// Runtime Monitoring is the exception `CreateDetector` documents: "When you don't
/// specify any features, with an exception to RUNTIME_MONITORING, all the optional
/// features are enabled by default"
/// (`smithy_query(query="doc",shape="CreateDetector")`). RDS Protection is opt-in
/// the same way: the recorded wire for a freshly created detector reports
/// `dataSources.database.rds.databaseEvents` DISABLED while every other data source
/// reports ENABLED.
pub const OPT_IN_FEATURES: [&str; 2] = [FEATURE_RUNTIME_MONITORING, FEATURE_RDS_LOGIN_EVENTS];

/// The three data sources GuardDuty always consumes for a detector — CloudTrail
/// management events, VPC flow logs and DNS logs.
///
/// They are reported as features (`DetectorFeatureResult` carries `CLOUD_TRAIL`,
/// `DNS_LOGS` and `FLOW_LOGS`, `smithy_query(shape="DetectorFeatureResult")`) but
/// are NOT members of `DetectorFeature` and have no `DataSourceConfigurations`
/// member, so no request can switch them off: they report `ENABLED` for the life of
/// the detector.
pub const FOUNDATIONAL_FEATURES: [&str; 3] =
    [FEATURE_CLOUD_TRAIL, FEATURE_DNS_LOGS, FEATURE_FLOW_LOGS];

/// Every feature a detector reports, in the order the service reports them: the
/// three foundational sources first, in the order `DataSourceConfigurationsResult`
/// lists them (`CloudTrail`, `DNSLogs`, `FlowLogs`), then every configurable
/// [`DETECTOR_FEATURES`] member in `DetectorFeature` model order.
///
/// The list is a total order over a SET, so a client may index it positionally.
pub fn reported_features() -> Vec<&'static str> {
    FOUNDATIONAL_FEATURES
        .iter()
        .chain(DETECTOR_FEATURES.iter())
        .copied()
        .collect()
}

/// The current GuardDuty finding schema version, the value every finding carries
/// in its required `SchemaVersion` member.
pub const FINDING_SCHEMA_VERSION: &str = "2.0";

/// The partition every emulated ARN lives in (`engine::emulator_arn` builds
/// `arn:aws:...`).
pub const PARTITION: &str = "aws";

/// The only member of the `DestinationType` enum: "Currently only Amazon S3
/// buckets are supported" (`smithy_query(shape="DestinationType")` lists `S3`).
pub const DESTINATION_TYPE_S3: &str = "S3";

/// Every member of `DestinationType`, in model order
/// (`smithy_query(shape="DestinationType")`).
pub const DESTINATION_TYPES: [&str; 1] = [DESTINATION_TYPE_S3];

/// A freshly created publishing destination's `PublishingStatus`: GuardDuty has
/// not yet verified that it can write to the destination
/// (`smithy_query(shape="PublishingStatus")`).
pub const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// A verified destination's `PublishingStatus`: GuardDuty is exporting findings
/// to it (`smithy_query(shape="PublishingStatus")`).
pub const PUBLISHING: &str = "PUBLISHING";

/// Every member of `PublishingStatus`, in model order
/// (`smithy_query(shape="PublishingStatus")`).
pub const PUBLISHING_STATUSES: [&str; 4] = [
    PENDING_VERIFICATION,
    PUBLISHING,
    "UNABLE_TO_PUBLISH_FIX_DESTINATION_PROPERTY",
    "STOPPED",
];

/// How long after `CreatePublishingDestination` the
/// `PENDING_VERIFICATION` -> [`PUBLISHING`] transition comes due.
///
/// A new destination is returned in a non-terminal status, so the move to
/// [`PUBLISHING`] is a transition the CLOCK owes. AWS documents no span for the
/// verification, so this one is chosen: non-zero, so a caller reading straight
/// back still observes `PENDING_VERIFICATION` (and whatever that status refuses),
/// and short enough that nothing has to wait long for the settled status.
pub const VERIFICATION_DELAY_MILLIS: i64 = 500;

/// How many publishing destinations one detector may hold.
///
/// `ListPublishingDestinations` reports a detector's destinations in pages of
/// `MaxResults` `range 1..50`
/// (`smithy_query(operation="ListPublishingDestinations")`), so a detector's whole
/// collection fits in one page of at most 50. Neither the model nor this
/// operation's documentation carries a destinations-per-detector figure, so — like
/// [`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`] — this is an
/// EMULATOR-CHOSEN bound rather than a published quota: a 51st destination is
/// refused here, where the real service may accept it.
///
/// The same number BOUNDS the store: a destination is retained until its detector
/// is deleted and has no expiry of its own, and it carries the `ClientToken` that
/// minted it, so without this maximum a caller varying or omitting `ClientToken`
/// could make the service retain records — and token mappings — without limit.
pub const PUBLISHING_DESTINATIONS_PER_DETECTOR: usize = 50;

/// The message a create past [`PUBLISHING_DESTINATIONS_PER_DETECTOR`] is refused
/// with. Like every other rejection here it names the resource class and the
/// limit, never the caller's values.
pub fn publishing_destination_limit_message() -> String {
    format!(
        "The request is rejected because the detector has reached the limit of {PUBLISHING_DESTINATIONS_PER_DETECTOR} publishing destinations."
    )
}

/// Is `arn` a well-formed ARN of `service`?
///
/// Form only — nothing resolves the resource it names. An ARN is
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>` with a non-empty
/// partition and resource; `region` and `account-id` are empty for the S3 bucket
/// ARNs GuardDuty publishes to (`arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/` —
/// `smithy_query(query="doc",shape="DestinationProperties",member="DestinationArn")`),
/// and the resource keeps any further `:` it carries.
pub fn is_arn_for(service: &str, arn: &str) -> bool {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && fields[2] == service
        && !fields[5].is_empty()
}

// ---------------------------------------------------------------------------
// Client rejections
//
// GuardDuty classifies a rejection by the REQUEST, not by what the store holds:
// a value that is not a well-formed id for its member is a parameter-validation
// failure, a well-formed id this account/Region does not own is an ownership
// failure, and neither message quotes the caller's value or the endpoint's
// Region back at them. The messages are built here, once, so every operation
// rejects the same input the same way.
// ---------------------------------------------------------------------------

/// The wire name of the `DetectorId` member, as the request carries it
/// (`smithy_query(operation="GetDetector")` renders `DetectorId` with the
/// `jsonName`/URI label `detectorId`). It is what a rejection names.
pub const DETECTOR_ID_MEMBER: &str = "detectorId";

/// The wire name of the `NextToken` member every paginated operation here carries
/// (`smithy_query(operation="ListDetectors")`), for the same reason.
pub const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The wire name of the `DestinationId` member
/// (`smithy_query(operation="DescribePublishingDestination")` renders it as the
/// `destinationId` JSON name / URI label).
pub const DESTINATION_ID_MEMBER: &str = "destinationId";

/// The wire path of `DestinationProperties.DestinationArn`, as a rejection names
/// it (`smithy_query(shape="DestinationProperties")` gives the members the JSON
/// names `destinationArn`, `kmsKeyArn` and `expectedBucketOwner`).
pub const DESTINATION_ARN_MEMBER: &str = "destinationProperties.destinationArn";

/// The wire path of `DestinationProperties.KmsKeyArn`. See
/// [`DESTINATION_ARN_MEMBER`].
pub const KMS_KEY_ARN_MEMBER: &str = "destinationProperties.kmsKeyArn";

/// The wire name of the `DestinationType` member
/// (`smithy_query(operation="CreatePublishingDestination")`).
pub const DESTINATION_TYPE_MEMBER: &str = "destinationType";

/// The wire path of `DestinationProperties.ExpectedBucketOwner`. See
/// [`DESTINATION_ARN_MEMBER`].
pub const EXPECTED_BUCKET_OWNER_MEMBER: &str = "destinationProperties.expectedBucketOwner";

/// The wire name of the `ThreatIntelSetId` member, as the request carries it in the
/// URI (`smithy_query(operation="GetThreatIntelSet")` renders it
/// `threatIntelSetId`).
pub const THREAT_INTEL_SET_ID_MEMBER: &str = "threatIntelSetId";

/// The wire name of the `ScanResourceCriteria` member of
/// `UpdateMalwareScanSettings` (`jsonName` `scanResourceCriteria`), for the same
/// reason.
pub const SCAN_RESOURCE_CRITERIA_MEMBER: &str = "scanResourceCriteria";

/// The wire name of the account-id member every members operation carries:
/// `AccountIds`' entries target `AccountId`
/// (`smithy_query(operation="DisassociateMembers")`), and `AccountDetail.AccountId`
/// renders as `accountId` (`smithy_query(shape="AccountDetail")`).
pub const ACCOUNT_ID_MEMBER: &str = "accountId";

/// The wire name of `AccountDetail`'s email member
/// (`smithy_query(shape="AccountDetail")` renders `Email` as `email`).
pub const EMAIL_MEMBER: &str = "email";

/// The length of a GuardDuty detector id: the 32 hexadecimal characters
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] mints, which is the
/// form every id this service hands out has. The model's own constraint
/// (`DetectorId`, len 1..300 — `smithy_query(shape="DetectorId")`) is far wider
/// than the id format, which is why a syntactically impossible id reaches the
/// handler at all instead of being refused at the boundary.
pub const DETECTOR_ID_LEN: usize = 32;

/// Is `id` a syntactically well-formed id of the kind this service MINTS — the 32
/// hexadecimal characters
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] hands out?
///
/// Form only: a well-formed id need not exist. Case is not significant, so an id
/// a caller upper-cased is still well formed — and is then refused as not owned,
/// which is the ownership failure and not a parameter one.
pub fn is_minted_id(id: &str) -> bool {
    id.len() == DETECTOR_ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Is `id` a syntactically well-formed `DetectorId`? See [`is_minted_id`].
pub fn is_detector_id(id: &str) -> bool {
    is_minted_id(id)
}

/// The message for a member whose VALUE cannot be a value of that member at all.
pub fn invalid_parameter_message(member: &str) -> String {
    format!("The request is rejected because the parameter {member} has an invalid value.")
}

/// The message for a member the request left out that the service cannot act
/// without.
pub fn missing_parameter_message(member: &str) -> String {
    format!("The request is rejected because the parameter {member} is required.")
}

/// The message for a well-formed id that this account does not own — an id
/// another account created, or one a delete removed. GuardDuty does not
/// distinguish "never existed" from "belongs to someone else": both are the same
/// refusal, which is also what keeps one account from probing another's ids.
pub fn not_owned_message(member: &str) -> String {
    format!(
        "The request is rejected because the input {member} is not owned by the current account."
    )
}

/// How many Regions this store will hold detectors for.
///
/// "You can have only one detector per account per Region"
/// (`smithy_query(query="doc",shape="CreateDetector")`) bounds one PARTITION of
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::detectors`] to a single
/// detector, but the partition key is the Region read from the request's credential
/// scope ([`request_region`]), so the caller chooses it: without a ceiling on the
/// number of partitions, a caller inventing Region names could make the process
/// retain one detector per distinct Region string — and each detector carries up to
/// [`MAX_MEMBERS_PER_DETECTOR`] members, [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`]
/// filters and [`IP_SETS_PER_DETECTOR`] IPSets. Together with the one-per-Region
/// rule this is what makes the detector map finite whatever a caller sends.
///
/// AWS runs orders of magnitude fewer Regions than this, so a request signed for a
/// real Region never meets it; a caller inventing Region names does. Emulator-chosen,
/// like [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`], which bounds the plan store the same
/// way, and deliberately set ABOVE [`MAX_CLIENT_TOKENS`] so the two detector guards
/// compose rather than mask one another: the idempotency index evicts its oldest
/// mapping while the detector map is still partitioning, so neither bound hides the
/// other. Enforced by `CreateDetector` before it writes.
pub const MAX_DETECTOR_REGIONS: usize = 2 * MAX_CLIENT_TOKENS;

/// The message a create that would partition the detector store into one Region
/// more than [`MAX_DETECTOR_REGIONS`] is refused with. The caller's Region is not
/// quoted back, as with every other rejection here.
pub const DETECTOR_REGION_LIMIT_MESSAGE: &str =
    "The request is rejected because the maximum number of detectors has been reached.";

/// The message a second `CreateDetector` is refused with. "You can have only one
/// detector per account per Region"
/// (`smithy_query(query="doc",shape="CreateDetector")`); the Region is the one the
/// request was signed for, so the rejection does not name it.
pub const DETECTOR_ALREADY_EXISTS_MESSAGE: &str =
    "The request is rejected because a detector already exists for the current account.";

/// Classify a minted id the store does not resolve, naming the member that carried
/// it: a value that is not an id of that form at all is a parameter failure, a
/// well-formed one this account/Region does not hold is an ownership failure.
///
/// One rule for every member that carries a minted id, so `detectorId` and
/// `threatIntelSetId` are refused the same way.
pub fn minted_id_rejection(member: &str, id: &str) -> String {
    if is_minted_id(id) {
        not_owned_message(member)
    } else {
        invalid_parameter_message(member)
    }
}

/// The length of an AWS account id: the 12 characters `AccountId` is constrained
/// to (`len 12..12` — `smithy_query(shape="AccountId")`). The model constrains the
/// LENGTH only, so a 12-character value that is not an account id at all reaches
/// the handler instead of being refused at the boundary.
pub const ACCOUNT_ID_LEN: usize = 12;

/// Is `id` a syntactically well-formed account id — exactly [`ACCOUNT_ID_LEN`]
/// decimal digits?
///
/// Form only: a well-formed id need not name an account this administrator has a
/// member record for, which is a different question and a different answer (see
/// [`not_a_member_message`]).
pub fn is_account_id(id: &str) -> bool {
    id.len() == ACCOUNT_ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

/// The administrator account the members of this emulator's detectors belong to:
/// the single account this emulator serves, which is the account every request it
/// answers is signed for.
///
/// It is NOT read from the request, so it is not caller input: a member's
/// `MasterId` / `AdministratorId` is the account that owns the detector the member
/// hangs off.
pub fn administrator_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// `UnprocessedAccount.Result` — "a reason why the account hasn't been processed"
/// (`smithy_query(shape="UnprocessedAccount")`) — for an account id the
/// administrator's detector holds no member record for.
///
/// A per-account failure is reported here rather than refusing the whole request:
/// the operations that take an `AccountIds` list answer 200 and report the accounts
/// they could not process, which is what `UnprocessedAccounts` is for.
pub fn not_a_member_message(administrator_account_id: &str) -> String {
    format!(
        "The request is rejected because the given account ID is not an associated member of account {administrator_account_id}."
    )
}

/// `UnprocessedAccount.Result` for an account `DisassociateMembers` was asked to
/// disassociate that is not ASSOCIATED with the administrator — one it holds no
/// member record for, and one whose relationship a disassociation already moved.
///
/// The condition is the association, not the record: the message states it ("the
/// latter is not yet associated to it"), and a disassociated member keeps its
/// record (see [`MemberRecord`]) while no longer being associated.
pub const NOT_ASSOCIATED_MESSAGE: &str = "The request is rejected because the current account cannot disassociate the given member account ID since the latter is not yet associated to it.";

/// `UnprocessedAccount.Result` for the administrator's OWN account id.
///
/// `CreateMembers` "creates member accounts of the current Amazon Web Services
/// account by specifying a list of Amazon Web Services account IDs"
/// (`smithy_query(query="doc",shape="CreateMembers")`): the current account is the
/// administrator in that relationship, so it cannot also be one of its own members.
pub const SELF_AS_MEMBER_MESSAGE: &str =
    "The request is rejected because the current account cannot be used as a member account.";

/// How many member accounts one administrator's detector holds.
///
/// GuardDuty's own quota is 5,000 member accounts per administrator account per
/// Region, and the store is bounded by it rather than by nothing: a member record
/// is keyed by a REQUEST-chosen account id, so without a bound a caller repeating
/// `CreateMembers` with fresh ids could grow the store without limit. Dropping a
/// member is not an option the way evicting an old finding is — the records are
/// retained on purpose (see [`MemberRecord`]) — so the quota is enforced by
/// REFUSING the accounts past it, which is what the real quota does.
pub const MAX_MEMBERS_PER_DETECTOR: usize = 5_000;

/// `UnprocessedAccount.Result` for an account that would take the administrator
/// past [`MAX_MEMBERS_PER_DETECTOR`] member accounts.
pub fn member_quota_message() -> String {
    format!(
        "The request is rejected because the current account cannot have more than {MAX_MEMBERS_PER_DETECTOR} member accounts."
    )
}

/// The lower bound on an `AccountDetail.Email`: "the email address must be a
/// minimum of 6 and a maximum of 64 characters long"
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`). The model
/// carries `len 1..64`, so only the maximum is refused at the boundary.
pub const EMAIL_MIN_LEN: usize = 6;

/// The characters the local name of an email address cannot contain, as the member
/// documents them: "whitespace, \" ' ( ) < > [ ] : ' , \\ | % &"
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`). Whitespace
/// is checked separately, since it is a class rather than a character.
pub const EMAIL_LOCAL_FORBIDDEN: [char; 14] = [
    '"', '\'', '(', ')', '<', '>', '[', ']', ':', ',', '\\', '|', '%', '&',
];

/// Is `email` a valid `AccountDetail.Email`?
///
/// Every rule is one the member itself documents
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`), in the order
/// it lists them: 6..64 characters, 7-bit ASCII only, exactly one `@` separating
/// the local name from the domain name, no [`EMAIL_LOCAL_FORBIDDEN`] character or
/// whitespace and no leading dot in the local name, and a domain name of
/// `[a-zA-Z0-9]`, hyphen and dot only that neither begins nor ends with a dot or a
/// hyphen and contains at least one dot.
pub fn is_member_email(email: &str) -> bool {
    if email.len() < EMAIL_MIN_LEN || !email.is_ascii() {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let local_ok = !local.is_empty()
        && !local.starts_with('.')
        && !local
            .chars()
            .any(|c| c.is_whitespace() || EMAIL_LOCAL_FORBIDDEN.contains(&c));
    let domain_ok = !domain.is_empty()
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && domain.contains('.');
    local_ok && domain_ok
}

/// `Member.RelationshipStatus` for an account `CreateMembers` has recorded and no
/// invitation has been sent to yet.
///
/// The member targets a plain `String` — "the status of the relationship between
/// the member and the administrator"
/// (`smithy_query(query="doc",shape="Member",member="RelationshipStatus")`) — so
/// the values are GuardDuty's documented relationship statuses, which are
/// mixed-case words rather than the SCREAMING_CASE of this service's modelled
/// enums.
pub const RELATIONSHIP_CREATED: &str = "Created";

/// `Member.RelationshipStatus` for a member the administrator disassociated with
/// `DisassociateMembers`. See [`RELATIONSHIP_CREATED`] for where the values come
/// from.
pub const RELATIONSHIP_REMOVED: &str = "Removed";

/// The message a request is DENIED with when the account is not authorized on the
/// resource `member` names.
///
/// Like [`not_owned_message`], it names the member and not the caller's value: a
/// denial must not disclose whether the id names anything at all.
pub fn not_authorized_message(member: &str) -> String {
    format!(
        "The request is rejected because the current account is not authorized to perform this operation on the input {member}."
    )
}

/// Classify a `DetectorId` the store does not resolve: a value that is not a
/// detector id at all is a parameter failure, a well-formed one this
/// account/Region does not hold is an ownership failure.
pub fn detector_id_rejection(detector_id: &str) -> String {
    minted_id_rejection(DETECTOR_ID_MEMBER, detector_id)
}

/// Classify a `DestinationId` the detector does not hold, the same way: a
/// publishing destination id is one of the ids this service mints, so it is
/// refused exactly as a detector id is.
pub fn destination_id_rejection(destination_id: &str) -> String {
    minted_id_rejection(DESTINATION_ID_MEMBER, destination_id)
}

/// The wire name of the `MalwareProtectionPlanId` member, as the request carries
/// it (`smithy_query(operation="GetMalwareProtectionPlan")` binds it as the URI
/// label `MalwareProtectionPlanId` and the create reports it as
/// `malwareProtectionPlanId`). It is what a rejection names.
pub const MALWARE_PROTECTION_PLAN_ID_MEMBER: &str = "malwareProtectionPlanId";

/// The wire name of `CreateMalwareProtectionPlan`'s `Role` member
/// (`smithy_query(operation="CreateMalwareProtectionPlan")` renders it `role`).
pub const ROLE_MEMBER: &str = "role";

/// The wire name of the `ProtectedResource` member, for the same reason.
pub const PROTECTED_RESOURCE_MEMBER: &str = "protectedResource";

/// The `MalwareProtectionPlanStatus` a plan GuardDuty accepted reports: the plan
/// is in force and nothing is wrong with it. The enum's other two members
/// (`WARNING`, `ERROR` — `smithy_query(shape="MalwareProtectionPlanStatus")`) both
/// describe a plan whose role or bucket the service could not use, which is a
/// condition this emulator never produces, so every member of the enum is
/// terminal and a created plan needs no later transition.
pub const MALWARE_PROTECTION_PLAN_ACTIVE: &str = "ACTIVE";

/// The `MalwareProtectionPlanTaggingActionStatus` a plan gets when the create
/// names no `Actions`.
///
/// Tagging is what "will be added to the S3 object after scanning"
/// (`smithy_query(operation="CreateMalwareProtectionPlan")`), and writing a tag
/// onto a scanned object needs a permission on the role beyond reading it, so a
/// plan that did not ask for it does not tag.
pub const DEFAULT_TAGGING_ACTION_STATUS: &str = DISABLED;

/// The message a create for an already-protected resource is refused with.
///
/// GuardDuty protects one S3 bucket with one plan, so the second create for the
/// same bucket conflicts with the first. The caller's bucket name is not quoted
/// back: the rejection states the rule, as every other rejection here does.
pub const MALWARE_PROTECTION_PLAN_CONFLICT_MESSAGE: &str =
    "The request is rejected because a Malware Protection plan already exists for the specified protected resource.";

/// How many Malware Protection plans one account may hold in one Region.
///
/// A plan is keyed by a REQUEST-chosen bucket name — the bucket is never required
/// to exist — so without a bound a caller naming fresh bucket names could grow the
/// store without limit. Neither the Smithy model nor this service's own
/// documentation carries a plans-per-account-per-Region figure, so unlike
/// [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`] (100, documented on `CreateFilter`) and
/// [`MAX_MEMBERS_PER_DETECTOR`] (5,000, GuardDuty's published member quota) this
/// number is an EMULATOR-CHOSEN bound, not a value AWS publishes: a 26th plan is
/// refused here, where the real service may accept it. The refusal states the rule
/// rather than the number, and it is what keeps the plan map, the plan idempotency
/// index (an entry is written only for a plan that was minted) and the per-create
/// duplicate scan bounded.
pub const MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION: usize = 25;

/// The message a create past
/// [`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`] is refused with.
pub const MALWARE_PROTECTION_PLAN_QUOTA_EXCEEDED_MESSAGE: &str =
    "The request is rejected because the maximum number of Malware Protection plans per account per Region has been reached.";

/// How many Regions this store will hold Malware Protection plans for.
///
/// [`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`] bounds each Region's
/// plans, but the Region is read from the REQUEST's credential scope
/// ([`request_region`]), so the caller chooses the partition KEY as well as what
/// goes in it: a per-Region quota bounds one partition and not the number of
/// partitions, so without this a caller varying the Region could grow the plan map
/// past every per-Region quota there is. AWS runs fewer Regions than this, so a
/// request signed for a real Region never meets it; a caller inventing Region
/// names does. Emulator-chosen, like the per-Region quota.
///
/// The detector-rooted store answers the same hazard the same way, which this does
/// not change: [`MAX_DETECTOR_REGIONS`] bounds how many Regions it partitions into,
/// a detector is one per Region, and its idempotency index is bounded by
/// [`MAX_CLIENT_TOKENS`] with eviction.
pub const MAX_MALWARE_PROTECTION_PLAN_REGIONS: usize = 50;

/// The most Malware Protection plans this store will ever hold: the per-Region
/// quota times the Region bound. It is what makes the plan map — and the plan
/// idempotency index, which gets an entry only for a plan that was minted — finite
/// whatever a caller sends.
pub const MAX_MALWARE_PROTECTION_PLANS_TOTAL: usize =
    MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION * MAX_MALWARE_PROTECTION_PLAN_REGIONS;

/// The message a create that would partition the plan store into one Region more
/// than [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`] is refused with. The caller's
/// Region is not quoted back, as with every other rejection here.
pub const MALWARE_PROTECTION_PLAN_TOTAL_LIMIT_MESSAGE: &str =
    "The request is rejected because the maximum number of Malware Protection plans has been reached.";

/// The message a read of a plan this account and Region does not hold answers
/// with. As with a detector id, "never existed" and "belongs to someone else" are
/// one refusal, so one account cannot probe another's plan ids.
pub fn malware_protection_plan_not_found_message() -> String {
    not_owned_message(MALWARE_PROTECTION_PLAN_ID_MEMBER)
}

/// Is `value` an IAM role ARN?
///
/// `Role` is the "Amazon Resource Name (ARN) of the IAM role that has the
/// permissions to scan and add tags to the associated protected resource"
/// (`smithy_query(operation="CreateMalwareProtectionPlan")`), and the model
/// constrains it no further than `String`, so the form is checked here:
/// `arn:<partition>:iam::<account>:role/<name>`. IAM is global, so a role ARN's
/// Region segment is empty. Form only — the role need not exist, and its account
/// need not be the caller's.
pub fn is_iam_role_arn(value: &str) -> bool {
    let mut parts = value.splitn(6, ':');
    parts.next() == Some("arn")
        && parts.next().is_some_and(|partition| !partition.is_empty())
        && parts.next() == Some("iam")
        && parts.next() == Some("")
        && parts
            .next()
            .is_some_and(|account| !account.is_empty() && account.bytes().all(|b| b.is_ascii_digit()))
        && parts
            .next()
            .and_then(|resource| resource.strip_prefix("role/"))
            .is_some_and(|name| !name.is_empty())
}

/// The ARN of the Malware Protection plan `id` in `region`.
///
/// Derived, never stored: the plan's own id and the Region that serves it are the
/// only variable parts, so a read cannot report an ARN that disagrees with the id
/// it was asked about.
pub fn malware_protection_plan_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        GUARDDUTY_ARN_SERVICE,
        region,
        &format!("malware-protection-plan/{id}"),
    )
}

/// This service's segment in an ARN it owns — the `arnNamespace`/`endpointPrefix`
/// from `smithy_query(query="service_info")`.
const GUARDDUTY_ARN_SERVICE: &str = "guardduty";

/// The wire name of the filter-name member on the operations that carry it as a
/// URI label — `GetFilter`, `UpdateFilter` and `DeleteFilter` bind
/// `FilterName` to `/detector/{DetectorId}/filter/{FilterName}`
/// (`smithy_query(operation="GetFilter")`).
pub const FILTER_NAME_MEMBER: &str = "filterName";

/// The wire name `CreateFilter` carries the filter name under: its `Name` member
/// has `jsonName` `name` (`smithy_query(operation="CreateFilter")`). A rejection
/// names the member AS THE REQUEST CARRIES IT, which is why creating and reading
/// a filter name it differently.
pub const CREATE_FILTER_NAME_MEMBER: &str = "name";

/// The `FilterName` length bounds (`smithy_query(shape="FilterName")`: len
/// 3..64). `GetFilter`/`UpdateFilter`/`DeleteFilter` take the name as a plain
/// `String` URI label with no constraint of its own, so a name outside these
/// bounds reaches the handler there and is classified by [`is_filter_name`].
pub const FILTER_NAME_MIN_LEN: usize = 3;
/// See [`FILTER_NAME_MIN_LEN`].
pub const FILTER_NAME_MAX_LEN: usize = 64;

/// Is `name` a syntactically well-formed `FilterName`?
///
/// "Valid characters include period (.), underscore (_), dash (-), and
/// alphanumeric characters. A whitespace is considered to be an invalid
/// character" (`smithy_query(query="doc",shape="CreateFilterRequest",member="Name")`),
/// within the modelled length bounds. Form only: a well-formed name need not
/// exist.
pub fn is_filter_name(name: &str) -> bool {
    (FILTER_NAME_MIN_LEN..=FILTER_NAME_MAX_LEN).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Classify a filter name the detector does not hold, the way
/// [`detector_id_rejection`] classifies a detector id: a value that cannot be a
/// filter name at all is a parameter failure, a well-formed one this account
/// does not hold is an ownership failure.
pub fn filter_name_rejection(name: &str) -> String {
    if is_filter_name(name) {
        not_owned_message(FILTER_NAME_MEMBER)
    } else {
        invalid_parameter_message(FILTER_NAME_MEMBER)
    }
}

/// The message a second `CreateFilter` under a name the detector already holds
/// is refused with. Creating is not updating — `UpdateFilter` is the operation
/// that changes an existing filter — and the rejection does not quote the name
/// back at the caller.
pub const FILTER_ALREADY_EXISTS_MESSAGE: &str =
    "The request is rejected because a filter with the same name already exists.";

/// "The maximum number of saved filters per Amazon Web Services account per
/// Region is 100" (`smithy_query(query="doc",shape="CreateFilter")`). A detector
/// is itself one per account per Region, so the per-detector count IS the
/// per-account-per-Region count.
pub const MAX_FILTERS_PER_ACCOUNT_PER_REGION: usize = 100;

/// The message a `CreateFilter` past [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`] is
/// refused with.
pub const FILTER_QUOTA_EXCEEDED_MESSAGE: &str =
    "The request is rejected because the maximum number of saved filters per account per Region has been reached.";

/// The `FilterAction` a filter gets when the request names none: "Default: NOOP"
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`).
pub const FILTER_ACTION_NOOP: &str = "NOOP";

/// The `FilterVersion` a freshly created filter carries. `FilterVersion` is
/// `range min 1` (`smithy_query(shape="FilterVersion")`), so a filter nothing has
/// updated is at version 1.
pub const FILTER_INITIAL_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// Modelled-constraint refusals
// ---------------------------------------------------------------------------

/// How GuardDuty states a refusal the request boundary made: a member the model
/// requires was absent, or a value fell outside a bound the model declares.
///
/// GuardDuty's model declares exactly two errors for every operation —
/// `BadRequestException` (`httpError` 400) and `InternalServerErrorException`
/// (500) (`smithy_query(query="service_info")` lists them as the service's common
/// errors) — and NO `ValidationException` shape. So a modelled-constraint
/// violation is the same refusal as any other unacceptable input, and is worded
/// the same way: the parameter at fault and the rule it broke, built from the
/// REQUEST, with the caller's value never quoted back.
///
/// `service.rs` routes the framework's pre-handler refusal through here, so a
/// constraint the model declares on ANY member of ANY operation in this service
/// is refused under the modelled error rather than under a shape no GuardDuty
/// client can catch.
pub fn constraint_rejection(violations: &[::aws_smithy_schema_server::Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(violation_clause).collect();
    if clauses.is_empty() {
        // The framework describes every refusal it hands us with at least one
        // violation; an empty list has no member to name, so it falls back to the
        // service's parameter wording without one.
        return "The request is rejected because it contains an invalid parameter.".to_string();
    }
    clauses.join(" ")
}

/// One violation, as a GuardDuty sentence.
fn violation_clause(violation: &::aws_smithy_schema_server::Violation) -> String {
    use ::aws_smithy_schema_server::ViolationKind;

    // The path is already the member's wire spelling (lowerCamel, with
    // `.member` steps for collections), which is how the caller named it.
    let member = violation.path.as_str();
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => {
            return format!(
                "The request is rejected because the parameter {member} is required."
            )
        }
        ViolationKind::MinLength(min) => format!("it must have a length of at least {min}"),
        ViolationKind::MaxLength(max) => format!("it must have a length of at most {max}"),
        ViolationKind::MinValue(min) => format!("it must be greater than or equal to {min}"),
        ViolationKind::MaxValue(max) => format!("it must be less than or equal to {max}"),
        ViolationKind::Pattern(pattern) => {
            format!("it must satisfy the pattern {pattern}")
        }
        ViolationKind::EnumValue(allowed) => format!("it must be one of {}", allowed.join(", ")),
        ViolationKind::UniqueItems => "its values must be unique".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a class this service has no
        // specific wording for still states the member at fault.
        _ => return invalid_parameter_message(member),
    };
    format!(
        "The request is rejected because the parameter {member} has an invalid value: {requirement}."
    )
}

/// The modelled server fault a handler answers when the engine it was built over
/// holds no state for this service.
///
/// `Type` is "The error type" (`smithy_query(shape="InternalServerErrorException")`)
/// — the error class, the same value the body's `__type` discriminator carries.
pub fn state_unavailable() -> crate::ops::types::shared_types::InternalServerErrorException {
    crate::ops::types::shared_types::InternalServerErrorException {
        message: Some("the GuardDuty state store is unavailable".to_string()),
        r#type: Some("InternalServerErrorException".to_string()),
    }
}

/// A configured detector feature and when its status last changed.
#[derive(Debug, Clone)]
pub struct FeatureRecord {
    pub name: String,
    pub status: String,
    pub updated_at_millis: i64,
    pub additional_configuration: Vec<AdditionalConfigRecord>,
}

/// A feature's nested additional configuration (e.g. a Runtime Monitoring agent
/// management switch).
#[derive(Debug, Clone)]
pub struct AdditionalConfigRecord {
    pub name: String,
    pub status: String,
    pub updated_at_millis: i64,
}

/// One `ScanConditionPair`: a tag key and the value that condition matches.
///
/// `Value` is optional while `Key` is required
/// (`smithy_query(shape="ScanConditionPair")`), so a pair that names only a key is
/// stored as it arrived rather than being filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanTagPair {
    pub key: String,
    pub value: Option<String>,
}

/// One `ScanCriterion`: each `ScanCriterionKey` mapped to the `MapEquals` pairs that
/// criterion matches. A `BTreeMap` so a read reports the criteria in a total order
/// whatever order the request listed them in.
pub type ScanCriterionRecord = BTreeMap<String, Vec<ScanTagPair>>;

/// A detector's Malware Protection scan settings: what
/// `UpdateMalwareScanSettings` writes and `GetMalwareScanSettings` reports.
///
/// `Include` is the "condition that when matched will allow a malware scan for a
/// certain resource" and `Exclude` the one that "will prevent" it
/// (`smithy_query(shape="ScanResourceCriteria")`), so the two are kept apart rather
/// than folded into one list.
#[derive(Debug, Clone)]
pub struct MalwareScanSettings {
    /// One of the `EbsSnapshotPreservation` members the generated
    /// [`crate::ops::types::shared_types::EBS_SNAPSHOT_PRESERVATION_VALUES`] lists;
    /// [`NO_RETENTION`] until an update asks for the other one.
    pub ebs_snapshot_preservation: String,
    /// `ScanResourceCriteria.Include`, empty when the detector has no inclusion
    /// criteria.
    pub include: ScanCriterionRecord,
    /// `ScanResourceCriteria.Exclude`, empty when the detector has no exclusion
    /// criteria.
    pub exclude: ScanCriterionRecord,
}

impl Default for MalwareScanSettings {
    /// The settings a detector has before any `UpdateMalwareScanSettings`: no scan
    /// criteria, so every eligible resource is in scope, and snapshots retained for
    /// nothing.
    fn default() -> Self {
        Self {
            ebs_snapshot_preservation: NO_RETENTION.to_string(),
            include: BTreeMap::new(),
            exclude: BTreeMap::new(),
        }
    }
}

/// A GuardDuty detector: the account/Region-level resource the whole service hangs
/// off.
#[derive(Debug, Clone)]
pub struct Detector {
    pub id: String,
    /// The Region whose endpoint created this detector, and the only one that
    /// serves it.
    pub region: String,
    /// `DetectorStatus`: [`ENABLED`] or [`DISABLED`].
    pub status: String,
    pub service_role: String,
    /// The Malware Protection service-linked role, which is a DIFFERENT role from
    /// the detector-level [`Self::service_role`]:
    /// `MalwareProtectionConfigurationResult.ServiceRole` is "The GuardDuty Malware
    /// Protection service role"
    /// (`smithy_query(shape="MalwareProtectionConfigurationResult")`).
    pub malware_protection_service_role: String,
    pub finding_publishing_frequency: Option<String>,
    pub features: Vec<FeatureRecord>,
    pub tags: BTreeMap<String, String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// This detector's findings, keyed by finding id.
    pub findings: BTreeMap<String, FindingRecord>,
    /// This detector's filters, keyed by filter name. `DeleteDetector` drops them
    /// with the detector.
    pub filters: BTreeMap<String, FilterRecord>,
    /// This detector's publishing destinations, keyed by `DestinationId`. A
    /// destination belongs to the detector it was created under, so
    /// `DeleteDetector` drops them with the detector.
    ///
    /// BOUNDED by [`PUBLISHING_DESTINATIONS_PER_DETECTOR`]: a destination is
    /// retained until its detector is deleted and has no expiry of its own, and
    /// each one carries the `ClientToken` that minted it
    /// ([`Detector::publishing_destination_for_token`]), so the idempotency index
    /// is bounded by the same maximum rather than growing with the
    /// REQUEST-chosen tokens.
    pub publishing_destinations: BTreeMap<String, PublishingDestinationRecord>,
    /// This detector's ThreatIntelSets, keyed by minted `ThreatIntelSetId`.
    /// `DeleteDetector` drops them with the detector.
    pub threat_intel_sets: BTreeMap<String, ThreatIntelSetRecord>,
    /// This detector's investigations, keyed by investigation id. They hang off the
    /// detector like its findings and filters, so `DeleteDetector` drops them with
    /// it.
    pub investigations: BTreeMap<String, InvestigationRecord>,
    /// This detector's trusted entity sets, keyed by the minted entity-set id.
    /// They hang off the detector — every operation on one is reached through
    /// `/detector/{DetectorId}/trustedentityset`
    /// (`smithy_query(operation="CreateTrustedEntitySet")`) — so
    /// `DeleteDetector` drops them with it.
    pub trusted_entity_sets: BTreeMap<String, EntitySetRecord>,
    /// This detector's threat entity sets, keyed by the minted entity-set id.
    /// They hang off the detector like the trusted entity sets above — every
    /// operation on one is reached through
    /// `/detector/{DetectorId}/threatentityset`
    /// (`smithy_query(operation="CreateThreatEntitySet")`) — so `DeleteDetector`
    /// drops them with it.
    ///
    /// BOUNDED by [`THREAT_ENTITY_SETS_PER_DETECTOR`], which
    /// `CreateThreatEntitySet` enforces before it writes.
    pub threat_entity_sets: BTreeMap<String, EntitySetRecord>,
    /// This detector's IPSets (trusted IP lists), keyed by the minted `IpSetId`.
    /// They are sub-resources of the detector — every IPSet operation is bound
    /// under `/detector/{DetectorId}/ipset`
    /// (`smithy_query(operation="GetIPSet")`) — so `DeleteDetector` drops them
    /// with it.
    pub ip_sets: BTreeMap<String, IpSetRecord>,
    /// This detector's Malware Protection scan settings.
    pub malware_scan_settings: MalwareScanSettings,
    /// The member accounts this detector's account administers, keyed by account
    /// id. `DeleteDetector` drops them with the detector, like the findings and
    /// filters above.
    pub members: BTreeMap<String, MemberRecord>,
}

/// A member account of the administrator that owns the detector this record hangs
/// off.
///
/// The record outlives the ASSOCIATION: when a member is disassociated "the member
/// account details obtained from the CreateMembers API, including the associated
/// email addresses, are retained. This is done so that the delegated administrator
/// can invoke the InviteMembers API without the need to invoke the CreateMembers
/// API again. To remove the details associated with a member account, the delegated
/// administrator must invoke the DeleteMembers API"
/// (`smithy_query(query="doc",shape="DisassociateMembers")`). So disassociating
/// moves [`Self::relationship_status`] and nothing else, and only `DeleteMembers`
/// drops the record.
#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub account_id: String,
    /// The member account's email address, as the request that recorded the account
    /// gave it.
    pub email: String,
    /// `Member.RelationshipStatus`: [`RELATIONSHIP_CREATED`] or
    /// [`RELATIONSHIP_REMOVED`].
    pub relationship_status: String,
    /// When the relationship last changed, in epoch milliseconds — the member's
    /// `UpdatedAt`.
    pub updated_at_millis: i64,
}

impl MemberRecord {
    /// Is this member currently ASSOCIATED with the administrator?
    ///
    /// "Is a member account of this administrator" and "is associated with it" are
    /// two different questions, because the record is retained across a
    /// disassociation (see the type's documentation): `GetMembers` answers the
    /// first, `DisassociateMembers` acts on the second.
    pub fn is_associated(&self) -> bool {
        self.relationship_status != RELATIONSHIP_REMOVED
    }
}

/// The modelled `Member` a read reports for one stored member account.
///
/// `ListMembers` and `GetMembers` both report the `Members` list, so the projection
/// lives here once: two reads of one member must not disagree.
///
/// `MasterId` and `AdministratorId` are both "the administrator account ID"
/// (`smithy_query(shape="Member")`) — the same fact under the old and the current
/// name — so both carry the account that owns the detector this member hangs off.
pub fn member_view(
    member: &MemberRecord,
    administrator: &str,
) -> crate::ops::types::shared_types::Member {
    crate::ops::types::shared_types::Member {
        account_id: member.account_id.clone(),
        // The member account's own detector is not a resource this emulator holds;
        // the member is optional, so it is absent rather than invented.
        detector_id: None,
        master_id: administrator.to_string(),
        administrator_id: Some(administrator.to_string()),
        email: member.email.clone(),
        relationship_status: member.relationship_status.clone(),
        // No invitation is modelled yet (`InviteMembers` is still a scaffold), so
        // there is no instant at which one was sent.
        invited_at: None,
        updated_at: iso8601(member.updated_at_millis),
    }
}

impl Detector {
    /// The status of one feature, or [`DISABLED`] when the detector does not carry
    /// it.
    pub fn feature_status(&self, name: &str) -> &str {
        self.features
            .iter()
            .find(|f| f.name == name)
            .map_or(DISABLED, |f| f.status.as_str())
    }

    /// What one data source reports: the status of the feature that governs it and
    /// the instant that feature's status last changed, in epoch milliseconds.
    ///
    /// The status is the feature's OWN status, never a function of the detector's
    /// `Status` member: that member carries whether the detector is enabled, while
    /// the feature and data-source views carry the configuration, which a detector
    /// keeps across being disabled and enabled again.
    pub fn data_source(&self, feature: &str) -> (String, i64) {
        self.features.iter().find(|f| f.name == feature).map_or_else(
            || (DISABLED.to_string(), self.updated_at_millis),
            |f| (f.status.clone(), f.updated_at_millis),
        )
    }

    /// Materialise every threat list's clock-owed status change, so no two readers
    /// of this detector disagree about where a list has got to.
    pub fn settle_threat_intel_sets(&mut self, now_millis: i64) {
        for set in self.threat_intel_sets.values_mut() {
            set.settle(now_millis);
        }
    }

    /// The threat list `id`, if this detector holds it.
    pub fn threat_intel_set(&self, id: &str) -> Option<&ThreatIntelSetRecord> {
        self.threat_intel_sets.get(id)
    }

    /// The threat list a create carrying `token` already minted, if any — the
    /// `@idempotencyToken` lookup, scoped to the detector the list hangs off.
    pub fn threat_intel_set_for_token(&self, token: &str) -> Option<&ThreatIntelSetRecord> {
        self.threat_intel_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token))
    }

    /// Set one feature's status, recording `now` as its `UpdatedAt`. Adds the
    /// feature when the detector does not carry it yet.
    pub fn set_feature(&mut self, name: &str, status: &str, now: i64) {
        if let Some(existing) = self.features.iter_mut().find(|f| f.name == name) {
            if existing.status != status {
                existing.status = status.to_string();
                existing.updated_at_millis = now;
            }
        } else {
            self.features.push(FeatureRecord {
                name: name.to_string(),
                status: status.to_string(),
                updated_at_millis: now,
                additional_configuration: Vec::new(),
            });
        }
    }

    /// The publishing destination a create carrying `token` already minted, if any
    /// — the `@idempotencyToken` lookup, scoped to the detector the destination
    /// hangs off, the way [`Self::threat_intel_set_for_token`] is.
    ///
    /// The token lives ON the record, so the index is bounded by
    /// [`PUBLISHING_DESTINATIONS_PER_DETECTOR`] with the destinations themselves
    /// and a mapping cannot outlive the resource it names.
    pub fn publishing_destination_for_token(
        &self,
        token: &str,
    ) -> Option<&PublishingDestinationRecord> {
        self.publishing_destinations
            .values()
            .find(|destination| destination.client_token.as_deref() == Some(token))
    }

    /// Put every named finding this detector holds into the `archived` state,
    /// recording `now` as the `UpdatedAt` of each finding the call actually moves.
    ///
    /// Archiving and unarchiving are the two directions of ONE write over the
    /// `FindingIds` a request names (`smithy_query(operation="ArchiveFindings")` /
    /// `("UnarchiveFindings")`), so both go through here and the LAST call to land
    /// decides the state. A finding already in the requested state is left
    /// untouched — `UpdatedAt` included, which is what makes a repeated call a true
    /// no-op — and an id this detector does not hold moves nothing, the same way a
    /// findings read reports only the ids it holds.
    ///
    /// The archive state is a property of the RECORD, not an observation of the
    /// activity, so only [`FindingRecord::updated_at_millis`] moves:
    /// [`FindingRecord::event_last_seen_millis`] stays where the last observation
    /// left it.
    pub fn set_findings_archived(&mut self, finding_ids: &[String], archived: bool, now: i64) {
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                if finding.archived != archived {
                    finding.archived = archived;
                    finding.updated_at_millis = now;
                }
            }
        }
    }

    /// The investigation a create already minted for `token`, if any — the seam
    /// that makes `CreateInvestigation` idempotent under a replayed `ClientToken`.
    pub fn investigation_for_token(&self, token: &str) -> Option<&InvestigationRecord> {
        self.investigations
            .values()
            .find(|investigation| investigation.client_token.as_deref() == Some(token))
    }

    /// The member account ids this detector administers, in account-id order.
    ///
    /// `only_associated` narrows the answer to the members that are ASSOCIATED
    /// with the administrator ([`MemberRecord::is_associated`]), which is the
    /// distinction `ListMembers` exposes: "whether to only return associated
    /// members or to return all members (including members who haven't been
    /// invited yet or have been disassociated)"
    /// (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`).
    pub fn member_ids(&self, only_associated: bool) -> Vec<String> {
        self.members
            .values()
            .filter(|member| !only_associated || member.is_associated())
            .map(|member| member.account_id.clone())
            .collect()
    }

    /// The names of the filters this detector holds, in filter-name order.
    ///
    /// The order is TOTAL — a filter name is unique within a detector, because it
    /// is the key the filters are stored under — which is what makes a page token
    /// mean the same thing on the call that hands it back.
    pub fn filter_names(&self) -> Vec<String> {
        self.filters.keys().cloned().collect()
    }

    /// How many investigations this detector holds that were initiated at or after
    /// `since`, in epoch milliseconds.
    pub fn investigations_started_since(&self, since: i64) -> usize {
        self.investigations
            .values()
            .filter(|investigation| investigation.started_at_millis >= since)
            .count()
    }

    /// The trusted entity set a create already minted for `token`, if any — the
    /// seam that makes `CreateTrustedEntitySet` idempotent under a replayed
    /// `ClientToken`.
    pub fn trusted_entity_set_for_token(&self, token: &str) -> Option<&EntitySetRecord> {
        self.trusted_entity_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token))
    }

    /// The threat entity set a create already minted for `token`, if any — the
    /// seam that makes `CreateThreatEntitySet` idempotent under a replayed
    /// `ClientToken`.
    ///
    /// Scoped to the THREAT sets: a token resolves to a resource of the type the
    /// create that carried it made, so one token replayed across the two
    /// entity-set creates never hands back the other one's id.
    pub fn threat_entity_set_for_token(&self, token: &str) -> Option<&EntitySetRecord> {
        self.threat_entity_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token))
    }

    /// The IPSet a create already minted for `token`, if any — the seam that makes
    /// `CreateIPSet` idempotent under a replayed `ClientToken`.
    ///
    /// An IPSet a delete has already accepted ([`IpSetRecord::is_deleting`])
    /// resolves to nothing, so a replayed create never answers with a list the
    /// service is no longer keeping — the same rule
    /// [`Self::detector_for_client_token`]'s mapping follows for a deleted detector.
    pub fn ip_set_for_token(&self, token: &str) -> Option<&IpSetRecord> {
        self.ip_sets
            .values()
            .find(|ip_set| ip_set.client_token.as_deref() == Some(token) && !ip_set.is_deleting())
    }

    /// The ids of the LIVE IPSets this detector holds, in `IpSetId` order.
    ///
    /// The order is TOTAL — the id is the key the IPSets are stored under, so it is
    /// unique within a detector — which is what makes a page token mean the same
    /// thing on the call that hands it back.
    ///
    /// LIVE is the same set [`Self::live_ip_set_count`] counts: a list whose delete
    /// has been accepted is no longer one of the detector's IPSets, so `ListIPSets`
    /// stops naming it. Its record is still retained
    /// ([`DELETED_IP_SETS_RETAINED`]) so `GetIPSet` can report the
    /// [`IP_SET_DELETE_PENDING`] / [`IP_SET_DELETED`] status the delete moved it
    /// to — being readable on its own id is not the same as being part of the
    /// collection a list enumerates.
    pub fn live_ip_set_ids(&self) -> Vec<String> {
        self.ip_sets
            .values()
            .filter(|ip_set| !ip_set.is_deleting())
            .map(|ip_set| ip_set.id.clone())
            .collect()
    }

    /// How many of this detector's IPSets are LIVE — held and not being deleted.
    ///
    /// This is the count [`IP_SETS_PER_DETECTOR`] bounds: a list whose delete has
    /// been accepted is only reported, never used, so it no longer occupies one of
    /// the detector's slots.
    pub fn live_ip_set_count(&self) -> usize {
        self.ip_sets
            .values()
            .filter(|ip_set| !ip_set.is_deleting())
            .count()
    }

    /// Accept the deletion of the IPSet `ip_set_id` names.
    ///
    /// `None` when this detector holds no such IPSet at all; `Some(true)` when this
    /// call is the one that accepted the deletion, and `Some(false)` when a delete
    /// had already been accepted for it — the caller needs that distinction because
    /// only the first delete changes the set of LIVE sub-resources the detector
    /// holds, and so only the first moves the detector's own `UpdatedAt`.
    ///
    /// The list moves to [`IP_SET_DELETE_PENDING`] and the clock owes it
    /// [`IP_SET_DELETED`], which is what `GetIPSet` reports afterwards; a list
    /// already being deleted is left exactly where it is, so a repeated delete is a
    /// true no-op.
    ///
    /// Keeping a deleted record is what lets a read report [`IP_SET_DELETED`], so
    /// this is also where that retention is BOUNDED: at most
    /// [`DELETED_IP_SETS_RETAINED`] deleted records survive per detector, and a
    /// delete past that drops the one deleted longest ago — the same
    /// oldest-first eviction [`ComAmazonawsGuarddutyGuarddutyapiserviceState::record_client_token`]
    /// uses, for the same reason: without it a caller could create and delete
    /// without limit and make the service retain every record.
    pub fn delete_ip_set(&mut self, ip_set_id: &str, now: i64) -> Option<bool> {
        let accepted = self.ip_sets.get_mut(ip_set_id)?.begin_delete(now);
        if accepted {
            self.evict_deleted_ip_sets(ip_set_id);
        }
        Some(accepted)
    }

    /// Drop deleted IPSets, oldest first, until at most [`DELETED_IP_SETS_RETAINED`]
    /// remain. `keep` is never evicted: it is the record the call that triggered this
    /// just deleted, and a caller reads that one back.
    fn evict_deleted_ip_sets(&mut self, keep: &str) {
        let surplus = self
            .ip_sets
            .values()
            .filter(|ip_set| ip_set.is_deleting())
            .count()
            .saturating_sub(DELETED_IP_SETS_RETAINED);
        if surplus == 0 {
            return;
        }
        // The oldest deletions first: `updated_at_millis` on a deleted record is the
        // instant its delete was accepted, and `tick_millis` makes those instants
        // strictly increasing, so the ordering is total and the id only breaks a tie
        // that cannot arise.
        let mut deleted: Vec<(i64, String)> = self
            .ip_sets
            .values()
            .filter(|ip_set| ip_set.is_deleting() && ip_set.id != keep)
            .map(|ip_set| (ip_set.updated_at_millis, ip_set.id.clone()))
            .collect();
        deleted.sort_unstable();
        for (_, id) in deleted.into_iter().take(surplus) {
            self.ip_sets.remove(&id);
        }
    }
}

// ---------------------------------------------------------------------------
// IPSets (trusted IP lists)
// ---------------------------------------------------------------------------

/// `IpSetStatus`: the list is being activated, so GuardDuty is not using it yet.
/// One of `INACTIVE | ACTIVATING | ACTIVE | DEACTIVATING | ERROR | DELETE_PENDING
/// | DELETED` (`smithy_query(shape="IpSetStatus")`).
pub const IP_SET_ACTIVATING: &str = "ACTIVATING";

/// `IpSetStatus`: the list is uploaded and in use. See [`IP_SET_ACTIVATING`] for
/// the enum.
pub const IP_SET_ACTIVE: &str = "ACTIVE";

/// `IpSetStatus`: the list is being taken out of use.
pub const IP_SET_DEACTIVATING: &str = "DEACTIVATING";

/// `IpSetStatus`: the list is stored but GuardDuty is not using it.
pub const IP_SET_INACTIVE: &str = "INACTIVE";

/// `IpSetStatus`: a delete has been accepted and the list is on its way out of
/// use. See [`IP_SET_ACTIVATING`] for the enum.
pub const IP_SET_DELETE_PENDING: &str = "DELETE_PENDING";

/// `IpSetStatus`: the list has been deleted. Terminal — nothing moves a deleted
/// IPSet to any other status.
pub const IP_SET_DELETED: &str = "DELETED";

/// How long an IPSet stays in a transitional [`IP_SET_ACTIVATING`] /
/// [`IP_SET_DEACTIVATING`] / [`IP_SET_DELETE_PENDING`] status before the CLOCK
/// owes it the settled status.
///
/// GuardDuty documents no duration for activating or deleting a trusted IP list,
/// so this emulator CHOOSES one. It is deliberately non-zero: a caller that reads
/// the IPSet back immediately after creating or deleting it still observes the
/// transient status.
pub const IP_SET_TRANSITION_MILLIS: i64 = 500;

/// How many DELETED IPSets one detector retains so a read can still report
/// [`IP_SET_DELETED`].
///
/// A deleted record is kept because `GetIPSet`'s `Status` is `@required` and the
/// enum carries `DELETE_PENDING` and `DELETED`
/// (`smithy_query(operation="GetIPSet")`), so the status after a delete is
/// observable rather than a 400. The retention is bounded for the same reason
/// [`IP_SETS_PER_DETECTOR`] bounds the live ones: a caller creating and deleting
/// in a loop must not be able to make the service retain records without limit.
pub const DELETED_IP_SETS_RETAINED: usize = IP_SETS_PER_DETECTOR;

/// How many LIVE IPSets one detector may hold ([`Detector::live_ip_set_count`]).
///
/// `ListIPSets` reports a detector's IPSets in a single page: `IpSetIds` is
/// `len 0..50` and `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListIPSets")`), so a detector's whole collection
/// fits in one page of at most 50 ids. The same number BOUNDS the store: an IPSet
/// is retained until its detector is deleted and has no expiry of its own, so
/// without this maximum a caller could make the service retain records without
/// limit.
pub const IP_SETS_PER_DETECTOR: usize = 50;

/// The message a create past [`IP_SETS_PER_DETECTOR`] is refused with. Like every
/// other rejection here it names the resource class and the limit, never the
/// caller's values.
pub fn ip_set_limit_message() -> String {
    format!(
        "The request is rejected because the detector has reached the limit of {IP_SETS_PER_DETECTOR} IPSets."
    )
}

/// The wire name of the `IpSetId` member, as the request carries it
/// (`smithy_query(operation="GetIPSet")` binds it as the URI label `IpSetId`,
/// rendered `ipSetId` in the JSON body of the sibling operations). It is what a
/// rejection names.
pub const IP_SET_ID_MEMBER: &str = "ipSetId";

/// Classify an `IpSetId` the detector does not hold: a value that is not an id
/// this service mints at all is a parameter failure, a well-formed one the
/// detector does not hold is an ownership failure. Same split as
/// [`detector_id_rejection`], for the same reason — the class comes from the
/// REQUEST, not from the store.
pub fn ip_set_id_rejection(ip_set_id: &str) -> String {
    if is_minted_id(ip_set_id) {
        not_owned_message(IP_SET_ID_MEMBER)
    } else {
        invalid_parameter_message(IP_SET_ID_MEMBER)
    }
}

/// One stored IPSet (trusted IP list). Timestamps are epoch milliseconds, the
/// single source for the instants anything reports.
#[derive(Debug, Clone)]
pub struct IpSetRecord {
    /// The minted `IpSetId`.
    pub id: String,
    /// `Name`: "The user-friendly name for the IPSet"
    /// (`smithy_query(operation="GetIPSet")`), stored verbatim.
    pub name: String,
    /// `IpSetFormat`: the format of the file that contains the IPSet.
    pub format: String,
    /// `Location`: the URI of the file that contains the IPSet.
    pub location: String,
    /// `ExpectedBucketOwner`, which a read reports "only if it was provided
    /// during IPSet creation or update" (`smithy_query(operation="GetIPSet")`).
    pub expected_bucket_owner: Option<String>,
    /// The tags of the IPSet resource.
    pub tags: BTreeMap<String, String>,
    /// `IpSetStatus`, the status of the uploaded file.
    pub status: String,
    /// When the clock owes [`Self::status`] its settled value, while the current
    /// one is transitional. Absent once nothing is owed.
    pub status_due_millis: Option<i64>,
    /// The `ClientToken` the create carried, so a replay returns this same IPSet
    /// instead of minting a second one.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl IpSetRecord {
    /// An IPSet created at `now`.
    ///
    /// `Activate` is "a Boolean value that indicates whether GuardDuty is to start
    /// using the uploaded IPSet" (`smithy_query(operation="CreateIPSet")`): a list
    /// the caller asked to be used starts in the transitional
    /// [`IP_SET_ACTIVATING`] and the clock owes it [`IP_SET_ACTIVE`], while one the
    /// caller did not ask to be used is stored unused — [`IP_SET_INACTIVE`], which
    /// is settled, so nothing is owed on it.
    #[allow(clippy::too_many_arguments)]
    pub fn created(
        id: String,
        name: String,
        format: String,
        location: String,
        expected_bucket_owner: Option<String>,
        tags: BTreeMap<String, String>,
        activate: bool,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        let mut record = Self {
            id,
            name,
            format,
            location,
            expected_bucket_owner,
            tags,
            status: IP_SET_INACTIVE.to_string(),
            status_due_millis: None,
            client_token,
            created_at_millis: now,
            updated_at_millis: now,
        };
        record.set_activation(activate, now);
        record
    }

    /// Has a delete already been accepted for this IPSet — is it
    /// [`IP_SET_DELETE_PENDING`] or [`IP_SET_DELETED`]?
    ///
    /// Such a record is no longer a live trusted IP list: it is still REPORTED, so
    /// a read can answer with the status the delete put it in, but it occupies none
    /// of the detector's [`IP_SETS_PER_DETECTOR`] slots and no update may move it.
    pub fn is_deleting(&self) -> bool {
        self.status == IP_SET_DELETE_PENDING || self.status == IP_SET_DELETED
    }

    /// Accept this IPSet's deletion: [`IP_SET_DELETE_PENDING`] now, with the clock
    /// owing [`IP_SET_DELETED`] at [`IP_SET_TRANSITION_MILLIS`]. Reports whether
    /// this call is the one that accepted it.
    ///
    /// A record already being deleted is left exactly where it is — neither the
    /// status nor the instant the clock owes it moves — so a repeated delete
    /// neither restarts the transition nor walks a `DELETED` record backwards, and
    /// it answers `false` so a caller writes nothing else either.
    pub fn begin_delete(&mut self, now: i64) -> bool {
        if self.is_deleting() {
            return false;
        }
        self.status = IP_SET_DELETE_PENDING.to_string();
        self.status_due_millis = Some(now.saturating_add(IP_SET_TRANSITION_MILLIS));
        self.updated_at_millis = now;
        true
    }

    /// Start the transition the caller's `Activate` asks for, from whatever status
    /// this IPSet currently has.
    ///
    /// Asking for the status it already has owes nothing, so a repeated request
    /// leaves the settled status alone instead of re-opening a transition.
    pub fn set_activation(&mut self, activate: bool, now: i64) {
        let (settled, transitional) = if activate {
            (IP_SET_ACTIVE, IP_SET_ACTIVATING)
        } else {
            (IP_SET_INACTIVE, IP_SET_DEACTIVATING)
        };
        if self.status == settled || self.status == transitional {
            return;
        }
        self.status = transitional.to_string();
        self.status_due_millis = Some(now.saturating_add(IP_SET_TRANSITION_MILLIS));
        self.updated_at_millis = now;
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of a transitional status was owed at
    /// [`Self::status_due_millis`] whether or not anybody called, which is why a
    /// reporting operation may apply it. It is applied exactly once: a settled
    /// status never moves on its own again.
    pub fn settle(&mut self, now: i64) {
        let Some(due) = self.status_due_millis else {
            return;
        };
        if now < due {
            return;
        }
        self.status = match self.status.as_str() {
            IP_SET_ACTIVATING => IP_SET_ACTIVE.to_string(),
            IP_SET_DEACTIVATING => IP_SET_INACTIVE.to_string(),
            IP_SET_DELETE_PENDING => IP_SET_DELETED.to_string(),
            // Nothing else is transitional, so nothing else is owed a move.
            other => other.to_string(),
        };
        self.status_due_millis = None;
    }
}

// ---------------------------------------------------------------------------
// ListMembers
// ---------------------------------------------------------------------------

/// The wire name of `ListMembers`'s `OnlyAssociated` query parameter
/// (`smithy_query(operation="ListMembers")`), for the same reason as
/// [`DETECTOR_ID_MEMBER`]: it is what a rejection names.
pub const ONLY_ASSOCIATED_MEMBER: &str = "onlyAssociated";

/// How GuardDuty refuses an `AccountDetail.Email` that is not formatted the way
/// [`is_member_email`]'s documented rules require: the service's own wording for
/// that rule, which names the member it refuses rather than echoing the value.
pub const MEMBER_EMAIL_FORMAT_MESSAGE: &str =
    "The request failed because provided `emailId` was formatted incorrectly.";

// ---------------------------------------------------------------------------
// Investigations
// ---------------------------------------------------------------------------

/// `InvestigationStatus`, the status a created investigation starts in: the
/// analysis is under way. One of `RUNNING | COMPLETED | FAILED`
/// (`smithy_query(shape="InvestigationStatus")`).
pub const INVESTIGATION_RUNNING: &str = "RUNNING";

/// The `InvestigationStatus` an investigation reaches when its analysis could not
/// be performed. See [`INVESTIGATION_RUNNING`] for the enum.
pub const INVESTIGATION_FAILED: &str = "FAILED";

/// How long a created investigation stays [`INVESTIGATION_RUNNING`] before the
/// clock owes it a terminal status.
///
/// GuardDuty documents no duration for an investigation, so this emulator CHOOSES
/// one. It is deliberately non-zero: a caller that reads the investigation back
/// immediately after creating it still observes the transient `RUNNING` status.
pub const INVESTIGATION_TRANSITION_MILLIS: i64 = 500;

/// The window the per-day investigation limit is counted over: "you can initiate
/// up to 10 investigations per account per day"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATION_DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// How many investigations an account may initiate per day: "During the preview,
/// you can initiate up to 10 investigations per account per day"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATIONS_PER_DAY: usize = 10;

/// How many investigations an account may initiate in total: "with a total limit
/// of 100 investigations per account"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATIONS_PER_ACCOUNT: usize = 100;

/// The Regions GuardDuty Investigation is available in, as `CreateInvestigation`
/// and `GetInvestigation` both list them: "US East (N. Virginia), US East (Ohio),
/// US West (Oregon), Canada (Central), Europe (Frankfurt), Europe (Ireland),
/// Europe (London), Europe (Paris), Europe (Stockholm), and Asia Pacific (Tokyo)"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`), in that order.
pub const INVESTIGATION_REGIONS: [&str; 10] = [
    "us-east-1",
    "us-east-2",
    "us-west-2",
    "ca-central-1",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-north-1",
    "ap-northeast-1",
];

/// Is GuardDuty Investigation available through `region`'s endpoint? Decided from
/// the documented [`INVESTIGATION_REGIONS`] catalogue, not from what the store
/// happens to hold.
pub fn investigations_available_in(region: &str) -> bool {
    INVESTIGATION_REGIONS.contains(&region)
}

/// The message a request to a Region the feature is not available in is refused
/// with. It does not name the caller's Region: the endpoint already carries it.
pub const INVESTIGATION_REGION_MESSAGE: &str =
    "The request is rejected because GuardDuty Investigation is not available in the current Region.";

/// `CloudProvider`, the only value the enum carries: "The cloud provider.
/// Currently, only AWS is supported" (`smithy_query(shape="CloudDetails")`).
pub const CLOUD_PROVIDER: &str = "AWS";

/// The message `GetInvestigation` answers an `InvestigationId` this detector does
/// not hold with. GuardDuty does not echo the caller's id back, so this names the
/// resource class only.
pub const INVESTIGATION_NOT_FOUND_MESSAGE: &str =
    "The request is rejected because the requested investigation could not be found.";

/// The message `CreateInvestigation` is denied with when the detector does not
/// have the AI_ANALYST feature enabled — the feature the operation requires
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATION_FEATURE_DENIED_MESSAGE: &str =
    "The request is rejected because the AI_ANALYST feature is not enabled for the detector.";

/// The message a request past the documented per-day investigation limit is
/// refused with.
pub fn investigation_daily_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {INVESTIGATIONS_PER_DAY} investigations per day."
    )
}

/// The message a request past the documented total investigation limit is refused
/// with.
pub fn investigation_total_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {INVESTIGATIONS_PER_ACCOUNT} investigations."
    )
}

/// Why an investigation this emulator accepted ends [`INVESTIGATION_FAILED`].
///
/// A GuardDuty investigation is produced by Amazon Bedrock models over the
/// account's findings (`smithy_query(query="doc",shape="CreateInvestigation")`).
/// This emulator has no such analysis engine, so it does NOT synthesise a summary,
/// a risk level or a confidence: the run it cannot perform ends in the modelled
/// failure state, carrying the reason it could not run.
pub const INVESTIGATION_ENGINE_UNAVAILABLE: &str = "The investigation analysis engine is not available in this emulator, so no investigation summary was produced.";

/// One stored investigation. Timestamps are epoch milliseconds, the single source
/// for the instants the wire carries.
#[derive(Debug, Clone)]
pub struct InvestigationRecord {
    pub id: String,
    /// `TriggerPrompt`: what the caller asked to be investigated, stored verbatim.
    pub trigger_prompt: String,
    /// `TriggeredBy`: "The account that initiated the investigation"
    /// (`smithy_query(shape="Investigation")`).
    pub triggered_by: String,
    /// The Region the investigation was initiated through.
    pub region: String,
    /// The `ClientToken` the create carried, so a replay returns this same
    /// investigation instead of initiating a second one.
    pub client_token: Option<String>,
    /// `InvestigationStatus`.
    pub status: String,
    pub started_at_millis: i64,
    /// When the clock owes this investigation its terminal status. See
    /// [`INVESTIGATION_TRANSITION_MILLIS`].
    pub terminal_due_millis: i64,
    /// When it reached a terminal status, once it has.
    pub ended_at_millis: Option<i64>,
    /// `Error`: "Details about the error if the investigation status is FAILED"
    /// (`smithy_query(shape="Investigation")`).
    pub error: Option<String>,
}

impl InvestigationRecord {
    /// An investigation initiated at `now`, still [`INVESTIGATION_RUNNING`].
    pub fn started(
        id: String,
        trigger_prompt: String,
        triggered_by: String,
        region: String,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        Self {
            id,
            trigger_prompt,
            triggered_by,
            region,
            client_token,
            status: INVESTIGATION_RUNNING.to_string(),
            started_at_millis: now,
            terminal_due_millis: now.saturating_add(INVESTIGATION_TRANSITION_MILLIS),
            ended_at_millis: None,
            error: None,
        }
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of `RUNNING` was owed at [`Self::terminal_due_millis`] whether
    /// or not anybody called, which is why a reporting operation may apply it. It
    /// is applied exactly once: a terminal investigation never moves again.
    pub fn settle(&mut self, now: i64) {
        if self.status != INVESTIGATION_RUNNING || now < self.terminal_due_millis {
            return;
        }
        self.status = INVESTIGATION_FAILED.to_string();
        self.ended_at_millis = Some(self.terminal_due_millis);
        self.error = Some(INVESTIGATION_ENGINE_UNAVAILABLE.to_string());
    }
}

// ---------------------------------------------------------------------------
// Entity sets
//
// A trusted entity set is an uploaded list of IP addresses and domains, held in
// S3 and taken into use by GuardDuty. The record below carries what the create
// stored plus the status the set is in; the status moves only when the CLOCK
// owes it, through [`EntitySetRecord::settle`].
// ---------------------------------------------------------------------------

/// `TrustedEntitySetStatus`: the status a set the caller did NOT ask to activate
/// stays in. The enum is `INACTIVE | ACTIVATING | ACTIVE | DEACTIVATING | ERROR
/// | DELETE_PENDING | DELETED` (`smithy_query(shape="TrustedEntitySetStatus")`).
pub const ENTITY_SET_INACTIVE: &str = "INACTIVE";

/// The `TrustedEntitySetStatus` a set the caller asked to activate starts in: the
/// uploaded list is being taken into use. See [`ENTITY_SET_INACTIVE`] for the
/// enum.
pub const ENTITY_SET_ACTIVATING: &str = "ACTIVATING";

/// The `TrustedEntitySetStatus` an activating set reaches once the clock owes it:
/// GuardDuty is using the list. See [`ENTITY_SET_INACTIVE`] for the enum.
pub const ENTITY_SET_ACTIVE: &str = "ACTIVE";

/// How long a set the caller asked to activate stays [`ENTITY_SET_ACTIVATING`]
/// before the clock owes it [`ENTITY_SET_ACTIVE`].
///
/// AWS documents no duration for taking an uploaded list into use, so this span
/// is CHOSEN — the same one [`INVESTIGATION_TRANSITION_MILLIS`] uses, and for the
/// same reason: non-zero, so a caller reading straight after the create still
/// observes the transient status, and short enough that nothing has to wait long
/// for the settled one.
pub const ENTITY_SET_TRANSITION_MILLIS: i64 = 500;

/// The wire name of the `TrustedEntitySetId` member, as the request carries it in
/// its URI label and the create reports it
/// (`smithy_query(operation="GetTrustedEntitySet")` renders the `jsonName`
/// `trustedEntitySetId`). It is what a rejection names.
pub const TRUSTED_ENTITY_SET_ID_MEMBER: &str = "trustedEntitySetId";

/// The wire name of the `ThreatEntitySetId` member, as the request carries it in
/// its URI label and the create reports it
/// (`smithy_query(operation="GetThreatEntitySet")` renders the `jsonName`
/// `threatEntitySetId`). It is what a rejection names.
pub const THREAT_ENTITY_SET_ID_MEMBER: &str = "threatEntitySetId";

/// The wire name of the `Name` member an entity-set create carries
/// (`jsonName` `name`), for the same reason.
pub const ENTITY_SET_NAME_MEMBER: &str = "name";

/// The wire name of the `Location` member an entity-set create carries
/// (`jsonName` `location`), for the same reason.
pub const ENTITY_SET_LOCATION_MEMBER: &str = "location";

/// The message a second `CreateTrustedEntitySet` is refused with. "At any given
/// time, you can have only one trusted entity set"
/// (`smithy_query(query="doc",shape="CreateTrustedEntitySet")`), and the set hangs
/// off the detector, which is the account/Region-level resource.
pub const TRUSTED_ENTITY_SET_ALREADY_EXISTS_MESSAGE: &str =
    "The request is rejected because a trusted entity set already exists for the input detectorId.";

/// How many threat entity sets one detector may hold.
///
/// Unlike a trusted entity set, of which "at any given time, you can have only
/// one" (`smithy_query(query="doc",shape="CreateTrustedEntitySet")`), AWS
/// documents no such rule for a threat entity set
/// (`smithy_query(query="doc",shape="CreateThreatEntitySet")`), so a detector may
/// hold several.
///
/// This ceiling is therefore EMULATOR-CHOSEN, not a published threat-entity-set
/// quota: it reuses the number AWS publishes for the account's threat lists in a
/// Region ([`THREAT_INTEL_SETS_PER_DETECTOR`]) because an entity set is the same
/// kind of uploaded list under a newer API, and a detector is the account's
/// single resource in a Region. Read it as "the emulator holds this many", never
/// as a documented AWS limit.
///
/// A ceiling of some kind is what BOUNDS this store: without it a caller varying
/// or omitting `ClientToken` would grow [`Detector::threat_entity_sets`] by one
/// entry per request, and the `@idempotencyToken` scan over it
/// ([`Detector::threat_entity_set_for_token`]) would grow with it.
pub const THREAT_ENTITY_SETS_PER_DETECTOR: usize = THREAT_INTEL_SETS_PER_DETECTOR;

/// The message a `CreateThreatEntitySet` past
/// [`THREAT_ENTITY_SETS_PER_DETECTOR`] is refused with. It names the limit, never
/// the caller's values.
pub fn threat_entity_set_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {THREAT_ENTITY_SETS_PER_DETECTOR} threat entity sets."
    )
}

/// Does `name` hold only the characters an entity-set `Name` may contain?
///
/// "The name of your list can include lowercase letters, uppercase letters,
/// numbers, dash (-), and underscore (_)"
/// (`smithy_query(query="doc",shape="CreateTrustedEntitySetRequest",member="Name")`)
/// — a documented constraint the model's `len 1..300` does not express, so it is
/// enforced here.
pub fn is_entity_set_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Is `location` a valid Amazon S3 URL for an uploaded entity-set list?
///
/// "The format of the Location URL must be a valid Amazon S3 URL format. Invalid
/// URL formats will result in an error, regardless of whether you activate the
/// entity set or not"
/// (`smithy_query(query="doc",shape="CreateTrustedEntitySetRequest",member="Location")`).
/// The accepted forms are the S3 URL forms: the `s3://bucket/key` scheme, the
/// path-style HTTPS endpoint (`https://s3[.-]<region>.amazonaws.com/bucket/key`)
/// and the virtual-hosted-style one
/// (`https://bucket.s3[.-]<region>.amazonaws.com/key`). Each needs BOTH a bucket
/// and a non-empty object key: a URL naming only a bucket does not name a list.
pub fn is_s3_location(location: &str) -> bool {
    let Ok(url) = ::url::Url::parse(location) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let key = url.path().trim_start_matches('/');
    match url.scheme() {
        // `s3://bucket/key`: the authority IS the bucket and the path the key.
        "s3" => !host.is_empty() && !key.is_empty(),
        "https" => match s3_endpoint_prefix(host) {
            // Path-style: the first path segment is the bucket, the rest the key.
            Some("") => key.split_once('/').is_some_and(|(bucket, object_key)| {
                !bucket.is_empty() && !object_key.is_empty()
            }),
            // Virtual-hosted style: the host carries the bucket, the path the key.
            Some(bucket) => !bucket.is_empty() && !key.is_empty(),
            None => false,
        },
        _ => false,
    }
}

/// What precedes the `s3` label in an S3 endpoint `host`, if `host` is one: the
/// empty string for a path-style endpoint, and the bucket for a virtual-hosted
/// one. `None` when the host is not an S3 endpoint at all.
///
/// Both the dotted (`s3.us-east-1.amazonaws.com`) and the dashed
/// (`s3-us-east-1.amazonaws.com`) regional spellings are S3 endpoints, as is the
/// Region-less `s3.amazonaws.com`.
fn s3_endpoint_prefix(host: &str) -> Option<&str> {
    let domain = host.strip_suffix(".amazonaws.com")?;
    for (index, _) in domain.match_indices("s3") {
        let (prefix, rest) = domain.split_at(index);
        // The `s3` label must start a label of its own, and must be the whole
        // label or be followed by a `-<region>` suffix within it.
        if !(prefix.is_empty() || prefix.ends_with('.')) {
            continue;
        }
        let rest = &rest["s3".len()..];
        if rest.is_empty() || rest.starts_with('.') || rest.starts_with('-') {
            return Some(prefix.trim_end_matches('.'));
        }
    }
    None
}

/// One stored entity set. Timestamps are epoch milliseconds, the single source for
/// the instants the wire carries.
#[derive(Debug, Clone)]
pub struct EntitySetRecord {
    pub id: String,
    pub name: String,
    /// `TrustedEntitySetFormat`, stored as the create gave it.
    pub format: String,
    /// The S3 URL the list was uploaded to, stored verbatim.
    pub location: String,
    pub expected_bucket_owner: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the create carried, so a replay returns this same set
    /// instead of minting a second one.
    pub client_token: Option<String>,
    /// `TrustedEntitySetStatus`.
    pub status: String,
    /// When the clock owes this set its next status, while one is owed. See
    /// [`ENTITY_SET_TRANSITION_MILLIS`].
    pub status_due_millis: Option<i64>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// `ErrorDetails`: only an `ERROR` set carries one.
    pub error_details: Option<String>,
}

impl EntitySetRecord {
    /// An entity set created at `now`: the record BOTH entity-set creates store,
    /// so the trusted and the threat set cannot drift on what a create persists.
    /// It is the entity-set counterpart of [`IpSetRecord::created`].
    ///
    /// `Activate` is "a boolean value that indicates whether GuardDuty should
    /// start using the uploaded threat entity set to generate findings"
    /// (`smithy_query(query="doc",shape="CreateThreatEntitySetRequest",member="Activate")`):
    /// a set the caller asked to activate is not in use yet, so it starts in the
    /// transitional [`ENTITY_SET_ACTIVATING`] and the clock owes it
    /// [`ENTITY_SET_ACTIVE`] at [`ENTITY_SET_TRANSITION_MILLIS`], while one the
    /// caller did not ask to activate is stored unused — [`ENTITY_SET_INACTIVE`],
    /// which is settled, so nothing is owed on it until an update activates it.
    ///
    /// Nothing failed, so a created set carries no [`Self::error_details`].
    #[allow(clippy::too_many_arguments)]
    pub fn created(
        id: String,
        name: String,
        format: String,
        location: String,
        expected_bucket_owner: Option<String>,
        tags: BTreeMap<String, String>,
        activate: bool,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        let (status, status_due_millis) = if activate {
            (
                ENTITY_SET_ACTIVATING,
                Some(now.saturating_add(ENTITY_SET_TRANSITION_MILLIS)),
            )
        } else {
            (ENTITY_SET_INACTIVE, None)
        };
        Self {
            id,
            name,
            format,
            location,
            expected_bucket_owner,
            tags,
            client_token,
            status: status.to_string(),
            status_due_millis,
            created_at_millis: now,
            updated_at_millis: now,
            error_details: None,
        }
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of [`ENTITY_SET_ACTIVATING`] was owed at
    /// [`Self::status_due_millis`] whether or not anybody called, which is why a
    /// reporting operation may apply it. `UpdatedAt` records the instant it was
    /// OWED, not the instant somebody looked, so two reads of a settled set agree.
    pub fn settle(&mut self, now: i64) {
        let Some(due) = self.status_due_millis else {
            return;
        };
        if now < due {
            return;
        }
        // Only a transitional status is owed a move, and each one settles to its
        // own status, so a status this function does not know stays where it is.
        self.status = match self.status.as_str() {
            ENTITY_SET_ACTIVATING => ENTITY_SET_ACTIVE.to_string(),
            other => other.to_string(),
        };
        self.status_due_millis = None;
        self.updated_at_millis = due;
    }
}

/// A saved finding filter. `CreateFilter` writes it, `GetFilter` reports it,
/// `ListFilters` names it and `DeleteDetector`'s cascade drops it with its
/// detector; `UpdateFilter` / `DeleteFilter` are still scaffolded.
#[derive(Debug, Clone, Default)]
pub struct FilterRecord {
    pub name: String,
    pub description: Option<String>,
    /// `FilterAction`: `ARCHIVE` or [`FILTER_ACTION_NOOP`].
    pub action: Option<String>,
    pub rank: Option<i32>,
    /// The criteria the filter selects findings by, kept as the modelled shape so
    /// a read reports exactly what the create stored.
    pub finding_criteria: crate::ops::types::FindingCriteria,
    pub tags: BTreeMap<String, String>,
    /// The `@idempotencyToken` the create carried, so a replayed `CreateFilter`
    /// returns the filter the first call saved instead of refusing the name.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// `FilterVersion`: 1 for a filter nothing has updated.
    pub version: i64,
}

/// One ThreatIntelSet: a list of "known malicious IP addresses" GuardDuty generates
/// findings from (`smithy_query(query="doc",shape="CreateThreatIntelSet")`).
///
/// The list is owned by the detector it was created under, so `DeleteDetector`
/// drops it with the detector.
#[derive(Debug, Clone)]
pub struct ThreatIntelSetRecord {
    /// The minted `ThreatIntelSetId`, the id a read resolves it by.
    pub id: String,
    /// "A user-friendly ThreatIntelSet name"
    /// (`smithy_query(operation="CreateThreatIntelSet")`).
    pub name: String,
    /// `ThreatIntelSetFormat`: the format of the file that contains the list.
    pub format: String,
    /// "The URI of the file that contains the ThreatIntelSet", stored verbatim: it
    /// is caller text, so a read hands back exactly what the create wrote.
    pub location: String,
    /// The account that owns the bucket the location names, when the create said.
    pub expected_bucket_owner: Option<String>,
    /// "The tags to be added to a new threat list resource".
    pub tags: BTreeMap<String, String>,
    /// The `@idempotencyToken` the create carried, so a replayed create returns the
    /// list the first call minted instead of minting a second one.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    /// `ThreatIntelSetStatus`, as a read reports it.
    pub status: String,
    /// The status the CLOCK owes this list and the instant it comes due, or `None`
    /// when nothing is outstanding. See [`Self::settle`].
    pub pending_status: Option<(String, i64)>,
}

impl ThreatIntelSetRecord {
    /// Materialise the status change the clock has ALREADY made due.
    ///
    /// The change is owed at its due instant whether or not anybody called, so a
    /// reporting operation may apply it: it shows no caller anything it would not
    /// have seen anyway, and it never moves a status the clock does not owe.
    pub fn settle(&mut self, now_millis: i64) {
        let Some((next, due)) = self.pending_status.as_ref() else {
            return;
        };
        if now_millis >= *due {
            self.status = next.clone();
            self.pending_status = None;
        }
    }
}

/// How many ThreatIntelSets one detector may hold.
///
/// AWS publishes a quota of 6 threat lists per account per Region for GuardDuty,
/// and a detector is the account's single resource in a Region ("you can have only
/// one detector per account per Region" —
/// `smithy_query(query="doc",shape="CreateDetector")`), so the account's quota is
/// the detector's.
///
/// The quota is also what BOUNDS this store: without it a caller varying or
/// omitting `ClientToken` would grow [`Detector::threat_intel_sets`] by one entry
/// per request, and the `@idempotencyToken` scan over it
/// ([`Detector::threat_intel_set_for_token`]) would grow with it.
pub const THREAT_INTEL_SETS_PER_DETECTOR: usize = 6;

/// The message a `CreateThreatIntelSet` past [`THREAT_INTEL_SETS_PER_DETECTOR`] is
/// refused with. It names the limit, never the caller's values.
pub fn threat_intel_set_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {THREAT_INTEL_SETS_PER_DETECTOR} threat lists."
    )
}

/// `ThreatIntelSetStatus`: the status a list that is not in use reports
/// (`smithy_query(shape="ThreatIntelSetStatus")`).
pub const THREAT_INTEL_SET_INACTIVE: &str = "INACTIVE";
/// The TRANSIENT status a list the create asked to activate reports until the
/// activation that create started comes due. See [`ThreatIntelSetRecord::settle`].
pub const THREAT_INTEL_SET_ACTIVATING: &str = "ACTIVATING";
/// The status an activated list reports once its activation is due.
pub const THREAT_INTEL_SET_ACTIVE: &str = "ACTIVE";

/// How long a list the create asked to activate spends in
/// [`THREAT_INTEL_SET_ACTIVATING`] before the clock owes it
/// [`THREAT_INTEL_SET_ACTIVE`].
///
/// AWS documents no duration for ingesting and activating a threat list, so this
/// span is CHOSEN: non-zero, so a caller reading straight after the create still
/// observes the transient status the real service reports, and short enough that
/// the activation lands without anybody sleeping on it.
pub const THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS: i64 = 500;

/// A Malware Protection plan: the resource `CreateMalwareProtectionPlan` mints for
/// one protected S3 bucket.
///
/// Everything a read reports comes from here, except the ARN, which
/// [`malware_protection_plan_arn`] derives from [`Self::region`] and
/// [`Self::id`].
#[derive(Debug, Clone)]
pub struct MalwareProtectionPlan {
    pub id: String,
    /// The Region whose endpoint created this plan, and the only one that serves
    /// it.
    pub region: String,
    /// The IAM role ARN the plan scans and tags the bucket through.
    pub role: String,
    /// The protected resource: the name of the S3 bucket. "Presently, S3Bucket is
    /// the only supported protected resource"
    /// (`smithy_query(shape="CreateProtectedResource")`), so the plan needs no
    /// resource-type discriminator.
    pub bucket_name: String,
    /// The object prefixes the scan is limited to: "The S3 object will be scanned
    /// only if it belongs to any of the specified object prefixes"
    /// (`smithy_query(shape="CreateS3BucketResource")`). Empty means the whole
    /// bucket — which is what an absent prefix list and an explicitly empty one
    /// both ask for, so the two are stored, and reported, identically.
    pub object_prefixes: Vec<String>,
    /// `MalwareProtectionPlanTaggingActionStatus`: whether a scanned object gets
    /// tagged.
    pub tagging_status: String,
    pub tags: BTreeMap<String, String>,
    pub created_at_millis: i64,
    /// `MalwareProtectionPlanStatus`.
    pub status: String,
}

// ---------------------------------------------------------------------------
// Malware scans
//
// `StartMalwareScan` starts a scan of the resource its `ResourceArn` names and
// hands back the `ScanId` the caller tracks it by: "When the malware scan
// starts, you can use the associated scan ID to track the status of the scan"
// (`smithy_query(query="doc",shape="StartMalwareScan")`). `GetMalwareScan`
// reports that scan. The CONTROL plane is real here — the scan resource, its id,
// the resource and configuration the start named, its status and its instants —
// while the scan ITSELF is work this emulator cannot perform: there is no
// malware scanner and no EBS volume content to read. So no result is
// synthesised; the run ends in the modelled failure state carrying the reason it
// could not run, exactly as an investigation does
// ([`INVESTIGATION_ENGINE_UNAVAILABLE`]).
// ---------------------------------------------------------------------------

/// `MalwareProtectionScanStatus`: the NON-terminal status a scan the start call
/// just accepted reports, until the clock owes it its terminal status. The enum is
/// `RUNNING | COMPLETED | COMPLETED_WITH_ISSUES | FAILED | SKIPPED`
/// (`smithy_query(shape="MalwareProtectionScanStatus")`).
pub const MALWARE_SCAN_RUNNING: &str = "RUNNING";

/// The terminal status a scan this emulator accepted ends in.
///
/// GuardDuty Malware Protection scans the EBS volumes behind the resource the
/// start named. This emulator has no scanner and no volume content, so it does NOT
/// synthesise a scan result, a threat list or a file count: the run it cannot
/// perform ends FAILED, the modelled failure member of
/// `MalwareProtectionScanStatus`.
pub const MALWARE_SCAN_FAILED: &str = "FAILED";

/// Why a scan this emulator accepted ends [`MALWARE_SCAN_FAILED`].
///
/// `ScanStatusReason` is a CLOSED enum (`smithy_query(shape="ScanStatusReason")`),
/// so the reason is one of its members rather than free text, and
/// `RESOURCE_UNAVAILABLE` is the member that says what is true here: the resource
/// the scan would have read was never available to the scanner.
pub const MALWARE_SCAN_UNAVAILABLE_REASON: &str = "RESOURCE_UNAVAILABLE";

/// How long a scan spends in [`MALWARE_SCAN_RUNNING`] before the clock owes it
/// [`MALWARE_SCAN_FAILED`].
///
/// AWS documents no duration for a malware scan, so this span is CHOSEN: non-zero,
/// so a caller reading straight after the start still observes the `RUNNING` status
/// the real service reports, and short enough that the terminal status lands
/// without anybody sleeping on it. Defined AS [`INVESTIGATION_TRANSITION_MILLIS`]:
/// it is the same span for the same reason, so the two cannot drift.
pub const MALWARE_SCAN_TERMINAL_SPAN_MILLIS: i64 = INVESTIGATION_TRANSITION_MILLIS;

/// `MalwareProtectionScanType`: the initiator of a scan a caller asked for.
///
/// Every scan this emulator holds was started through `StartMalwareScan`, which is
/// the caller asking on demand; `GUARDDUTY_INITIATED` and `BACKUP_INITIATED` name
/// the scans GuardDuty and AWS Backup start by themselves, which nothing here does
/// (`smithy_query(shape="MalwareProtectionScanType")`).
pub const MALWARE_SCAN_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// `ScanCategory`: the whole resource is scanned unless the start asked for an
/// incremental scan (`smithy_query(shape="ScanCategory")`).
pub const MALWARE_SCAN_CATEGORY_FULL: &str = "FULL_SCAN";

/// `ScanCategory`: "When specified, the scan will only process changes since the
/// baseline resource"
/// (`smithy_query(query="doc",shape="StartMalwareScanConfiguration",member="IncrementalScanDetails")`),
/// so a start carrying `IncrementalScanDetails` is an incremental scan.
pub const MALWARE_SCAN_CATEGORY_INCREMENTAL: &str = "INCREMENTAL_SCAN";

/// The wire name of the `ScanId` member `GetMalwareScan` carries in its URI
/// (`smithy_query(operation="GetMalwareScan")`), for a rejection to name.
pub const SCAN_ID_MEMBER: &str = "scanId";

/// The wire name of `StartMalwareScan`'s `ResourceArn` member, for the same reason.
pub const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The message a read of a scan this account and Region does not hold answers with.
///
/// Built through the shared [`not_owned_message`], like
/// [`malware_protection_plan_not_found_message`] and for the same reason: "never
/// existed" and "belongs to someone else" are one refusal, so one account cannot
/// probe another's scan ids, and every operation rejects the same input the same
/// way.
pub fn malware_scan_not_found_message() -> String {
    not_owned_message(SCAN_ID_MEMBER)
}

/// The message a start is refused with when the Region's endpoint holds no
/// detector.
///
/// Malware Protection is a GuardDuty detector feature
/// (`EBS_MALWARE_PROTECTION` is a `DetectorFeature` member —
/// `smithy_query(shape="DetectorFeature")`) and a detector is per account per
/// Region, so an endpoint with no detector has no Malware Protection to run a scan
/// with.
pub const MALWARE_SCAN_WITHOUT_DETECTOR_MESSAGE: &str =
    "The request is rejected because GuardDuty is not enabled for this account in this Region.";

/// The most malware scans one Region's endpoint retains.
///
/// A STORE GUARD, not a modelled quota, like [`MAX_CLIENT_TOKENS`]: AWS publishes
/// no ceiling a caller can read, and a scan has no expiry of its own, so without
/// this a caller could make the store retain records without limit. Past it a start
/// is refused rather than an existing scan evicted: a scan a caller holds the id of
/// must stay readable.
pub const MAX_MALWARE_SCANS_PER_REGION: usize = 1_000;

/// How many Regions the scan store partitions into, the same ceiling and for the
/// same reason as [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`]: the Region comes from the
/// request's credential scope, so the partition KEY is request-chosen too.
pub const MAX_MALWARE_SCAN_REGIONS: usize = MAX_MALWARE_PROTECTION_PLAN_REGIONS;

/// The message a start past [`MAX_MALWARE_SCANS_PER_REGION`] is refused with.
pub fn malware_scan_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {MAX_MALWARE_SCANS_PER_REGION} malware scans in this Region."
    )
}

/// The `MalwareProtectionResourceType` the ARN `resource_arn` names, or `None` when
/// it names nothing Malware Protection scans.
///
/// The TYPE is derived from the ARN rather than taken from the request, which
/// carries no type member at all: `StartMalwareScan` documents that the resource an
/// ARN names decides what the call does ("if the resourceArn belongs to an EC2
/// instance" — `smithy_query(query="doc",shape="StartMalwareScan")`). The four
/// resources below are the EC2/EBS members of
/// [`crate::ops::types::shared_types::MALWARE_PROTECTION_RESOURCE_TYPE_VALUES`],
/// matched on the resource-type segment of the standard EC2 ARN forms
/// (`…:instance/i-…`, `…:volume/vol-…`, `…:snapshot/snap-…`, `…:image/ami-…`). The
/// S3 and recovery-point members are not reachable through this operation: an S3
/// bucket ARN carries no account id, which `ResourceArn`'s own pattern
/// `^arn:[A-Za-z-]+:[A-Za-z0-9]+:[A-Za-z0-9-]+:\d+:…` requires
/// (`smithy_query(operation="StartMalwareScan")`), and a recovery point's type is
/// not derivable from its ARN.
pub fn malware_scan_resource_type(resource_arn: &str) -> Option<&'static str> {
    if !is_arn_for("ec2", resource_arn) {
        return None;
    }
    let resource = resource_arn.splitn(6, ':').nth(5)?;
    let (kind, id) = resource.split_once('/')?;
    if id.is_empty() {
        return None;
    }
    match kind {
        "instance" => Some("EC2_INSTANCE"),
        "volume" => Some("EBS_VOLUME"),
        "snapshot" => Some("EBS_SNAPSHOT"),
        "image" => Some("EC2_AMI"),
        _ => None,
    }
}

/// One malware scan: what the start call named, plus the status the scan is in.
///
/// The status moves only when the CLOCK owes it, through [`Self::settle`], so a
/// reporting operation materialises it without deciding it.
#[derive(Debug, Clone)]
pub struct MalwareScan {
    /// The minted `ScanId`.
    pub id: String,
    /// The Region whose endpoint started this scan, and the only one that serves
    /// it.
    pub region: String,
    /// The detector that Region's endpoint held when the scan started, if any:
    /// "The unique ID of the detector that is associated with the request, if it
    /// belongs to an account which is a GuardDuty customer"
    /// (`smithy_query(operation="GetMalwareScan")`).
    pub detector_id: Option<String>,
    /// `ResourceArn`: the resource the scan was invoked on, stored verbatim.
    pub resource_arn: String,
    /// The `MalwareProtectionResourceType` [`malware_scan_resource_type`] derived
    /// from [`Self::resource_arn`].
    pub resource_type: String,
    /// `MalwareProtectionScanType`.
    pub scan_type: String,
    /// `ScanCategory`.
    pub scan_category: String,
    /// `ScanConfiguration.Role`: the IAM role the scan was asked to run as.
    pub role: Option<String>,
    /// `IncrementalScanDetails.BaselineResourceArn`, when the start asked for an
    /// incremental scan.
    pub baseline_resource_arn: Option<String>,
    /// `RecoveryPoint.BackupVaultName`, when the start named a recovery point.
    pub recovery_point_vault: Option<String>,
    /// `ContinuousScanDetails.StartTime`, in epoch milliseconds.
    pub continuous_scan_start_millis: Option<i64>,
    /// `ContinuousScanDetails.EndTime`, in epoch milliseconds. Present exactly when
    /// the start carried `ContinuousScanDetails`, whose `EndTime` is required
    /// (`smithy_query(shape="ContinuousScanDetails")`).
    pub continuous_scan_end_millis: Option<i64>,
    /// The `ClientToken` the start carried, so a replay answers with this same scan
    /// instead of starting a second one.
    pub client_token: Option<String>,
    /// `MalwareProtectionScanStatus`.
    pub status: String,
    /// `ScanStatusReason`, once the scan has one.
    pub status_reason: Option<String>,
    pub started_at_millis: i64,
    /// When the clock owes this scan its terminal status. See
    /// [`MALWARE_SCAN_TERMINAL_SPAN_MILLIS`].
    pub terminal_due_millis: i64,
    /// When it reached a terminal status, once it has.
    pub completed_at_millis: Option<i64>,
}

impl MalwareScan {
    /// A scan started at `now`, still [`MALWARE_SCAN_RUNNING`].
    #[allow(clippy::too_many_arguments)]
    pub fn started(
        id: String,
        region: String,
        detector_id: Option<String>,
        resource_arn: String,
        resource_type: &str,
        scan_category: &str,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        Self {
            id,
            region,
            detector_id,
            resource_arn,
            resource_type: resource_type.to_string(),
            scan_type: MALWARE_SCAN_TYPE_ON_DEMAND.to_string(),
            scan_category: scan_category.to_string(),
            role: None,
            baseline_resource_arn: None,
            recovery_point_vault: None,
            continuous_scan_start_millis: None,
            continuous_scan_end_millis: None,
            client_token,
            status: MALWARE_SCAN_RUNNING.to_string(),
            status_reason: None,
            started_at_millis: now,
            terminal_due_millis: now.saturating_add(MALWARE_SCAN_TERMINAL_SPAN_MILLIS),
            completed_at_millis: None,
        }
    }

    /// Has the scan reached a terminal status?
    pub fn is_terminal(&self) -> bool {
        self.status != MALWARE_SCAN_RUNNING
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of `RUNNING` was owed at [`Self::terminal_due_millis`] whether
    /// or not anybody called, which is why a reporting operation may apply it. It is
    /// applied exactly once: a terminal scan never moves again, and it carries the
    /// due instant rather than the instant somebody happened to read it.
    pub fn settle(&mut self, now: i64) {
        if self.is_terminal() || now < self.terminal_due_millis {
            return;
        }
        self.status = MALWARE_SCAN_FAILED.to_string();
        self.status_reason = Some(MALWARE_SCAN_UNAVAILABLE_REASON.to_string());
        self.completed_at_millis = Some(self.terminal_due_millis);
    }
}

/// One publishing destination: the resource a detector exports its findings to,
/// plus the KMS key those findings are encrypted with.
///
/// The destination properties are stored exactly as the create accepted them —
/// nothing is published, so the ARNs are only ever reported back.
#[derive(Debug, Clone)]
pub struct PublishingDestinationRecord {
    pub id: String,
    /// `DestinationType`: a member of [`DESTINATION_TYPES`].
    pub destination_type: String,
    /// "The ARN of the resource to publish to"
    /// (`smithy_query(shape="DestinationProperties")`).
    pub destination_arn: String,
    /// "The ARN of the KMS key to use for encryption"
    /// (`smithy_query(shape="DestinationProperties")`).
    pub kms_key_arn: String,
    /// The expected owner of the destination bucket. "This field appears in the
    /// response only if it was provided when the publishing destination was
    /// created or updated"
    /// (`smithy_query(query="doc",shape="DestinationProperties",member="ExpectedBucketOwner")`),
    /// so it stays absent until a request supplies it.
    pub expected_bucket_owner: Option<String>,
    /// `PublishingStatus`: a member of [`PUBLISHING_STATUSES`].
    pub status: String,
    /// The instant the verification this destination is waiting on comes due, in
    /// epoch milliseconds; `None` once [`Self::settle`] has materialised it.
    pub verification_due_millis: Option<i64>,
    /// "The time, in epoch millisecond format, at which GuardDuty was first unable
    /// to publish findings to the destination"
    /// (`smithy_query(query="doc",shape="DescribePublishingDestinationResponse",member="PublishingFailureStartTimestamp")`).
    /// Zero while publishing to this destination has never failed, which is every
    /// destination this emulator holds: nothing is published, so nothing fails.
    pub publishing_failure_start_millis: i64,
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the create that minted this destination carried, if any —
    /// the `@idempotencyToken` index, kept on the record so it cannot outlive the
    /// destination it names. Read it through
    /// [`Detector::publishing_destination_for_token`].
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl PublishingDestinationRecord {
    /// Materialise the verification the clock already owes: a destination created
    /// at `t` reports [`PENDING_VERIFICATION`] until `t +
    /// VERIFICATION_DELAY_MILLIS` and [`PUBLISHING`] from then on.
    ///
    /// Only the clock moves the status here, and it moves it at an instant fixed
    /// by the create, so a read that settles it shows exactly what a read at the
    /// same instant would have shown had nobody called in between. A status a
    /// request moved away from `PENDING_VERIFICATION` is left alone.
    pub fn settle(&mut self, now: i64) {
        let Some(due) = self.verification_due_millis else {
            return;
        };
        if now < due {
            return;
        }
        self.verification_due_millis = None;
        if self.status == PENDING_VERIFICATION {
            self.status = PUBLISHING.to_string();
        }
    }
}

/// One stored finding. Timestamps are kept as epoch milliseconds — the single
/// source for both the ISO-8601 strings the wire carries and the numeric
/// comparisons `FindingCriteria` makes.
#[derive(Debug, Clone)]
pub struct FindingRecord {
    pub id: String,
    pub arn: String,
    pub account_id: String,
    pub region: String,
    pub partition: String,
    pub schema_version: String,
    pub r#type: String,
    pub severity: f64,
    pub confidence: Option<f64>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub resource_type: String,
    pub detector_id: String,
    pub resource_role: String,
    /// `Service.Count`: "The total count of the occurrences of this finding type"
    /// (`smithy_query(shape="Service")`).
    pub count: i32,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// `Service.EventLastSeen`: when the ACTIVITY this finding reports was last
    /// observed, which is a different clock from [`Self::updated_at_millis`] — the
    /// instant the finding RECORD last changed. A recurrence of the activity moves
    /// both; a change to the record's own metadata (its archive state, its
    /// feedback) moves at most the record's clock, because nothing was observed.
    pub event_last_seen_millis: i64,
    pub archived: bool,
    /// True for a finding `CreateSampleFindings` generated.
    pub sample: bool,
    /// `Service.UserFeedback` — "Feedback that was submitted about the finding"
    /// (`smithy_query(shape="Service")`): the `Feedback` value of the latest
    /// `UpdateFindingsFeedback` call that named this finding, and absent until one
    /// does.
    pub user_feedback: Option<String>,
    /// The `Comments` submitted beside [`Self::user_feedback`] — "additional feedback
    /// about the GuardDuty findings"
    /// (`smithy_query(operation="UpdateFindingsFeedback")`). The member is
    /// `@sensitive` and no modelled read reports it (`Service` has no comments
    /// member), so it is kept with the feedback it arrived with rather than published.
    pub feedback_comments: Option<String>,
}

/// One finding attribute, resolved for comparison or sorting.
#[derive(Debug, Clone, PartialEq)]
pub enum Attribute {
    Text(String),
    Number(f64),
}

impl Attribute {
    /// The textual form `Equals` / `NotEquals` compare against: GuardDuty's
    /// `findingCriteria` carries every value as a string, including numeric ones.
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Number(n) => format_number(*n),
        }
    }

    /// The numeric form the ordering conditions compare against, when the
    /// attribute has one.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Text(_) => None,
            Self::Number(n) => Some(*n),
        }
    }
}

/// Render a finding's numeric attribute without a trailing `.0`, so `severity`
/// equality against the string `"8"` behaves the way it does on the real service.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The finding attribute paths this emulator can filter and sort on, as the
/// `Finding` shape's `jsonName`s name them
/// (`smithy_query(shape="Finding")`), plus the nested paths GuardDuty's own filter
/// documentation uses.
pub const FINDING_ATTRIBUTES: [&str; 16] = [
    "accountId",
    "arn",
    "confidence",
    "createdAt",
    "description",
    "id",
    "partition",
    "region",
    "resource.resourceType",
    "schemaVersion",
    "service.archived",
    "service.detectorId",
    "severity",
    "title",
    "type",
    "updatedAt",
];

/// Can this emulator evaluate `path` as a finding attribute? Decided from the
/// catalogue rather than from a sample record, so an empty result set refuses an
/// attribute path exactly as a populated one does.
pub fn supports_attribute(path: &str) -> bool {
    FINDING_ATTRIBUTES.contains(&path)
}

/// Resolve one finding attribute by its wire path, or `None` when this emulator
/// does not model that path.
///
/// `updatedAt` is accepted as well as `createdAt`; both are compared as epoch
/// milliseconds by the ordering conditions and as ISO-8601 text by the equality
/// ones, which is how GuardDuty's own filters treat them.
pub fn finding_attribute(finding: &FindingRecord, path: &str) -> Option<Attribute> {
    let attr = match path {
        "accountId" => Attribute::Text(finding.account_id.clone()),
        "arn" => Attribute::Text(finding.arn.clone()),
        "confidence" => Attribute::Number(finding.confidence.unwrap_or(0.0)),
        "createdAt" => Attribute::Number(finding.created_at_millis as f64),
        "updatedAt" => Attribute::Number(finding.updated_at_millis as f64),
        "description" => Attribute::Text(finding.description.clone().unwrap_or_default()),
        "id" => Attribute::Text(finding.id.clone()),
        "partition" => Attribute::Text(finding.partition.clone()),
        "region" => Attribute::Text(finding.region.clone()),
        "resource.resourceType" => Attribute::Text(finding.resource_type.clone()),
        "schemaVersion" => Attribute::Text(finding.schema_version.clone()),
        "service.archived" => Attribute::Text(finding.archived.to_string()),
        "service.detectorId" => Attribute::Text(finding.detector_id.clone()),
        "severity" => Attribute::Number(finding.severity),
        "title" => Attribute::Text(finding.title.clone().unwrap_or_default()),
        "type" => Attribute::Text(finding.r#type.clone()),
        _ => return None,
    };
    Some(attr)
}

/// A finding attribute rendered for EQUALITY comparison. `createdAt` /
/// `updatedAt` compare as their ISO-8601 text, every other attribute as its own
/// textual form.
fn equality_text(finding: &FindingRecord, path: &str) -> Option<String> {
    match path {
        "createdAt" => Some(iso8601(finding.created_at_millis)),
        "updatedAt" => Some(iso8601(finding.updated_at_millis)),
        _ => finding_attribute(finding, path).map(|a| a.as_text()),
    }
}

/// Does `finding` satisfy one `Condition` on `path`?
///
/// `Err(path)` means the emulator does not model that attribute path, which the
/// caller turns into a refusal rather than silently returning every finding.
/// Within one condition every populated operator must hold, which is how
/// GuardDuty composes a criterion.
pub fn condition_matches(
    finding: &FindingRecord,
    path: &str,
    condition: &crate::ops::types::Condition,
) -> Result<bool, String> {
    if !supports_attribute(path) {
        return Err(path.to_string());
    }
    let Some(attr) = finding_attribute(finding, path) else {
        return Err(path.to_string());
    };
    let text = equality_text(finding, path).unwrap_or_else(|| attr.as_text());

    // `Equals`/`NotEquals` and the deprecated `Eq`/`Neq` are the same comparison;
    // a list holds alternatives, so `Equals` passes when ANY entry matches and
    // `NotEquals` passes only when NONE does.
    for list in [condition.equals.as_ref(), condition.eq.as_ref()].into_iter().flatten() {
        if !list.is_empty() && !list.contains(&text) {
            return Ok(false);
        }
    }
    for list in [condition.not_equals.as_ref(), condition.neq.as_ref()]
        .into_iter()
        .flatten()
    {
        if list.contains(&text) {
            return Ok(false);
        }
    }

    // The ordering conditions only apply to an attribute with a numeric form.
    let bounds: [(Option<f64>, Ordering); 8] = [
        (condition.greater_than.map(|v| v as f64), Ordering::Gt),
        (condition.gt.map(f64::from), Ordering::Gt),
        (condition.greater_than_or_equal.map(|v| v as f64), Ordering::Gte),
        (condition.gte.map(f64::from), Ordering::Gte),
        (condition.less_than.map(|v| v as f64), Ordering::Lt),
        (condition.lt.map(f64::from), Ordering::Lt),
        (condition.less_than_or_equal.map(|v| v as f64), Ordering::Lte),
        (condition.lte.map(f64::from), Ordering::Lte),
    ];
    for (bound, ordering) in bounds {
        let Some(bound) = bound else { continue };
        let Some(value) = attr.as_number() else {
            return Err(path.to_string());
        };
        let holds = match ordering {
            Ordering::Gt => value > bound,
            Ordering::Gte => value >= bound,
            Ordering::Lt => value < bound,
            Ordering::Lte => value <= bound,
        };
        if !holds {
            return Ok(false);
        }
    }

    // `Matches` / `NotMatches` compare the attribute against a pattern in which
    // `*` stands for any run of characters; like the equality lists, a list holds
    // alternatives.
    if let Some(patterns) = condition.matches.as_ref() {
        if !patterns.is_empty() && !patterns.iter().any(|p| wildcard_matches(p, &text)) {
            return Ok(false);
        }
    }
    if let Some(patterns) = condition.not_matches.as_ref() {
        if patterns.iter().any(|p| wildcard_matches(p, &text)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Does `value` match `pattern`, where `*` in the pattern stands for any run of
/// characters (including none)?
fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let mut segments = pattern.split('*');
    let Some(first) = segments.next() else {
        return pattern == value;
    };
    if !value.starts_with(first) {
        return false;
    }
    let mut rest = &value[first.len()..];
    let segments: Vec<&str> = segments.collect();
    let Some((last, middle)) = segments.split_last() else {
        // No `*` at all: the pattern is a literal.
        return rest.is_empty();
    };
    for segment in middle {
        match rest.find(segment) {
            Some(at) => rest = &rest[at + segment.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Which way an ordering condition compares.
enum Ordering {
    Gt,
    Gte,
    Lt,
    Lte,
}

/// Does `finding` satisfy every criterion in `criteria`? An absent or empty
/// criteria map matches every finding.
pub fn finding_matches(
    finding: &FindingRecord,
    criteria: Option<&crate::ops::types::FindingCriteria>,
) -> Result<bool, String> {
    let Some(criterion) = criteria.and_then(|c| c.criterion.as_ref()) else {
        return Ok(true);
    };
    for (path, condition) in criterion {
        if !condition_matches(finding, path, condition)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Refuse a `FindingCriteria` this emulator cannot evaluate, naming the attribute
/// path.
///
/// Checked BEFORE any finding is looked at, so whether a criterion is refused does
/// not depend on how many findings happen to be stored: a detector with none must
/// answer an unevaluable criterion exactly as a populated one does.
pub fn validate_finding_criteria(
    criteria: Option<&crate::ops::types::FindingCriteria>,
) -> Result<(), String> {
    let Some(criterion) = criteria.and_then(|c| c.criterion.as_ref()) else {
        return Ok(());
    };
    for (path, condition) in criterion {
        if !supports_attribute(path) {
            return Err(path.clone());
        }
        let orders_numerically = condition.greater_than.is_some()
            || condition.greater_than_or_equal.is_some()
            || condition.less_than.is_some()
            || condition.less_than_or_equal.is_some()
            || condition.gt.is_some()
            || condition.gte.is_some()
            || condition.lt.is_some()
            || condition.lte.is_some();
        if orders_numerically && !attribute_is_numeric(path) {
            return Err(path.clone());
        }
    }
    Ok(())
}

/// Which finding attributes an ordering condition can compare. `createdAt` /
/// `updatedAt` order as epoch milliseconds, the way GuardDuty's own filters compare
/// them.
fn attribute_is_numeric(path: &str) -> bool {
    matches!(path, "confidence" | "createdAt" | "severity" | "updatedAt")
}

/// Order `findings` for `ListFindings` / `GetFindings`.
///
/// With no `SortCriteria` the newest finding comes first, with the finding id
/// breaking ties so the order is total and a page token stays meaningful.
/// `Err(attribute)` means the emulator does not model that sort attribute.
pub fn sort_findings(
    findings: &mut [&FindingRecord],
    sort: Option<&crate::ops::types::SortCriteria>,
) -> Result<(), String> {
    let attribute = sort
        .and_then(|s| s.attribute_name.as_deref())
        .filter(|a| !a.is_empty());
    let descending = match sort.and_then(|s| s.order_by.as_deref()) {
        Some("ASC") => false,
        Some("DESC") => true,
        // No explicit order: newest-first for the default attribute, ascending for
        // an attribute the caller named.
        _ => attribute.is_none(),
    };
    let attribute = attribute.unwrap_or("createdAt");
    if !supports_attribute(attribute) {
        return Err(attribute.to_string());
    }

    findings.sort_by(|a, b| {
        let key = |f: &FindingRecord| match finding_attribute(f, attribute) {
            Some(Attribute::Number(n)) => (n, String::new()),
            Some(Attribute::Text(t)) => (0.0, t),
            None => (0.0, String::new()),
        };
        let (an, at) = key(a);
        let (bn, bt) = key(b);
        let primary = an
            .partial_cmp(&bn)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| at.cmp(&bt));
        let primary = if descending { primary.reverse() } else { primary };
        primary.then_with(|| a.id.cmp(&b.id))
    });
    Ok(())
}

/// Take one page out of an ordered key list.
///
/// `token` is the number of entries already returned, as a decimal string — the
/// only thing a caller may do with it is hand it back. `Err(token)` means the token
/// was not one this service issued: either it is not a decimal offset at all, or it
/// points past the end of the list, which this service never hands out (it answers
/// the last page with no `NextToken`).
pub fn paginate(
    keys: &[String],
    max_results: Option<i32>,
    token: Option<&str>,
    page_limit: usize,
) -> Result<(Vec<String>, Option<String>), String> {
    let start = match token.filter(|t| !t.is_empty()) {
        Some(t) => {
            let start = t.parse::<usize>().map_err(|_| t.to_string())?;
            // A token this service issued always names an entry that exists: it is
            // emitted only while `consumed < keys.len()`.
            if start >= keys.len() {
                return Err(t.to_string());
            }
            start
        }
        None => 0,
    };
    let limit = max_results
        .and_then(|m| usize::try_from(m).ok())
        .filter(|m| *m > 0)
        .unwrap_or(page_limit)
        .min(page_limit);
    let page: Vec<String> = keys.iter().skip(start).take(limit).cloned().collect();
    let consumed = start.saturating_add(page.len());
    let next = if consumed < keys.len() {
        Some(consumed.to_string())
    } else {
        None
    };
    Ok((page, next))
}

/// The feature set `CreateDetector` gives a detector when the request names no
/// features.
///
/// Every configurable feature is enabled except the [`OPT_IN_FEATURES`]: "When you
/// don't specify any features, with an exception to RUNTIME_MONITORING, all the
/// optional features are enabled by default"
/// (`smithy_query(query="doc",shape="CreateDetector")`). The three
/// [`FOUNDATIONAL_FEATURES`] are not optional and are always enabled.
///
/// The records are created in [`reported_features`] order, which is the order a
/// read reports them in.
pub fn default_features(now: i64) -> Vec<FeatureRecord> {
    reported_features()
        .into_iter()
        .map(|name| FeatureRecord {
            name: name.to_string(),
            status: if OPT_IN_FEATURES.contains(&name) {
                DISABLED.to_string()
            } else {
                ENABLED.to_string()
            },
            updated_at_millis: now,
            additional_configuration: Vec::new(),
        })
        .collect()
}

/// Refuse a `Features` list this service cannot accept, with the message the
/// refusal carries.
///
/// Two rules, both from `CreateDetector`'s own documentation: a feature name must
/// be a `DetectorFeature` member, and EKS Runtime Monitoring and Runtime
/// Monitoring cannot both be requested because Runtime Monitoring already includes
/// Amazon EKS.
pub fn validate_feature_configurations(
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        if !DETECTOR_FEATURES.contains(&name) {
            return Err(format!(
                "Features: '{name}' is not a supported detector feature. Supported features are {}.",
                DETECTOR_FEATURES.join(", ")
            ));
        }
        if let Some(status) = feature.status.as_deref() {
            if status != ENABLED && status != DISABLED {
                return Err(format!(
                    "Features: '{status}' is not a supported status for feature '{name}'. Supported values are {ENABLED}, {DISABLED}."
                ));
            }
        }
    }
    let named = |wanted: &str| {
        features
            .iter()
            .any(|f| f.name.as_deref() == Some(wanted) && f.status.as_deref() != Some(DISABLED))
    };
    if named(FEATURE_EKS_RUNTIME_MONITORING) && named(FEATURE_RUNTIME_MONITORING) {
        return Err(format!(
            "Features: {FEATURE_EKS_RUNTIME_MONITORING} and {FEATURE_RUNTIME_MONITORING} cannot both be enabled because {FEATURE_RUNTIME_MONITORING} already includes threat detection for Amazon EKS resources."
        ));
    }
    Ok(())
}

/// Refuse a `DestinationType` this service does not support, returning the AWS
/// service a destination of that type is an ARN of.
///
/// The accepted set is the `DestinationType` enum itself
/// (`smithy_query(shape="DestinationType")`), checked here rather than at the
/// boundary because this service validates its enum-valued members in the
/// handler, the way `Features` is checked above.
pub fn validate_destination_type(destination_type: &str) -> Result<&'static str, String> {
    DESTINATION_SERVICES
        .iter()
        .find(|(name, _)| *name == destination_type)
        .map(|(_, service)| *service)
        .ok_or_else(|| {
            format!(
                "{DESTINATION_TYPE_MEMBER}: '{destination_type}' is not a supported destination type. Supported types are {}.",
                DESTINATION_TYPES.join(", ")
            )
        })
}

/// Refuse a `DestinationProperties` this service cannot accept, returning the
/// destination and KMS key ARNs it validated, or the message the refusal carries.
///
/// `CreatePublishingDestination` takes "the properties of the publishing
/// destination, including the ARNs for the destination and the KMS key used for
/// encryption"
/// (`smithy_query(query="doc",shape="CreatePublishingDestinationRequest",member="DestinationProperties")`),
/// so both ARNs are required; each must be a well-formed ARN of the service its
/// own member names ("The ARN of the resource to publish to", "The ARN of the KMS
/// key to use for encryption" — `smithy_query(shape="DestinationProperties")`),
/// and `ExpectedBucketOwner`, where supplied, must be "The 12-digit Amazon Web
/// Services account ID of the account that owns the Amazon S3 bucket".
///
/// `destination_service` is the one [`validate_destination_type`] resolved, so
/// the destination ARN is checked against the service its TYPE names rather than
/// against a hard-coded one.
pub fn validate_destination_properties(
    destination_service: &str,
    properties: &crate::ops::types::DestinationProperties,
) -> Result<(String, String), String> {
    let destination_arn = properties
        .destination_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| missing_parameter_message(DESTINATION_ARN_MEMBER))?;
    if !is_arn_for(destination_service, destination_arn) {
        return Err(invalid_parameter_message(DESTINATION_ARN_MEMBER));
    }
    let kms_key_arn = properties
        .kms_key_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| missing_parameter_message(KMS_KEY_ARN_MEMBER))?;
    if !is_arn_for(KMS_SERVICE, kms_key_arn) {
        return Err(invalid_parameter_message(KMS_KEY_ARN_MEMBER));
    }
    if let Some(owner) = properties.expected_bucket_owner.as_deref() {
        if !owner.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid_parameter_message(EXPECTED_BUCKET_OWNER_MEMBER));
        }
    }
    Ok((destination_arn.to_string(), kms_key_arn.to_string()))
}

/// The AWS service each `DestinationType` member publishes to: an `S3`
/// destination is an Amazon S3 bucket ARN
/// (`smithy_query(shape="DestinationType")`, whose only member is `S3`).
const DESTINATION_SERVICES: [(&str, &str); 1] = [(DESTINATION_TYPE_S3, S3_SERVICE)];

/// The service namespace of an Amazon S3 ARN.
const S3_SERVICE: &str = "s3";

/// The service namespace of a KMS key ARN.
const KMS_SERVICE: &str = "kms";

/// Write a `Features` list onto `detector`. A feature the list does not name keeps
/// the status it already had.
pub fn apply_feature_configurations(
    detector: &mut Detector,
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
    now: i64,
) {
    let Some(features) = features else { return };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        let status = feature.status.as_deref().unwrap_or(ENABLED);
        detector.set_feature(name, status, now);
        if let Some(additional) = feature.additional_configuration.as_ref() {
            let configured: Vec<AdditionalConfigRecord> = additional
                .iter()
                .filter_map(|cfg| {
                    cfg.name.as_deref().map(|name| AdditionalConfigRecord {
                        name: name.to_string(),
                        status: cfg.status.as_deref().unwrap_or(ENABLED).to_string(),
                        updated_at_millis: now,
                    })
                })
                .collect();
            if let Some(record) = detector.features.iter_mut().find(|f| f.name == name) {
                record.additional_configuration = configured;
            }
        }
    }
}

/// Write the deprecated `DataSources` view onto `detector`.
///
/// `DataSources` is "deprecated, use Features instead"
/// (`smithy_query(operation="CreateDetector")`), so each data source switch is
/// stored as the feature that governs it — one source of truth, read back through
/// either view.
pub fn apply_data_sources(
    detector: &mut Detector,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    now: i64,
) {
    let Some(data_sources) = data_sources else {
        return;
    };
    let status = |enable: bool| if enable { ENABLED } else { DISABLED };
    if let Some(s3) = data_sources.s3_logs.as_ref() {
        detector.set_feature(FEATURE_S3_DATA_EVENTS, status(s3.enable), now);
    }
    if let Some(kubernetes) = data_sources.kubernetes.as_ref() {
        detector.set_feature(
            FEATURE_EKS_AUDIT_LOGS,
            status(kubernetes.audit_logs.enable),
            now,
        );
    }
    if let Some(enable) = data_sources
        .malware_protection
        .as_ref()
        .and_then(|m| m.scan_ec2_instance_with_findings.as_ref())
        .and_then(|s| s.ebs_volumes)
    {
        detector.set_feature(FEATURE_EBS_MALWARE_PROTECTION, status(enable), now);
    }
}

/// Refuse a `ScanResourceCriteria` this service cannot accept, with the message the
/// refusal carries.
///
/// Both rules come from the model: a criterion key must be a `ScanCriterionKey`
/// member, which is the generated
/// [`crate::ops::types::shared_types::SCAN_CRITERION_KEY_VALUES`] list, and a
/// condition's tag key must not use the reserved `aws:` prefix, which
/// `ScanConditionPair.Key`'s own pattern `^(?!aws:)[a-zA-Z+-=._:/]+$` excludes
/// (`smithy_query(shape="ScanConditionPair")`).
pub fn validate_scan_resource_criteria(
    criteria: &crate::ops::types::ScanResourceCriteria,
) -> Result<(), String> {
    let supported = crate::ops::types::shared_types::SCAN_CRITERION_KEY_VALUES;
    for (member, criterion) in [
        ("include", criteria.include.as_ref()),
        ("exclude", criteria.exclude.as_ref()),
    ] {
        let Some(criterion) = criterion else { continue };
        for (key, condition) in criterion {
            if !supported.contains(&key.as_str()) {
                return Err(format!(
                    "{SCAN_RESOURCE_CRITERIA_MEMBER}.{member}: '{key}' is not a supported scan criterion key. Supported keys are {}.",
                    supported.join(", ")
                ));
            }
            for pair in &condition.map_equals {
                if pair.key.starts_with(RESERVED_TAG_PREFIX) {
                    return Err(format!(
                        "{SCAN_RESOURCE_CRITERIA_MEMBER}.{member}: a tag key must not begin with the reserved prefix '{RESERVED_TAG_PREFIX}'."
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Store one `ScanCriterion` as it arrived on the wire.
pub fn scan_criterion_record(
    criterion: Option<&std::collections::HashMap<String, crate::ops::types::ScanCondition>>,
) -> ScanCriterionRecord {
    criterion
        .map(|criterion| {
            criterion
                .iter()
                .map(|(key, condition)| {
                    let pairs = condition
                        .map_equals
                        .iter()
                        .map(|pair| ScanTagPair {
                            key: pair.key.clone(),
                            value: pair.value.clone(),
                        })
                        .collect();
                    (key.clone(), pairs)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Project one stored `ScanCriterion` back onto the wire shape, or `None` when the
/// detector carries no criterion of that kind: `Include` and `Exclude` are both
/// optional, so an absent one is reported as absent rather than as an empty map.
pub fn scan_criterion_shape(
    record: &ScanCriterionRecord,
) -> Option<std::collections::HashMap<String, crate::ops::types::ScanCondition>> {
    if record.is_empty() {
        return None;
    }
    Some(
        record
            .iter()
            .map(|(key, pairs)| {
                let condition = crate::ops::types::ScanCondition {
                    map_equals: pairs
                        .iter()
                        .map(|pair| crate::ops::types::ScanConditionPair {
                            key: pair.key.clone(),
                            value: pair.value.clone(),
                        })
                        .collect(),
                };
                (key.clone(), condition)
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Custom detection rules
//
// A custom detection rule is published BY GuardDuty: the API models no create,
// update or delete for one, only the associations an account makes against it,
// so the catalogue below is the service's own and the same for every caller.
// ---------------------------------------------------------------------------

/// The wire name of the `RuleId` member, as a request carries it — a `jsonName`
/// `ruleId` on `CreateCustomDetectionRuleAssociation` and the URI label on
/// `GetCustomDetectionRule`
/// (`smithy_query(operation="GetCustomDetectionRule,CreateCustomDetectionRuleAssociation")`).
pub const RULE_ID_MEMBER: &str = "ruleId";

/// The wire name of `ListCustomDetectionRules`'s filter list: its `Filters` member
/// has `jsonName` `filters` (`smithy_query(operation="ListCustomDetectionRules")`).
pub const FILTERS_MEMBER: &str = "filters";

/// `RuleLanguage`'s only member (`smithy_query(shape="RuleLanguage")`): the
/// language every published rule expresses its detection logic in.
pub const RULE_LANGUAGE_SQL: &str = "SQL";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RULE_LANGUAGE_VALUES,
    RULE_LANGUAGE_SQL
));

/// `RuleSchema`'s only member (`smithy_query(shape="RuleSchema")`): the schema a
/// rule over CloudTrail management events is written against. A rule over another
/// data source has no schema member to report, so it reports none.
pub const RULE_SCHEMA_CLOUD_TRAIL: &str = "CloudTrail";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RULE_SCHEMA_VALUES,
    RULE_SCHEMA_CLOUD_TRAIL
));

/// The two `DetectionRuleDataSource` members — "The data source that the rule
/// analyzes" (`smithy_query(shape="DetectionRuleDataSource")`).
pub const DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL: &str = "CloudTrailManagementEvent";
/// See [`DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL`].
pub const DETECTION_RULE_DATA_SOURCE_RUNTIME: &str = "RuntimeMonitoring";

/// The `DetectionRuleSeverity` members (`smithy_query(shape="DetectionRuleSeverity")`).
pub const DETECTION_RULE_SEVERITY_CRITICAL: &str = "CRITICAL";
/// See [`DETECTION_RULE_SEVERITY_CRITICAL`].
pub const DETECTION_RULE_SEVERITY_HIGH: &str = "HIGH";
/// See [`DETECTION_RULE_SEVERITY_CRITICAL`].
pub const DETECTION_RULE_SEVERITY_MEDIUM: &str = "MEDIUM";
/// See [`DETECTION_RULE_SEVERITY_CRITICAL`].
pub const DETECTION_RULE_SEVERITY_LOW: &str = "LOW";

/// The `DetectionRuleFilterCondition` members — "The condition to apply to the
/// filter. For example, `EQUALS` or `CONTAINS`"
/// (`smithy_query(query="doc",shape="DetectionRuleFilter",member="Condition")`).
/// [`FILTER_CONDITION_EQUALS`] is what a filter that names no condition asks for:
/// a filter is a selection by value, and the exact value is the narrower reading.
pub const FILTER_CONDITION_EQUALS: &str = "EQUALS";
/// See [`FILTER_CONDITION_EQUALS`].
pub const FILTER_CONDITION_CONTAINS: &str = "CONTAINS";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_FILTER_CONDITION_VALUES,
    FILTER_CONDITION_EQUALS
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_FILTER_CONDITION_VALUES,
    FILTER_CONDITION_CONTAINS
));

/// Every `FilterFieldName` member — the rule fields a `DetectionRuleFilter` may
/// name (`smithy_query(shape="FilterFieldName")`). Each one is the `jsonName` of a
/// `RuleSummary` member, which is why [`CustomDetectionRuleRecord::field`] resolves
/// them straight onto the record.
pub const DETECTION_RULE_FILTER_FIELDS: &[&str] =
    crate::ops::types::shared_types::FILTER_FIELD_NAME_VALUES;

/// The platforms a Runtime Monitoring rule can run on, as
/// `DetectionRuleRuntimePlatforms` names them: "The presence of a platform member
/// indicates that the rule is supported on that platform"
/// (`smithy_query(query="doc",shape="DetectionRuleRuntimeMonitoringDetails",member="Platforms")`).
pub const RUNTIME_PLATFORM_EKS: &str = "EKS";
/// See [`RUNTIME_PLATFORM_EKS`].
pub const RUNTIME_PLATFORM_EC2: &str = "EC2";
/// See [`RUNTIME_PLATFORM_EKS`].
pub const RUNTIME_PLATFORM_ECS_FARGATE: &str = "ECS_FARGATE";

/// The message a read of a rule the catalogue does not publish answers with.
///
/// It names the member at fault and states the rule it broke, without quoting the
/// caller's value back — the wording every rejection on this service follows.
pub fn custom_detection_rule_not_found_message() -> String {
    format!(
        "The request is rejected because the input {RULE_ID_MEMBER} does not refer to a custom detection rule published by GuardDuty."
    )
}

/// The ARN of the custom detection rule `rule_id` as `region`'s endpoint reports
/// it.
///
/// Derived, never stored, exactly as [`malware_protection_plan_arn`] is: the rule's
/// own id and the Region serving the read are the only variable parts, so a read
/// cannot report an ARN that disagrees with the id it was asked about. The resource
/// type is the one the ARN's own shape names (`DetectionRuleArn`).
pub fn custom_detection_rule_arn(region: &str, rule_id: &str) -> String {
    ::engine::emulator_arn(
        GUARDDUTY_ARN_SERVICE,
        region,
        &format!("detection-rule/{rule_id}"),
    )
}

/// One custom detection rule as the store holds it.
///
/// `Language`/`Schema`/`UpdatedAt` are the model's optional members and are held as
/// `Option`, so a rule that has none reports none rather than an invented value.
#[derive(Debug, Clone)]
pub struct CustomDetectionRuleRecord {
    /// `RuleId`: `len 1..100`, `pattern ^[a-z0-9]+(-[a-z0-9]+)*$`
    /// (`smithy_query(shape="RuleId")`).
    pub rule_id: String,
    /// `RuleSummary.Name` — the rule's display name.
    pub name: String,
    /// `RuleSummary.Description` — "A description of what the rule detects".
    pub description: String,
    /// One of the [`DETECTION_RULE_SEVERITY_CRITICAL`] family.
    pub severity: String,
    /// [`DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL`] or
    /// [`DETECTION_RULE_DATA_SOURCE_RUNTIME`].
    pub data_source: String,
    /// The MITRE ATT&CK tactic the rule detects.
    pub tactic: String,
    /// The MITRE ATT&CK technique the rule detects.
    pub technique: String,
    /// The Amazon Web Services service the rule is about.
    pub service: String,
    /// `RuleDefinition.Expression` — the rule's detection logic, which only
    /// `GetCustomDetectionRule` reports (`RuleSummary` carries no definition).
    pub expression: String,
    /// The language the expression is written in, or `None` when the rule declares
    /// none.
    pub language: Option<String>,
    /// The schema the expression is written against, or `None` when the rule
    /// declares none.
    pub schema: Option<String>,
    /// The Runtime Monitoring platforms the rule is supported on, empty for a rule
    /// that is not a Runtime Monitoring rule.
    pub runtime_platforms: Vec<String>,
    /// When the store first held this rule.
    pub created_at_millis: i64,
    /// When it last changed, or `None` for a rule nothing has changed since it was
    /// published — `UpdatedAt` is optional on both `RuleSummary` and `RuleDetail`.
    pub updated_at_millis: Option<i64>,
}

impl CustomDetectionRuleRecord {
    /// The value of the rule field a `DetectionRuleFilter` names, by the field's
    /// own wire name.
    ///
    /// The `FilterFieldName` members are the `jsonName`s of `RuleSummary`'s own
    /// members, so a filter selects on the value a list REPORTS for that member.
    /// A name outside [`DETECTION_RULE_FILTER_FIELDS`] resolves to nothing, so a
    /// filter over a field no rule carries selects no rule.
    pub fn field(&self, field: &str) -> Option<&str> {
        match field {
            "name" => Some(&self.name),
            "description" => Some(&self.description),
            "dataSource" => Some(&self.data_source),
            "severity" => Some(&self.severity),
            "tactic" => Some(&self.tactic),
            "technique" => Some(&self.technique),
            "service" => Some(&self.service),
            _ => None,
        }
    }
}

// Every field name the model lists is one `field` above resolves: the arms there
// and the enum's members cannot drift apart without failing the build, so a member
// the model gains is a build error rather than a filter that silently selects
// nothing.
const _: () = {
    let mut index = 0;
    while index < DETECTION_RULE_FILTER_FIELDS.len() {
        assert!(enum_carries(
            &[
                "name",
                "description",
                "dataSource",
                "severity",
                "tactic",
                "technique",
                "service"
            ],
            DETECTION_RULE_FILTER_FIELDS[index]
        ));
        index += 1;
    }
    assert!(DETECTION_RULE_FILTER_FIELDS.len() == 7);
};

/// GuardDuty's catalogue of custom detection rules, held by this service's store.
///
/// Opaque on purpose: the catalogue is READ by every operation that resolves a
/// rule, and written by nothing, so it hands out `&`-references and no way to
/// insert. That is what makes "a rule a caller cannot create" also "a rule a
/// caller cannot change".
#[derive(Debug)]
pub struct CustomDetectionRuleCatalog {
    /// Every published rule, in `RuleId` order — the total order a page token is an
    /// offset into.
    rules: Vec<CustomDetectionRuleRecord>,
}

impl Default for CustomDetectionRuleCatalog {
    /// Materialise the published catalogue, every rule carrying the instant the
    /// store came up as its `CreatedAt`.
    ///
    /// The instant is read ONCE here, when the store is created, rather than per
    /// call: `CreatedAt` is required on both `RuleSummary` and `RuleDetail`, and a
    /// rule that reported a fresh instant on every read would not be the same rule
    /// twice.
    fn default() -> Self {
        let published_at = now_millis();
        let mut rules: Vec<CustomDetectionRuleRecord> = PUBLISHED_CUSTOM_DETECTION_RULES
            .iter()
            .map(|published| published.record(published_at))
            .collect();
        rules.sort_by(|left, right| left.rule_id.cmp(&right.rule_id));
        Self { rules }
    }
}

impl CustomDetectionRuleCatalog {
    /// Every published rule, in `RuleId` order.
    pub fn rules(&self) -> &[CustomDetectionRuleRecord] {
        &self.rules
    }

    /// The published rule `rule_id` names, if the catalogue carries it.
    ///
    /// Matching is EXACT: a rule id is an identifier, so a near miss is a different
    /// string and not a rule. A linear scan over a fixed table of single-digit
    /// length: the catalogue has no key a map would index it by faster.
    pub fn rule(&self, rule_id: &str) -> Option<&CustomDetectionRuleRecord> {
        self.rules.iter().find(|rule| rule.rule_id == rule_id)
    }
}

/// One entry of the published catalogue.
///
/// `Language` and `Schema` are not members here: every published rule expresses its
/// logic in the one `RuleLanguage` member the model carries, and the one
/// `RuleSchema` member is `CloudTrail`, so both follow from the data source — see
/// [`Self::record`].
struct PublishedRule {
    rule_id: &'static str,
    name: &'static str,
    description: &'static str,
    severity: &'static str,
    data_source: &'static str,
    tactic: &'static str,
    technique: &'static str,
    service: &'static str,
    expression: &'static str,
    /// The Runtime Monitoring platforms the rule runs on; empty for a rule over
    /// another data source, which has no Runtime Monitoring details to report.
    runtime_platforms: &'static [&'static str],
}

impl PublishedRule {
    /// This entry as a stored record, published at `published_at`.
    fn record(&self, published_at: i64) -> CustomDetectionRuleRecord {
        CustomDetectionRuleRecord {
            rule_id: self.rule_id.to_string(),
            name: self.name.to_string(),
            description: self.description.to_string(),
            severity: self.severity.to_string(),
            data_source: self.data_source.to_string(),
            tactic: self.tactic.to_string(),
            technique: self.technique.to_string(),
            service: self.service.to_string(),
            expression: self.expression.to_string(),
            language: Some(RULE_LANGUAGE_SQL.to_string()),
            // `RuleSchema` carries exactly one member, `CloudTrail`, so a rule over
            // CloudTrail management events reports it and a rule over any other
            // data source has no schema the model can name.
            schema: (self.data_source == DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL)
                .then(|| RULE_SCHEMA_CLOUD_TRAIL.to_string()),
            runtime_platforms: self
                .runtime_platforms
                .iter()
                .map(|platform| (*platform).to_string())
                .collect(),
            created_at_millis: published_at,
            // Nothing in the API changes a published rule, so a rule the store has
            // never changed reports no `UpdatedAt` rather than repeating
            // `CreatedAt`.
            updated_at_millis: None,
        }
    }
}

/// Every custom detection rule this emulator publishes.
///
/// The catalogue is the SERVICE's own, the way [`INVESTIGATION_REGIONS`] and the
/// sample-finding type catalogue are: the Smithy model names no rule and binds no
/// operation that creates one, so there is nothing for a caller to add and nothing
/// to read a rule list out of. Each entry's `severity` and `dataSource` are members
/// of the model's own enums (asserted below), each `tactic`/`technique` pair names
/// an MITRE ATT&CK Enterprise tactic and technique, and each `expression` is the
/// SQL that detects it over the entry's data source.
///
/// The rows span the whole surface a caller can select on: both
/// `DetectionRuleDataSource` members, all four `DetectionRuleSeverity` members,
/// several services and tactics, and both a rule with Runtime Monitoring platform
/// requirements and rules with none.
///
/// The ids are this emulator's, because AWS publishes none the model or any
/// response here names: an operation handed a `ruleId` from the real service — a
/// `CreateCustomDetectionRuleAssociation`, say — answers the modelled "no such
/// rule" rather than inventing a rule to match it, which is the correct answer for
/// a rule this service does not publish. Should a recorded response ever expose the
/// real catalogue, replace these rows with it; nothing outside this table names a
/// rule.
const PUBLISHED_CUSTOM_DETECTION_RULES: &[PublishedRule] = &[
    PublishedRule {
        rule_id: "cloudtrail-logging-disabled",
        name: "CloudTrail logging disabled",
        description: "Detects a call that stops or deletes an AWS CloudTrail trail, which an adversary uses to hide the activity that follows.",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Defense Evasion",
        technique: "T1562.008 Impair Defenses: Disable or Modify Cloud Logs",
        service: "cloudtrail",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'cloudtrail.amazonaws.com' AND eventName IN ('StopLogging', 'DeleteTrail', 'UpdateTrail')",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "guardduty-detector-disabled",
        name: "GuardDuty detector disabled or deleted",
        description: "Detects a call that disables or deletes a GuardDuty detector, which removes threat detection for the Region it served.",
        severity: DETECTION_RULE_SEVERITY_CRITICAL,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Defense Evasion",
        technique: "T1562.001 Impair Defenses: Disable or Modify Tools",
        service: "guardduty",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'guardduty.amazonaws.com' AND eventName IN ('DeleteDetector', 'UpdateDetector')",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "iam-access-key-created-for-another-user",
        name: "Access key created for another IAM user",
        description: "Detects an IAM access key created for a principal other than the caller, a durable credential an adversary leaves behind.",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Persistence",
        technique: "T1098.001 Account Manipulation: Additional Cloud Credentials",
        service: "iam",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'iam.amazonaws.com' AND eventName = 'CreateAccessKey' AND requestParameters.userName IS NOT NULL AND requestParameters.userName <> userIdentity.userName",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "kms-key-scheduled-for-deletion",
        name: "KMS key scheduled for deletion",
        description: "Detects a call that schedules an AWS KMS key for deletion or disables it, which makes the data it protects unreadable.",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Impact",
        technique: "T1485 Data Destruction",
        service: "kms",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'kms.amazonaws.com' AND eventName IN ('ScheduleKeyDeletion', 'DisableKey')",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "root-credential-api-activity",
        name: "Root credential API activity",
        description: "Detects an API call signed with the account root user's credentials, which day-to-day activity does not use.",
        severity: DETECTION_RULE_SEVERITY_MEDIUM,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Privilege Escalation",
        technique: "T1078.004 Valid Accounts: Cloud Accounts",
        service: "iam",
        expression: "SELECT * FROM cloudtrail WHERE userIdentity.type = 'Root' AND eventType <> 'AwsServiceEvent'",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "s3-bucket-public-access-granted",
        name: "S3 bucket opened to public access",
        description: "Detects a call that grants public access to an Amazon S3 bucket by relaxing its policy, ACL or public access block.",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Exfiltration",
        technique: "T1567 Exfiltration Over Web Service",
        service: "s3",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 's3.amazonaws.com' AND eventName IN ('PutBucketPolicy', 'PutBucketAcl', 'DeletePublicAccessBlock', 'PutPublicAccessBlock')",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "console-login-without-mfa",
        name: "Console sign-in without MFA",
        description: "Detects a successful AWS Management Console sign-in that presented no multi-factor authentication.",
        severity: DETECTION_RULE_SEVERITY_LOW,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Initial Access",
        technique: "T1078.004 Valid Accounts: Cloud Accounts",
        service: "signin",
        expression: "SELECT * FROM cloudtrail WHERE eventName = 'ConsoleLogin' AND responseElements.ConsoleLogin = 'Success' AND additionalEventData.MFAUsed = 'No'",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "runtime-shell-launched-by-server-process",
        name: "Interactive shell launched by a server process",
        description: "Detects an interactive shell launched by a long-running server process, the usual first step after a remote code execution.",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_RUNTIME,
        tactic: "Execution",
        technique: "T1059.004 Command and Scripting Interpreter: Unix Shell",
        service: "ec2",
        expression: "SELECT * FROM runtime_monitoring WHERE eventType = 'PROCESS_LAUNCH' AND parentProcessName IN ('httpd', 'nginx', 'java', 'node') AND processName IN ('sh', 'bash', 'dash', 'zsh')",
        runtime_platforms: &[
            RUNTIME_PLATFORM_EC2,
            RUNTIME_PLATFORM_EKS,
            RUNTIME_PLATFORM_ECS_FARGATE,
        ],
    },
    PublishedRule {
        rule_id: "runtime-kernel-module-loaded",
        name: "Kernel module loaded at runtime",
        description: "Detects a process loading a kernel module, which an adversary uses to hide activity below the processes a host reports.",
        severity: DETECTION_RULE_SEVERITY_MEDIUM,
        data_source: DETECTION_RULE_DATA_SOURCE_RUNTIME,
        tactic: "Defense Evasion",
        technique: "T1547.006 Boot or Logon Autostart Execution: Kernel Modules and Extensions",
        service: "ec2",
        expression: "SELECT * FROM runtime_monitoring WHERE eventType = 'PROCESS_LAUNCH' AND processName IN ('insmod', 'modprobe', 'kmod')",
        runtime_platforms: &[RUNTIME_PLATFORM_EC2, RUNTIME_PLATFORM_EKS],
    },
];

// Every published rule carries values the model admits: a `severity` and a
// `dataSource` from their own enums, and a `ruleId` matching `RuleId`'s pattern and
// length. The build fails rather than the wire carrying a value the model does not
// name.
const _: () = {
    let mut index = 0;
    while index < PUBLISHED_CUSTOM_DETECTION_RULES.len() {
        let rule = &PUBLISHED_CUSTOM_DETECTION_RULES[index];
        assert!(enum_carries(
            crate::ops::types::shared_types::DETECTION_RULE_SEVERITY_VALUES,
            rule.severity
        ));
        assert!(enum_carries(
            crate::ops::types::shared_types::DETECTION_RULE_DATA_SOURCE_VALUES,
            rule.data_source
        ));
        assert!(is_rule_id_pattern(rule.rule_id));
        assert!(!rule.name.is_empty() && rule.name.len() <= 255);
        assert!(!rule.description.is_empty() && rule.description.len() <= 1024);
        assert!(!rule.tactic.is_empty() && rule.tactic.len() <= 255);
        assert!(!rule.technique.is_empty() && rule.technique.len() <= 255);
        assert!(!rule.service.is_empty() && rule.service.len() <= 128);
        assert!(!rule.expression.is_empty() && rule.expression.len() <= 4096);
        index += 1;
    }
};

/// Does `rule_id` satisfy `RuleId`'s own form — `len 1..100`, `pattern
/// ^[a-z0-9]+(-[a-z0-9]+)*$` (`smithy_query(shape="RuleId")`)?
///
/// A `const fn` so the catalogue above is checked at BUILD time; the request
/// boundary checks a caller's `ruleId` against the same model constraint before any
/// handler runs.
const fn is_rule_id_pattern(rule_id: &str) -> bool {
    let bytes = rule_id.as_bytes();
    if bytes.is_empty() || bytes.len() > 100 {
        return false;
    }
    let mut index = 0;
    // A segment must be non-empty, so a dash may neither open nor close the id nor
    // follow another dash.
    let mut segment_len = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'-' {
            if segment_len == 0 {
                return false;
            }
            segment_len = 0;
        } else if matches!(byte, b'a'..=b'z' | b'0'..=b'9') {
            segment_len += 1;
        } else {
            return false;
        }
        index += 1;
    }
    segment_len > 0
}

/// The AWS Region a request was signed for, which is the Region the resources it
/// creates live in. Falls back to the emulator's default Region for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The Region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGuarddutyGuarddutyapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGuarddutyGuarddutyapiserviceState>)
}
