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

/// The Runtime Monitoring coverage of the account's compute resources, which more than
/// one operation reports on.
pub mod coverage;

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
    /// Every custom detection rule ASSOCIATION this account has made, keyed by
    /// `(region, RuleId)`.
    ///
    /// An association binds a PUBLISHED rule
    /// ([`Self::custom_detection_rules`]) to this account, so the RULE identifies
    /// it: a second create for a rule the account already associated is the
    /// `ConflictException` the operation models rather than a second record,
    /// whatever execution mode either names, and
    /// `UpdateCustomDetectionRuleAssociation` is what moves the one association a
    /// rule has between modes. The `AssociationId` the model also carries is a
    /// minted handle onto that rule's association, resolved by
    /// [`Self::detection_rule_association_by_id`].
    ///
    /// BOUNDED by construction: a Region holds at most one association per
    /// published rule — a fixed catalogue — across at most
    /// [`MAX_DETECTION_RULE_REGIONS`] Regions, which the create enforces before it
    /// writes ([`Self::detection_rule_regions_exhausted`]). The `ClientToken` a
    /// create carried lives ON the record rather than in an index of its own, so
    /// the idempotency mappings are bounded by the same ceiling.
    pub custom_detection_rule_associations:
        BTreeMap<(String, String), DetectionRuleAssociationRecord>,
    /// Every custom detection rule ORGANIZATION configuration this account has
    /// created, keyed by `(region, RuleId, Mode)` — the Region for the same reason
    /// [`Self::custom_detection_rule_associations`] is keyed by it.
    ///
    /// Separate from the associations on purpose: an association enrolls THIS
    /// account in a rule, while an organization configuration says which member
    /// accounts the organization enrolls, so one `(RuleId, Mode)` pair can carry
    /// either, both or neither, and a read of one must never answer with the
    /// other.
    ///
    /// BOUNDED exactly as the association map is, by the same create-time guard.
    pub custom_detection_rule_org_configurations:
        BTreeMap<(String, String, String), DetectionRuleOrgConfigurationRecord>,
    /// The GuardDuty delegated administrator the organization has, keyed by the
    /// Region the designating request was signed for.
    ///
    /// At most ONE record per Region: the list of delegated administrators a read
    /// reports is `len 0..1`
    /// (`smithy_query(operation="ListOrganizationAdminAccounts")`), so a Region's
    /// organization has one delegated administrator or none. Regional because every
    /// GuardDuty resource is — the designation is made through one Region's
    /// endpoint and only that endpoint reports it
    /// ([`Self::organization_admin_account_in`]).
    ///
    /// BOUNDED at one record per Region across at most
    /// [`MAX_ORGANIZATION_ADMIN_REGIONS`] Regions, which the designation enforces
    /// before it writes ([`Self::organization_admin_regions_exhausted`]), so the
    /// map is finite whatever Region a caller signs for.
    pub organization_admin_accounts: BTreeMap<String, OrganizationAdminAccountRecord>,
    /// Last instant handed out by [`Self::tick_millis`], so two mutations in the
    /// same millisecond still get strictly increasing timestamps.
    last_millis: i64,
    /// Monotonic counter folded into every minted id.
    seq: u64,
}

/// What a member account's request to LEAVE the administrator relationship it
/// accepted an invitation into found, as
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::disassociate_detector_from_administrator`]
/// reports it to the two operations bound to that behaviour.
///
/// Only [`Self::Disassociated`] is a success; each other variant is a refusal with
/// its own message, and none of them writes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdministratorDisassociation {
    /// The Region's endpoint holds no detector under that id, so this account
    /// cannot leave anything through it.
    DetectorNotHeld,
    /// The organization this detector's stored configuration describes auto-enables
    /// GuardDuty for every one of its accounts, so the organization — not this call
    /// — owns the relationship (see
    /// [`organization_managed_administrator_message`]).
    OrganizationManaged,
    /// The detector is held, but its account has accepted no invitation, so there is
    /// no relationship to leave.
    NoAdministrator,
    /// This call dropped the relationship.
    Disassociated,
}

impl ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// The current instant in epoch milliseconds, strictly greater than every
    /// instant this state handed out before.
    ///
    /// Call it only from a MUTATING operation, and only once that operation knows
    /// it is going to write: it is the "this resource changed now" clock, so a
    /// reporting operation must not advance it and neither may a call that leaves
    /// the store exactly as it found it — an accepted request that writes nothing
    /// would otherwise push this counter past the wall clock and make the next
    /// real change stamp a future instant. Monotonicity is what makes
    /// `UpdatedAt > CreatedAt` observable for two changes that land in the same
    /// wall-clock millisecond.
    ///
    /// An operation that cannot tell whether it is going to write until it is
    /// writing — a batch that moves only the stored records it finds in a state it
    /// can move — reads [`Self::pending_millis`] and commits it with
    /// [`Self::commit_millis`] instead, which is this same pair of steps with the
    /// write in between.
    pub fn tick_millis(&mut self) -> i64 {
        let next = self.pending_millis();
        self.commit_millis(next);
        next
    }

    /// The instant [`Self::tick_millis`] would hand out, taking NOTHING off the
    /// clock.
    ///
    /// For a mutation whose writes are decided one stored record at a time — a
    /// batch that moves only the records it finds in a state it can move and leaves
    /// every other one exactly as it was — "is this call going to write" is
    /// answerable only while writing. Such a handler reads the instant here, stamps
    /// whatever it moves with it, and takes it off the clock with
    /// [`Self::commit_millis`] once a write has really landed: a call that moved
    /// nothing commits nothing, so it leaves the clock exactly where it found it and
    /// the next real change still stamps a wall-clock instant. A mutation that knows
    /// before it writes whether it is going to reads [`Self::tick_millis`] and
    /// returns ahead of it on the path that writes nothing.
    pub fn pending_millis(&self) -> i64 {
        let wall = now_millis();
        if wall > self.last_millis {
            wall
        } else {
            self.last_millis.saturating_add(1)
        }
    }

    /// Take the instant [`Self::pending_millis`] handed out off the clock, now that
    /// a write carrying it has landed.
    ///
    /// Never moves the clock backwards, so an instant committed twice — and one
    /// committed after a later mutation has already taken a further instant — still
    /// leaves the instants this state hands out strictly increasing.
    pub fn commit_millis(&mut self, millis: i64) {
        self.last_millis = self.last_millis.max(millis);
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

    /// Drop the administrator relationship the account that owns the detector
    /// `region`'s endpoint serves for `id` is monitored by.
    ///
    /// Every outcome a caller has to tell apart is a variant of
    /// [`AdministratorDisassociation`]: only the last of them is a success, and the
    /// two names bound to this behaviour both read it here, so neither can refuse
    /// what the other accepts.
    ///
    /// The ORGANIZATION precondition is checked before the relationship is read,
    /// the way the administrator-side operations check it before they match any
    /// account: it holds for the whole call rather than for one relationship (see
    /// [`organization_managed_administrator_message`]).
    ///
    /// The mutation clock ([`Self::tick_millis`]) is read ONLY on the path that
    /// writes, so no refusal advances it — which is what lets this be one walk in
    /// the handler instead of a resolve-then-tick-then-resolve-again one. The write
    /// itself is [`Detector::disassociate_from_administrator`].
    pub fn disassociate_detector_from_administrator(
        &mut self,
        region: &str,
        id: &str,
    ) -> AdministratorDisassociation {
        let Some(detector) = self.detector_in(region, id) else {
            return AdministratorDisassociation::DetectorNotHeld;
        };
        if detector.organization.auto_enables_all_members() {
            return AdministratorDisassociation::OrganizationManaged;
        }
        if detector.administrator.is_none() {
            return AdministratorDisassociation::NoAdministrator;
        }
        let now = self.tick_millis();
        if self
            .detector_in_mut(region, id)
            .is_some_and(|detector| detector.disassociate_from_administrator(now))
        {
            AdministratorDisassociation::Disassociated
        } else {
            // Unreachable: the relationship was read above under this same borrow of
            // the store. Reported as the absence it would be rather than panicked.
            AdministratorDisassociation::NoAdministrator
        }
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
    /// This is the WHOLE rule both directions of the archive write follow — resolve
    /// the detector, resolve every named finding against it, then write — with the
    /// direction carried by `archived`, so neither direction can drift from the
    /// other.
    ///
    /// `Err` carries the message the request is refused with, already classified by
    /// [`detector_id_rejection`] / [`finding_id_rejection`]: an endpoint that holds
    /// no such detector, or a batch naming a finding id that detector does not hold.
    /// Every id is resolved BEFORE anything is written, and the detector is resolved
    /// before the clock is read at all, so a refused request moves nothing and
    /// advances nothing — and an ACCEPTED request that moves no finding consumes no
    /// instant either ([`Self::pending_millis`]).
    pub fn set_findings_archived_in(
        &mut self,
        region: &str,
        detector_id: &str,
        finding_ids: &[String],
        archived: bool,
    ) -> Result<(), String> {
        let Some(detector) = self.detector_in(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        if let Some(unheld) = detector.first_unheld_finding_id(finding_ids) {
            return Err(finding_id_rejection(unheld));
        }
        // Whether any finding moves is known only as the write walks the named ids —
        // a batch naming none of them, and one naming only findings already in the
        // requested state, each leave every record exactly as it was — so the
        // instant is read here and taken off the clock only once a write has landed.
        let now = self.pending_millis();
        let Some(detector) = self.detector_in_mut(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        if detector.set_findings_archived(finding_ids, archived, now) {
            self.commit_millis(now);
        }
        Ok(())
    }

    /// Record the feedback a caller submitted on the named findings of the detector
    /// `region`'s endpoint serves for `detector_id`, recording the instant of the
    /// write as the `UpdatedAt` of each finding it changes.
    ///
    /// This is the WHOLE rule the feedback write follows — resolve the detector,
    /// then write — and it is the same shape the archive write takes
    /// ([`Self::set_findings_archived_in`]), so neither metadata write can drift
    /// from the other on which clock a record-metadata change moves. Unlike the
    /// archive write, an id this detector does not hold is NOT a refusal here: the
    /// feedback write takes the reading a findings read takes, where such an id is
    /// simply absent.
    ///
    /// `Err` carries the message the request is refused with, already classified by
    /// [`detector_id_rejection`]: an endpoint that holds no such detector. The
    /// detector is resolved before the clock is read at all, so a refused request
    /// moves nothing and advances nothing — and an accepted submission that changes
    /// no finding consumes no instant either ([`Self::pending_millis`]).
    pub fn set_findings_feedback_in(
        &mut self,
        region: &str,
        detector_id: &str,
        finding_ids: &[String],
        feedback: &str,
        comments: Option<&str>,
    ) -> Result<(), String> {
        if self.detector_in(region, detector_id).is_none() {
            return Err(detector_id_rejection(detector_id));
        }
        // Read without being taken off the clock, for the reason
        // [`Self::set_findings_archived_in`] reads it that way: a submission that
        // names no finding this detector holds, and one that re-submits the feedback
        // already recorded, both write nothing.
        let now = self.pending_millis();
        let Some(detector) = self.detector_in_mut(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        if detector.set_findings_feedback(finding_ids, feedback, comments, now) {
            self.commit_millis(now);
        }
        Ok(())
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

    /// The invitations this account's inbox holds through `region`'s endpoint, in
    /// a total order.
    ///
    /// The invitations hang off [`Detector::received_invitations`], and the
    /// operations that report the inbox name no detector at all
    /// (`smithy_query(operation="ListInvitations")` binds `GET /invitation`), so
    /// the inbox a request addresses is the one the Region it was signed for
    /// serves — a detector is a REGIONAL resource, so an invitation delivered to
    /// one Region's endpoint is not in another's inbox.
    ///
    /// Ordered by the inviting account id and then by the invitation id, which is
    /// a TOTAL order over the records: that is what makes a page token mean the
    /// same thing on the call that hands it back.
    ///
    /// One inviting account appears at most once: the key of each inbox IS the
    /// inviting account id, and a Region holds at most one detector ("you can
    /// have only one detector per account per Region", which `CreateDetector`
    /// refuses a second create under), so the walk below cannot see two inboxes
    /// of one account.
    pub fn received_invitations_in(&self, region: &str) -> Vec<&ReceivedInvitationRecord> {
        let mut invitations: Vec<&ReceivedInvitationRecord> = self
            .detectors
            .values()
            .filter(|detector| detector.region == region)
            .flat_map(|detector| detector.received_invitations.values())
            .collect();
        invitations.sort_by(|left, right| {
            left.administrator_account_id
                .cmp(&right.administrator_account_id)
                .then_with(|| left.invitation_id.cmp(&right.invitation_id))
        });
        invitations
    }

    /// Drop the invitation the inbox `region`'s endpoint serves holds from
    /// `administrator_account_id`, reporting whether there was one to drop.
    ///
    /// The inbox is resolved the way [`Self::received_invitations_in`] resolves it
    /// and for the same reason: the operations that answer an invitation by the
    /// account that sent it name no detector, so the inbox a request addresses is
    /// the one the Region it was signed for serves. An account the inbox holds
    /// nothing from is not an error here — the caller reports it as unprocessed —
    /// so this is a lookup that answers `false` and writes NOTHING.
    ///
    /// No instant is read: an invitation carries only the instant it was SENT
    /// ([`ReceivedInvitationRecord::invited_at_millis`], the one member of
    /// `Invitation` that is a time — `smithy_query(shape="Invitation")`), and the
    /// record that held it is gone, so there is nothing left for a clock to
    /// stamp.
    ///
    /// One inbox per Region, for the reason [`Self::received_invitations_in`]
    /// gives: a Region holds at most one detector, so the walk stops at the first
    /// inbox the invitation was in.
    pub fn delete_received_invitation_in(
        &mut self,
        region: &str,
        administrator_account_id: &str,
    ) -> bool {
        self.detectors
            .values_mut()
            .filter(|detector| detector.region == region)
            .find_map(|detector| {
                detector
                    .received_invitations
                    .remove(administrator_account_id)
            })
            .is_some()
    }

    /// Deliver into the inbox `region`'s endpoint serves the invitation an account
    /// that sends one through that endpoint has just sent, reporting whether this
    /// call delivered one.
    ///
    /// The MEMBER side of sending an invitation: "the invited accounts can either
    /// accept or decline the invitation from their GuardDuty accounts"
    /// (`smithy_query(query="doc",shape="InviteMembers")`), so an invitation that
    /// was sent is pending for the account it was delivered to until that account
    /// answers it — which is what [`Detector::accept_invitation`],
    /// [`Self::take_received_invitations_in`] and the inbox reads resolve. The
    /// administrator side of the same send is the member records the caller moves.
    ///
    /// One inbox per Region, resolved the way [`Self::received_invitations_in`]
    /// resolves it: the operations that answer an invitation name no detector, so
    /// the inbox a request addresses is the one the Region it was signed for
    /// serves, and an invitation delivered through one Region's endpoint is not in
    /// another's inbox.
    ///
    /// ONE invitation is pending per inviting account at a time — an acceptance
    /// identifies the invitation by that account and the id alone
    /// (`smithy_query(operation="AcceptInvitation")`) — so an account whose
    /// invitation is still pending KEEPS it: re-sending re-sends that same
    /// invitation rather than issuing a second one, exactly as the member record a
    /// re-send leaves where it was, and no id is minted for it. An account whose
    /// invitation has been accepted, declined or deleted has none pending, so a
    /// later send delivers a fresh one.
    pub fn deliver_invitation_in(
        &mut self,
        region: &str,
        administrator_account_id: &str,
        now: i64,
    ) -> bool {
        // The whole condition the delivery writes under is answered BEFORE anything
        // is minted — the Region's inbox is resolved, and
        // [`Detector::inbox_has_room_for`] answers both of the write's guards — so a
        // call that writes nothing consumes no id: the ids this state hands out are a
        // sequence, and one burned on a path that writes nothing would shift every
        // later id.
        let Some(detector) = self.inbox_in(region) else {
            return false;
        };
        if !detector.inbox_has_room_for(administrator_account_id) {
            return false;
        }
        let detector_id = detector.id.clone();
        let invitation_id = self.mint_id();
        self.detectors
            .get_mut(&detector_id)
            .is_some_and(|detector| {
                detector.deliver_invitation(administrator_account_id, &invitation_id, now)
            })
    }

    /// The inbox `region`'s endpoint serves, if that Region holds a detector at all.
    ///
    /// One inbox per Region, for the reason [`Self::received_invitations_in`] gives:
    /// the operations that deliver an invitation and the operations that answer one
    /// name no detector, and a Region holds at most one detector ("you can have only
    /// one detector per account per Region", which `CreateDetector` refuses a second
    /// create under), so the Region's one detector IS the inbox a request through its
    /// endpoint addresses.
    fn inbox_in(&self, region: &str) -> Option<&Detector> {
        self.detectors
            .values()
            .find(|detector| detector.region == region)
    }

    /// Is the invitation `administrator_account_id` sent through `region`'s endpoint
    /// still pending?
    ///
    /// The ONE question both halves of a send are decided by. Sending an invitation
    /// writes the administrator's own member records AND delivers the invitation to
    /// the inbox the Region's endpoint serves ([`Self::deliver_invitation_in`]), and
    /// an invitation stays pending only until the account it was delivered to answers
    /// it — so taking it out of that inbox, which is what an acceptance
    /// ([`Detector::accept_invitation`]), a decline and a delete
    /// ([`Self::take_received_invitations_in`]) each do, is what makes it no longer
    /// pending for the member records of that same send as much as for the inbox.
    /// Both halves therefore ask HERE, so neither can report an invitation pending
    /// that the other no longer holds.
    pub fn invitation_pending_in(&self, region: &str, administrator_account_id: &str) -> bool {
        self.inbox_in(region)
            .is_some_and(|detector| detector.has_invitation_pending_from(administrator_account_id))
    }

    /// Take out of the inbox `region`'s endpoint serves the invitation each account
    /// in `account_ids` sent, reporting the accounts the inbox held none from as the
    /// `UnprocessedAccounts` the caller answers with.
    ///
    /// The write the two operations that END a pending invitation without accepting
    /// it SHARE: an invited account "can either accept or decline the invitation"
    /// (`smithy_query(query="doc",shape="InviteMembers")`), so declining is an ANSWER
    /// and takes the invitation out of the pending inbox exactly as
    /// [`Detector::accept_invitation`] does, and deleting one discards it from the
    /// same inbox. Both leave the inbox holding nothing from that account, so neither
    /// can be followed by an acceptance, and both report per-account rather than
    /// refusing. They are one write under two names, the way `AcceptInvitation` and
    /// `AcceptAdministratorInvitation` are, so neither operation can drift from the
    /// other on which accounts it acts on or reports.
    ///
    /// `AccountIds` is a SET of accounts: one account's invitation is taken once, so
    /// a repeated id is reported at most once. The reports keep the order the request
    /// named the accounts in, so a caller can read them against its own list.
    ///
    /// Each account resolves through [`Self::delete_received_invitation_in`], which
    /// is where the Region and the writes-nothing-for-a-miss rules live.
    pub fn take_received_invitations_in(
        &mut self,
        region: &str,
        account_ids: &[String],
    ) -> Vec<crate::ops::types::shared_types::UnprocessedAccount> {
        let mut unprocessed = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for account_id in account_ids {
            if !seen.insert(account_id.as_str()) {
                continue;
            }
            if !self.delete_received_invitation_in(region, account_id) {
                // Nothing to take: this account has sent the current one no
                // invitation, or the one it sent has already been answered or
                // deleted. Reported, and nothing is persisted for it.
                unprocessed.push(crate::ops::types::shared_types::UnprocessedAccount {
                    account_id: account_id.clone(),
                    result: NO_INVITATION_FROM_ACCOUNT_MESSAGE.to_string(),
                });
            }
        }
        unprocessed
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

    /// [`Self::malware_protection_plan_for_bucket`], for a caller that writes to the
    /// plan it resolves — `SendObjectMalwareScan` recording the object scan it
    /// accepted against the plan protecting the object's bucket.
    pub fn malware_protection_plan_for_bucket_mut(
        &mut self,
        region: &str,
        bucket_name: &str,
    ) -> Option<&mut MalwareProtectionPlan> {
        self.malware_protection_plans
            .values_mut()
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

    /// The ids of the Malware Protection plans `region`'s endpoint serves, in a
    /// total order.
    ///
    /// The listing counterpart of [`Self::malware_protection_plan_in`], regional for
    /// the same reason, and the same set [`Self::malware_protection_plan_count_in`]
    /// counts — so a plan a delete removed stops being listed. The order is total,
    /// which is what makes a page token mean the same entry on the call that hands
    /// it back.
    pub fn malware_protection_plan_ids_in(&self, region: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .malware_protection_plans
            .values()
            .filter(|plan| plan.region == region)
            .map(|plan| plan.id.clone())
            .collect();
        ids.sort();
        ids
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

    /// Every malware scan `region`'s endpoint serves, in scan-id order — a total
    /// order, so a page token stays meaningful before any sort is applied.
    ///
    /// The listing counterpart of [`Self::malware_scan_in`], and regional for the
    /// same reason: a scan started through another Region's endpoint is not served
    /// here.
    pub fn malware_scans_in(&self, region: &str) -> Vec<&MalwareScan> {
        let mut scans: Vec<&MalwareScan> = self
            .malware_scans
            .values()
            .filter(|scan| scan.region == region)
            .collect();
        scans.sort_by(|a, b| a.id.cmp(&b.id));
        scans
    }

    /// Materialise the status change the CLOCK already owes EVERY scan this store
    /// holds, through [`MalwareScan::settle`].
    ///
    /// The per-scan counterpart is what `GetMalwareScan` applies to the one scan it
    /// reports; a listing reports a set, so it owes the same move to each of them.
    /// Each was owed at that scan's own `terminal_due_millis` whether or not
    /// anybody called, which is why a reporting operation may apply it.
    pub fn settle_malware_scans(&mut self) {
        let now = now_millis();
        for scan in self.malware_scans.values_mut() {
            scan.settle(now);
        }
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

    /// The association `region`'s endpoint holds for `rule_id`, if any.
    ///
    /// A rule carries at most ONE association per account, whatever mode it is in,
    /// so this is both the duplicate check a create makes and the way the rule's
    /// association is reached. Regional for the same reason a detector is: an
    /// association is made through one Region's endpoint and only that endpoint
    /// reports it.
    pub fn detection_rule_association(
        &self,
        region: &str,
        rule_id: &str,
    ) -> Option<&DetectionRuleAssociationRecord> {
        self.custom_detection_rule_associations
            .get(&(region.to_string(), rule_id.to_string()))
    }

    /// [`Self::detection_rule_association`], for an operation that is going to
    /// change the association it finds.
    pub fn detection_rule_association_mut(
        &mut self,
        region: &str,
        rule_id: &str,
    ) -> Option<&mut DetectionRuleAssociationRecord> {
        self.custom_detection_rule_associations
            .get_mut(&(region.to_string(), rule_id.to_string()))
    }

    /// The association `region`'s endpoint holds under `association_id` for
    /// `rule_id`, if any.
    ///
    /// The id is a handle onto the rule's association, so this resolves it by
    /// checking the entry that rule's key names: an id naming no association of
    /// `rule_id` resolves to nothing, exactly as one naming no association at all
    /// does.
    pub fn detection_rule_association_by_id(
        &self,
        region: &str,
        rule_id: &str,
        association_id: &str,
    ) -> Option<&DetectionRuleAssociationRecord> {
        self.detection_rule_association(region, rule_id)
            .filter(|record| record.association_id == association_id)
    }

    /// The association `region`'s endpoint made under `token`, if a create
    /// recorded one.
    ///
    /// The token is a pure REPLAY key, as it is for every other create on this
    /// service: it resolves to the association the first call made and nothing
    /// about the replaying request is compared against it, so a replay answers
    /// with that association rather than making a second one or mutating the
    /// first.
    pub fn detection_rule_association_for_client_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&DetectionRuleAssociationRecord> {
        self.custom_detection_rule_associations
            .values()
            .filter(|record| record.region == region)
            .find(|record| record.client_token.as_deref() == Some(token))
    }

    /// The organization configuration `region`'s endpoint holds for
    /// `(rule_id, mode)`, if any.
    pub fn detection_rule_org_configuration(
        &self,
        region: &str,
        rule_id: &str,
        mode: &str,
    ) -> Option<&DetectionRuleOrgConfigurationRecord> {
        self.custom_detection_rule_org_configurations.get(&(
            region.to_string(),
            rule_id.to_string(),
            mode.to_string(),
        ))
    }

    /// The `AssociationMode`s `region`'s endpoint holds an organization
    /// configuration for `rule_id` in, in mode order.
    ///
    /// `UpdateCustomDetectionRuleOrgConfiguration` identifies its configuration by
    /// `RuleId` ALONE — the operation binds `RuleId` as the URI label and `Mode` to
    /// the body, where the read and the delete bind `Mode` as a query parameter
    /// (`smithy_query(operation="UpdateCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration")`)
    /// — so it needs the modes a rule is configured in rather than one
    /// `(rule_id, mode)` lookup. A Region holds at most one configuration per
    /// published rule per mode, so this scans a table of fixed,
    /// single-digit-times-two length, exactly as
    /// [`Self::detection_rule_association_by_id`] does.
    pub fn detection_rule_org_configuration_modes(
        &self,
        region: &str,
        rule_id: &str,
    ) -> Vec<String> {
        self.custom_detection_rule_org_configurations
            .values()
            .filter(|record| record.region == region && record.rule_id == rule_id)
            .map(|record| record.mode.clone())
            .collect()
    }

    /// The organization configuration `region`'s endpoint created under `token`,
    /// if a create recorded one. A pure REPLAY key, exactly as
    /// [`Self::detection_rule_association_for_client_token`] is.
    pub fn detection_rule_org_configuration_for_client_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&DetectionRuleOrgConfigurationRecord> {
        self.custom_detection_rule_org_configurations
            .values()
            .filter(|record| record.region == region)
            .find(|record| record.client_token.as_deref() == Some(token))
    }

    /// Would a custom-detection-rule write through `region`'s endpoint partition
    /// the association and organization-configuration stores into one Region more
    /// than they will hold ([`MAX_DETECTION_RULE_REGIONS`])?
    ///
    /// The Region is request-chosen ([`request_region`]), so this is the bound on
    /// the number of PARTITIONS that makes the per-Region ceiling — one association
    /// per published rule, and one organization configuration per published rule
    /// per `AssociationMode` member — a bound on the whole store. A
    /// Region the store already holds a record for is never refused by it, so the
    /// bound is invisible to a caller using the Regions AWS runs. Counted over
    /// BOTH maps, because both are keyed by the same request-chosen Region.
    pub fn detection_rule_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for record in self.custom_detection_rule_associations.values() {
            regions.insert(record.region.as_str());
        }
        for record in self.custom_detection_rule_org_configurations.values() {
            regions.insert(record.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_DETECTION_RULE_REGIONS
    }

    /// The delegated administrator `region`'s endpoint holds, if any.
    ///
    /// A designation is made through one Region's endpoint, so a designation made
    /// through another Region's is not visible here — the same rule
    /// [`Self::detector_in`] applies to a detector.
    pub fn organization_admin_account_in(
        &self,
        region: &str,
    ) -> Option<&OrganizationAdminAccountRecord> {
        self.organization_admin_accounts.get(region)
    }

    /// Designate `admin_account_id` as the delegated administrator `region`'s
    /// endpoint serves, replacing nothing: the caller has already established that
    /// the Region holds no administrator.
    pub fn designate_organization_admin_account(&mut self, region: &str, admin_account_id: &str) {
        self.organization_admin_accounts.insert(
            region.to_string(),
            OrganizationAdminAccountRecord {
                admin_account_id: admin_account_id.to_string(),
                disable_due_millis: None,
            },
        );
    }

    /// Accept the removal of the delegated administrator `region`'s endpoint holds,
    /// through [`OrganizationAdminAccountRecord::begin_disable`].
    ///
    /// `None` when that endpoint holds no delegated administrator at all;
    /// `Some(false)` when the one it holds is a different account than
    /// `admin_account_id`, which is not a removal this call may make; `Some(true)`
    /// when the removal was accepted, whether this call is the one that accepted it
    /// or a repeat of one already in flight. The mutation clock is read only on the
    /// path that writes, so neither refusal advances it and neither does that
    /// repeat — the discipline
    /// [`Self::disassociate_detector_from_administrator`] keeps too.
    pub fn disable_organization_admin_account(
        &mut self,
        region: &str,
        admin_account_id: &str,
    ) -> Option<bool> {
        let record = self.organization_admin_account_in(region)?;
        if record.admin_account_id != admin_account_id {
            return Some(false);
        }
        // Asking again for a removal still in flight asks for the state the record is
        // already transitioning to: the instant the first request made due stands
        // ([`OrganizationAdminAccountRecord::begin_disable`]), so such a call is
        // accepted and writes nothing at all. Tested BEFORE the clock is read, exactly
        // as [`Self::disassociate_detector_from_administrator`] tests the relationship
        // it drops, so a repeat consumes no instant of the "this resource changed now"
        // counter either.
        if record.disable_due_millis.is_some() {
            return Some(true);
        }
        let now = self.tick_millis();
        if let Some(record) = self.organization_admin_accounts.get_mut(region) {
            record.begin_disable(now);
        }
        Some(true)
    }

    /// Turn around the removal still in flight for `admin_account_id`, the
    /// delegated administrator `region`'s endpoint holds, through
    /// [`OrganizationAdminAccountRecord::cancel_disable`].
    ///
    /// `true` when a pending removal was turned around, `false` when that
    /// endpoint holds no administrator, holds a DIFFERENT one — a designation of
    /// another account is a second administrator rather than a reversal, so this
    /// never replaces the account the record holds — or holds this one with no
    /// removal pending, in which case nothing was owed and nothing changed,
    /// exactly as a repeated request leaves [`EntitySetRecord::set_activation`]
    /// where it is. The account is checked HERE, where the record is reached, so
    /// the reversal cannot be applied to a record the request does not name.
    pub fn redesignate_organization_admin_account(
        &mut self,
        region: &str,
        admin_account_id: &str,
    ) -> bool {
        self.organization_admin_accounts
            .get_mut(region)
            .is_some_and(|record| {
                record.admin_account_id == admin_account_id && record.cancel_disable()
            })
    }

    /// Materialise the removal the CLOCK already owes every delegated
    /// administrator whose disable has fallen due.
    ///
    /// Each removal was owed at that record's own
    /// [`OrganizationAdminAccountRecord::disable_due_millis`] whether or not
    /// anybody called, which is why a reporting operation may apply it — the same
    /// reason [`Self::settle_malware_scans`] may.
    pub fn settle_organization_admin_accounts(&mut self) {
        let now = now_millis();
        self.organization_admin_accounts
            .retain(|_, record| !record.disable_settled(now));
    }

    /// Would a designation through `region`'s endpoint partition this store into
    /// one Region more than it will hold ([`MAX_ORGANIZATION_ADMIN_REGIONS`])?
    ///
    /// The Region is request-chosen ([`request_region`]), so the per-Region ceiling
    /// of one record needs a ceiling on the number of PARTITIONS to bound the whole
    /// map — exactly as [`Self::detection_rule_regions_exhausted`] does for the
    /// rule stores. A Region the store already holds a record for is never refused
    /// by it.
    pub fn organization_admin_regions_exhausted(&self, region: &str) -> bool {
        !self.organization_admin_accounts.contains_key(region)
            && self.organization_admin_accounts.len() >= MAX_ORGANIZATION_ADMIN_REGIONS
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

/// One instant of an SDK timestamp in epoch milliseconds, the form this service holds
/// every instant in — the ONE conversion every instant, reported or received, goes
/// through.
///
/// It SATURATES at the representable bound rather than failing, because the bound is
/// this store's and not the API's: a modelled `Timestamp` carries no `@range`, so an
/// instant whose milliseconds do not fit an `i64` is input the service must accept.
/// Saturating keeps the instant — and so the member that carries it — present and in
/// time order; a fallible conversion would turn a value the caller sent into no value
/// at all.
pub fn millis_of(instant: &::aws_smithy_types::DateTime) -> i64 {
    instant
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(instant.subsec_nanos() / 1_000_000))
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
/// it exists only for compile-time checks like the one above — a handler that names a
/// modelled enum member in a constant asserts it the same way, so the BUILD fails if
/// the model stops carrying that member.
pub const fn enum_carries(values: &[&str], value: &str) -> bool {
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
/// Runtime Monitoring is the ONE exception `CreateDetector` documents: "When you
/// don't specify any features, with an exception to RUNTIME_MONITORING, all the
/// optional features are enabled by default"
/// (`smithy_query(query="doc",shape="CreateDetector")`). Every other configurable
/// feature — RDS Protection included — is therefore enabled on a detector created
/// without a `Features` list, and reports so under both views of its switch.
pub const OPT_IN_FEATURES: [&str; 1] = [FEATURE_RUNTIME_MONITORING];

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
    arn_fields(arn).is_some_and(|[_, partition, arn_service, _, _, resource]| {
        !partition.is_empty() && arn_service == service && !resource.is_empty()
    })
}

/// The six fields of `arn` — `arn`, partition, service, region, account id and
/// resource — or `None` unless it has all six and opens with `arn`.
///
/// The ONE definition of how an ARN splits in this crate: [`is_arn_for`] judges
/// the fields and [`parse_tagged_resource_arn`] reads them, so the two cannot
/// disagree about what a well-formed ARN is. The resource keeps any further `:`
/// it carries.
fn arn_fields(arn: &str) -> Option<[&str; 6]> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" {
        return None;
    }
    Some([prefix, partition, service, region, account_id, resource])
}

// ---------------------------------------------------------------------------
// Client rejections
//
// GuardDuty classifies a rejection by the REQUEST, not by what the store holds,
// and no message quotes the caller's value or the endpoint's Region back at
// them. Which SENTENCE it answers with belongs to the resource family the
// identifier addresses, and to the class of operation that was asked:
//
//   * the DETECTOR family names the member at fault — a value that is not a
//     well-formed detector id is a parameter-validation failure, a well-formed
//     one this account/Region does not hold an ownership failure;
//   * every family rooted IN a detector answers its own fixed absent-resource
//     sentence instead, which names no member at all and makes no claim about
//     the account ([`NO_SUCH_RESOURCE_MESSAGE`] and the family constants beside
//     it).
//
// The messages are built here, once, so every operation of a family rejects the
// same input the same way.
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
/// `destinationId` JSON name / URI label). The family's own absence refusals name no
/// member ([`DESTINATION_NOT_FOUND_MESSAGE`], [`DESTINATION_ABSENT_MESSAGE`]).
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

/// The wire name of the TOP-LEVEL `ExpectedBucketOwner` member the IPSet, threat
/// list and entity set creates and updates carry
/// (`smithy_query(operation="CreateIPSet")` renders it `expectedBucketOwner`), as
/// distinct from the one nested in `DestinationProperties`
/// ([`EXPECTED_BUCKET_OWNER_MEMBER`]): the same fact, named at a different place in
/// the request.
pub const LIST_EXPECTED_BUCKET_OWNER_MEMBER: &str = "expectedBucketOwner";

/// The wire name of the `ThreatIntelSetId` member, as the request carries it in the
/// URI (`smithy_query(operation="GetThreatIntelSet")` renders it
/// `threatIntelSetId`). The family's own absence refusal names no member
/// ([`SET_ID_NOT_FOUND_MESSAGE`]).
pub const THREAT_INTEL_SET_ID_MEMBER: &str = "threatIntelSetId";

/// The name GuardDuty gives the threat-list `Location` member in a rejection of
/// it: the threat-list counterpart of [`IP_SET_LOCATION_MEMBER`], qualified by
/// its own resource the way [`THREAT_INTEL_SET_ID_MEMBER`] is.
pub const THREAT_INTEL_SET_LOCATION_MEMBER: &str = "threatIntelSetLocation";

/// The wire name of the `ScanResourceCriteria` member of
/// `UpdateMalwareScanSettings` (`jsonName` `scanResourceCriteria`), for the same
/// reason.
pub const SCAN_RESOURCE_CRITERIA_MEMBER: &str = "scanResourceCriteria";

/// The wire name of the account-id member every members operation carries:
/// `AccountIds`' entries target `AccountId`
/// (`smithy_query(operation="DisassociateMembers")`), and `AccountDetail.AccountId`
/// renders as `accountId` (`smithy_query(shape="AccountDetail")`).
pub const ACCOUNT_ID_MEMBER: &str = "accountId";

/// The wire name of the `AccountIds` list every batch members operation carries the
/// account ids in (`smithy_query(operation="DisassociateMembers")` renders it
/// `accountIds`), including the one `GetUsageStatistics` nests inside its criteria
/// (`smithy_query(query="shape",shape="UsageCriteria")`).
pub const ACCOUNT_IDS_MEMBER: &str = "accountIds";

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
/// which is the ownership failure and not a parameter one. One reader for every
/// member of that form, so the form cannot drift between them.
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

/// The message GuardDuty refuses a STRUCTURED member it will not accept with: the
/// member the request carried named, and nothing from inside the structure echoed.
///
/// A criteria structure is unacceptable as a WHOLE — the service does not report
/// which key inside it was at fault, nor repeat the key the caller sent — so the
/// refusal names the member the request carried and stops there.
pub fn invalid_parameter_value_message(member: &str) -> String {
    format!("The request was rejected because the parameter {member} has an invalid value.")
}

/// The message GuardDuty refuses a criteria structure whose CONDITION VALUES it
/// will not take with: the member the request carried named, the fault located in a
/// condition, and nothing from inside the structure echoed.
///
/// It is a different sentence from [`invalid_parameter_value_message`] because it
/// is a different answer: the member's own shape is one the service takes, and what
/// it will not take is a value a condition inside it carries.
pub fn invalid_condition_values_message(member: &str) -> String {
    format!("The request was rejected because the parameter {member} has invalid values in a condition.")
}

/// The message GuardDuty refuses an unusable input value with where it does NOT
/// name the member at fault: its generic parameter-validation wording.
///
/// Which of the two a member gets is the service's own choice, not ours: the
/// corpus records this wording for `GetMalwareScan`'s `scanId` and the
/// member-naming [`invalid_parameter_message`] for `detectorId`, so each member
/// keeps the message the service answers for it.
pub const INVALID_INPUT_VALUE_MESSAGE: &str =
    "The request is rejected because an invalid or out-of-range value is specified as an input parameter.";

/// The message for a member the request left out that the service cannot act
/// without.
pub fn missing_parameter_message(member: &str) -> String {
    format!("The request is rejected because the parameter {member} is required.")
}

/// The message for a well-formed id that this account does not own — an id
/// another account created, or one a delete removed. GuardDuty does not
/// distinguish "never existed" from "belongs to someone else": both are the same
/// refusal, which is also what keeps one account from probing another's ids.
///
/// It is the DETECTOR family's refusal and makes a claim about the ACCOUNT, so it
/// is not a generic not-found text: a family whose own refusal says only that
/// nothing of that name is there answers with that family's sentence instead —
/// [`NO_SUCH_RESOURCE_MESSAGE`], [`INVALID_FILTER_NAME_MESSAGE`],
/// [`MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE`], [`INVALID_FINDING_IDS_MESSAGE`]
/// or [`INVALID_INPUT_PARAMETERS_MESSAGE`].
pub fn not_owned_message(member: &str) -> String {
    format!(
        "The request is rejected because the input {member} is not owned by the current account."
    )
}

/// The sentence GuardDuty refuses an identifier that resolves to no resource with
/// on the families whose answer is that nothing of that name is there: ONE fixed
/// text for the whole class, naming neither the member that carried the identifier
/// nor the value itself.
///
/// The detector-rooted uploaded lists — an IPSet and a threat list, read, updated
/// or deleted — and a filter READ answer with this one, whatever the FORM of the
/// identifier they were handed: a value that was never minted, one minted under
/// another detector and one that could never have been minted at all are the same
/// absence, so the refusal depends on neither the store nor the spelling. It is not
/// [`not_owned_message`], which claims the id belongs to another ACCOUNT, and not
/// [`invalid_parameter_message`], which claims the value could not be an id at all.
pub const NO_SUCH_RESOURCE_MESSAGE: &str = "The request is rejected since no such resource found.";

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
/// [`NOT_AN_ASSOCIATED_MEMBER_MESSAGE`]).
pub fn is_account_id(id: &str) -> bool {
    id.len() == ACCOUNT_ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

/// The message a value that cannot be an account id at all is refused with where the
/// request carried it in a STRUCTURE member of its own rather than in a list of ids:
/// the member name in single quotes, the format class stated, and nothing of the
/// caller's value echoed.
///
/// A malformed id is a FORMAT failure, which GuardDuty words as its own class rather
/// than as the generic unacceptable-value refusal
/// ([`invalid_parameter_message`]): a caller can tell a syntactically impossible
/// account id from one that was understood and rejected on its merits.
pub const ACCOUNT_ID_FORMAT_MESSAGE: &str =
    "The request failed because the 'accountId' was formatted incorrectly.";

/// The same format failure where the request carried the account ids as a LIST:
/// GuardDuty names the list `member` the caller sent and the `accountId` values
/// inside it, so the refusal says which parameter of a batch call it objected to.
///
/// `member` is the list's wire name as the request spells it
/// ([`ACCOUNT_IDS_MEMBER`] for the members operations, the include/exclude list for
/// an organization scope), so one sentence serves every batch that carries account
/// ids.
pub fn account_ids_format_message(member: &str) -> String {
    format!(
        "The request failed because parameter {member} has accountId values that are formatted incorrectly."
    )
}

/// Classify a LIST of account ids the caller named: `Some(message)` when an entry
/// cannot be an account id at all, `None` when every entry is well formed.
///
/// One reader for every operation that takes account ids, so the form cannot drift
/// between them, and one message — [`account_ids_format_message`] for the list
/// `member` the request carried — because a batch refusal names the list it was
/// handed. The whole call is refused rather than the entry reported:
/// `UnprocessedAccounts` reports accounts that were not PROCESSED, never a request
/// that was not understood.
///
/// The ids argument is any iterator of string-like entries, so an operation whose
/// scope is carried by TWO lists (an include list and an exclude list, which together
/// name one scope) chains them into one call; `member` then names the list the
/// request actually sent.
pub fn account_ids_rejection<I>(member: &str, account_ids: I) -> Option<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    account_ids
        .into_iter()
        .any(|account_id| !is_account_id(account_id.as_ref()))
        .then(|| account_ids_format_message(member))
}

/// Classify a SINGLE account id a structure member carried: `Some(message)` when it
/// cannot be an account id at all, `None` when it is well formed.
///
/// The same form question as [`account_ids_rejection`] under the sentence GuardDuty
/// answers when the id is a member of its own ([`ACCOUNT_ID_FORMAT_MESSAGE`]) rather
/// than an element of a list the request named.
pub fn account_id_rejection(account_id: &str) -> Option<&'static str> {
    (!is_account_id(account_id)).then_some(ACCOUNT_ID_FORMAT_MESSAGE)
}

/// Classify the top-level `ExpectedBucketOwner` a request carried: `Some(message)`
/// when the value supplied cannot be an account id at all, `None` when the request
/// omitted the member or supplied a well-formed id.
///
/// The member is "the Amazon Web Services account ID that owns the Amazon S3 bucket
/// specified in the location parameter"
/// (`smithy_query(query="doc",shape="CreateIPSetRequest",member="ExpectedBucketOwner")`),
/// so its value space is an account id's ([`is_account_id`]) and nothing wider.
/// Where the member targets `AccountId` the model constrains the LENGTH only, so a
/// 12-character value that is not an account id reaches the handler and the handler
/// is the enforcement point; where it targets a shape the model gives a
/// digits-only pattern the boundary refuses the same values first. One reader for
/// every operation that carries the member, so the form cannot drift between them,
/// and one message — [`invalid_parameter_message`] for
/// [`LIST_EXPECTED_BUCKET_OWNER_MEMBER`] — because the member at fault is the same
/// one whichever operation carried it.
pub fn expected_bucket_owner_rejection(owner: Option<&str>) -> Option<String> {
    owner
        .is_some_and(|owner| !is_account_id(owner))
        .then(|| invalid_parameter_message(LIST_EXPECTED_BUCKET_OWNER_MEMBER))
}

/// The account that owns the S3 bucket an ENTITY SET's location names: the one the
/// request supplied, or the account that owns the location when it supplied none.
///
/// `GetThreatEntitySet` and `GetTrustedEntitySet` report `ExpectedBucketOwner` as
/// "the Amazon Web Services account ID that owns the Amazon S3 bucket specified in
/// the location parameter"
/// (`smithy_query(query="doc",shape="GetThreatEntitySetResponse",member="ExpectedBucketOwner")`)
/// with no qualification on how the set was created — unlike `GetIPSet` and
/// `GetThreatIntelSet`, whose own documentation for the same member adds "this
/// field appears in the response only if it was provided during … creation or
/// update". So an entity set always KNOWS its bucket owner: a request that names
/// none is uploading from a bucket of its own account, which is the single account
/// this emulator serves ([`administrator_account_id`]), and that is what a read
/// reports and an update round-trips.
pub fn entity_set_bucket_owner(requested: Option<&str>) -> Option<String> {
    Some(
        requested
            .map(str::to_string)
            .unwrap_or_else(administrator_account_id),
    )
}

/// The DISTINCT entries of an id list a request carried, in the order the request
/// first named each of them.
///
/// An id list on this service names a SET of resources, so an id named twice names
/// the same one thing: the operations that answer such a list resolve each distinct
/// id once, and an operation that STORES the list normalises it here so a read of
/// the stored value reports each id once too. The request's order is kept, because
/// it is the order the stored list is reported in.
pub fn distinct_ids<I>(ids: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut seen = ::std::collections::BTreeSet::new();
    ids.into_iter()
        .map(|id| id.as_ref().to_string())
        .filter(|id| seen.insert(id.clone()))
        .collect()
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
/// (`smithy_query(shape="UnprocessedAccount")`) — for an account id that is not an
/// ASSOCIATED member of the administrator: one it holds no member record for, and
/// one whose record is not associated ([`MemberRecord::is_associated`]).
///
/// A per-account failure is reported here rather than refusing the whole request:
/// the operations that take an `AccountIds` list answer 200 and report the accounts
/// they could not process, which is what `UnprocessedAccounts` is for.
///
/// The administrator is named as "the current account", never by its account
/// number: the reason says WHICH account the membership is judged against, and the
/// account a request is signed for is the one it is answered for, so the sentence
/// is the same whatever account id this emulator serves.
pub const NOT_AN_ASSOCIATED_MEMBER_MESSAGE: &str = "The request is rejected because the given account ID is not an associated member of the current account.";

/// `UnprocessedAccount.Result` for an account `DeleteMembers` dropped no member
/// record for.
///
/// Deleting is what "remove[s] the details associated with a member account" that a
/// disassociation RETAINS (`smithy_query(query="doc",shape="DisassociateMembers")`),
/// so the record it does not drop is the one of a member GuardDuty MONITORS under
/// the administrator ([`MemberRecord::is_monitored`]): that member has to be
/// disassociated first, which is what this reason states. GuardDuty answers the SAME
/// reason for an account the administrator holds no member record for at all — there
/// was no member record to drop in either case — so it is the one reason for the
/// whole class, and it carries no account id.
pub const MEMBER_NOT_DELETED_MESSAGE: &str = "The request is rejected because the current account cannot delete the given member account ID since it is still associated to it.";

/// `UnprocessedAccount.Result` for an account `DisassociateMembers` was asked to
/// disassociate that is not ASSOCIATED with the administrator — one it holds no
/// member record for, one it has recorded and not invited yet, and one whose
/// relationship a disassociation already moved.
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

/// `UnprocessedAccount.Result` for an account id the current account's inbox holds
/// no invitation from.
///
/// The operations that answer an invitation by the account that SENT it name the
/// invitation by that account alone, so an account that has not invited this one
/// identifies nothing to answer. It is reported rather than refusing the whole
/// request, like every other per-account failure this service reports: the result
/// states the condition, which is what "a reason why the account hasn't been
/// processed" (`smithy_query(shape="UnprocessedAccount")`) is for.
pub const NO_INVITATION_FROM_ACCOUNT_MESSAGE: &str = "The request is rejected because the current account has not received an invitation from the given account ID.";

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

/// The relationship status of an invitation that has been delivered and not yet
/// answered: `Member.RelationshipStatus` once `InviteMembers` has sent the account
/// an invitation, and `Invitation.RelationshipStatus` on the invitation itself. See
/// [`RELATIONSHIP_CREATED`] for where the values come from.
pub const RELATIONSHIP_INVITED: &str = "Invited";

/// The relationship status of a member account that is being MONITORED:
/// `Master.RelationshipStatus` once the member account has accepted the
/// invitation, and `Member.RelationshipStatus` once `StartMonitoringMembers` has
/// turned monitoring back on for an account `StopMonitoringMembers` had stopped —
/// "use this operation to restart monitoring of accounts that you stopped
/// monitoring with the StopMonitoringMembers operation"
/// (`smithy_query(query="doc",shape="StartMonitoringMembers")`). See
/// [`RELATIONSHIP_CREATED`] for where the values come from.
pub const RELATIONSHIP_ENABLED: &str = "Enabled";

/// `Member.RelationshipStatus` for a member account the administrator has stopped
/// monitoring with `StopMonitoringMembers` — "stops GuardDuty monitoring for the
/// specified member accounts. Use the StartMonitoringMembers operation to restart
/// monitoring for those accounts"
/// (`smithy_query(query="doc",shape="StopMonitoringMembers")`). The member stays
/// ASSOCIATED ([`MemberRecord::is_associated`]): only `DisassociateMembers` ends
/// the association and only `DeleteMembers` drops the record. See
/// [`RELATIONSHIP_CREATED`] for where the values come from.
pub const RELATIONSHIP_DISABLED: &str = "Disabled";

/// How many received invitations one detector's inbox retains.
///
/// A bound on [`Detector::received_invitations`], whose key is an ACCOUNT ID and
/// so request-chosen, exactly as [`MAX_MEMBERS_PER_DETECTOR`] bounds the
/// request-keyed member store. GuardDuty allows one administrator per member
/// account, so a member account has at most a handful of competing invitations
/// pending at once.
pub const MAX_INVITATIONS_PER_DETECTOR: usize = 50;

/// The wire name of the `MasterId` member — `jsonName` `masterId`
/// (`smithy_query(operation="AcceptInvitation")`). It is what a rejection names.
pub const MASTER_ID_MEMBER: &str = "masterId";

/// The wire name of the `InvitationId` member — `jsonName` `invitationId`
/// (`smithy_query(operation="AcceptInvitation")`).
pub const INVITATION_ID_MEMBER: &str = "invitationId";

/// The wire name of the `AdministratorId` member — `jsonName` `administratorId`
/// (`smithy_query(operation="AcceptAdministratorInvitation")`). The current name of
/// the member [`MASTER_ID_MEMBER`] carries on the deprecated twin, so a rejection
/// names whichever one the request used.
pub const ADMINISTRATOR_ID_MEMBER: &str = "administratorId";

/// The message an acceptance naming no pending invitation is refused with, naming
/// the member that carried the administrator's account id.
///
/// It names the two members that together identify the invitation and states the
/// rule they broke, without quoting either value back — the wording every
/// rejection on this service follows.
pub fn invitation_not_pending_message_for(administrator_member: &str) -> String {
    format!(
        "The request is rejected because the input {administrator_member} and {INVITATION_ID_MEMBER} do not identify an invitation pending for the current account."
    )
}

/// [`invitation_not_pending_message_for`] for the deprecated `AcceptInvitation`,
/// whose administrator member is [`MASTER_ID_MEMBER`].
pub fn invitation_not_pending_message() -> String {
    invitation_not_pending_message_for(MASTER_ID_MEMBER)
}

/// The message the MEMBER-side disassociation is refused with when the account
/// that owns the named detector is monitored by no administrator.
///
/// `DisassociateFromAdministratorAccount` "disassociates the current GuardDuty
/// member account from its administrator account"
/// (`smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")`): it
/// acts on the one relationship [`Detector::administrator`] holds, so an account
/// that has accepted no invitation has nothing for it to act on. The only
/// client-error channel the operation models is `BadRequestException`/400 and its
/// output carries no per-account result the way `DisassociateMembers`'
/// `UnprocessedAccounts` does, so the absence is reported as the refusal rather
/// than as a hollow success. HTTP 400 under the `InvalidInputException` class is
/// the requirement here; this wording follows the refusal style every other
/// rejection on this service uses — it states the rule and quotes nothing back.
pub const NO_ADMINISTRATOR_MESSAGE: &str =
    "The request is rejected because the current account is not associated with an administrator account.";

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

/// The message a publishing-destination READ refuses a `DestinationId` it cannot
/// resolve with: `DescribePublishingDestination` reports a destination id that names
/// nothing as an unusable input PARAMETER ([`INVALID_INPUT_PARAMETERS_MESSAGE`])
/// rather than as an absent resource or a foreign one, and interpolates nothing — so
/// the one sentence answers an id a delete removed and one that could never have been
/// minted alike.
pub const DESTINATION_NOT_FOUND_MESSAGE: &str = INVALID_INPUT_PARAMETERS_MESSAGE;

/// The message the MUTATING publishing-destination paths refuse a `DestinationId`
/// they cannot resolve with: a write says the destination is not there, where the
/// read reports a bad parameter ([`DESTINATION_NOT_FOUND_MESSAGE`]) — the same
/// read-versus-write split the filter family makes
/// ([`NO_SUCH_RESOURCE_MESSAGE`] against [`INVALID_FILTER_NAME_MESSAGE`]).
///
/// Fixed text: the sentence names the parameter in AWS's own words for this refusal,
/// which is not the request's wire member name, and nothing of the caller's is
/// interpolated into it.
pub const DESTINATION_ABSENT_MESSAGE: &str =
    "The request failed because no publishingDestination exists with the provided publishingDestinationId.";

/// The wire name of the `FindingIds` member — `jsonName` `findingIds` on every
/// operation that addresses findings by id
/// (`smithy_query(operation="ArchiveFindings")`). It is what a rejection names.
pub const FINDING_IDS_MEMBER: &str = "findingIds";

/// The sentence a findings write is refused with when the batch names a finding
/// the detector does not hold — AWS's own wording for that class, its disagreement
/// in number included, and with none of the ids quoted back.
pub const INVALID_FINDING_IDS_MESSAGE: &str =
    "The request is rejected because an invalid finding identifiers is specified.";

/// Classify a finding id the detector does not hold: a finding `Id` is one of the
/// ids this service mints ([`is_minted_id`]), so a well-formed one the detector does
/// not hold is the finding-identifier refusal ([`INVALID_FINDING_IDS_MESSAGE`]),
/// while a value that is not of that form at all is a parameter failure — a value
/// the request could not have meant as a finding id, which is a different class
/// from a finding that is simply not this detector's.
pub fn finding_id_rejection(finding_id: &str) -> String {
    if is_minted_id(finding_id) {
        INVALID_FINDING_IDS_MESSAGE.to_string()
    } else {
        invalid_parameter_message(FINDING_IDS_MEMBER)
    }
}

/// The wire name of the `MalwareProtectionPlanId` member, as the request carries
/// it (`smithy_query(operation="GetMalwareProtectionPlan")` binds it as the URI
/// label `MalwareProtectionPlanId` and the create reports it as
/// `malwareProtectionPlanId`). It is what a rejection names.
pub const MALWARE_PROTECTION_PLAN_ID_MEMBER: &str = "malwareProtectionPlanId";

/// The wire name of `CreateMalwareProtectionPlan`'s `Role` member
/// (`smithy_query(operation="CreateMalwareProtectionPlan")` renders it `role`).
/// `UpdateMalwareProtectionPlan` carries the same member under the same wire name.
pub const ROLE_MEMBER: &str = "role";

/// The wire path of the `Role` a malware-scan start carries inside its
/// `ScanConfiguration` (`smithy_query(shape="StartMalwareScanConfiguration")`
/// renders the member `role`, under the request's `scanConfiguration`). Dotted the
/// way this service names a nested member in a rejection, as
/// [`DESTINATION_ARN_MEMBER`] is.
pub const SCAN_CONFIGURATION_ROLE_MEMBER: &str = "scanConfiguration.role";

/// The wire path of the `BackupVaultName` a malware-scan start carries inside its
/// `ScanConfiguration.RecoveryPoint` (`smithy_query(shape="RecoveryPoint")` renders
/// the member `backupVaultName`, under the request's
/// `scanConfiguration.recoveryPoint`). Dotted the way this service names a nested
/// member in a rejection, as [`SCAN_CONFIGURATION_ROLE_MEMBER`] is.
pub const SCAN_CONFIGURATION_RECOVERY_POINT_VAULT_MEMBER: &str =
    "scanConfiguration.recoveryPoint.backupVaultName";

/// The length bounds of `NonEmptyString` (`smithy_query(shape="NonEmptyString")`:
/// len 1..200) — the type a malware scan's configuration is REPORTED through
/// (`ScanConfigurationRecoveryPoint$BackupVaultName`,
/// `ScanConfiguration$Role`, `IncrementalScanDetails$BaselineResourceArn`).
pub const NON_EMPTY_STRING_MIN_LEN: usize = 1;
/// See [`NON_EMPTY_STRING_MIN_LEN`].
pub const NON_EMPTY_STRING_MAX_LEN: usize = 200;

/// Can `value` be carried by a `NonEmptyString` member — is its length within
/// [`NON_EMPTY_STRING_MIN_LEN`]..=[`NON_EMPTY_STRING_MAX_LEN`]?
///
/// A length bound counts Unicode code points, not bytes, so a multi-byte name is
/// measured the way the model measures it — the same counting
/// [`is_filter_name`] applies to the filter-name bounds. ONE reader for every
/// member of that type, so the bound cannot drift between the paths that must
/// honour it: where a request member the service persists is modelled LESS
/// constrained than the member a read reports the same fact through, the handler
/// is the only place the reporting member's bound can be honoured.
pub fn fits_non_empty_string(value: &str) -> bool {
    (NON_EMPTY_STRING_MIN_LEN..=NON_EMPTY_STRING_MAX_LEN).contains(&value.chars().count())
}

/// The wire name of the `ProtectedResource` member, for the same reason.
pub const PROTECTED_RESOURCE_MEMBER: &str = "protectedResource";

/// The wire name of `SendObjectMalwareScan`'s `S3Object` member
/// (`smithy_query(operation="SendObjectMalwareScan")` renders it `s3Object`), for a
/// rejection to name.
pub const S3_OBJECT_MEMBER: &str = "s3Object";

/// The wire name of that member's `Bucket`: "The name of the S3 bucket containing
/// the object to scan" (`smithy_query(shape="S3ObjectForSendObjectMalwareScan")`).
/// Dotted the way this service names a nested member in a rejection, as
/// [`DESTINATION_ARN_MEMBER`] is.
pub const S3_OBJECT_BUCKET_MEMBER: &str = "s3Object.bucket";

/// The wire name of that member's `Key`: "The key (name) of the S3 object to scan
/// for malware" (`smithy_query(shape="S3ObjectForSendObjectMalwareScan")`).
pub const S3_OBJECT_KEY_MEMBER: &str = "s3Object.key";

/// The message an object scan is refused with when the Region's endpoint holds no
/// Malware Protection plan for the bucket the request names.
///
/// "The bucket must have a Malware Protection plan configured to use this API"
/// (`smithy_query(query="doc",shape="SendObjectMalwareScanRequest",member="S3Object")`),
/// so a bucket with no plan has nothing to scan the object with. Built from the
/// REQUEST — the condition the caller broke — and naming neither the bucket nor the
/// Region, like every other rejection this service answers.
pub const OBJECT_MALWARE_SCAN_WITHOUT_PLAN_MESSAGE: &str =
    "The request is rejected because the S3 bucket does not have a Malware Protection plan configured.";

/// The message an object scan is refused with when the bucket's plan limits its
/// scanning to object prefixes and the key the request names is under none of
/// them.
///
/// "The S3 object will be scanned only if it belongs to any of the specified
/// object prefixes"
/// (`smithy_query(query="doc",shape="CreateS3BucketResource",member="ObjectPrefixes")`),
/// so an object outside the plan's prefixes is not an object that plan scans.
/// Built from the REQUEST — the condition the caller broke — and, like
/// [`OBJECT_MALWARE_SCAN_WITHOUT_PLAN_MESSAGE`], naming neither the key nor the
/// prefixes the plan holds.
pub const OBJECT_MALWARE_SCAN_OUTSIDE_PREFIXES_MESSAGE: &str =
    "The request is rejected because the S3 object key does not belong to any of the object prefixes configured in the Malware Protection plan.";

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
/// same bucket conflicts with the first. AWS words this refusal in terms of the
/// RESOURCE the plan protects — the S3 bucket — in one fixed sentence; the
/// caller's bucket name is not quoted back.
pub const MALWARE_PROTECTION_PLAN_CONFLICT_MESSAGE: &str =
    "The request was rejected because the provided S3 bucket is already protected.";

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

/// The sentence a plan this account and Region does not hold is refused with.
///
/// "Never existed" and "belongs to someone else" are ONE refusal, with one fixed
/// text and the plan id not quoted back, so one account cannot probe another's plan
/// ids — and the text says the plan was not found rather than that the caller does
/// not own it ([`not_owned_message`]), which would disclose the very distinction the
/// single wording hides.
pub const MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE: &str =
    "The request was rejected because the specified malware protection plan was not found.";

/// The message a read of a plan this account and Region does not hold answers
/// with: [`MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE`], owned by this one reader so
/// every plan operation refuses an unresolvable id the same way.
pub fn malware_protection_plan_not_found_message() -> String {
    MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE.to_string()
}

/// Is `value` an IAM role ARN?
///
/// `Role` is the "Amazon Resource Name (ARN) of the IAM role that has the
/// permissions to scan and add tags to the associated protected resource"
/// (`smithy_query(operation="CreateMalwareProtectionPlan")`), and the model
/// constrains it no further than `String`, so the form is checked here:
/// ONE reader for every request member that carries an IAM role ARN — the plan
/// create and update, and a malware scan's `ScanConfiguration.Role`, "the IAM role
/// that is used for scanning the resource"
/// (`smithy_query(shape="StartMalwareScanConfiguration")`) — so the form cannot
/// drift between the paths that take one:
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

/// The value `Service.ServiceName` carries on every finding: "The name of the Amazon
/// Web Services service (GuardDuty) that generated a finding"
/// (`smithy_query(shape="Service")`).
///
/// Held beside the finding record because both the reported finding and the criterion
/// resolver read it, so a filter on `service.serviceName` cannot disagree with the
/// value a findings read reports.
pub const GUARDDUTY_SERVICE_NAME: &str = "guardduty";

/// The wire name of the filter-name member on the operations that carry it as a
/// URI label — `GetFilter`, `UpdateFilter` and `DeleteFilter` bind
/// `FilterName` to `/detector/{DetectorId}/filter/{FilterName}`
/// (`smithy_query(operation="GetFilter")`). The filter family's own refusals name no
/// member ([`NO_SUCH_RESOURCE_MESSAGE`], [`INVALID_FILTER_NAME_MESSAGE`]), so this is
/// the wire name itself and not a rejection's subject.
pub const FILTER_NAME_MEMBER: &str = "filterName";

/// The wire name `CreateFilter` carries the filter name under: its `Name` member
/// has `jsonName` `name` (`smithy_query(operation="CreateFilter")`). A rejection that
/// names a member names it AS THE REQUEST CARRIES IT, which is why a create and a read
/// would name a filter name differently.
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

/// The sentence the MUTATING filter paths refuse a `FilterName` they cannot act on
/// with: `UpdateFilter` and `DeleteFilter` answer that the name they were given is
/// not a usable one, where the READ answers that no such resource was found
/// ([`NO_SUCH_RESOURCE_MESSAGE`]). Fixed text — the name is not quoted back — and it
/// is the answer for a name the detector does not hold and for a value that could
/// not be a filter name at all alike.
pub const INVALID_FILTER_NAME_MESSAGE: &str =
    "The request is rejected because the given filter name is invalid.";

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

/// The `FilterAction` that SUPPRESSES: "the action that is to be applied to the
/// findings that match the filter"
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`) is one
/// of two values (`smithy_query(shape="FilterAction")`), and this is the one that
/// changes a finding — it archives every finding the filter's criteria select
/// ([`Detector::apply_filter_action`]). The other names no change at all.
pub const FILTER_ACTION_ARCHIVE: &str = "ARCHIVE";

/// The `FilterVersion` a freshly created filter carries. `FilterVersion` is
/// `range min 1` (`smithy_query(shape="FilterVersion")`), so a filter nothing has
/// updated is at version 1.
pub const FILTER_INITIAL_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// Modelled-constraint refusals
// ---------------------------------------------------------------------------

/// The sentence GuardDuty refuses a value outside a constraint its model declares
/// with: ONE fixed text for the whole class, naming no member and interpolating
/// neither the bound that was broken nor the value that broke it.
///
/// The bound is already published in the model the caller built its request from, so
/// the refusal restates nothing: it says only that some input parameter held a value
/// the service will not take. A client that branches on this text, or snapshots it,
/// sees the same sentence whichever member and whichever bound was at fault.
pub const INVALID_INPUT_PARAMETERS_MESSAGE: &str =
    "The request is rejected because the one or more input parameters have invalid values.";

/// The sentence GuardDuty refuses a value its request payload could not be read as
/// with — the answer to a payload that did not DESERIALIZE into the modelled input,
/// which is what a value outside an enumerated type's value set is.
///
/// It is a different class from [`INVALID_INPUT_PARAMETERS_MESSAGE`]: the value was
/// not understood at all rather than understood and rejected, so the allowed values
/// are not enumerated back at the caller and a client reading this text knows to
/// re-serialize rather than to re-think the value.
pub const UNPROCESSABLE_PAYLOAD_MESSAGE: &str =
    "The request is rejected because the JSON could not be processed.";

/// How GuardDuty states a refusal the request boundary made: a member the model
/// requires was absent, or a value fell outside a bound the model declares.
///
/// GuardDuty's model declares exactly two errors for every operation —
/// `BadRequestException` (`httpError` 400) and `InternalServerErrorException`
/// (500) (`smithy_query(query="service_info")` lists them as the service's common
/// errors) — and NO `ValidationException` shape. So a modelled-constraint
/// violation is the same refusal as any other unacceptable input, under one of the
/// two FIXED texts above, chosen by whether the value was unreadable or merely
/// unacceptable; neither quotes the caller's value, the member at fault or the
/// bound back.
///
/// `service.rs` routes the framework's pre-handler refusal through here, so a
/// constraint the model declares on ANY member of ANY operation in this service
/// is refused under the modelled error rather than under a shape no GuardDuty
/// client can catch.
pub fn constraint_rejection(violations: &[::aws_smithy_schema_server::Violation]) -> String {
    use ::aws_smithy_schema_server::ViolationKind;

    // A value outside an enumerated type's value set is not a value of the member's
    // modelled type at all, so reading the payload into that input is what failed.
    if violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::EnumValue(_)))
    {
        return UNPROCESSABLE_PAYLOAD_MESSAGE.to_string();
    }
    // An ABSENT member is not a value at all, so neither text above describes it:
    // the service states which member it cannot act without, as it does for a
    // member a handler finds missing.
    if let Some(absent) = violations
        .iter()
        .find(|violation| matches!(violation.kind, ViolationKind::MissingRequiredMember))
    {
        // The path is already the member's wire spelling (lowerCamel, with
        // `.member` steps for collections), which is how the caller named it.
        return missing_parameter_message(absent.path.as_str());
    }
    // Every remaining class — a length, a range, a pattern, uniqueness, and any
    // class `ViolationKind` (which is `#[non_exhaustive]`) gains later — is a value
    // the service will not take, and they all share the one sentence.
    INVALID_INPUT_PARAMETERS_MESSAGE.to_string()
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
///
/// Compared by value, so a write over a stored configuration can answer whether it
/// moved anything: re-applying the status a switch already carries leaves an EQUAL
/// record behind ([`set_feature_in`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureRecord {
    pub name: String,
    pub status: String,
    pub updated_at_millis: i64,
    pub additional_configuration: Vec<AdditionalConfigRecord>,
    /// When this feature's free trial began for the account that holds this
    /// record, in epoch milliseconds, or `None` while the feature has never been
    /// enabled for it and its trial has therefore not started.
    ///
    /// Recorded ONCE, at the instant the switch first becomes [`ENABLED`], and kept
    /// across every later switch: the trial is a one-off per account per feature, so
    /// turning a feature off and on again does not buy a second one. It is a
    /// different fact from [`Self::updated_at_millis`], which moves every time the
    /// switch changes, and it is the single stored fact
    /// [`free_trial_days_remaining`] counts the remaining days from.
    pub free_trial_started_at_millis: Option<i64>,
}

/// A feature's nested additional configuration (e.g. a Runtime Monitoring agent
/// management switch).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The invitations this detector's ACCOUNT has received, keyed by the account
    /// id of the administrator that sent each one.
    ///
    /// The inbox side of the member relationship: `InviteMembers` is called by an
    /// administrator and sends an invitation the invited account then answers, so
    /// an entry here is an invitation that was SENT and is still pending — which
    /// is what `AcceptInvitation` / `AcceptAdministratorInvitation` resolve
    /// `(MasterId, InvitationId)` against, and what `ListInvitations` reports.
    ///
    /// WRITTEN by `InviteMembers`, which is the one operation that SENDS an
    /// invitation: sending one is a write to both sides of the relationship — the
    /// administrator's own view of the account it invited ([`MemberRecord`]) and
    /// the pending invitation the invited account answers — so the send delivers
    /// it here through
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::deliver_invitation_in`].
    /// Every account this emulator presents carries the one canonical account id
    /// ([`administrator_account_id`]), the sending administrator included, so the
    /// inbox an invitation is delivered to is the one the Region's endpoint
    /// serves and the key it lands under is that account id.
    ///
    /// This is state rather than a constant because both the refusal and the
    /// report are a lookup: an invitation this store holds is accepted by one and
    /// reported by the other, read through [`Self::received_invitations_in`] and
    /// projected through [`invitation_view`]. `GetInvitationsCount` reports the
    /// SIZE of that same inbox, through the same reader. `DeleteInvitations` and
    /// `DeclineInvitations` both TAKE one out, through
    /// [`Self::take_received_invitations_in`], so an invitation this store holds
    /// leaves the inbox accepted, declined or deleted and never twice.
    ///
    /// BOUNDED by [`MAX_INVITATIONS_PER_DETECTOR`], so the delivery cannot grow
    /// it without limit: the key is an account id, which is request-chosen.
    pub received_invitations: BTreeMap<String, ReceivedInvitationRecord>,
    /// The administrator this detector's account is monitored by — what
    /// `GetMasterAccount` / `GetAdministratorAccount` report — or `None` while the
    /// account has accepted no invitation.
    ///
    /// Written only by accepting an invitation, and dropped by
    /// `DisassociateFromMasterAccount`; a detector a create just minted
    /// administers itself, so it starts with none.
    pub administrator: Option<AdministratorRecord>,
    /// What this detector's account has configured for the OTHER accounts in its
    /// organization — what `DescribeOrganizationConfiguration` reports and
    /// `UpdateOrganizationConfiguration` writes.
    ///
    /// It hangs off the detector because that is what identifies it on the wire
    /// (`/detector/{DetectorId}/admin`), so `DeleteDetector` drops it with the
    /// detector. Bounded by construction: one record per `AutoEnableMembers` value
    /// and one per [`ORG_FEATURES`] member, whatever a caller sends.
    pub organization: OrganizationConfiguration,
}

/// An invitation this detector's account RECEIVED, as the store holds it.
///
/// Every member is one the `Invitation` shape carries
/// (`smithy_query(shape="Invitation")`), with the instant held as epoch
/// milliseconds — the single source for the `InvitedAt` anything reports.
#[derive(Debug, Clone)]
pub struct ReceivedInvitationRecord {
    /// `Invitation.AccountId` — "the ID of the account that the invitation was
    /// sent from", which is the `MasterId` / `AdministratorId` an acceptance names.
    pub administrator_account_id: String,
    /// `Invitation.InvitationId` — "the ID of the invitation. This value is used
    /// to validate the inviter account to the member account".
    pub invitation_id: String,
    /// `Invitation.RelationshipStatus`: [`RELATIONSHIP_INVITED`] while the
    /// invitation is pending.
    pub relationship_status: String,
    /// When the invitation was sent.
    pub invited_at_millis: i64,
}

/// The administrator relationship an accepted invitation established.
///
/// Every member is one the `Master` shape carries
/// (`smithy_query(shape="Master")`); `GetAdministratorAccount` reports the same
/// facts under `Administrator`.
#[derive(Debug, Clone)]
pub struct AdministratorRecord {
    /// `Master.AccountId` — "the ID of the account used as the administrator
    /// account".
    pub account_id: String,
    /// `Master.InvitationId` — "the value used to validate the administrator
    /// account to the member account": the id of the invitation that was accepted.
    pub invitation_id: String,
    /// `Master.RelationshipStatus`: [`RELATIONSHIP_ENABLED`] once the invitation
    /// has been accepted.
    pub relationship_status: String,
    /// When the accepted invitation was sent — `Master.InvitedAt`, which is the
    /// invitation's instant and not the acceptance's.
    pub invited_at_millis: i64,
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
    /// `Member.RelationshipStatus`: [`RELATIONSHIP_CREATED`],
    /// [`RELATIONSHIP_INVITED`] once `InviteMembers` has sent the account an
    /// invitation, [`RELATIONSHIP_DISABLED`] once `StopMonitoringMembers` has
    /// stopped monitoring the account, [`RELATIONSHIP_ENABLED`] once
    /// `StartMonitoringMembers` has turned monitoring back on for it, or
    /// [`RELATIONSHIP_REMOVED`] once `DisassociateMembers` has ended the
    /// association.
    pub relationship_status: String,
    /// When the invitation `InviteMembers` sent this account was sent, in epoch
    /// milliseconds — the member's `InvitedAt`, "the timestamp when the invitation
    /// was sent" (`smithy_query(shape="Member",member="InvitedAt")`) — or `None`
    /// while the account has been recorded and not invited, which is why the member
    /// is optional.
    ///
    /// Retained across a disassociation like the rest of the record: the invitation
    /// WAS sent at that instant, and only `DeleteMembers` drops the record that says
    /// so.
    pub invited_at_millis: Option<i64>,
    /// When the relationship last changed, in epoch milliseconds — the member's
    /// `UpdatedAt`.
    ///
    /// The RELATIONSHIP's instant, not the configuration's:
    /// [`Self::features`] carries its own per-feature `UpdatedAt`, so configuring
    /// the member account's detector moves that and leaves this where it was.
    pub updated_at_millis: i64,
    /// The configuration of the member account's own detector, as the
    /// administrator maintains it — what `UpdateMemberDetectors` writes and
    /// `GetMemberDetectors` reports, under both the `Features` and the deprecated
    /// `DataSources` view.
    ///
    /// Held as the same [`FeatureRecord`] list a detector carries, because it is
    /// the same configuration seen from the administrator's side: one stored
    /// switch per feature, each with the instant it last changed. A member starts
    /// with the feature set a detector created without a `Features` list gets
    /// ([`default_features`]), and only the features the `OrgFeature` enum carries
    /// ([`MEMBER_FEATURES`]) can be configured through a member request.
    pub features: Vec<FeatureRecord>,
}

impl MemberRecord {
    /// Is this member currently ASSOCIATED with the administrator?
    ///
    /// "Is a member account of this administrator" and "is associated with it" are
    /// two different questions, because a record is held before the association
    /// begins and retained after it ends (see the type's documentation):
    /// `GetMembers` answers the first, and the operations that act on the
    /// association answer the second.
    ///
    /// The members that are NOT associated are the two classes `ListMembers` names
    /// when it offers "to only return associated members or to return all members
    /// (including members who haven't been invited yet or have been disassociated)"
    /// (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`):
    /// an account the administrator has recorded and not invited yet
    /// ([`RELATIONSHIP_CREATED`]), and one a disassociation moved
    /// ([`RELATIONSHIP_REMOVED`]). The invitation is what associates the account, so
    /// the relationship is an association from there on — whether or not the account
    /// has answered it, which is the narrower [`Self::is_monitored`].
    pub fn is_associated(&self) -> bool {
        self.relationship_status == RELATIONSHIP_INVITED
            || self.relationship_status == RELATIONSHIP_ENABLED
            || self.relationship_status == RELATIONSHIP_DISABLED
    }

    /// Is GuardDuty MONITORING this member account under the administrator?
    ///
    /// The association and the monitoring are two facts, and the monitoring is the
    /// narrower one: it begins when the member ACCOUNT accepts the administrator's
    /// invitation — [`RELATIONSHIP_ENABLED`] while it runs and
    /// [`RELATIONSHIP_DISABLED`] while the administrator has stopped it, which is the
    /// state `StartMonitoringMembers` turns back on ("use this operation to restart
    /// monitoring of accounts that you stopped monitoring with the
    /// StopMonitoringMembers operation",
    /// `smithy_query(query="doc",shape="StartMonitoringMembers")`). An account the
    /// administrator has recorded or invited is not monitored under it yet, whatever
    /// else it may already be associated with it for.
    pub fn is_monitored(&self) -> bool {
        self.relationship_status == RELATIONSHIP_ENABLED
            || self.relationship_status == RELATIONSHIP_DISABLED
    }

    /// Has the administrator ENDED this member's association with
    /// `DisassociateMembers`, leaving the record it retains behind
    /// ([`RELATIONSHIP_REMOVED`])?
    ///
    /// Not the complement of [`Self::is_associated`]: an account recorded and not
    /// invited yet, and one whose invitation is still unanswered, are neither
    /// associated nor disassociated, which is why the two questions are asked
    /// separately.
    pub fn is_disassociated(&self) -> bool {
        self.relationship_status == RELATIONSHIP_REMOVED
    }

    /// Set one feature's status on the member account's configuration, recording
    /// `now` as its `UpdatedAt` — [`Detector::set_feature`] for the member side of
    /// the same switch.
    pub fn set_feature(&mut self, name: &str, status: &str, now: i64) {
        set_feature_in(&mut self.features, name, status, now);
    }

    /// What one of the member's data sources reports: the status of the feature
    /// that governs it and the instant that feature last changed.
    pub fn data_source(&self, feature: &str) -> (String, i64) {
        feature_data_source(&self.features, feature, self.updated_at_millis)
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
        // Present only once an invitation has been sent to the account, which is
        // what `InviteMembers` does; a recorded, un-invited member has no instant at
        // which one was sent, so the optional member is absent rather than invented.
        invited_at: member.invited_at_millis.map(iso8601),
        updated_at: iso8601(member.updated_at_millis),
    }
}

/// The modelled `Invitation` a read reports for one invitation in the inbox.
///
/// Every member of the shape is optional (`smithy_query(shape="Invitation")`) and
/// every one of them is a fact the record holds, so the projection fills them all
/// and lives here once: the operations that report the inbox must not disagree
/// about one invitation.
pub fn invitation_view(
    invitation: &ReceivedInvitationRecord,
) -> crate::ops::types::list_invitations_types::Invitation {
    crate::ops::types::list_invitations_types::Invitation {
        // "The ID of the account that the invitation was sent from"
        // (`smithy_query(shape="Invitation")`), which is the key the inbox holds
        // the record under.
        account_id: Some(invitation.administrator_account_id.clone()),
        invitation_id: Some(invitation.invitation_id.clone()),
        relationship_status: Some(invitation.relationship_status.clone()),
        // `Invitation.InvitedAt` targets a `String`, rendered the way every other
        // instant this service reports as text is.
        invited_at: Some(iso8601(invitation.invited_at_millis)),
    }
}

impl Detector {
    /// The status of one feature, or [`DISABLED`] when the detector does not carry
    /// it.
    pub fn feature_status(&self, name: &str) -> &str {
        feature_status_in(&self.features, name)
    }

    /// What one data source reports: the status of the feature that governs it and
    /// the instant that feature's status last changed, in epoch milliseconds.
    ///
    /// The status is the feature's OWN status, never a function of the detector's
    /// `Status` member: that member carries whether the detector is enabled, while
    /// the feature and data-source views carry the configuration, which a detector
    /// keeps across being disabled and enabled again.
    pub fn data_source(&self, feature: &str) -> (String, i64) {
        feature_data_source(&self.features, feature, self.updated_at_millis)
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
    ///
    /// A list a delete has already accepted ([`ThreatIntelSetRecord::is_deleting`])
    /// resolves to nothing, so a replayed create never answers with a list the
    /// service is no longer keeping — the same rule [`Self::ip_set_for_token`]
    /// follows for a deleted IPSet.
    pub fn threat_intel_set_for_token(&self, token: &str) -> Option<&ThreatIntelSetRecord> {
        self.threat_intel_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token) && !set.is_deleting())
    }

    /// How many of this detector's threat lists are LIVE — held and not being
    /// deleted.
    ///
    /// This is the count [`THREAT_INTEL_SETS_PER_DETECTOR`] bounds: a list whose
    /// delete has been accepted is only still REPORTED, never used, so it occupies
    /// none of the detector's slots — the same rule
    /// [`Self::live_ip_set_count`] applies to a deleted IPSet.
    pub fn live_threat_intel_set_count(&self) -> usize {
        self.threat_intel_sets
            .values()
            .filter(|set| !set.is_deleting())
            .count()
    }

    /// The ids of the LIVE threat lists this detector holds, in
    /// `ThreatIntelSetId` order (the id is the key they are stored under, so the
    /// order is TOTAL — what makes a page token mean the same thing on the call
    /// that handed it back).
    ///
    /// LIVE is the same set [`Self::live_threat_intel_set_count`] counts: a list
    /// whose delete has been accepted is no longer one of the detector's threat
    /// lists, so `ListThreatIntelSets` stops naming it. Its record is still
    /// retained ([`DELETED_THREAT_INTEL_SETS_RETAINED`]) so `GetThreatIntelSet` can
    /// report the status the delete moved it to — being readable on its own id is
    /// not the same as being part of the collection a list enumerates, which is the
    /// rule [`Self::live_ip_set_ids`] follows for a deleted IPSet.
    pub fn live_threat_intel_set_ids(&self) -> Vec<String> {
        self.threat_intel_sets
            .values()
            .filter(|set| !set.is_deleting())
            .map(|set| set.id.clone())
            .collect()
    }

    /// Accept the deletion of the threat list `id` names.
    ///
    /// `None` when this detector holds no such list at all; `Some(true)` when this
    /// call is the one that accepted the deletion, and `Some(false)` when a delete
    /// had already been accepted for it — the caller needs that distinction because
    /// only the first delete changes the set of LIVE sub-resources the detector
    /// holds, and so only the first moves the detector's own `UpdatedAt`.
    ///
    /// The list moves to [`THREAT_INTEL_SET_DELETE_PENDING`] and the clock owes it
    /// [`THREAT_INTEL_SET_DELETED`], which is what `GetThreatIntelSet` reports
    /// afterwards.
    ///
    /// Keeping a deleted record is what lets a read report that status, so this is
    /// also where the retention is BOUNDED: at most
    /// [`DELETED_THREAT_INTEL_SETS_RETAINED`] deleted records survive per detector,
    /// and a delete past that drops the one deleted longest ago — the same
    /// oldest-first eviction [`Self::delete_ip_set`] uses, and for the same reason:
    /// without it a caller could create and delete without limit and make the
    /// service retain every record.
    pub fn delete_threat_intel_set(&mut self, id: &str, now: i64) -> Option<bool> {
        let accepted = self.threat_intel_sets.get_mut(id)?.begin_delete(now);
        if accepted {
            self.evict_deleted_threat_intel_sets(Some(id));
        }
        Some(accepted)
    }

    /// Drop deleted threat lists, oldest deletion first, until at most
    /// [`DELETED_THREAT_INTEL_SETS_RETAINED`] remain, so the TOTAL this detector
    /// holds is bounded at [`THREAT_INTEL_SETS_PER_DETECTOR`] live records plus that
    /// allowance however many create-and-delete cycles a caller sends. Called on
    /// BOTH write paths — the delete that leaves a record behind and the create that
    /// adds one — so neither can grow the map past that total.
    ///
    /// `keep` is never evicted: it is the record the delete that triggered this just
    /// accepted, and a caller reads that one back.
    pub fn evict_deleted_threat_intel_sets(&mut self, keep: Option<&str>) {
        let surplus = self
            .threat_intel_sets
            .values()
            .filter(|set| set.is_deleting())
            .count()
            .saturating_sub(DELETED_THREAT_INTEL_SETS_RETAINED);
        if surplus == 0 {
            return;
        }
        // The oldest deletions first: `deleted_at_millis` is the instant the delete
        // was ACCEPTED and nothing moves it afterwards, and `tick_millis` makes
        // those instants strictly increasing, so the ordering is total and the id
        // only breaks a tie that cannot arise.
        let mut deleted: Vec<(i64, String)> = self
            .threat_intel_sets
            .values()
            .filter(|set| set.is_deleting() && Some(set.id.as_str()) != keep)
            .map(|set| (set.deleted_at_millis.unwrap_or_default(), set.id.clone()))
            .collect();
        deleted.sort_unstable();
        for (_, id) in deleted.into_iter().take(surplus) {
            self.threat_intel_sets.remove(&id);
        }
    }

    /// Set one feature's status, recording `now` as its `UpdatedAt`. Adds the
    /// feature when the detector does not carry it yet.
    pub fn set_feature(&mut self, name: &str, status: &str, now: i64) {
        set_feature_in(&mut self.features, name, status, now);
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
    ///
    /// Answers whether this call moved any finding at all, which is what lets the
    /// caller take the instant it stamped off the clock only when one really moved.
    pub fn set_findings_archived(
        &mut self,
        finding_ids: &[String],
        archived: bool,
        now: i64,
    ) -> bool {
        let mut moved = false;
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                if finding.archived != archived {
                    finding.archived = archived;
                    finding.updated_at_millis = now;
                    moved = true;
                }
            }
        }
        moved
    }

    /// Is `finding` selected by at least one of this detector's SUPPRESSION filters
    /// — a saved filter whose action archives
    /// ([`FilterRecord::archives_matching_findings`])?
    ///
    /// A filter selects findings by exactly the rule a findings read selects them by
    /// ([`finding_matches`]), so the set a filter suppresses is the set a
    /// `ListFindings` carrying its criteria reports and the two cannot drift apart.
    /// A saved filter's criteria are resolved as fields of a finding before the
    /// filter is stored, so the matcher cannot reject them here; one it still could
    /// not evaluate selects NOTHING rather than suppressing everything.
    pub fn finding_is_suppressed(&self, finding: &FindingRecord) -> bool {
        self.filters.values().any(|filter| {
            filter.archives_matching_findings()
                && finding_matches(finding, Some(&filter.finding_criteria)).unwrap_or(false)
        })
    }

    /// Apply ONE saved filter's action: when the named filter archives, put every
    /// finding this detector holds that its criteria select into the archived state,
    /// recording `now` as the `UpdatedAt` of each finding the call actually moves. A
    /// filter whose action names no change, and a name this detector does not hold,
    /// move nothing.
    ///
    /// Scoped to the filter a call just saved, so saving one filter never moves the
    /// archive state a caller set through the explicit write for findings that filter
    /// does not select. The write goes through [`Self::set_findings_archived`], the
    /// ONE archive write, so a suppressed finding is archived exactly as an explicit
    /// `ArchiveFindings` archives it and the last call to land still decides the
    /// state: a finding a caller unarchives stays unarchived until the filter is
    /// saved over it again or the activity is observed again.
    ///
    /// Answers whether this call moved any finding at all, as
    /// [`Self::set_findings_archived`] does and for the same reason.
    pub fn apply_filter_action(&mut self, filter_name: &str, now: i64) -> bool {
        let Some(filter) = self.filters.get(filter_name) else {
            return false;
        };
        if !filter.archives_matching_findings() {
            return false;
        }
        let criteria = filter.finding_criteria.clone();
        let selected: Vec<String> = self
            .findings
            .values()
            .filter(|finding| finding_matches(finding, Some(&criteria)).unwrap_or(false))
            .map(|finding| finding.id.clone())
            .collect();
        self.set_findings_archived(&selected, true, now)
    }

    /// Apply the archive action of EVERY saved suppression filter to one finding: the
    /// path that has just generated or re-observed it archives it when any standing
    /// filter selects it ([`Self::finding_is_suppressed`]), so generating a finding
    /// never moves the archive state of a finding it is not about.
    pub fn archive_if_suppressed(&mut self, finding_id: &str, now: i64) -> bool {
        let suppressed = self
            .findings
            .get(finding_id)
            .is_some_and(|finding| self.finding_is_suppressed(finding));
        suppressed && self.set_findings_archived(&[finding_id.to_string()], true, now)
    }

    /// Record `feedback` (and the `comments` it came with) on every named finding
    /// this detector holds, recording `now` as the `UpdatedAt` of each finding the
    /// call actually changes.
    ///
    /// The submission REPLACES whatever was recorded before: the feedback is one
    /// value per finding, not a log. An id this detector does not hold writes
    /// nothing and is not an error, the same way a findings read reports only the
    /// ids it holds.
    ///
    /// The feedback is a property of the RECORD, not an observation of the
    /// activity, so — exactly as for [`Self::set_findings_archived`] — only
    /// [`FindingRecord::updated_at_millis`] moves and
    /// [`FindingRecord::event_last_seen_millis`] stays where the last observation
    /// left it. A submission that leaves every stored value where it was moves
    /// nothing at all, which is what makes a re-submission of the same feedback a
    /// true no-op.
    ///
    /// Answers whether this call changed any finding at all, as
    /// [`Self::set_findings_archived`] does and for the same reason.
    pub fn set_findings_feedback(
        &mut self,
        finding_ids: &[String],
        feedback: &str,
        comments: Option<&str>,
        now: i64,
    ) -> bool {
        let mut moved = false;
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                let changed = finding.user_feedback.as_deref() != Some(feedback)
                    || finding.feedback_comments.as_deref() != comments;
                if changed {
                    finding.user_feedback = Some(feedback.to_string());
                    finding.feedback_comments = comments.map(str::to_string);
                    finding.updated_at_millis = now;
                    moved = true;
                }
            }
        }
        moved
    }

    /// The first id in `finding_ids` this detector holds no finding for, in the
    /// order the request listed them.
    ///
    /// The archive-state mutations resolve every id BEFORE they write
    /// ([`Self::set_findings_archived`]), so a batch naming an id the detector
    /// does not hold is refused whole and moves nothing: a caller archiving in
    /// bulk learns that one of its ids is wrong instead of having the rest
    /// silently applied. The FIRST unheld id is reported so the rejection is a
    /// function of the request alone and not of iteration order over the store.
    pub fn first_unheld_finding_id<'ids>(&self, finding_ids: &'ids [String]) -> Option<&'ids str> {
        finding_ids
            .iter()
            .find(|id| !self.findings.contains_key(id.as_str()))
            .map(String::as_str)
    }

    /// Put a pending invitation from `administrator_account_id` in this detector's
    /// inbox, reporting whether this call delivered one.
    ///
    /// The inverse of [`Self::accept_invitation`]: the record it writes is exactly
    /// the one an acceptance resolves, an inbox read reports and a decline or a
    /// delete takes out. A delivered invitation is [`RELATIONSHIP_INVITED`] — it
    /// is pending — and carries `now` as the instant it was SENT, which is the
    /// `InvitedAt` every read of it reports and the one the accepted relationship
    /// keeps.
    ///
    /// Writes NOTHING, and reports `false`, for an account that already has an
    /// invitation pending here (that invitation stays as it is, so the inviter
    /// cannot overwrite the id the invited account was handed) and for an inbox
    /// already holding [`MAX_INVITATIONS_PER_DETECTOR`] invitations from other
    /// accounts, which is the ceiling that keeps a request-keyed store bounded.
    ///
    /// The detector itself does not move: an invitation is a record of its own,
    /// not a change to the detector's configuration, so `UpdatedAt` is left where
    /// it was until an acceptance establishes the relationship.
    pub fn deliver_invitation(
        &mut self,
        administrator_account_id: &str,
        invitation_id: &str,
        now: i64,
    ) -> bool {
        if !self.inbox_has_room_for(administrator_account_id) {
            return false;
        }
        self.received_invitations.insert(
            administrator_account_id.to_string(),
            ReceivedInvitationRecord {
                administrator_account_id: administrator_account_id.to_string(),
                invitation_id: invitation_id.to_string(),
                relationship_status: RELATIONSHIP_INVITED.to_string(),
                invited_at_millis: now,
            },
        );
        true
    }

    /// Can this detector's inbox take an invitation from `administrator_account_id`?
    ///
    /// The condition [`Self::deliver_invitation`] writes under, read over an
    /// IMMUTABLE borrow so a delivery can answer it BEFORE it mints an id: an
    /// account that already has an invitation pending here keeps that invitation,
    /// and an inbox at [`MAX_INVITATIONS_PER_DETECTOR`] takes no further account.
    /// One definition, so the check and the write cannot disagree on what the
    /// inbox will accept.
    pub fn inbox_has_room_for(&self, administrator_account_id: &str) -> bool {
        !self.has_invitation_pending_from(administrator_account_id)
            && self.received_invitations.len() < MAX_INVITATIONS_PER_DETECTOR
    }

    /// Is an invitation from `administrator_account_id` pending for this detector's
    /// account, whatever its id?
    ///
    /// ONE invitation is pending per inviting account at a time (see
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::deliver_invitation_in`]), so
    /// this single lookup is what "still pending" means for that account: it is the
    /// condition a delivery takes its room from ([`Self::inbox_has_room_for`]) and
    /// the condition a send leaves its member records untouched under, read over an
    /// IMMUTABLE borrow so either caller can answer it before it writes. One
    /// definition, so the two halves of a send cannot disagree about what is
    /// pending.
    pub fn has_invitation_pending_from(&self, administrator_account_id: &str) -> bool {
        self.received_invitations
            .contains_key(administrator_account_id)
    }

    /// Is an invitation pending for this detector's account under exactly the pair
    /// `(administrator_account_id, invitation_id)`?
    ///
    /// The condition [`Self::accept_invitation`] accepts on, read over an IMMUTABLE
    /// borrow so the operations bound to it can classify their refusal before they
    /// touch anything — one definition, so the check and the write cannot disagree
    /// on what is pending.
    pub fn invitation_pending(&self, administrator_account_id: &str, invitation_id: &str) -> bool {
        self.received_invitations
            .get(administrator_account_id)
            .is_some_and(|invitation| invitation.invitation_id == invitation_id)
    }

    /// Accept the invitation `(administrator_account_id, invitation_id)` names, if
    /// this detector's account has one pending under exactly that pair.
    ///
    /// Reports whether it did. Both members must match: `InvitationId` is "the
    /// value that is used to validate the administrator account to the member
    /// account" (`smithy_query(operation="AcceptInvitation")`), so an id that
    /// belongs to a different administrator's invitation validates nothing.
    ///
    /// The accepted invitation leaves the inbox — it is no longer pending — and
    /// the relationship it established becomes [`Self::administrator`], carrying
    /// the invitation's own `InvitedAt` rather than the instant of the acceptance.
    /// A request that matches nothing writes NOTHING, which is what makes the
    /// refusal leave master-account state exactly where it was.
    pub fn accept_invitation(
        &mut self,
        administrator_account_id: &str,
        invitation_id: &str,
        now: i64,
    ) -> bool {
        if !self.invitation_pending(administrator_account_id, invitation_id) {
            return false;
        }
        let Some(invitation) = self.received_invitations.remove(administrator_account_id) else {
            return false;
        };
        self.administrator = Some(AdministratorRecord {
            account_id: invitation.administrator_account_id,
            invitation_id: invitation.invitation_id,
            relationship_status: RELATIONSHIP_ENABLED.to_string(),
            invited_at_millis: invitation.invited_at_millis,
        });
        self.updated_at_millis = now;
        true
    }

    /// Drop the administrator relationship this detector's account is monitored
    /// by, reporting whether there was one to drop.
    ///
    /// The MEMBER side of the association, and the exact inverse of
    /// [`Self::accept_invitation`]: it clears [`Self::administrator`], so the
    /// administrator reads (`GetAdministratorAccount` / `GetMasterAccount`) report
    /// none afterwards. The accepted invitation is NOT put back in the inbox — "the
    /// member account details obtained from the CreateMembers API ... are retained.
    /// This is done so that the delegated administrator can invoke the
    /// InviteMembers API without the need to invoke the CreateMembers API again"
    /// (`smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")`):
    /// re-association goes through a fresh invitation, which is the
    /// administrator's call to make and not this one's.
    ///
    /// Nothing else moves: the member records this detector's account administers
    /// are the OTHER side of the relationship (they belong to the accounts this one
    /// administers, not to the administrator it is leaving), so they are untouched.
    /// `false` writes NOTHING at all, which is what lets the caller refuse without
    /// having changed anything.
    ///
    /// Shared with the deprecated `DisassociateFromMasterAccount` ("this operation
    /// is deprecated, use DisassociateFromAdministratorAccount instead",
    /// `smithy_query(operation="DisassociateFromMasterAccount")`), which is still a
    /// scaffold with an issue of its own: the two names are one behaviour, so
    /// neither can drop a relationship the other would keep.
    pub fn disassociate_from_administrator(&mut self, now: i64) -> bool {
        if self.administrator.take().is_none() {
            return false;
        }
        self.updated_at_millis = now;
        true
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

    /// Apply the status change the CLOCK already owes each of this detector's
    /// investigations, so a report answers what the service owed at `now` whether
    /// or not anybody polled. The counterpart of
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::settle_malware_scans`] for
    /// the investigations one detector holds; every move goes through
    /// [`InvestigationRecord::settle`], so a read and a list cannot disagree about
    /// when an investigation left `RUNNING`.
    pub fn settle_investigations(&mut self, now: i64) {
        for investigation in self.investigations.values_mut() {
            investigation.settle(now);
        }
    }

    /// Every investigation this detector holds, in investigation-id order — the
    /// total order the map is keyed by, which a sort then re-orders and a page
    /// token stays meaningful against.
    pub fn investigation_records(&self) -> Vec<&InvestigationRecord> {
        self.investigations.values().collect()
    }

    /// The trusted entity set a create already minted for `token`, if any — the
    /// seam that makes `CreateTrustedEntitySet` idempotent under a replayed
    /// `ClientToken`.
    /// A set whose delete has already been accepted
    /// ([`EntitySetRecord::is_deleting`]) resolves to nothing, so a replayed create
    /// never answers with a set the service is no longer keeping — the same rule
    /// [`Self::ip_set_for_token`] follows for a deleted IPSet.
    pub fn trusted_entity_set_for_token(&self, token: &str) -> Option<&EntitySetRecord> {
        self.trusted_entity_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token) && !set.is_deleting())
    }

    /// The LIVE trusted entity set this detector holds, if any.
    ///
    /// LIVE excludes a set whose delete has been accepted: "at any given time, you
    /// can have only one trusted entity set"
    /// (`smithy_query(query="doc",shape="CreateTrustedEntitySet")`) bounds the sets
    /// GuardDuty is keeping, and a deleted one is only still REPORTED, so
    /// `DeleteTrustedEntitySet` frees the detector's single slot. Its record is
    /// retained ([`DELETED_ENTITY_SETS_RETAINED`]) so `GetTrustedEntitySet` can
    /// report the status the delete moved it to.
    pub fn live_trusted_entity_set(&self) -> Option<&EntitySetRecord> {
        self.trusted_entity_sets
            .values()
            .find(|set| !set.is_deleting())
    }

    /// The ids of the LIVE trusted entity sets this detector holds, in
    /// `TrustedEntitySetId` order (the id is the key they are stored under, so the
    /// order is TOTAL — what makes a page token mean the same thing on the call
    /// that handed it back).
    ///
    /// LIVE excludes a set whose delete has already been accepted
    /// ([`EntitySetRecord::is_deleting`]): such a record is no longer one of the
    /// detector's entity sets, so `ListTrustedEntitySets` stops naming it, even
    /// though it is retained ([`DELETED_ENTITY_SETS_RETAINED`]) so
    /// `GetTrustedEntitySet` can report the status the delete moved it to — the
    /// same rule [`Self::live_threat_entity_set_ids`] follows for a threat set and
    /// [`Self::live_ip_set_ids`] for a deleted IPSet.
    ///
    /// It reports a LIST and not the single [`Self::live_trusted_entity_set`]
    /// because the one-at-a-time rule bounds what a create may add, not what the
    /// listing is: the answer is every live record the map holds.
    pub fn live_trusted_entity_set_ids(&self) -> Vec<String> {
        self.trusted_entity_sets
            .values()
            .filter(|set| !set.is_deleting())
            .map(|set| set.id.clone())
            .collect()
    }

    /// Accept the deletion of the trusted entity set `id` names.
    ///
    /// `None` when this detector holds no such set at all; `Some(true)` when this
    /// call is the one that accepted the deletion, and `Some(false)` when a delete
    /// had already been accepted for it — the caller needs that distinction because
    /// only the first delete changes the set of LIVE sub-resources the detector
    /// holds, and so only the first moves the detector's own `UpdatedAt`.
    pub fn delete_trusted_entity_set(&mut self, id: &str, now: i64) -> Option<bool> {
        delete_entity_set(&mut self.trusted_entity_sets, id, now)
    }

    /// The threat entity set a create already minted for `token`, if any — the
    /// seam that makes `CreateThreatEntitySet` idempotent under a replayed
    /// `ClientToken`.
    ///
    /// Scoped to the THREAT sets: a token resolves to a resource of the type the
    /// create that carried it made, so one token replayed across the two
    /// entity-set creates never hands back the other one's id.
    ///
    /// A set whose delete has already been accepted
    /// ([`EntitySetRecord::is_deleting`]) resolves to nothing, so a replayed
    /// create never answers with a set the service is no longer keeping — the
    /// same rule [`Self::trusted_entity_set_for_token`] follows for a deleted
    /// trusted set and [`Self::ip_set_for_token`] for a deleted IPSet.
    pub fn threat_entity_set_for_token(&self, token: &str) -> Option<&EntitySetRecord> {
        self.threat_entity_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token) && !set.is_deleting())
    }

    /// The ids of the LIVE threat entity sets this detector holds, in
    /// `ThreatEntitySetId` order (the id is the key they are stored under, so the
    /// order is TOTAL — what makes a page token mean the same thing on the call
    /// that handed it back).
    ///
    /// LIVE excludes a set whose delete has already been accepted
    /// ([`EntitySetRecord::is_deleting`]): such a record is no longer one of the
    /// detector's entity sets, so `ListThreatEntitySets` stops naming it, even
    /// though it is retained ([`DELETED_ENTITY_SETS_RETAINED`]) so
    /// `GetThreatEntitySet` can report the status the delete moved it to — the
    /// same rule [`Self::live_ip_set_ids`] follows for a deleted IPSet.
    pub fn live_threat_entity_set_ids(&self) -> Vec<String> {
        self.threat_entity_sets
            .values()
            .filter(|set| !set.is_deleting())
            .map(|set| set.id.clone())
            .collect()
    }

    /// How many of this detector's threat entity sets are LIVE — held and not
    /// being deleted.
    ///
    /// This is the count [`THREAT_ENTITY_SETS_PER_DETECTOR`] bounds: a set whose
    /// delete has been accepted is only still REPORTED, never used, so it
    /// occupies none of the detector's slots — the same rule
    /// [`Self::live_ip_set_count`] applies to a deleted IPSet and
    /// [`Self::live_trusted_entity_set`] to a deleted trusted set.
    pub fn live_threat_entity_set_count(&self) -> usize {
        self.threat_entity_sets
            .values()
            .filter(|set| !set.is_deleting())
            .count()
    }

    /// Accept the deletion of the threat entity set `id` names.
    ///
    /// `None` when this detector holds no such set at all; `Some(true)` when this
    /// call is the one that accepted the deletion, and `Some(false)` when a delete
    /// had already been accepted for it — the caller needs that distinction because
    /// only the first delete changes the set of LIVE sub-resources the detector
    /// holds, and so only the first moves the detector's own `UpdatedAt`.
    pub fn delete_threat_entity_set(&mut self, id: &str, now: i64) -> Option<bool> {
        delete_entity_set(&mut self.threat_entity_sets, id, now)
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
        self.live_ip_set_count_excluding(None)
    }

    /// How many of this detector's IPSets are LIVE — held and not being deleted —
    /// other than the one `except` names.
    ///
    /// This is the count [`ACTIVATED_IP_SETS_PER_ACCOUNT`] bounds when a request
    /// asks GuardDuty to start USING a list: the allowance is for the lists the
    /// account keeps, so a list whose delete has been accepted releases the one it
    /// held, and `except` is how a request about one list asks the question about
    /// the OTHERS.
    pub fn live_ip_set_count_excluding(&self, except: Option<&str>) -> usize {
        self.ip_sets
            .values()
            .filter(|ip_set| {
                !ip_set.is_deleting() && except.is_none_or(|id| ip_set.id != id)
            })
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

/// How many trusted IP lists an account may have GuardDuty USE in one Region.
///
/// AWS publishes an allowance of one trusted IP list per account per Region. The
/// number of lists a detector may HOLD is the larger [`IP_SETS_PER_DETECTOR`]:
/// uploading a further list unused is accepted, and what the allowance governs is
/// putting a list INTO USE — so a request that asks GuardDuty to start using a
/// list is refused once the account's lists have taken the allowance up, while the
/// same request that does not ask for it is served
/// ([`Detector::live_ip_set_count_excluding`]).
///
/// A detector is the account's one resource in a Region ("you can have only one
/// detector per account per Region" —
/// `smithy_query(query="doc",shape="CreateDetector")`), so the account's allowance
/// in a Region is the detector's.
///
/// The threat-list family needs no allowance of its own: AWS publishes six threat
/// lists per account per Region and that is already the ceiling on the lists a
/// detector HOLDS ([`THREAT_INTEL_SETS_PER_DETECTOR`]), so no sequence of creates
/// or updates can put more than the allowance into use.
pub const ACTIVATED_IP_SETS_PER_ACCOUNT: usize = 1;

/// The message GuardDuty refuses a request that would take the account past one
/// of its resource allowances with.
///
/// A server-side allowance rather than a malformed request: the class is the
/// modelled `InternalServerErrorException` (`httpError` 500,
/// `smithy_query(shape="InternalServerErrorException")`), so a client reading the
/// class alone still tells it apart from the 400 a bad parameter gets. The
/// sentence names neither the resource nor the caller's values.
pub const ACCOUNT_LIMIT_MESSAGE: &str =
    "The request is rejected because it\u{2019}s an attempt to create resources beyond the current AWS account limits.";

/// The modelled server-side failure a request past an account allowance is
/// refused with, carrying [`ACCOUNT_LIMIT_MESSAGE`].
///
/// `Type` is "The error type" (`smithy_query(shape="InternalServerErrorException")`)
/// — the error class, the same value the body's `__type` discriminator carries,
/// which is how [`state_unavailable`] fills it too.
pub fn account_limit_exceeded() -> crate::ops::types::shared_types::InternalServerErrorException {
    crate::ops::types::shared_types::InternalServerErrorException {
        message: Some(ACCOUNT_LIMIT_MESSAGE.to_string()),
        r#type: Some("InternalServerErrorException".to_string()),
    }
}

/// The wire name of the `IpSetId` member, as the request carries it
/// (`smithy_query(operation="GetIPSet")` binds it as the URI label `IpSetId`,
/// rendered `ipSetId` in the JSON body of the sibling operations). The family's own
/// absence refusal names no member ([`SET_ID_NOT_FOUND_MESSAGE`]), so this is the
/// wire name itself and not a rejection's subject.
pub const IP_SET_ID_MEMBER: &str = "ipSetId";

/// The message the uploaded-list families — an IPSet and a threat list, read,
/// updated or deleted — refuse an id the detector does not hold with:
/// [`NO_SUCH_RESOURCE_MESSAGE`], one fixed sentence that names neither the member
/// nor the value, and the same one whatever the FORM of the id, since a list that
/// was never minted, one minted under another detector and a value that could never
/// have been minted at all are the same absence.
pub const SET_ID_NOT_FOUND_MESSAGE: &str = NO_SUCH_RESOURCE_MESSAGE;

/// The wire name of the `Name` member an IPSet create or update carries: both
/// bind it in the JSON body under `jsonName` `name`
/// (`smithy_query(operation="CreateIPSet")`). It is what a rejection names,
/// AS THE REQUEST CARRIES IT.
pub const IP_SET_NAME_MEMBER: &str = "name";

/// The name GuardDuty gives the IPSet `Location` member in a rejection of it.
///
/// It is the ONE member name where the service does not use the request's own
/// spelling: the create and the update bind the member in the JSON body under
/// `jsonName` `location` (`smithy_query(operation="CreateIPSet")`), while the
/// service names the parameter it refuses by its resource-qualified name — the
/// same convention its other IPSet-qualified member names follow
/// ([`IP_SET_ID_MEMBER`]).
pub const IP_SET_LOCATION_MEMBER: &str = "ipSetLocation";

/// Does `name` hold only the characters an IPSet `Name` may contain?
///
/// "Allowed characters are alphanumeric, whitespace, dash (-), and underscores
/// (_)" (`smithy_query(query="doc",shape="CreateIPSetRequest",member="Name")`)
/// — a documented constraint the model's `len 1..300` does not express, so it is
/// enforced here. The whitespace a user-friendly name carries is the space: an
/// ASCII control character (a tab, a newline) is not a character such a name
/// displays.
pub fn is_ip_set_name(name: &str) -> bool {
    is_list_name(name, true)
}

/// Does `name` hold only characters from the alphabet an uploaded list's `Name`
/// is documented to allow — ASCII alphanumerics, dash (-) and underscore (_),
/// plus the space when `space_allowed`?
///
/// The list families document the same alphabet and differ only on whether the
/// space is in it, so the alphabet lives in ONE place and each family's
/// predicate ([`is_ip_set_name`], [`is_entity_set_name`]) says which of the two
/// it is. An empty name is in neither, though the modelled `len 1..300` already
/// refuses it before any handler runs.
fn is_list_name(name: &str, space_allowed: bool) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || (space_allowed && b == b' ')
        })
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

    /// Would [`Self::set_activation`] move anything — does `activate` ask for a
    /// status this IPSet has neither settled on nor already started towards?
    ///
    /// Answered WITHOUT writing, so a caller can decide whether its request
    /// changes this record at all BEFORE it reads the store's clock. The answer
    /// does not depend on whether [`Self::settle`] has run yet: settling only ever
    /// moves a status to the other member of the same pair.
    pub fn activation_changes(&self, activate: bool) -> bool {
        let (settled, transitional) = if activate {
            (IP_SET_ACTIVE, IP_SET_ACTIVATING)
        } else {
            (IP_SET_INACTIVE, IP_SET_DEACTIVATING)
        };
        self.status != settled && self.status != transitional
    }

    /// Start the transition the caller's `Activate` asks for, from whatever status
    /// this IPSet currently has.
    ///
    /// Asking for the status it already has owes nothing, so a repeated request
    /// leaves the settled status alone instead of re-opening a transition — the
    /// same question [`Self::activation_changes`] answers, asked through it so the
    /// two cannot drift.
    pub fn set_activation(&mut self, activate: bool, now: i64) {
        if !self.activation_changes(activate) {
            return;
        }
        let transitional = if activate {
            IP_SET_ACTIVATING
        } else {
            IP_SET_DEACTIVATING
        };
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
pub const INVESTIGATION_DAY_MILLIS: i64 = DAY_MILLIS;

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

/// `OrderBy`, the order a sort runs in: the enum is `ASC | DESC`
/// (`smithy_query(shape="OrderBy")`), the two values the generated
/// [`crate::ops::types::shared_types::ORDER_BY_VALUES`] list carries. Every sort
/// in this service reads the member through these two constants, so none of them
/// can spell the order differently.
pub const ORDER_BY_ASCENDING: &str = "ASC";

/// The other `OrderBy` member — see [`ORDER_BY_ASCENDING`].
pub const ORDER_BY_DESCENDING: &str = "DESC";

/// The attributes a `ListInvestigations` sort may name: the
/// `InvestigationSortField` members, which the model lists as `START_TIME |
/// END_TIME | STATUS | RISK_LEVEL | CONFIDENCE`
/// (`smithy_query(shape="InvestigationSortField")`) and the generated enum carries
/// as [`crate::ops::types::shared_types::INVESTIGATION_SORT_FIELD_VALUES`]. Read
/// from the generated list so the two can never drift.
pub fn investigation_sort_fields() -> &'static [&'static str] {
    crate::ops::types::shared_types::INVESTIGATION_SORT_FIELD_VALUES
}

/// The attribute a `ListInvestigations` sort orders by when the request names
/// none: when the investigation started, which with [`DESCENDING_BY_DEFAULT`] puts
/// the newest investigation first — the order the other reporting operations of
/// this service answer an unordered request in.
pub const INVESTIGATION_DEFAULT_SORT_FIELD: &str = "START_TIME";

/// Resolve one investigation attribute by its `InvestigationSortField` name, or
/// `None` when that is not a member of the enum.
///
/// `RISK_LEVEL` and `CONFIDENCE` are produced by the analysis
/// ([`INVESTIGATION_ENGINE_UNAVAILABLE`]), which this emulator does not run, so
/// they resolve to the neutral text key rather than to a synthesised assessment:
/// ordering by an absent value is ordering by the empty string, exactly as
/// [`malware_scan_attribute`] treats a scan result nothing produced.
pub fn investigation_attribute(investigation: &InvestigationRecord, field: &str) -> Option<Attribute> {
    let value = match field {
        "START_TIME" => Attribute::Number(investigation.started_at_millis as f64),
        "END_TIME" => Attribute::Number(investigation.ended_at_millis.unwrap_or(0) as f64),
        "STATUS" => Attribute::Text(investigation.status.clone()),
        "RISK_LEVEL" | "CONFIDENCE" => Attribute::Text(String::new()),
        _ => return None,
    };
    Some(value)
}

/// Order `investigations` for `ListInvestigations`.
///
/// With no `SortCriteria` the newest investigation comes first, with the
/// investigation id breaking ties so the order is TOTAL and a page token means the
/// same thing on the call that hands it back. `Err(attribute)` means the request
/// named something that is not an `InvestigationSortField` member.
pub fn sort_investigations(
    investigations: &mut [&InvestigationRecord],
    sort: Option<&crate::ops::types::InvestigationSortCriteria>,
) -> Result<(), String> {
    let attribute = sort
        .and_then(|s| s.attribute_name.as_deref())
        .filter(|a| !a.is_empty());
    let descending = match sort.and_then(|s| s.order_by.as_deref()) {
        Some(ORDER_BY_ASCENDING) => false,
        Some(ORDER_BY_DESCENDING) => true,
        // No explicit order: newest-first for the default attribute, ascending for
        // an attribute the caller named — the rule `sort_malware_scans` follows.
        _ => attribute.is_none() && DESCENDING_BY_DEFAULT,
    };
    let attribute = attribute.unwrap_or(INVESTIGATION_DEFAULT_SORT_FIELD);
    if !investigation_sort_fields().contains(&attribute) {
        return Err(attribute.to_string());
    }

    investigations.sort_by(|a, b| {
        compare_by_attribute(
            *a,
            *b,
            descending,
            |investigation| investigation_attribute(investigation, attribute),
            |investigation| investigation.id.clone(),
        )
    });
    Ok(())
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

/// The `TrustedEntitySetStatus` a set an update asked to STOP using is in until
/// the clock owes it [`ENTITY_SET_INACTIVE`]: the mirror of
/// [`ENTITY_SET_ACTIVATING`]. See [`ENTITY_SET_INACTIVE`] for the enum.
pub const ENTITY_SET_DEACTIVATING: &str = "DEACTIVATING";

/// The `TrustedEntitySetStatus` a set whose deletion has been accepted is in: the
/// set is on its way out of use. See [`ENTITY_SET_INACTIVE`] for the enum.
pub const ENTITY_SET_DELETE_PENDING: &str = "DELETE_PENDING";

/// The `TrustedEntitySetStatus` a deleted set reaches once the clock owes it.
/// TERMINAL — nothing moves a deleted entity set to any other status. See
/// [`ENTITY_SET_INACTIVE`] for the enum.
pub const ENTITY_SET_DELETED: &str = "DELETED";

/// How long a set the caller asked to activate stays [`ENTITY_SET_ACTIVATING`],
/// a set an update asked to stop using stays [`ENTITY_SET_DEACTIVATING`], or a
/// set whose delete was accepted stays [`ENTITY_SET_DELETE_PENDING`], before
/// the clock owes it the settled status.
///
/// AWS documents no duration for taking an uploaded list into use or for removing
/// one, so this span is CHOSEN — the same one
/// [`INVESTIGATION_TRANSITION_MILLIS`] uses, and for the same reason: non-zero, so
/// a caller reading straight after the create or the delete still observes the
/// transient status, and short enough that nothing has to wait long for the
/// settled one.
pub const ENTITY_SET_TRANSITION_MILLIS: i64 = 500;

/// How many DELETED entity sets one of a detector's entity-set maps retains so a
/// read can still report [`ENTITY_SET_DELETE_PENDING`] / [`ENTITY_SET_DELETED`].
///
/// A deleted record is kept because `GetTrustedEntitySet`'s `Status` is
/// `@required` and the enum carries both
/// (`smithy_query(operation="GetTrustedEntitySet")`), so the status after a delete
/// is something a read reports rather than a 400. The retention is BOUNDED for
/// the same reason [`DELETED_IP_SETS_RETAINED`] bounds the IPSet one: a caller
/// creating and deleting in a loop must not be able to make the service retain
/// records without limit. It reuses [`THREAT_ENTITY_SETS_PER_DETECTOR`], the
/// number of LIVE entity sets a detector's larger entity-set map holds.
pub const DELETED_ENTITY_SETS_RETAINED: usize = THREAT_ENTITY_SETS_PER_DETECTOR;

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
/// The alphabet is the shared one ([`is_list_name`]) WITHOUT the space, which
/// the entity-set documentation does not list.
pub fn is_entity_set_name(name: &str) -> bool {
    is_list_name(name, false)
}

/// Is `location` a valid Amazon S3 URL for an UPLOADED LIST?
///
/// "The format of the Location URL must be a valid Amazon S3 URL format. Invalid
/// URL formats will result in an error, regardless of whether you activate the
/// entity set or not"
/// (`smithy_query(query="doc",shape="CreateTrustedEntitySetRequest",member="Location")`).
///
/// The rule is the list FAMILY's, not the entity set's: every uploaded list's
/// `Location` is "the URI of the file that contains the …"
/// (`smithy_query(query="doc",shape="CreateIPSetRequest",member="Location")` and
/// its threat-list sibling) — the file GuardDuty fetches from Amazon S3 — so the
/// IPSet, the threat list and the entity sets are all held to this one syntax,
/// and a `Location` the service could never fetch is refused instead of stored.
/// The modelled constraint on each of those members is a `len 1..300` with no
/// pattern, so the handler is the enforcement point.
///
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
    /// The instant a delete was ACCEPTED for this set, or `None` while no delete
    /// has been. It is the key [`evict_deleted_entity_sets`] evicts the retained
    /// deleted records by, oldest deletion first, so it records the acceptance and
    /// nothing else moves it — not the clock settling
    /// [`ENTITY_SET_DELETE_PENDING`] to [`ENTITY_SET_DELETED`], which rewrites
    /// [`Self::updated_at_millis`] to the instant the settled status was owed and
    /// would otherwise reorder two deletions accepted within one span.
    pub deleted_at_millis: Option<i64>,
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
    ///
    /// The bucket owner the record keeps is resolved through
    /// [`entity_set_bucket_owner`], so a set created without one still knows which
    /// account owns the location it was uploaded from.
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
            expected_bucket_owner: entity_set_bucket_owner(expected_bucket_owner.as_deref()),
            tags,
            client_token,
            status: status.to_string(),
            status_due_millis,
            created_at_millis: now,
            updated_at_millis: now,
            deleted_at_millis: None,
            error_details: None,
        }
    }

    /// Has a delete already been accepted for this entity set — is it
    /// [`ENTITY_SET_DELETE_PENDING`] or [`ENTITY_SET_DELETED`]?
    ///
    /// Such a record is no longer a live entity set: it is still REPORTED, so a
    /// read can answer with the status the delete put it in, but GuardDuty is not
    /// keeping it, so it occupies none of the detector's entity-set slots.
    ///
    /// [`Self::deleted_at_millis`] answers it, so the instant a delete was accepted
    /// and the fact that one was are the SAME field and cannot drift:
    /// [`Self::begin_delete`] is the only thing that accepts a deletion, and it is
    /// what puts the set in [`ENTITY_SET_DELETE_PENDING`], which [`Self::settle`]
    /// only ever moves on to [`ENTITY_SET_DELETED`].
    pub fn is_deleting(&self) -> bool {
        self.deleted_at_millis.is_some()
    }

    /// Accept this entity set's deletion: [`ENTITY_SET_DELETE_PENDING`] now, with
    /// the clock owing [`ENTITY_SET_DELETED`] at
    /// [`ENTITY_SET_TRANSITION_MILLIS`]. Reports whether this call is the one that
    /// accepted it.
    ///
    /// A record already being deleted is left exactly where it is — neither the
    /// status nor the instant the clock owes it moves — so a repeated delete
    /// neither restarts the transition nor walks a `DELETED` record backwards, and
    /// it answers `false` so a caller writes nothing else either.
    pub fn begin_delete(&mut self, now: i64) -> bool {
        if self.is_deleting() {
            return false;
        }
        self.status = ENTITY_SET_DELETE_PENDING.to_string();
        self.status_due_millis = Some(now.saturating_add(ENTITY_SET_TRANSITION_MILLIS));
        self.updated_at_millis = now;
        self.deleted_at_millis = Some(now);
        true
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of [`ENTITY_SET_ACTIVATING`], [`ENTITY_SET_DEACTIVATING`] or
    /// [`ENTITY_SET_DELETE_PENDING`] was owed at [`Self::status_due_millis`]
    /// whether or not anybody called, which is why a reporting operation may apply
    /// it. `UpdatedAt` records the instant it was OWED, not the instant somebody
    /// looked, so two reads of a settled set agree.
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
            ENTITY_SET_DEACTIVATING => ENTITY_SET_INACTIVE.to_string(),
            ENTITY_SET_DELETE_PENDING => ENTITY_SET_DELETED.to_string(),
            other => other.to_string(),
        };
        self.status_due_millis = None;
        self.updated_at_millis = due;
    }

    /// Start the transition the `Activate` an UPDATE carries asks for, from
    /// whatever status this set currently has: [`ENTITY_SET_ACTIVATING`] then
    /// [`ENTITY_SET_ACTIVE`] when the caller asks GuardDuty to start using the
    /// set, [`ENTITY_SET_DEACTIVATING`] then [`ENTITY_SET_INACTIVE`] when it asks
    /// it to stop. It is the entity-set counterpart of
    /// [`ThreatIntelSetRecord::set_activation`], shared by both entity-set
    /// updates so the trusted and the threat set cannot drift on what `Activate`
    /// does.
    ///
    /// Asking for the status the set has already settled on, or is already
    /// transitioning to, owes nothing and answers `false`, so a repeated request
    /// leaves the status and the instant the clock owes it exactly where they are
    /// rather than re-opening a transition. Asking for the other one replaces
    /// whatever was owed, so a request that reverses a transition still running
    /// turns it around instead of queueing behind it.
    ///
    /// The transition is never immediate: the clock owes the settled status
    /// [`ENTITY_SET_TRANSITION_MILLIS`] later and only [`Self::settle`]
    /// materialises it, so a caller reading straight afterwards still observes
    /// the transitional status.
    pub fn set_activation(&mut self, activate: bool, now: i64) -> bool {
        if !self.activation_changes(activate) {
            return false;
        }
        let transitional = if activate {
            ENTITY_SET_ACTIVATING
        } else {
            ENTITY_SET_DEACTIVATING
        };
        self.status = transitional.to_string();
        self.status_due_millis = Some(now.saturating_add(ENTITY_SET_TRANSITION_MILLIS));
        true
    }

    /// Would [`Self::set_activation`] move anything — does `activate` ask for a
    /// status this set has neither settled on nor already started towards?
    ///
    /// Answered WITHOUT writing, so a caller can decide whether its request
    /// changes this record at all BEFORE it reads the store's clock. The answer
    /// does not depend on whether [`Self::settle`] has run yet: settling only ever
    /// moves a status to the other member of the same pair.
    pub fn activation_changes(&self, activate: bool) -> bool {
        let (settled, transitional) = if activate {
            (ENTITY_SET_ACTIVE, ENTITY_SET_ACTIVATING)
        } else {
            (ENTITY_SET_INACTIVE, ENTITY_SET_DEACTIVATING)
        };
        self.status != settled && self.status != transitional
    }
}

/// Accept the deletion of the entity set `id` names within `sets`, one of a
/// detector's entity-set maps.
///
/// Shared by BOTH entity-set deletes, so the trusted and the threat set cannot
/// drift on what a delete does: the set moves to [`ENTITY_SET_DELETE_PENDING`]
/// and the clock owes it [`ENTITY_SET_DELETED`], and a set already being deleted
/// is left exactly where it is. `None` when `sets` holds no such id.
///
/// Keeping a deleted record is what lets a read report the status the delete put
/// it in, so this is also where that retention is BOUNDED: at most
/// [`DELETED_ENTITY_SETS_RETAINED`] deleted records survive per map.
pub fn delete_entity_set(
    sets: &mut BTreeMap<String, EntitySetRecord>,
    id: &str,
    now: i64,
) -> Option<bool> {
    let accepted = sets.get_mut(id)?.begin_delete(now);
    if accepted {
        evict_deleted_entity_sets(sets, id);
    }
    Some(accepted)
}

/// Drop deleted entity sets, oldest first, until at most
/// [`DELETED_ENTITY_SETS_RETAINED`] remain. `keep` is never evicted: it is the
/// record the call that triggered this just deleted, and a caller reads that one
/// back.
fn evict_deleted_entity_sets(sets: &mut BTreeMap<String, EntitySetRecord>, keep: &str) {
    let surplus = sets
        .values()
        .filter(|set| set.is_deleting())
        .count()
        .saturating_sub(DELETED_ENTITY_SETS_RETAINED);
    if surplus == 0 {
        return;
    }
    // The oldest deletions first: `deleted_at_millis` is the instant the delete was
    // ACCEPTED and nothing moves it afterwards — unlike `updated_at_millis`, which
    // `EntitySetRecord::settle` rewrites to the instant `DELETED` was owed — and
    // `tick_millis` makes those instants strictly increasing, so the ordering is
    // total and the id only breaks a tie that cannot arise.
    let mut deleted: Vec<(i64, String)> = sets
        .values()
        .filter(|set| set.is_deleting() && set.id != keep)
        .map(|set| (set.deleted_at_millis.unwrap_or_default(), set.id.clone()))
        .collect();
    deleted.sort_unstable();
    for (_, id) in deleted.into_iter().take(surplus) {
        sets.remove(&id);
    }
}

/// A saved finding filter. `CreateFilter` writes it, `UpdateFilter` changes it,
/// `GetFilter` reports it, `ListFilters` names it, `DeleteFilter` drops it and
/// `DeleteDetector`'s cascade drops it with its detector.
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

impl FilterRecord {
    /// Does this filter ARCHIVE the findings its criteria select?
    ///
    /// The stored action is one of the two `FilterAction` values
    /// (`smithy_query(shape="FilterAction")`) and only
    /// [`FILTER_ACTION_ARCHIVE`] is applied to a finding; a filter carrying the
    /// other value, or carrying none at all (whose default is
    /// [`FILTER_ACTION_NOOP`]), leaves every finding exactly as it is.
    pub fn archives_matching_findings(&self) -> bool {
        self.action.as_deref() == Some(FILTER_ACTION_ARCHIVE)
    }
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
    /// The instant a delete was ACCEPTED for this list, or `None` while no delete
    /// has been. It is the key [`Detector::delete_threat_intel_set`] evicts the
    /// retained deleted records by, oldest deletion first, so it records the
    /// acceptance and nothing else moves it — not the clock settling
    /// [`THREAT_INTEL_SET_DELETE_PENDING`] to [`THREAT_INTEL_SET_DELETED`], which
    /// would otherwise reorder two deletions accepted within one span.
    pub deleted_at_millis: Option<i64>,
}

impl ThreatIntelSetRecord {
    /// Materialise the status change the clock has ALREADY made due.
    ///
    /// The change is owed at its due instant whether or not anybody called, so a
    /// reporting operation may apply it: it shows no caller anything it would not
    /// have seen anyway, and it never moves a status the clock does not owe.
    ///
    /// The status it settles to was fixed when the transition started, not when
    /// somebody looked, so two reads of a settled list agree.
    pub fn settle(&mut self, now_millis: i64) {
        let Some((next, due)) = self.pending_status.clone() else {
            return;
        };
        if now_millis >= due {
            self.status = next;
            self.pending_status = None;
        }
    }

    /// Has a delete already been accepted for this threat list — is it
    /// [`THREAT_INTEL_SET_DELETE_PENDING`] or [`THREAT_INTEL_SET_DELETED`]?
    ///
    /// Such a record is no longer a live threat list: it is still REPORTED, so a
    /// read can answer with the status the delete put it in, but GuardDuty is not
    /// using it, so it occupies none of the detector's
    /// [`THREAT_INTEL_SETS_PER_DETECTOR`] slots and no list names it.
    pub fn is_deleting(&self) -> bool {
        self.deleted_at_millis.is_some()
    }

    /// Accept this list's deletion: [`THREAT_INTEL_SET_DELETE_PENDING`] now, with
    /// the clock owing [`THREAT_INTEL_SET_DELETED`] at
    /// [`THREAT_INTEL_SET_DELETION_SPAN_MILLIS`]. Reports whether this call is the
    /// one that accepted it.
    ///
    /// A record already being deleted is left exactly where it is — neither the
    /// status nor the instant the clock owes it moves — so a repeated delete
    /// neither restarts the transition nor walks a `DELETED` record backwards, and
    /// it answers `false` so a caller writes nothing else either. Whatever status
    /// the list was in when the delete arrived, including a transition the clock
    /// had not settled yet, the deletion replaces what was owed: nothing activates
    /// a list on its way out.
    pub fn begin_delete(&mut self, now: i64) -> bool {
        if self.is_deleting() {
            return false;
        }
        self.status = THREAT_INTEL_SET_DELETE_PENDING.to_string();
        self.pending_status = Some((
            THREAT_INTEL_SET_DELETED.to_string(),
            now.saturating_add(THREAT_INTEL_SET_DELETION_SPAN_MILLIS),
        ));
        self.deleted_at_millis = Some(now);
        true
    }

    /// Start the transition `Activate` asks for, from whatever status this list
    /// currently has: [`THREAT_INTEL_SET_ACTIVATING`] then
    /// [`THREAT_INTEL_SET_ACTIVE`] when it asks for the list to be used,
    /// [`THREAT_INTEL_SET_DEACTIVATING`] then [`THREAT_INTEL_SET_INACTIVE`] when it
    /// asks for it to stop.
    ///
    /// Asking for the status the list has already settled on, or is already
    /// transitioning to, owes nothing, so a repeated request leaves the status and
    /// the instant the clock owes it exactly where they are rather than re-opening
    /// a transition. Asking for the other one replaces whatever was owed, so a
    /// request that reverses a transition still running turns it around instead of
    /// queueing behind it.
    ///
    /// The transition is never immediate: the clock owes the settled status
    /// [`THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS`] later, and only
    /// [`Self::settle`] materialises it.
    pub fn set_activation(&mut self, activate: bool, now: i64) {
        if !self.activation_changes(activate) {
            return;
        }
        let (settled, transitional) = if activate {
            (THREAT_INTEL_SET_ACTIVE, THREAT_INTEL_SET_ACTIVATING)
        } else {
            (THREAT_INTEL_SET_INACTIVE, THREAT_INTEL_SET_DEACTIVATING)
        };
        self.status = transitional.to_string();
        self.pending_status = Some((
            settled.to_string(),
            now.saturating_add(THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS),
        ));
    }

    /// Would [`Self::set_activation`] move anything — does `activate` ask for a
    /// status this list has neither settled on nor already started towards?
    ///
    /// Answered WITHOUT writing, so a caller can decide whether its request
    /// changes this record at all BEFORE it reads the store's clock. The answer
    /// does not depend on whether [`Self::settle`] has run yet: settling only ever
    /// moves a status to the other member of the same pair.
    pub fn activation_changes(&self, activate: bool) -> bool {
        let (settled, transitional) = if activate {
            (THREAT_INTEL_SET_ACTIVE, THREAT_INTEL_SET_ACTIVATING)
        } else {
            (THREAT_INTEL_SET_INACTIVE, THREAT_INTEL_SET_DEACTIVATING)
        };
        self.status != settled && self.status != transitional
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
/// The TRANSIENT status a list asked to stop being used reports until that change
/// comes due — the counterpart of [`THREAT_INTEL_SET_ACTIVATING`]
/// (`smithy_query(shape="ThreatIntelSetStatus")` carries both). See
/// [`ThreatIntelSetRecord::set_activation`].
pub const THREAT_INTEL_SET_DEACTIVATING: &str = "DEACTIVATING";

/// How long a list asked to activate spends in [`THREAT_INTEL_SET_ACTIVATING`]
/// before the clock owes it [`THREAT_INTEL_SET_ACTIVE`], and — the change being
/// the same one in reverse — how long a list asked to stop being used spends in
/// [`THREAT_INTEL_SET_DEACTIVATING`] before the clock owes it
/// [`THREAT_INTEL_SET_INACTIVE`].
///
/// AWS documents no duration for ingesting and activating a threat list, so this
/// span is CHOSEN: non-zero, so a caller reading straight after the create still
/// observes the transient status the real service reports, and short enough that
/// the activation lands without anybody sleeping on it.
pub const THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS: i64 = 500;

/// `ThreatIntelSetStatus`: a delete has been accepted and the list is on its way
/// out of use. TRANSIENT — the clock owes it [`THREAT_INTEL_SET_DELETED`]
/// (`smithy_query(shape="ThreatIntelSetStatus")` carries both).
pub const THREAT_INTEL_SET_DELETE_PENDING: &str = "DELETE_PENDING";

/// `ThreatIntelSetStatus`: the list has been deleted. Terminal — nothing moves a
/// deleted list anywhere else.
pub const THREAT_INTEL_SET_DELETED: &str = "DELETED";

/// How long a list a delete has accepted spends in
/// [`THREAT_INTEL_SET_DELETE_PENDING`] before the clock owes it
/// [`THREAT_INTEL_SET_DELETED`].
///
/// AWS documents no duration for removing a threat list, so this span is CHOSEN,
/// matching the activation span this record already uses: non-zero, so a caller
/// reading straight after the delete still observes the transient
/// `DELETE_PENDING` the real service reports, and short enough that the deletion
/// lands without anybody sleeping on it.
pub const THREAT_INTEL_SET_DELETION_SPAN_MILLIS: i64 = THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS;

/// How many DELETED threat-list records one detector retains, so a read can still
/// report the [`THREAT_INTEL_SET_DELETE_PENDING`] / [`THREAT_INTEL_SET_DELETED`]
/// status a delete moved a list to.
///
/// Retention needs its own ceiling because [`THREAT_INTEL_SETS_PER_DETECTOR`]
/// bounds only the LIVE lists: a caller creating and deleting in a loop would
/// otherwise make the detector keep a record per request. The ceiling is the live
/// quota, the way [`DELETED_IP_SETS_RETAINED`] is the IPSet one.
pub const DELETED_THREAT_INTEL_SETS_RETAINED: usize = THREAT_INTEL_SETS_PER_DETECTOR;

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
    /// The on-demand object scans `SendObjectMalwareScan` asked this plan for,
    /// oldest first — see [`ObjectMalwareScanRequest`]. They hang off the PLAN
    /// because the plan is what the API requires ("The bucket must have a Malware
    /// Protection plan configured to use this API",
    /// `smithy_query(query="doc",shape="SendObjectMalwareScanRequest",member="S3Object")`),
    /// so a plan a delete removes takes the scans it accepted with it.
    ///
    /// BOUNDED at [`MAX_OBJECT_MALWARE_SCANS_PER_PLAN`] entries, oldest first:
    /// write it through [`MalwareProtectionPlan::record_object_scan`], never by
    /// pushing directly.
    pub object_scans: Vec<ObjectMalwareScanRequest>,
}

impl MalwareProtectionPlan {
    /// Is `key` an object this plan scans — does it fall inside the plan's
    /// configured scope?
    ///
    /// The scope is [`Self::object_prefixes`]: "The S3 object will be scanned only
    /// if it belongs to any of the specified object prefixes"
    /// (`smithy_query(query="doc",shape="CreateS3BucketResource",member="ObjectPrefixes")`),
    /// so a key is in scope when it STARTS WITH one of them — an S3 object key
    /// belongs to a prefix exactly when the prefix is a prefix of the key, compared
    /// byte for byte with no case folding, trimming or separator fixing, because an
    /// S3 key is an opaque sequence of characters and `a/b` and `A/B` are two
    /// different objects. An EMPTY list configures no restriction and so covers the
    /// whole bucket, which is what both an absent prefix list and an explicitly
    /// empty one ask for.
    pub fn scans_object_key(&self, key: &str) -> bool {
        self.object_prefixes.is_empty()
            || self
                .object_prefixes
                .iter()
                .any(|prefix| key.starts_with(prefix.as_str()))
    }

    /// Record one accepted object scan against this plan, dropping the oldest
    /// entries past [`MAX_OBJECT_MALWARE_SCANS_PER_PLAN`].
    ///
    /// Eviction rather than refusal: the caller is handed no identifier for the
    /// scan it asked for (`SendObjectMalwareScan`'s response has no members —
    /// `smithy_query(shape="SendObjectMalwareScanResponse")`) and no operation
    /// reports one, so no caller can hold a reference to an entry this drops. A
    /// send the real service accepts is therefore never refused for the sake of a
    /// store guard.
    pub fn record_object_scan(&mut self, request: ObjectMalwareScanRequest) {
        self.object_scans.push(request);
        let overflow = self
            .object_scans
            .len()
            .saturating_sub(MAX_OBJECT_MALWARE_SCANS_PER_PLAN);
        if overflow > 0 {
            self.object_scans.drain(..overflow);
        }
    }
}

/// One on-demand S3 object scan `SendObjectMalwareScan` accepted, held on the
/// [`MalwareProtectionPlan`] that protects the object's bucket.
///
/// It records WHAT was asked for and WHEN, and nothing else: GuardDuty Malware
/// Protection would read the object's bytes and scan them, which this emulator has
/// no scanner and no S3 object content for. So no verdict, status, tag or file count
/// is synthesised for a scan nothing ran — the same line
/// [`MALWARE_SCAN_FAILED`] draws for an EC2 scan, except that this operation models
/// no result for a caller to read at all: its response carries no members
/// (`smithy_query(shape="SendObjectMalwareScanResponse")`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMalwareScanRequest {
    /// `S3Object.Key`, stored verbatim: "the full key path of the object within the
    /// bucket" (`smithy_query(shape="S3ObjectForSendObjectMalwareScan")`). The
    /// bucket is not repeated here — the plan this record hangs off names it.
    pub object_key: String,
    /// `S3Object.VersionId`, when the request named one. "If not specified, the
    /// latest version of the object is scanned"
    /// (`smithy_query(shape="S3ObjectForSendObjectMalwareScan")`), so an absent
    /// version id is stored as absent rather than as a value.
    pub version_id: Option<String>,
    /// When the send was accepted, in epoch milliseconds.
    pub requested_at_millis: i64,
}

/// The most accepted object scans one Malware Protection plan retains.
///
/// A STORE GUARD, not a modelled quota, like [`MAX_CLIENT_TOKENS`]: AWS publishes no
/// ceiling on how many objects may be sent for scanning, the object key is
/// request-chosen, and an accepted scan has no expiry of its own, so without this a
/// caller could make one plan retain records without limit. Per PLAN, so the whole
/// log is bounded by this times the plan ceilings
/// ([`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`] across
/// [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`] Regions).
pub const MAX_OBJECT_MALWARE_SCANS_PER_PLAN: usize = 200;

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

/// The wire name of `StartMalwareScan`'s `ResourceArn` member, for the same
/// reason. The tagging operations carry a `ResourceArn` of their own — the URI
/// label of `/tags/{ResourceArn}` (`smithy_query(operation="UntagResource")`) —
/// and a rejection names it by the same wire name.
pub const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The message a read of a scan this account and Region does not hold answers with.
///
/// The service's own constant wording for a resource it cannot find, which names
/// neither the member nor the value: "never existed" and "belongs to someone else"
/// are one refusal, so one account cannot probe another's scan ids. The literal is
/// the one the corpus records GuardDuty answering; the model's prose for the shape
/// ("The requested resource can't be found",
/// `smithy_query(shape="ResourceNotFoundException")`) describes the error, it is not
/// the wire message — do not "correct" the value to it.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found.";

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

    /// Put the destination back in [`PENDING_VERIFICATION`] and owe the move to
    /// [`PUBLISHING`] again, as of `now`.
    ///
    /// The destination properties say WHERE GuardDuty publishes and with which key,
    /// so a destination whose properties changed is one GuardDuty has not verified
    /// it can write to — the same state a create leaves behind, reached again by the
    /// same clock-owed span. This is what makes the status a destination reports
    /// follow its current properties: a destination whose properties were fixed is
    /// verified again rather than keeping the verdict reached for the properties it
    /// no longer has.
    pub fn begin_verification(&mut self, now: i64) {
        self.status = PENDING_VERIFICATION.to_string();
        self.verification_due_millis = Some(now.saturating_add(VERIFICATION_DELAY_MILLIS));
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

/// The second segment of a finding type — the AFFECTED RESOURCE TYPE, in GuardDuty's
/// own abbreviations: what sits between the `:` and the `/` of
/// `ThreatPurpose:ResourceTypeAffected/ThreatFamilyName`.
///
/// Shared because more than one operation reads that convention: the sample-finding
/// generator derives the `Resource.ResourceType` it stores from this segment, and the
/// usage report derives the feature whose data source generated the finding from it.
/// One reader, so the two can never disagree about what a finding type says.
pub fn finding_type_affected_resource(finding_type: &str) -> &str {
    finding_type
        .split_once(':')
        .map_or("", |(_, rest)| rest.split('/').next().unwrap_or(""))
}

/// The last segment of a finding type: the threat family name with its suffixes — the
/// part that says what the finding was generated FROM (`!DNS` for DNS query data, a
/// `…MaliciousFile` / `…SuspiciousFile` verdict for a malware scan).
pub fn finding_type_threat_family(finding_type: &str) -> &str {
    finding_type.rsplit('/').next().unwrap_or("")
}

/// The finding fields a STORED record populates, so the ones a criterion or a sort
/// actually discriminates on, as the `Finding`, `Resource` and `Service` shapes'
/// `jsonName`s name them (`smithy_query(shape="Finding")`,
/// `smithy_query(shape="Resource")`, `smithy_query(shape="Service")`).
///
/// This is NOT the set a criterion key may name — that is every field of a finding
/// ([`is_finding_field`]). A field a finding may carry but this emulator's records do
/// not populate selects no finding rather than refusing the request, exactly as a
/// finding without a value for it behaves on the real service.
pub const FINDING_ATTRIBUTES: [&str; 23] = [
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
    "service.additionalInfo.sample",
    "service.archived",
    "service.count",
    "service.detectorId",
    "service.eventFirstSeen",
    "service.eventLastSeen",
    "service.resourceRole",
    "service.serviceName",
    "service.userFeedback",
    "severity",
    "title",
    "type",
    "updatedAt",
];

/// The subset of [`FINDING_ATTRIBUTES`] that resolves to a NUMBER, so the subset an
/// ordering condition can compare. The timestamps are compared as epoch milliseconds
/// and reported as ISO-8601 text, which is how GuardDuty's own filters treat them
/// ("Type: Timestamp in Unix Epoch millisecond format",
/// `smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`).
const NUMERIC_FINDING_ATTRIBUTES: [&str; 7] = [
    "confidence",
    "createdAt",
    "service.count",
    "service.eventFirstSeen",
    "service.eventLastSeen",
    "severity",
    "updatedAt",
];

/// The `jsonName` of the `Finding` member carrying the finding's severity
/// (`smithy_query(shape="Finding")`), which is the one numeric finding field whose
/// value domain is a published SCALE rather than an open-ended number.
const SEVERITY_ATTRIBUTE: &str = "severity";

/// The lowest value on GuardDuty's finding severity scale.
///
/// The scale runs from 1 to 10 and is divided into the bands this service already
/// assigns a generated sample finding by its finding type — Low 1.0–3.9, Medium
/// 4.0–6.9, High 7.0–8.9, Critical 9.0–10.0. The model types the member as a plain
/// double and states no range at all
/// (`smithy_query(query="doc",shape="Finding",member="Severity")`), so the scale is
/// the service's own and is enforced here.
pub const FINDING_SEVERITY_MIN: f64 = 1.0;

/// The highest value on GuardDuty's finding severity scale. See
/// [`FINDING_SEVERITY_MIN`].
pub const FINDING_SEVERITY_MAX: f64 = 10.0;

/// The [`NUMERIC_FINDING_ATTRIBUTES`] that carry an INSTANT, and so the ones whose
/// values have two admissible forms: the epoch milliseconds a criterion's ordering
/// compares ("Type: Timestamp in Unix Epoch millisecond format",
/// `smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`)
/// and the ISO-8601 text a finding reports. Either is a value the field can hold,
/// so an equality value on one of them is not held to the numeric form.
const TIMESTAMP_FINDING_ATTRIBUTES: [&str; 4] = [
    "createdAt",
    "service.eventFirstSeen",
    "service.eventLastSeen",
    "updatedAt",
];

/// Can the finding field at `path` HOLD `value` — is it a value of that field?
///
/// A criterion carries every value as text, so the field's own form is what decides:
/// a field this service resolves as a number cannot hold a value that is not one,
/// and the one field whose numbers run over a published scale cannot hold a number
/// off that scale ([`FINDING_SEVERITY_MIN`]). A field resolved as text, an instant
/// (which has two admissible forms, see [`TIMESTAMP_FINDING_ATTRIBUTES`]) and a
/// field a stored record does not populate have no form here to contradict, so any
/// value is one they could hold — the same reasoning
/// [`attribute_is_orderable`] applies to an ordering condition.
fn is_finding_field_value(path: &str, value: &str) -> bool {
    if !NUMERIC_FINDING_ATTRIBUTES.contains(&path) || TIMESTAMP_FINDING_ATTRIBUTES.contains(&path) {
        return true;
    }
    let Ok(number) = value.parse::<f64>() else {
        return false;
    };
    if !number.is_finite() {
        return false;
    }
    if path != SEVERITY_ATTRIBUTE {
        return true;
    }
    (FINDING_SEVERITY_MIN..=FINDING_SEVERITY_MAX).contains(&number)
}

/// The `Finding` members that carry a SCALAR, by `jsonName`
/// (`smithy_query(shape="Finding")`): a criterion key or a sort attribute names one
/// of these on its own, with nothing beneath it.
const FINDING_SCALAR_FIELDS: [&str; 15] = [
    "accountId",
    "arn",
    "associatedAttackSequenceArn",
    "confidence",
    "createdAt",
    "customDetectionRuleName",
    "description",
    "id",
    "partition",
    "region",
    "schemaVersion",
    "severity",
    "title",
    "type",
    "updatedAt",
];

/// The `jsonName` of the `Finding` member carrying the affected resource, and of the
/// one carrying what the service observed (`smithy_query(shape="Finding")`). A
/// criterion key reaches BELOW the top level only through these two.
const RESOURCE_FIELD: &str = "resource";
/// See [`RESOURCE_FIELD`].
const SERVICE_FIELD: &str = "service";

/// The `Resource` members that carry a scalar, by `jsonName`
/// (`smithy_query(shape="Resource")`).
const RESOURCE_SCALAR_FIELDS: [&str; 1] = ["resourceType"];

/// The `Resource` members that carry a nested detail document, by `jsonName`
/// (`smithy_query(shape="Resource")`): the fields of that document are what the
/// remaining segments of a criterion key address.
const RESOURCE_DOCUMENT_FIELDS: [&str; 17] = [
    "accessKeyDetails",
    "bedrockGuardrailDetails",
    "containerDetails",
    "ebsSnapshotDetails",
    "ebsVolumeDetails",
    "ec2ImageDetails",
    "ecsClusterDetails",
    "eksClusterDetails",
    "instanceDetails",
    "kubernetesDetails",
    "lambdaDetails",
    "modelDetails",
    "rdsDbInstanceDetails",
    "rdsDbUserDetails",
    "rdsLimitlessDbDetails",
    "recoveryPointDetails",
    "s3BucketDetails",
];

/// The `Service` members that carry a scalar, by `jsonName`
/// (`smithy_query(shape="Service")`).
const SERVICE_SCALAR_FIELDS: [&str; 9] = [
    "archived",
    "count",
    "detectorId",
    "eventFirstSeen",
    "eventLastSeen",
    "featureName",
    "resourceRole",
    "serviceName",
    "userFeedback",
];

/// The `Service` members that carry a nested document, by `jsonName`
/// (`smithy_query(shape="Service")`). See [`RESOURCE_DOCUMENT_FIELDS`].
const SERVICE_DOCUMENT_FIELDS: [&str; 7] = [
    "action",
    "additionalInfo",
    "detection",
    "ebsVolumeScanDetails",
    "evidence",
    "malwareScanDetails",
    "runtimeDetails",
];

/// Is `path` a FIELD OF A FINDING — a name a `FindingCriteria.Criterion` key or a
/// `SortCriteria.AttributeName` may carry?
///
/// Decided from the finding's own shape rather than from what this emulator happens to
/// store, and before any finding is looked at, so the answer does not depend on how
/// many findings exist: the first segment must be a `Finding` member's `jsonName`, and
/// a path that continues must continue through the `resource` or `service`
/// sub-document, whose next segment must be a member of that shape. A member carrying
/// a scalar ENDS the path — there is nothing beneath it to name — while a member
/// carrying a nested detail document takes at least one further segment, naming a field
/// of that document the way the model's list of fields available for filtering does
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`).
pub fn is_finding_field(path: &str) -> bool {
    let mut segments = path.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let rest: Vec<&str> = segments.collect();
    match first {
        RESOURCE_FIELD => {
            sub_document_field(&rest, &RESOURCE_SCALAR_FIELDS, &RESOURCE_DOCUMENT_FIELDS)
        }
        SERVICE_FIELD => sub_document_field(&rest, &SERVICE_SCALAR_FIELDS, &SERVICE_DOCUMENT_FIELDS),
        _ => rest.is_empty() && FINDING_SCALAR_FIELDS.contains(&first),
    }
}

/// Do the segments BELOW `resource` or `service` name a field of that sub-document?
///
/// `scalars` are its members that end a path and `documents` those that take one, so
/// the two shapes are checked by the same rule and cannot drift apart.
fn sub_document_field(rest: &[&str], scalars: &[&str], documents: &[&str]) -> bool {
    let Some((member, deeper)) = rest.split_first() else {
        // The sub-document itself is not a field: there is no value to compare.
        return false;
    };
    if scalars.contains(member) {
        return deeper.is_empty();
    }
    documents.contains(member) && !deeper.is_empty() && deeper.iter().all(|s| is_field_name(s))
}

/// Is `segment` a well-formed field name — the lower camel-case run of ASCII letters
/// and digits every field name in the model's filterable-field list is?
fn is_field_name(segment: &str) -> bool {
    segment.starts_with(|c: char| c.is_ascii_lowercase())
        && segment.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Why a criterion key or a sort attribute was refused, with the message that says so.
///
/// One type for every operation that accepts a `FindingCriteria` or a `SortCriteria`,
/// so all of them answer an unacceptable name with the same words under their own
/// member name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CriterionRejection {
    /// The name is not a field of a finding at all, so nothing could ever be selected
    /// or ordered by it.
    UnknownField(String),
    /// It names a field, but not one an ordering condition can compare.
    NotOrderable(String),
    /// The criterion applies a `Condition` member the model restricts to the filter
    /// APIs — `matches` or `notMatches` — on an operation that is not one of them
    /// (see [`filter_only_condition`]). Carries that member's `jsonName`, which is a
    /// name from the MODEL and not a value from the request.
    ConditionNotAvailable(&'static str),
    /// A condition on the field asserts it equals a value that field cannot hold
    /// ([`is_finding_field_value`]). Carries the field path, which is a name from
    /// the request, so nothing of it reaches the message.
    ValueNotInDomain(String),
}

impl CriterionRejection {
    /// The refusal message, naming the request `member` that carried the name.
    ///
    /// GuardDuty answers a criteria structure it will not accept with its fixed
    /// invalid-value sentence for that member ([`invalid_parameter_value_message`]):
    /// which key inside the structure was at fault, and why, is not echoed — the
    /// offending name came from the request, and a refusal does not quote request
    /// input back. The variants are the same refusal to a caller and differ only
    /// in why this service would not take the criterion; they are kept apart because
    /// the readers that produce them answer different questions.
    pub fn message(&self, member: &str) -> String {
        match self {
            Self::UnknownField(_) | Self::NotOrderable(_) | Self::ConditionNotAvailable(_) => {
                invalid_parameter_value_message(member)
            }
            // A criteria structure whose SHAPE the service takes but whose
            // condition values it does not gets the sentence that says so: the
            // fault is located in a condition rather than in the member as a
            // whole, and the value is still not echoed.
            Self::ValueNotInDomain(_) => invalid_condition_values_message(member),
        }
    }
}

/// Resolve one finding attribute by its wire path, or `None` when the finding carries
/// NO VALUE for that field — either because the field is one this emulator's records do
/// not populate, or because this record has not got it (no feedback submitted, not a
/// generated sample).
///
/// `updatedAt` is accepted as well as `createdAt`; both are compared as epoch
/// milliseconds by the ordering conditions and as ISO-8601 text by the equality
/// ones, which is how GuardDuty's own filters treat them, and the service document's
/// two activity timestamps follow the same convention.
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
        // The additional-information document carries the `sample` marker on a
        // generated sample finding and nothing at all on anything else, so the field
        // is absent rather than false on a finding that is not a sample.
        "service.additionalInfo.sample" if finding.sample => Attribute::Text(true.to_string()),
        "service.archived" => Attribute::Text(finding.archived.to_string()),
        "service.count" => Attribute::Number(f64::from(finding.count)),
        "service.detectorId" => Attribute::Text(finding.detector_id.clone()),
        "service.eventFirstSeen" => Attribute::Number(finding.created_at_millis as f64),
        "service.eventLastSeen" => Attribute::Number(finding.event_last_seen_millis as f64),
        "service.resourceRole" => Attribute::Text(finding.resource_role.clone()),
        "service.serviceName" => Attribute::Text(GUARDDUTY_SERVICE_NAME.to_string()),
        // Absent until an `UpdateFindingsFeedback` call records one.
        "service.userFeedback" => Attribute::Text(finding.user_feedback.clone()?),
        "severity" => Attribute::Number(finding.severity),
        "title" => Attribute::Text(finding.title.clone().unwrap_or_default()),
        "type" => Attribute::Text(finding.r#type.clone()),
        _ => return None,
    };
    Some(attr)
}

/// A finding attribute rendered for EQUALITY comparison. A timestamp compares as its
/// ISO-8601 text — the form the finding reports it in — and every other attribute as
/// its own textual form.
fn equality_text(finding: &FindingRecord, path: &str) -> Option<String> {
    match path {
        "createdAt" | "service.eventFirstSeen" => Some(iso8601(finding.created_at_millis)),
        "updatedAt" => Some(iso8601(finding.updated_at_millis)),
        "service.eventLastSeen" => Some(iso8601(finding.event_last_seen_millis)),
        _ => finding_attribute(finding, path).map(|a| a.as_text()),
    }
}

/// Does `finding` satisfy one `Condition` on `path`?
///
/// `Err` means `path` names no field of a finding, which the caller turns into a
/// refusal rather than silently returning every finding. A finding that carries NO
/// VALUE for a field it could have satisfies only the negative conditions on it: an
/// absent value equals nothing and matches no pattern, so it cannot be selected by an
/// equality, a pattern or an ordering, and cannot be excluded by a negation.
/// Within one condition every populated operator must hold, which is how
/// GuardDuty composes a criterion.
pub fn condition_matches(
    finding: &FindingRecord,
    path: &str,
    condition: &crate::ops::types::Condition,
) -> Result<bool, CriterionRejection> {
    if !is_finding_field(path) {
        return Err(CriterionRejection::UnknownField(path.to_string()));
    }
    let resolved = finding_attribute(finding, path);
    let text = equality_text(finding, path);

    // `Equals`/`NotEquals` and the deprecated `Eq`/`Neq` are the same comparison;
    // a list holds alternatives, so `Equals` passes when ANY entry matches and
    // `NotEquals` passes only when NONE does.
    for list in [condition.equals.as_ref(), condition.eq.as_ref()].into_iter().flatten() {
        if list.is_empty() {
            continue;
        }
        if !text.as_ref().is_some_and(|value| list.contains(value)) {
            return Ok(false);
        }
    }
    for list in [condition.not_equals.as_ref(), condition.neq.as_ref()]
        .into_iter()
        .flatten()
    {
        if text.as_ref().is_some_and(|value| list.contains(value)) {
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
        let Some(value) = resolved.as_ref().and_then(Attribute::as_number) else {
            if !attribute_is_orderable(path) {
                return Err(CriterionRejection::NotOrderable(path.to_string()));
            }
            // A field with no value to order selects nothing.
            return Ok(false);
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
    // alternatives. The model offers the two only to the filter APIs, so the criteria
    // that reach here carrying them are a SAVED filter's — a findings read is refused
    // them up front ([`validate_finding_criteria`]) instead of evaluating them.
    if let Some(patterns) = condition.matches.as_ref() {
        let selected = text
            .as_ref()
            .is_some_and(|value| patterns.iter().any(|p| wildcard_matches(p, value)));
        if !patterns.is_empty() && !selected {
            return Ok(false);
        }
    }
    if let Some(patterns) = condition.not_matches.as_ref() {
        let excluded = text
            .as_ref()
            .is_some_and(|value| patterns.iter().any(|p| wildcard_matches(p, value)));
        if excluded {
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
) -> Result<bool, CriterionRejection> {
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

/// Which OPERATION CLASS a `FindingCriteria` arrived at, and so which `Condition`
/// members are available on it.
///
/// `Condition` is the one condition shape a `FindingCriteria.Criterion` targets, so the
/// filter APIs and the findings reads are handed the same structure and the shape alone
/// cannot say which members each of them takes; the model says it per member
/// ([`filter_only_condition`]). Every caller of [`validate_finding_criteria`] therefore
/// states which class it is, and a new operation taking a `FindingCriteria` cannot be
/// written without answering the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriteriaSurface {
    /// The filter APIs — the create and update that SAVE a `FindingCriteria` — on
    /// which every `Condition` member is available.
    Filter,
    /// A findings read that QUERIES with a `FindingCriteria`, on which the members the
    /// model reserves for the filter APIs are not available.
    FindingsQuery,
}

/// The `Condition` member the model documents as available only to the filter APIs that
/// `condition` applies, by `jsonName`, if it applies one.
///
/// The restriction is the model's, stated per member rather than by the shape: `Matches`
/// is "available only for create-filter and update-filter APIs"
/// (`smithy_query(query="doc",shape="Condition",member="Matches")`) and `NotMatches`
/// says the same of itself
/// (`smithy_query(query="doc",shape="Condition",member="NotMatches")`), while no other
/// member of the shape is narrowed that way. The pair is read here, in one place, so the
/// operations handed a `FindingCriteria` cannot disagree about which members a caller
/// may apply.
///
/// PRESENCE is what counts, not whether the list holds a pattern: a member the caller
/// sent is a condition the caller applied, and a member an operation does not offer is
/// not offered emptily either.
fn filter_only_condition(condition: &crate::ops::types::Condition) -> Option<&'static str> {
    [
        ("matches", condition.matches.is_some()),
        ("notMatches", condition.not_matches.is_some()),
    ]
    .into_iter()
    .find_map(|(member, present)| present.then_some(member))
}

/// Does a `Condition` on the finding field at `path` assert that field equals a
/// value it cannot hold?
///
/// `Equals` and the deprecated `Eq` are the members that CLAIM the field takes one
/// of the values they carry, so they are the ones held to the field's domain
/// ([`is_finding_field_value`]): a claim no value of the field can satisfy selects
/// nothing, whatever findings exist. The other members make no such claim and are
/// therefore not checked against it — `NotEquals`/`Neq` name values to EXCLUDE,
/// which is well defined for a value the field never takes (it excludes nothing),
/// and an ordering member carries a THRESHOLD rather than a value, so a bound past
/// the field's domain selects everything or nothing instead of being unsatisfiable.
fn condition_value_not_in_domain(path: &str, condition: &crate::ops::types::Condition) -> bool {
    [condition.equals.as_ref(), condition.eq.as_ref()]
        .into_iter()
        .flatten()
        .flatten()
        .any(|value| !is_finding_field_value(path, value))
}

/// Refuse a `FindingCriteria` this service cannot answer on `surface`, naming the field
/// path or the unavailable condition.
///
/// Checked BEFORE any finding is looked at, so whether a criterion is refused does
/// not depend on how many findings happen to be stored: a detector with none must
/// answer an unevaluable criterion exactly as a populated one does.
pub fn validate_finding_criteria(
    criteria: Option<&crate::ops::types::FindingCriteria>,
    surface: CriteriaSurface,
) -> Result<(), CriterionRejection> {
    let Some(criterion) = criteria.and_then(|c| c.criterion.as_ref()) else {
        return Ok(());
    };
    for (path, condition) in criterion {
        if !is_finding_field(path) {
            return Err(CriterionRejection::UnknownField(path.clone()));
        }
        // A condition the model offers only to the filter APIs is refused on a
        // findings read rather than evaluated there: the operation does not offer it,
        // so a caller who sends it is told so instead of being answered 200 with a
        // page selected by a condition this operation does not have.
        if surface == CriteriaSurface::FindingsQuery {
            if let Some(member) = filter_only_condition(condition) {
                return Err(CriterionRejection::ConditionNotAvailable(member));
            }
        }
        let orders_numerically = condition.greater_than.is_some()
            || condition.greater_than_or_equal.is_some()
            || condition.less_than.is_some()
            || condition.less_than_or_equal.is_some()
            || condition.gt.is_some()
            || condition.gte.is_some()
            || condition.lt.is_some()
            || condition.lte.is_some();
        if orders_numerically && !attribute_is_orderable(path) {
            return Err(CriterionRejection::NotOrderable(path.clone()));
        }
        // A criterion is a claim about the VALUE of a finding field, so a value the
        // field cannot hold is refused here rather than saved in a filter that can
        // never select anything or evaluated against findings it cannot describe.
        // Checked on every surface, for the same reason the field check is: the two
        // answers must not depend on which operation was handed the criteria.
        if condition_value_not_in_domain(path, condition) {
            return Err(CriterionRejection::ValueNotInDomain(path.clone()));
        }
    }
    Ok(())
}

/// Can an ordering condition compare the field at `path`?
///
/// A field this service resolves from a stored record has a KNOWN form, so an ordering
/// condition on one it resolves as text is refused rather than quietly selecting
/// nothing. A field a finding may carry but a stored record does not populate has no
/// form here to contradict, so an ordering condition on it is accepted and simply
/// selects no finding.
fn attribute_is_orderable(path: &str) -> bool {
    NUMERIC_FINDING_ATTRIBUTES.contains(&path) || !FINDING_ATTRIBUTES.contains(&path)
}

/// Order `findings` for `ListFindings` / `GetFindings`.
///
/// With no `SortCriteria` the newest finding comes first, with the finding id
/// breaking ties so the order is total and a page token stays meaningful. A sort
/// attribute that names no field of a finding is refused; one that names a field no
/// stored record populates leaves every record equal, so the tiebreaker alone orders
/// them.
pub fn sort_findings(
    findings: &mut [&FindingRecord],
    sort: Option<&crate::ops::types::SortCriteria>,
) -> Result<(), CriterionRejection> {
    let attribute = sort
        .and_then(|s| s.attribute_name.as_deref())
        .filter(|a| !a.is_empty());
    let descending = match sort.and_then(|s| s.order_by.as_deref()) {
        Some(ORDER_BY_ASCENDING) => false,
        Some(ORDER_BY_DESCENDING) => true,
        // No explicit order: newest-first for the default attribute, ascending for
        // an attribute the caller named.
        _ => attribute.is_none(),
    };
    let attribute = attribute.unwrap_or("createdAt");
    if !is_finding_field(attribute) {
        return Err(CriterionRejection::UnknownField(attribute.to_string()));
    }

    findings.sort_by(|a, b| {
        compare_by_attribute(
            *a,
            *b,
            descending,
            |finding| finding_attribute(finding, attribute),
            |finding| finding.id.clone(),
        )
    });
    Ok(())
}

/// Compare two records by one resolved attribute, in `descending` order or not,
/// with `tiebreak` making the order total.
///
/// Both [`sort_findings`] and [`sort_malware_scans`] order a borrowed record set
/// by an attribute that is either a number or a text, so the comparison lives here
/// once: the two differ only in the attribute resolver and the tiebreaker, and
/// cannot drift apart. An attribute the resolver cannot produce sorts as the
/// neutral key rather than dropping the record.
fn compare_by_attribute<T, K: Ord>(
    left: &T,
    right: &T,
    descending: bool,
    attribute: impl Fn(&T) -> Option<Attribute>,
    tiebreak: impl Fn(&T) -> K,
) -> std::cmp::Ordering {
    let key = |record: &T| match attribute(record) {
        Some(Attribute::Number(n)) => (n, String::new()),
        Some(Attribute::Text(t)) => (0.0, t),
        None => (0.0, String::new()),
    };
    let (left_number, left_text) = key(left);
    let (right_number, right_text) = key(right);
    let primary = left_number
        .partial_cmp(&right_number)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left_text.cmp(&right_text));
    let primary = if descending { primary.reverse() } else { primary };
    primary.then_with(|| tiebreak(left).cmp(&tiebreak(right)))
}

/// WHICH ordered list a page token is a position in: the report that minted it,
/// plus every request value that selects or orders the entries it walks.
///
/// A position only means something for the list it was taken in, so the token
/// carries this identity beside the position: a token handed to any other listing
/// resolves against a different identity and is refused, instead of silently
/// resuming in the middle of a list the caller never paged.
///
/// Built by the report that pages: [`Self::of`] names the operation and each
/// [`Self::by`] adds one value the selection or the order depends on.
#[derive(Debug, Clone)]
pub struct Listing {
    /// The identity, rendered: the operation name and the selectors, separated by a
    /// character no rendering of a selector can contain unescaped.
    identity: String,
}

/// How many bytes of the identity's digest a token carries. Eight is far more
/// distinct listings than this service can hold, and keeps the token short.
const LISTING_TAG_BYTES: usize = 8;

/// The tag's width in a token, which is where the token splits into tag and
/// position: a byte renders as two hex digits.
const LISTING_TAG_WIDTH: usize = LISTING_TAG_BYTES * 2;

/// The width the position is rendered to, zero-padded: the minimum width, so every
/// token of a list this service can page comes out the same length.
const LISTING_POSITION_WIDTH: usize = 8;

impl Listing {
    /// The list `operation` reports, before any selector narrows or orders it.
    pub fn of(operation: &str) -> Self {
        Self {
            identity: operation.to_string(),
        }
    }

    /// Add one request value the selection or the ordering of the entries depends on
    /// — a resource the list belongs to, a criteria structure, a sort order.
    ///
    /// Rendered with `Debug`, so a modelled structure identifies the selection it
    /// makes without the caller spelling its members out, and two different
    /// selections cannot render alike. The rendering must be the SAME for two
    /// requests carrying the same value, or a caller's own token stops resolving:
    /// a selector holding a `HashMap` is added through [`Self::by_entries`]
    /// instead, because a hash map iterates in an order private to the instance the
    /// request was deserialized into.
    #[must_use]
    pub fn by(mut self, selector: impl std::fmt::Debug) -> Self {
        // A `Debug` rendering escapes control characters, so the unit separator
        // appears in the identity only where this puts it.
        self.identity.push('\u{1f}');
        self.identity.push_str(&format!("{selector:?}"));
        self
    }

    /// Add a MAP-valued selector: its entries, each rendered with its key, in KEY
    /// order.
    ///
    /// What such a selector selects by is the SET of entries the request carried, so
    /// that set is the identity — never the order the map hands them out, which is
    /// the iteration order of one deserialized instance and differs between two
    /// requests carrying the very same criteria.
    #[must_use]
    pub fn by_entries<K: Ord + std::fmt::Debug, V: std::fmt::Debug>(
        self,
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut rendered: Vec<String> = entries
            .into_iter()
            .map(|(key, value)| format!("{key:?}={value:?}"))
            .collect();
        rendered.sort();
        self.by(rendered)
    }

    /// The tag a token of this listing carries.
    fn tag(&self) -> String {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(self.identity.as_bytes());
        hex::encode(&digest[..LISTING_TAG_BYTES])
    }

    /// The token that asks this listing to resume at `position`.
    fn token_at(&self, position: usize) -> String {
        token_spelling(&self.tag(), position)
    }

    /// The position `token` names in THIS listing, or `None` when it is not a token
    /// this listing issued.
    ///
    /// A token is OPAQUE: the one string that resolves to a position is the one
    /// [`Self::token_at`] writes for it, so every other way of spelling that same
    /// position — a position padded to another width, a hexadecimal digit in upper
    /// case, a sign in front of it — resolves to nothing, exactly like a token
    /// belonging to another listing. This is what makes the token reader accept
    /// nothing but what the token minter wrote: the two cannot drift, because the
    /// reader decides by SPELLING the candidate position again and comparing.
    fn position_in(&self, token: &str) -> Option<usize> {
        let tag = self.tag();
        let (carried, position) = token.split_at_checked(LISTING_TAG_WIDTH)?;
        if carried != tag {
            return None;
        }
        // `from_str_radix` is the lax step — it reads spellings this listing never
        // writes — so its answer is only a CANDIDATE until the round trip below
        // confirms this listing would have written exactly `token` for it.
        let position = usize::from_str_radix(position, 16).ok()?;
        (token_spelling(&tag, position) == token).then_some(position)
    }
}

/// How a token is SPELLED: the listing's `tag` followed by `position` in hexadecimal,
/// zero-padded to [`LISTING_POSITION_WIDTH`].
///
/// The one place the spelling is written, so the token a report issues and the token
/// it accepts are the same string by construction.
fn token_spelling(tag: &str, position: usize) -> String {
    format!("{tag}{position:0width$x}", width = LISTING_POSITION_WIDTH)
}

/// Take one page out of the ordered key list `listing` identifies.
///
/// `token` is one THIS listing issued: it carries the listing's identity beside the
/// number of entries already returned, and the only thing a caller may do with it
/// is hand it back to the same listing. `Err(token)` means the token was not one
/// this listing issued: it does not resolve at all, it is another SPELLING of a
/// position than the one a token carries, it belongs to another listing, or it
/// points past the end of the list, which a listing never hands out (it answers
/// the last page with no `NextToken`).
pub fn paginate(
    listing: &Listing,
    keys: &[String],
    max_results: Option<i32>,
    token: Option<&str>,
    page_limit: usize,
) -> Result<(Vec<String>, Option<String>), String> {
    let (window, next) = paginate_window(listing, keys.len(), max_results, token, page_limit)?;
    Ok((keys[window].to_vec(), next))
}

/// The WINDOW of an ordered list of `len` entries one page covers, and the token
/// the page after it is asked for with.
///
/// [`paginate`] over the keys themselves; this is for a caller that holds the
/// ORDERED RECORDS and wants that same page of them without looking any record up
/// again by key. Both go through this one function, so the token a report issues
/// and the token it accepts cannot drift between them. `Err(token)` is the token
/// rejection [`paginate`] documents.
pub fn paginate_window(
    listing: &Listing,
    len: usize,
    max_results: Option<i32>,
    token: Option<&str>,
    page_limit: usize,
) -> Result<(std::ops::Range<usize>, Option<String>), String> {
    let start = match token.filter(|t| !t.is_empty()) {
        Some(t) => {
            // A token resolves only against the listing that minted it, and names an
            // entry of it that exists: it is emitted only while `end < len`.
            listing
                .position_in(t)
                .filter(|start| *start < len)
                .ok_or_else(|| t.to_string())?
        }
        None => 0,
    };
    let limit = max_results
        .and_then(|m| usize::try_from(m).ok())
        .filter(|m| *m > 0)
        .unwrap_or(page_limit)
        .min(page_limit);
    let end = start.saturating_add(limit).min(len);
    let next = if end < len {
        Some(listing.token_at(end))
    } else {
        None
    };
    Ok((start..end, next))
}

/// Refuse a feature list that asks for Runtime Monitoring and EKS Runtime
/// Monitoring at once, with the message the refusal carries.
///
/// "You can add only one of these two features because Runtime Monitoring already
/// includes the threat detection for Amazon EKS resources"
/// (`smithy_query(query="doc",shape="UpdateOrganizationConfiguration")`, and the
/// same sentence on `CreateDetector`), so the rule holds for a DETECTOR's feature
/// list and for an ORGANIZATION's alike. The two carry different status
/// vocabularies, so each caller supplies `asked_for` — whether its list asks for
/// the feature it names — and the rule and its message live here once.
pub fn refuse_runtime_monitoring_overlap(asked_for: impl Fn(&str) -> bool) -> Result<(), String> {
    if asked_for(FEATURE_EKS_RUNTIME_MONITORING) && asked_for(FEATURE_RUNTIME_MONITORING) {
        return Err(format!(
            "Features: {FEATURE_EKS_RUNTIME_MONITORING} and {FEATURE_RUNTIME_MONITORING} cannot both be enabled because {FEATURE_RUNTIME_MONITORING} already includes threat detection for Amazon EKS resources."
        ));
    }
    Ok(())
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
        .map(|name| {
            let status = if OPT_IN_FEATURES.contains(&name) {
                DISABLED
            } else {
                ENABLED
            };
            FeatureRecord {
                name: name.to_string(),
                status: status.to_string(),
                updated_at_millis: now,
                additional_configuration: Vec::new(),
                // A feature this account starts with ENABLED starts its free trial
                // at the same instant; one it starts with DISABLED has not started
                // its trial yet.
                free_trial_started_at_millis: (status == ENABLED).then_some(now),
            }
        })
        .collect()
}

/// The feature set a create gives a detector, resolved against the `Features` list
/// the create itself carries.
///
/// [`default_features`] is what a detector created without a `Features` list gets.
/// A create that ENABLES one of the two mutually exclusive Runtime Monitoring
/// features and leaves the other one to the default is accepted — only asking for
/// both is "an error" — so the default the SERVICE picks for the other one yields to
/// what the request asked for and starts [`DISABLED`], and its free trial has
/// therefore not begun either. That way the set a create stores never carries the
/// combination the service "can add only one of"
/// (`smithy_query(query="doc",shape="DetectorFeatureConfiguration")`).
pub fn default_features_for(
    now: i64,
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Vec<FeatureRecord> {
    let mut records = default_features(now);
    // The LAST entry naming a feature is the one that lands, as applying the list
    // writes the entries in order, and an entry that states no status enables the
    // feature ([`apply_feature_configurations`]).
    let requested = |wanted: &str| -> Option<bool> {
        Some(
            features?
                .iter()
                .rfind(|feature| feature.name.as_deref() == Some(wanted))?
                .status
                .as_deref()
                != Some(DISABLED),
        )
    };
    for (asked_for, other) in [
        (FEATURE_RUNTIME_MONITORING, FEATURE_EKS_RUNTIME_MONITORING),
        (FEATURE_EKS_RUNTIME_MONITORING, FEATURE_RUNTIME_MONITORING),
    ] {
        // Only where the request says nothing about the other one: a request that
        // names it states its status itself, and a request that enables both is
        // refused before anything is stored.
        if requested(asked_for) != Some(true) || requested(other).is_some() {
            continue;
        }
        if let Some(record) = records.iter_mut().find(|record| record.name == other) {
            record.status = DISABLED.to_string();
            record.free_trial_started_at_millis = None;
        }
    }
    records
}

/// Refuse a `Features` list whose own entries this service cannot accept, with the
/// message the refusal carries.
///
/// Two rules, both from `CreateDetector`'s own documentation: a feature name must be
/// a `DetectorFeature` member, and — since each entry configures one member of that
/// enum — a list may carry no more entries than the enum has members
/// ([`validate_feature_list_length`]). Both are judged on the REQUEST alone, so they
/// are answered whatever the store holds; the rule that spans the list and the
/// configuration it is applied over is [`validate_feature_exclusion`].
pub fn validate_feature_configurations(
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_feature_list_length(&DETECTOR_FEATURES, features.len(), FEATURES_MEMBER)?;
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        validate_feature_entry(&DETECTOR_FEATURES, name, feature.status.as_deref())?;
        if let Some(additional) = feature.additional_configuration.as_ref() {
            validate_feature_list_length(
                DETECTOR_ADDITIONAL_CONFIGURATIONS,
                additional.len(),
                ADDITIONAL_CONFIGURATION_MEMBER,
            )?;
        }
    }
    Ok(())
}

/// Refuse a member-account `Features` list whose own entries this service cannot
/// accept, with the message the refusal carries.
///
/// The member side of [`validate_feature_configurations`], and the same two rules:
/// `MemberFeaturesConfiguration.Name` targets `OrgFeature`
/// (`smithy_query(shape="MemberFeaturesConfiguration")`), so the accepted catalog
/// is [`MEMBER_FEATURES`] rather than [`DETECTOR_FEATURES`]. An
/// `AdditionalConfiguration` entry is checked the same way against the
/// `OrgFeatureAdditionalConfiguration` members it may name.
pub fn validate_member_feature_configurations(
    features: Option<&Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_feature_list_length(MEMBER_FEATURES, features.len(), FEATURES_MEMBER)?;
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        validate_feature_entry(MEMBER_FEATURES, name, feature.status.as_deref())?;
        if let Some(additional) = feature.additional_configuration.as_ref() {
            validate_feature_list_length(
                MEMBER_ADDITIONAL_CONFIGURATIONS,
                additional.len(),
                ADDITIONAL_CONFIGURATION_MEMBER,
            )?;
        }
        for additional in feature.additional_configuration.iter().flatten() {
            let Some(additional_name) = additional.name.as_deref() else {
                continue;
            };
            validate_feature_entry(
                MEMBER_ADDITIONAL_CONFIGURATIONS,
                additional_name,
                additional.status.as_deref(),
            )?;
        }
    }
    Ok(())
}

/// Refuse a `Features` list that would leave the detector it is applied to with
/// both Runtime Monitoring features enabled, with the message the refusal carries.
///
/// `stored` is the configuration the list is applied over — the detector's own
/// switches — and is EMPTY where no configuration exists yet, a create resolving
/// its default set against the request afterwards ([`default_features_for`]).
pub fn validate_feature_exclusion(
    stored: &[FeatureRecord],
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_runtime_monitoring_pair(
        stored,
        features
            .iter()
            .map(|f| (f.name.as_deref(), f.status.as_deref())),
    )
}

/// Refuse a member-account `Features` list that would leave the member account's
/// configuration with both Runtime Monitoring features enabled: the member side of
/// [`validate_feature_exclusion`], which `UpdateMemberDetectors` documents the same
/// exclusion for.
pub fn validate_member_feature_exclusion(
    stored: &[FeatureRecord],
    features: Option<
        &Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>,
    >,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_runtime_monitoring_pair(
        stored,
        features
            .iter()
            .map(|f| (f.name.as_deref(), f.status.as_deref())),
    )
}

/// Refuse a feature list longer than the enum `catalog` its entries name: each
/// entry configures one member of that enum, so a list with more entries than the
/// enum has members necessarily names one twice, and the request is refused rather
/// than stored. `member` is the wire name of the list the refusal is about.
///
/// This is also what keeps a request from growing the store (or the work one call
/// does) by the length of a list the model declares no `@length` for: the stored
/// configuration is keyed by name, so it can never be longer than the catalog, and
/// neither can the list a request is allowed to send.
fn validate_feature_list_length(
    catalog: &[&str],
    length: usize,
    member: &str,
) -> Result<(), String> {
    if length > catalog.len() {
        return Err(format!(
            "{member}: at most {} entries may be given, one for each supported value, but {length} were given.",
            catalog.len()
        ));
    }
    Ok(())
}

/// The wire name of the per-feature additional-configuration list both the
/// detector and the member feature shapes carry
/// (`smithy_query(shape="MemberFeaturesConfiguration")`).
const ADDITIONAL_CONFIGURATION_MEMBER: &str = "features.additionalConfiguration";

/// The wire name of the feature list itself.
const FEATURES_MEMBER: &str = "features";

/// Refuse one `(name, status)` entry of a feature list: the name must be a member
/// of the enum `catalog` the request's member targets, and the status, where
/// supplied, a `FeatureStatus` member.
fn validate_feature_entry(
    catalog: &[&str],
    name: &str,
    status: Option<&str>,
) -> Result<(), String> {
    if !catalog.contains(&name) {
        return Err(format!(
            "Features: '{name}' is not a supported detector feature. Supported features are {}.",
            catalog.join(", ")
        ));
    }
    if let Some(status) = status {
        if status != ENABLED && status != DISABLED {
            return Err(format!(
                "Features: '{status}' is not a supported status for feature '{name}'. Supported values are {ENABLED}, {DISABLED}."
            ));
        }
    }
    Ok(())
}

/// Refuse a feature list that would ADD EKS Runtime Monitoring to a configuration
/// that has Runtime Monitoring, or the other way round: "Specifying both EKS Runtime
/// Monitoring (EKS_RUNTIME_MONITORING) and Runtime Monitoring (RUNTIME_MONITORING)
/// will cause an error. You can add only one of these two features because Runtime
/// Monitoring already includes the threat detection for Amazon EKS resources"
/// (`smithy_query(query="doc",shape="UpdateMemberDetectors")`, which
/// `CreateDetector` and `UpdateDetector` document identically).
///
/// A `Features` list is a PARTIAL update — a feature it does not name keeps the
/// status it already had ([`apply_feature_configurations`]) — so the exclusion is
/// judged on the status each of the two ENDS UP with once the list has been applied,
/// not on what the request happens to mention: the list's own value where it names
/// the feature (the LAST entry naming it, which is the one the applier leaves
/// behind, and an entry that states no status enables it), and the status `stored`
/// already holds otherwise. This is the reading
/// [`validate_org_feature_configurations`] gives an organization's list, over the
/// same primitive.
///
/// It is the list's own ADDITION that is refused: a list that enables neither of the
/// two configures neither, so it is accepted whatever the stored configuration
/// holds.
fn validate_runtime_monitoring_pair<'a>(
    stored: &[FeatureRecord],
    entries: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)> + Clone,
) -> Result<(), String> {
    let requested = |wanted: &str| -> Option<bool> {
        entries
            .clone()
            .filter(|(name, _)| *name == Some(wanted))
            .map(|(_, status)| status != Some(DISABLED))
            .last()
    };
    if requested(FEATURE_EKS_RUNTIME_MONITORING) != Some(true)
        && requested(FEATURE_RUNTIME_MONITORING) != Some(true)
    {
        return Ok(());
    }
    refuse_runtime_monitoring_overlap(|wanted| {
        requested(wanted).unwrap_or_else(|| feature_status_in(stored, wanted) == ENABLED)
    })
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
        .ok_or_else(|| missing_parameter_message(DESTINATION_ARN_MEMBER))?
        .to_string();
    let kms_key_arn = properties
        .kms_key_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| missing_parameter_message(KMS_KEY_ARN_MEMBER))?
        .to_string();
    // Both members the create cannot do without are present, so what is left is
    // the FORM of each value supplied — the same rule, for the same members, that
    // an UPDATE applies to the members it carries. Presence is checked for BOTH
    // members before either form is: a request that leaves a required member out
    // has not said what to publish to at all, so the missing member is the one the
    // refusal names even when another member it did supply is also malformed.
    validate_destination_property_forms(destination_service, properties)?;
    Ok((destination_arn, kms_key_arn))
}

/// Refuse a `DestinationProperties` member whose supplied VALUE is not a value of
/// that member, leaving the members the request omits alone.
///
/// Every member of `DestinationProperties` is optional
/// (`smithy_query(shape="DestinationProperties")`), so this is the half of the
/// check that does not depend on whether the operation requires a member:
/// [`validate_destination_properties`] requires the two ARNs first and then calls
/// this, while an update of an existing destination carries only the members it
/// changes and calls it directly.
///
/// `destination_service` is the AWS service the destination's own
/// `DestinationType` names ([`validate_destination_type`]), so the destination ARN
/// is checked against that service rather than against a hard-coded one.
pub fn validate_destination_property_forms(
    destination_service: &str,
    properties: &crate::ops::types::DestinationProperties,
) -> Result<(), String> {
    if let Some(destination_arn) = properties.destination_arn.as_deref() {
        if !is_arn_for(destination_service, destination_arn) {
            return Err(invalid_parameter_message(DESTINATION_ARN_MEMBER));
        }
    }
    if let Some(kms_key_arn) = properties.kms_key_arn.as_deref() {
        if !is_arn_for(KMS_SERVICE, kms_key_arn) {
            return Err(invalid_parameter_message(KMS_KEY_ARN_MEMBER));
        }
    }
    if let Some(owner) = properties.expected_bucket_owner.as_deref() {
        // "The 12-digit Amazon Web Services account ID of the account that owns the
        // Amazon S3 bucket"
        // (`smithy_query(query="doc",shape="DestinationProperties",member="ExpectedBucketOwner")`),
        // which is the account-id form [`is_account_id`] reads — the one reader
        // every account-id-valued member on this service is held to. The model's
        // own `len 12..12` refuses another length at the boundary; the length is
        // part of that reader too, so this helper states the whole rule.
        if !is_account_id(owner) {
            return Err(invalid_parameter_message(EXPECTED_BUCKET_OWNER_MEMBER));
        }
    }
    Ok(())
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
///
/// Answers whether the list configured any switch at all: an absent member, an
/// empty list, and a list whose entries name no feature each write nothing, which
/// is what lets a caller tell a request that configured the record from one that
/// asked for no change.
pub fn apply_feature_configurations(
    detector: &mut Detector,
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
    now: i64,
) -> bool {
    let Some(features) = features else {
        return false;
    };
    let mut configured = false;
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        configured = true;
        let status = feature.status.as_deref().unwrap_or(ENABLED);
        detector.set_feature(name, status, now);
        if let Some(additional) = feature.additional_configuration.as_ref() {
            let additional_records = additional_configuration_records(
                additional
                    .iter()
                    .map(|cfg| (cfg.name.as_deref(), cfg.status.as_deref())),
                now,
            );
            set_additional_configuration_in(&mut detector.features, name, additional_records);
        }
    }
    configured
}

/// Write a member-account `Features` list onto `member`'s stored configuration,
/// the member side of [`apply_feature_configurations`]. A feature the list does
/// not name keeps the status it already had, so one request configures only the
/// switches it names.
///
/// Private because a member's switches are the ORGANIZATION's where its
/// configuration auto-enables them: [`apply_member_configuration`] is the one
/// entry point a handler writes them through, so no per-account write can bypass
/// that rule.
fn apply_member_feature_configurations(
    member: &mut MemberRecord,
    features: Option<
        &Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>,
    >,
    now: i64,
) {
    let Some(features) = features else { return };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        let status = feature.status.as_deref().unwrap_or(ENABLED);
        member.set_feature(name, status, now);
        if let Some(additional) = feature.additional_configuration.as_ref() {
            let configured = additional_configuration_records(
                additional
                    .iter()
                    .map(|cfg| (cfg.name.as_deref(), cfg.status.as_deref())),
                now,
            );
            set_additional_configuration_in(&mut member.features, name, configured);
        }
    }
}

/// Write the organization's auto-enable settings onto one member account's stored
/// configuration.
///
/// A feature the organization auto-enables — any `OrgFeatureStatus` other than
/// [`AUTO_ENABLE_NONE`], which is the same "is this an auto-enablement" reading
/// [`org_status_flag`] gives the deprecated boolean view — is ENABLED for the
/// account, together with every additional configuration that feature auto-enables.
/// A feature the organization auto-enables for no account keeps whatever the
/// member's configuration already said, because then "the administrator must manage
/// the feature for each account individually"
/// (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`)
/// — so nothing here switches a feature OFF.
///
/// Only the features a member request can configure ([`MEMBER_FEATURES`]) can be
/// auto-enabled, which is exactly the set an organization configuration holds
/// ([`ORG_FEATURES`]): the foundational sources are on for every account anyway and
/// no organization setting names them.
///
/// Answers whether this call wrote any switch at all — re-applying the status a
/// switch already carries writes nothing ([`set_feature_in`]) — so a caller can
/// tell a write that landed from one that left the record exactly as it was.
pub fn apply_organization_auto_enable(
    member: &mut MemberRecord,
    organization: &OrganizationConfiguration,
    now: i64,
) -> bool {
    // The configuration as it stood before, so "did anything move" is answered on
    // the stored switches themselves rather than on what the settings named.
    let stored = member.features.clone();
    for name in MEMBER_FEATURES {
        if !org_status_flag(organization.feature_auto_enable(name)) {
            continue;
        }
        member.set_feature(name, ENABLED, now);
        let auto_enabled: Vec<&str> = organization
            .features
            .iter()
            .filter(|feature| feature.name == *name)
            .flat_map(|feature| feature.additional_configuration.iter())
            .filter(|additional| org_status_flag(&additional.auto_enable))
            .map(|additional| additional.name.as_str())
            .collect();
        for additional in auto_enabled {
            set_additional_configuration_status_in(
                &mut member.features,
                name,
                additional,
                ENABLED,
                now,
            );
        }
    }
    member.features != stored
}

/// Write one member account's REQUESTED configuration onto its stored record under
/// the organization's auto-enable settings.
///
/// A switch the organization auto-enables — a feature, or one of a feature's
/// additional configurations, whose `OrgFeatureStatus` is anything other than
/// [`AUTO_ENABLE_NONE`] ([`org_status_flag`]) — belongs to the ORGANIZATION: the
/// request leaves it exactly where it stood, its instant included, and
/// [`apply_organization_auto_enable`] then puts the auto-enablement over it, so it
/// ends ENABLED whatever the request asked for. A switch auto-enabled for no account
/// is one "the administrator must manage ... for each account individually"
/// (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`,
/// and the same sentence on `OrganizationAdditionalConfiguration`), so it takes
/// exactly what the request said.
///
/// So a member's stored configuration agrees with what
/// `DescribeOrganizationConfiguration` reports for the same organization HOWEVER it
/// was written — the invariant `CreateMembers` states and maintains for the accounts
/// it records, here for the per-account write of the same stored switches.
///
/// Answers whether this call wrote any switch at all, as
/// [`apply_organization_auto_enable`] does: a request that names no configuration,
/// and one that re-states the switches the account already carries, each leave the
/// record exactly as it was.
pub fn apply_member_configuration(
    member: &mut MemberRecord,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    features: Option<
        &Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>,
    >,
    organization: &OrganizationConfiguration,
    now: i64,
) -> bool {
    // The configuration as it stood BEFORE the request, so the switches the
    // organization owns can be put back exactly once the request has passed over
    // them.
    let stored = member.features.clone();
    // The deprecated `DataSources` view is applied first so an explicit `Features`
    // entry for the same switch wins — the order `CreateDetector` and
    // `UpdateDetector` apply the two views in.
    apply_member_data_sources(member, data_sources, now);
    apply_member_feature_configurations(member, features, now);
    restore_organization_managed_switches(&mut member.features, &stored, organization);
    // Reported from the stored switches, not from this call's return: the
    // auto-enablement is written over a configuration the request may itself have
    // moved, so the one comparison that answers "did this request write" is the
    // whole configuration against the one it found.
    apply_organization_auto_enable(member, organization, now);
    member.features != stored
}

/// Put every switch the organization manages back the way `stored` held it, after a
/// per-account write has passed over the whole configuration.
///
/// Restoring rather than filtering the request is what keeps a switch's instant
/// saying when it last CHANGED ([`set_feature_in`]): a per-account write of a switch
/// the organization owns moves nothing at all.
fn restore_organization_managed_switches(
    features: &mut [FeatureRecord],
    stored: &[FeatureRecord],
    organization: &OrganizationConfiguration,
) {
    for name in MEMBER_FEATURES {
        if !org_status_flag(organization.feature_auto_enable(name)) {
            continue;
        }
        let (Some(record), Some(was)) = (
            features.iter_mut().find(|record| record.name == *name),
            stored.iter().find(|record| record.name == *name),
        ) else {
            continue;
        };
        record.status = was.status.clone();
        record.updated_at_millis = was.updated_at_millis;
        record.free_trial_started_at_millis = was.free_trial_started_at_millis;
    }
    // Each additional configuration is a switch of its own, with its own
    // auto-enablement, so the organization owns them one at a time: one it
    // auto-enables goes back even though the feature that carries it is managed per
    // account, and one auto-enabled for nobody keeps what the request wrote even
    // though its feature is not.
    for feature in &organization.features {
        for additional in &feature.additional_configuration {
            if !org_status_flag(&additional.auto_enable) {
                continue;
            }
            let (Some(record), Some(was)) = (
                features.iter_mut().find(|record| record.name == feature.name),
                stored
                    .iter()
                    .find(|record| record.name == feature.name)
                    .and_then(|record| {
                        record
                            .additional_configuration
                            .iter()
                            .find(|configured| configured.name == additional.name)
                    }),
            ) else {
                // Nothing to put back: the member did not carry this configuration
                // before the write, so the auto-enablement is what records it, at
                // the instant it does.
                continue;
            };
            restore_additional_configuration_in(record, was);
        }
    }
}

/// Put one additional configuration back into a stored feature record, re-inserting
/// it at its place in the stored order — by NAME, the order
/// [`set_additional_configuration_status_in`] keeps the set in — when the write that
/// passed over the feature dropped it.
fn restore_additional_configuration_in(record: &mut FeatureRecord, was: &AdditionalConfigRecord) {
    if let Some(existing) = record
        .additional_configuration
        .iter_mut()
        .find(|configured| configured.name == was.name)
    {
        *existing = was.clone();
        return;
    }
    let at = record
        .additional_configuration
        .partition_point(|configured| configured.name < was.name);
    record.additional_configuration.insert(at, was.clone());
}

/// Set one feature's status in a stored feature list, recording `now` as its
/// `UpdatedAt`, and adding the feature when the list does not carry it yet.
///
/// The one place a feature switch is written, so a detector's configuration and a
/// member's follow the same rule: re-applying the status a feature already has
/// changes nothing, `UpdatedAt` included, which is what makes the instant say when
/// the switch last CHANGED rather than when it was last written.
pub fn set_feature_in(features: &mut Vec<FeatureRecord>, name: &str, status: &str, now: i64) {
    if let Some(existing) = features.iter_mut().find(|f| f.name == name) {
        if existing.status != status {
            existing.status = status.to_string();
            existing.updated_at_millis = now;
        }
        // Enabling a feature for the first time starts its free trial, and only the
        // first time: a feature whose trial has already begun keeps the instant it
        // began at, however often the switch moves afterwards.
        if status == ENABLED && existing.free_trial_started_at_millis.is_none() {
            existing.free_trial_started_at_millis = Some(now);
        }
    } else {
        features.push(FeatureRecord {
            name: name.to_string(),
            status: status.to_string(),
            updated_at_millis: now,
            additional_configuration: Vec::new(),
            free_trial_started_at_millis: (status == ENABLED).then_some(now),
        });
    }
}

/// Build one feature's stored additional-configuration set from the `(name,
/// status)` entries a request gave, keyed by NAME.
///
/// Each entry configures one member of the additional-configuration enum, so the
/// stored set is keyed by that name the way [`set_feature_in`] keys a feature
/// switch: a repeated name is ONE configuration and the last entry naming it wins,
/// which is also what keeps the stored set no longer than the enum's own member
/// list however long the request's list is. An entry naming nothing configures
/// nothing and is dropped.
fn additional_configuration_records<'a>(
    entries: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)>,
    now: i64,
) -> Vec<AdditionalConfigRecord> {
    let mut by_name: BTreeMap<&str, AdditionalConfigRecord> = BTreeMap::new();
    for (name, status) in entries {
        let Some(name) = name else { continue };
        by_name.insert(
            name,
            AdditionalConfigRecord {
                name: name.to_string(),
                status: status.unwrap_or(ENABLED).to_string(),
                updated_at_millis: now,
            },
        );
    }
    by_name.into_values().collect()
}

/// Replace one feature's additional configuration in a stored feature list.
fn set_additional_configuration_in(
    features: &mut [FeatureRecord],
    name: &str,
    configured: Vec<AdditionalConfigRecord>,
) {
    if let Some(record) = features.iter_mut().find(|f| f.name == name) {
        record.additional_configuration = configured;
    }
}

/// Set ONE of a feature's additional configurations in a stored feature list,
/// leaving that feature's other additional configurations where they are — the
/// single-switch counterpart of [`set_additional_configuration_in`].
///
/// Follows [`set_feature_in`]'s rule for the instant: re-applying the status a
/// configuration already has changes nothing, `UpdatedAt` included. A configuration
/// the feature does not carry yet is inserted at its place in the stored order,
/// which is by NAME — the order [`additional_configuration_records`] builds a
/// request's set in — so one read reports the set in a total order however it was
/// written.
fn set_additional_configuration_status_in(
    features: &mut [FeatureRecord],
    feature: &str,
    name: &str,
    status: &str,
    now: i64,
) {
    let Some(record) = features.iter_mut().find(|f| f.name == feature) else {
        return;
    };
    if let Some(existing) = record
        .additional_configuration
        .iter_mut()
        .find(|configured| configured.name == name)
    {
        if existing.status != status {
            existing.status = status.to_string();
            existing.updated_at_millis = now;
        }
        return;
    }
    let at = record
        .additional_configuration
        .partition_point(|configured| configured.name.as_str() < name);
    record.additional_configuration.insert(
        at,
        AdditionalConfigRecord {
            name: name.to_string(),
            status: status.to_string(),
            updated_at_millis: now,
        },
    );
}

/// The status a stored feature list holds for one feature, or [`DISABLED`] where
/// the list does not carry it at all.
///
/// [`Detector::feature_status`] over a feature list held anywhere, so a detector's
/// own switches and a member account's are read the same way.
pub fn feature_status_in<'a>(features: &'a [FeatureRecord], name: &str) -> &'a str {
    features
        .iter()
        .find(|feature| feature.name == name)
        .map_or(DISABLED, |feature| feature.status.as_str())
}

/// What one data source reports, read off the feature that governs it: that
/// feature's own status and the instant it last changed, in epoch milliseconds.
///
/// `fallback_millis` is the instant reported for a feature the list does not carry
/// at all, which reports [`DISABLED`].
pub fn feature_data_source(
    features: &[FeatureRecord],
    feature: &str,
    fallback_millis: i64,
) -> (String, i64) {
    features.iter().find(|f| f.name == feature).map_or_else(
        || (DISABLED.to_string(), fallback_millis),
        |f| (f.status.clone(), f.updated_at_millis),
    )
}

/// How long GuardDuty's free trial of one feature runs, in days.
///
/// GuardDuty's own documentation of the trial: an account enabling GuardDuty, or
/// enabling one of its protection plans, gets a 30-day free trial of it. The model
/// carries no duration — `FreeTrialDaysRemaining` is a plain integer
/// (`smithy_query(shape="DataSourceFreeTrial")`) — so the length is stated here
/// once, and everything a read reports about the trial is derived from it.
pub const FREE_TRIAL_DAYS: i32 = 30;

/// Milliseconds in one day: the unit a duration this service counts in DAYS is
/// measured against, the stored instants being epoch milliseconds.
pub const DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// The whole days left in one feature's free trial at `now` — what
/// `FreeTrialDaysRemaining` reports for it.
///
/// The trial runs [`FREE_TRIAL_DAYS`] days from the instant it began
/// ([`FeatureRecord::free_trial_started_at_millis`]), so each whole day that has
/// elapsed since then consumes one: a feature whose trial has not begun has the
/// whole of it left, and one whose window has run out has none. The count follows
/// the CLOCK and not the current switch, because the window is a span of time and
/// not a measure of use: a feature switched off part-way through its trial has the
/// same days left as one left on.
///
/// One reader for the detector's own features and for a member account's, so two
/// reports of one account's trial cannot disagree.
pub fn free_trial_days_remaining(features: &[FeatureRecord], feature: &str, now: i64) -> i32 {
    let Some(started) = features
        .iter()
        .find(|f| f.name == feature)
        .and_then(|f| f.free_trial_started_at_millis)
    else {
        return FREE_TRIAL_DAYS;
    };
    let elapsed_days = i32::try_from(now.saturating_sub(started) / DAY_MILLIS).unwrap_or(i32::MAX);
    FREE_TRIAL_DAYS
        .saturating_sub(elapsed_days)
        .clamp(0, FREE_TRIAL_DAYS)
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
) -> bool {
    apply_data_sources_to(&mut detector.features, data_sources, now)
}

/// Write the deprecated `DataSources` view onto `member`'s stored configuration,
/// the member side of [`apply_data_sources`]. Private for the reason
/// [`apply_member_feature_configurations`] is.
fn apply_member_data_sources(
    member: &mut MemberRecord,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    now: i64,
) -> bool {
    apply_data_sources_to(&mut member.features, data_sources, now)
}

/// Write the deprecated `DataSources` view onto a stored feature list: the switch
/// each data source names is the feature that governs it, so the detector view and
/// the member view cannot disagree about which switch a node writes.
///
/// Answers whether the view configured any switch, the same reading
/// [`apply_feature_configurations`] gives its own list: a view that names no data
/// source writes nothing.
fn apply_data_sources_to(
    features: &mut Vec<FeatureRecord>,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    now: i64,
) -> bool {
    let Some(data_sources) = data_sources else {
        return false;
    };
    let mut configured = false;
    let status = |enable: bool| if enable { ENABLED } else { DISABLED };
    if let Some(s3) = data_sources.s3_logs.as_ref() {
        set_feature_in(features, FEATURE_S3_DATA_EVENTS, status(s3.enable), now);
        configured = true;
    }
    if let Some(kubernetes) = data_sources.kubernetes.as_ref() {
        set_feature_in(
            features,
            FEATURE_EKS_AUDIT_LOGS,
            status(kubernetes.audit_logs.enable),
            now,
        );
        configured = true;
    }
    if let Some(enable) = data_sources
        .malware_protection
        .as_ref()
        .and_then(|m| m.scan_ec2_instance_with_findings.as_ref())
        .and_then(|s| s.ebs_volumes)
    {
        set_feature_in(features, FEATURE_EBS_MALWARE_PROTECTION, status(enable), now);
        configured = true;
    }
    configured
}

/// Every feature a member-account request may configure: the `OrgFeature` members
/// `MemberFeaturesConfiguration.Name` targets
/// (`smithy_query(shape="MemberFeaturesConfiguration")`), taken from the crate's
/// MODEL-DERIVED list so a member the model gains cannot be missed here.
///
/// A narrower catalog than [`DETECTOR_FEATURES`]: the features an administrator
/// configures for a member account are the organisation-level ones.
pub const MEMBER_FEATURES: &[&str] = crate::ops::types::shared_types::ORG_FEATURE_VALUES;

/// Every additional configuration a member-account request may name: the
/// `OrgFeatureAdditionalConfiguration` members
/// `MemberAdditionalConfiguration.Name` targets
/// (`smithy_query(shape="MemberAdditionalConfiguration")`).
pub const MEMBER_ADDITIONAL_CONFIGURATIONS: &[&str] =
    crate::ops::types::shared_types::ORG_FEATURE_ADDITIONAL_CONFIGURATION_VALUES;

/// Every additional configuration a DETECTOR request may name: the
/// `FeatureAdditionalConfiguration` members
/// `DetectorAdditionalConfiguration.Name` targets
/// (`smithy_query(shape="DetectorAdditionalConfiguration")`).
pub const DETECTOR_ADDITIONAL_CONFIGURATIONS: &[&str] =
    crate::ops::types::shared_types::FEATURE_ADDITIONAL_CONFIGURATION_VALUES;

/// Every feature a MEMBER's configuration reports, in the order it reports them:
/// the [`MEMBER_FEATURES`] in [`DETECTOR_FEATURES`] (model) order, so one member
/// read orders its features the way a detector read orders its own.
///
/// The list is a total order over a SET, so a client may index it positionally.
pub fn member_reported_features() -> Vec<&'static str> {
    DETECTOR_FEATURES
        .iter()
        .copied()
        .filter(|name| MEMBER_FEATURES.contains(name))
        .collect()
}

/// The deprecated `DataSources` view of one stored feature list.
///
/// `DataSources` is "deprecated, use Features instead"
/// (`smithy_query(operation="GetDetector")`), so every node here is read back off
/// the feature that governs it: the same stored switch, projected under its other
/// name. Each node carries that switch's status AND the instant it last changed.
///
/// One projection for both readers — `GetDetector` for a detector and
/// `GetMemberDetectors` for a member account — so two reads of one switch cannot
/// disagree. `malware_protection_service_role` is the Malware Protection
/// service-linked role to report, which is `None` for a role this emulator does
/// not hold (the member account's own), and the member is optional, so it is
/// omitted rather than invented.
pub fn data_source_configurations_view(
    features: &[FeatureRecord],
    fallback_millis: i64,
    malware_protection_service_role: Option<&str>,
) -> crate::ops::types::shared_types::DataSourceConfigurationsResult {
    use crate::ops::types::shared_types::*;

    let reported = |feature: &str| {
        let (status, millis) = feature_data_source(features, feature, fallback_millis);
        (status, iso8601(millis))
    };
    let (cloud_trail, cloud_trail_at) = reported(FEATURE_CLOUD_TRAIL);
    let (dns_logs, dns_logs_at) = reported(FEATURE_DNS_LOGS);
    let (flow_logs, flow_logs_at) = reported(FEATURE_FLOW_LOGS);
    let (s3_logs, s3_logs_at) = reported(FEATURE_S3_DATA_EVENTS);
    let (audit_logs, audit_logs_at) = reported(FEATURE_EKS_AUDIT_LOGS);
    let (ebs_volumes, ebs_volumes_at) = reported(FEATURE_EBS_MALWARE_PROTECTION);
    let (database_events, database_events_at) = reported(FEATURE_RDS_LOGIN_EVENTS);
    DataSourceConfigurationsResult {
        cloud_trail: CloudTrailConfigurationResult {
            status: cloud_trail,
            updated_at: Some(cloud_trail_at),
        },
        dns_logs: DNSLogsConfigurationResult {
            status: dns_logs,
            updated_at: Some(dns_logs_at),
        },
        flow_logs: FlowLogsConfigurationResult {
            status: flow_logs,
            updated_at: Some(flow_logs_at),
        },
        s3_logs: S3LogsConfigurationResult {
            status: s3_logs,
            updated_at: Some(s3_logs_at),
        },
        kubernetes: Some(KubernetesConfigurationResult {
            audit_logs: KubernetesAuditLogsConfigurationResult {
                status: audit_logs,
                updated_at: Some(audit_logs_at),
            },
        }),
        malware_protection: Some(MalwareProtectionConfigurationResult {
            scan_ec2_instance_with_findings: Some(ScanEc2InstanceWithFindingsResult {
                ebs_volumes: Some(EbsVolumesResult {
                    status: Some(ebs_volumes),
                    // Nothing stopped the source from being enabled, so there is no
                    // reason to report.
                    reason: None,
                    updated_at: Some(ebs_volumes_at),
                }),
            }),
            service_role: malware_protection_service_role.map(str::to_string),
        }),
        // RDS login activity, the `Database` view of the RDS_LOGIN_EVENTS feature.
        database: Some(DatabaseConfigurationResult {
            rds: Some(RdsConfigurationResult {
                database_events: Some(DatabaseEventsConfigurationResult {
                    status: Some(database_events),
                    updated_at: Some(database_events_at),
                }),
            }),
        }),
    }
}

/// The modelled `MemberDataSourceConfiguration` a read reports for one stored
/// member account: its configuration under both the `Features` and the deprecated
/// `DataSources` view, the same two views `GetDetector` reports for a detector.
///
/// The projection lives here, beside the record, because it is what a member
/// configuration READ answers with — `GetMemberDetectors` today, and any later
/// reader of the same facts.
pub fn member_data_source_configuration_view(
    member: &MemberRecord,
) -> crate::ops::types::get_member_detectors_types::MemberDataSourceConfiguration {
    use crate::ops::types::get_member_detectors_types::*;

    MemberDataSourceConfiguration {
        account_id: member.account_id.clone(),
        data_sources: data_source_configurations_view(
            &member.features,
            member.updated_at_millis,
            // The Malware Protection service-linked role of a member account lives
            // in THAT account, which this emulator does not hold, and the member is
            // optional: omitted rather than invented.
            None,
        ),
        features: Some(
            member_reported_features()
                .into_iter()
                .map(|name| {
                    let record = member.features.iter().find(|f| f.name == name);
                    MemberFeaturesConfigurationResult {
                        name: Some(name.to_string()),
                        status: Some(
                            record
                                .map_or(DISABLED, |f| f.status.as_str())
                                .to_string(),
                        ),
                        updated_at: Some(::aws_smithy_types::DateTime::from_millis(
                            record.map_or(member.updated_at_millis, |f| f.updated_at_millis),
                        )),
                        additional_configuration: record
                            .filter(|f| !f.additional_configuration.is_empty())
                            .map(|f| {
                                f.additional_configuration
                                    .iter()
                                    .map(|cfg| MemberAdditionalConfigurationResult {
                                        name: Some(cfg.name.clone()),
                                        status: Some(cfg.status.clone()),
                                        updated_at: Some(
                                            ::aws_smithy_types::DateTime::from_millis(
                                                cfg.updated_at_millis,
                                            ),
                                        ),
                                        // Nothing in this emulator configures a
                                        // member through a GuardDuty policy, so the
                                        // optional member is absent.
                                        managed_by: None,
                                    })
                                    .collect()
                            }),
                        managed_by: None,
                    }
                })
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// The organization configuration
//
// A delegated administrator configures, once per account per Region, what
// GuardDuty does for the OTHER accounts in its organization: whether the service
// itself is turned on for them and, per feature, whether that feature is. The
// configuration is a property of the administrator's detector — every operation
// on it is bound under `/detector/{DetectorId}/admin`
// (`smithy_query(operation="DescribeOrganizationConfiguration")`) — so it hangs
// off [`Detector`] and `DeleteDetector` drops it with the detector.
//
// Each switch is stored ONCE, as the feature that governs it, and read back
// through either the `Features` view or the deprecated `DataSources` one, exactly
// as the detector's own switches are ([`apply_data_sources`] above).
// ---------------------------------------------------------------------------

/// `AutoEnableMembers` / `OrgFeatureStatus`: nothing is enabled automatically.
///
/// Both enums carry the same three members
/// (`smithy_query(shape="AutoEnableMembers,OrgFeatureStatus")`) — one says whether
/// GuardDuty itself is auto-enabled for the organization's accounts, the other
/// whether one feature is — so one constant serves both.
pub const AUTO_ENABLE_NONE: &str = "NONE";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::AUTO_ENABLE_MEMBERS_VALUES,
    AUTO_ENABLE_NONE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ORG_FEATURE_STATUS_VALUES,
    AUTO_ENABLE_NONE
));

/// `AutoEnableMembers` / `OrgFeatureStatus`: auto-enabled for the accounts that
/// JOIN the organization from now on. See [`AUTO_ENABLE_NONE`] for the enums.
pub const AUTO_ENABLE_NEW: &str = "NEW";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::AUTO_ENABLE_MEMBERS_VALUES,
    AUTO_ENABLE_NEW
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ORG_FEATURE_STATUS_VALUES,
    AUTO_ENABLE_NEW
));

/// `AutoEnableMembers` / `OrgFeatureStatus`: auto-enabled for EVERY account in the
/// organization — the accounts already in it as well as the ones that join. See
/// [`AUTO_ENABLE_NONE`] for the enums.
pub const AUTO_ENABLE_ALL: &str = "ALL";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::AUTO_ENABLE_MEMBERS_VALUES,
    AUTO_ENABLE_ALL
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ORG_FEATURE_STATUS_VALUES,
    AUTO_ENABLE_ALL
));

/// Every `OrgFeature` member, in the order a read reports them: the crate's
/// MODEL-DERIVED list, so a feature the model gains cannot be missed here.
pub const ORG_FEATURES: &[&str] = crate::ops::types::shared_types::ORG_FEATURE_VALUES;

/// The wire name of the `AutoEnableOrganizationMembers` member
/// (`smithy_query(operation="UpdateOrganizationConfiguration")`).
pub const AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER: &str = "autoEnableOrganizationMembers";

/// The wire name of the deprecated organization-level `AutoEnable` member.
pub const AUTO_ENABLE_MEMBER: &str = "autoEnable";

/// The message a request that names both auto-enable views, or neither, is refused
/// with: "You must provide a value for either `autoEnableOrganizationMembers` or
/// `autoEnable`, but not both"
/// (`smithy_query(query="doc",shape="UpdateOrganizationConfiguration")`).
pub fn auto_enable_exclusive_message() -> String {
    format!(
        "The request is rejected because exactly one of {AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER} and {AUTO_ENABLE_MEMBER} must be specified."
    )
}

/// The `OrgFeatureStatus` the deprecated BOOLEAN auto-enable view names.
///
/// A view that can only say yes or no cannot distinguish the two "yes" members, so
/// yes is [`AUTO_ENABLE_NEW`]: the deprecated flag is about the accounts that join
/// ("whether GuardDuty is automatically enabled for accounts added to the
/// organization" — `smithy_query(query="doc",shape="DescribeOrganizationConfigurationResponse",member="AutoEnable")`),
/// which is exactly what `NEW` means.
pub fn org_status_for_flag(auto_enable: bool) -> &'static str {
    if auto_enable {
        AUTO_ENABLE_NEW
    } else {
        AUTO_ENABLE_NONE
    }
}

/// How a stored `OrgFeatureStatus` reads back through the deprecated BOOLEAN view:
/// anything other than [`AUTO_ENABLE_NONE`] is an auto-enablement, so it reads
/// back as true.
pub fn org_status_flag(status: &str) -> bool {
    status != AUTO_ENABLE_NONE
}

/// One `OrgFeature`'s auto-enablement, as the store holds it.
#[derive(Debug, Clone)]
pub struct OrgFeatureRecord {
    /// An `OrgFeature` member — one of [`ORG_FEATURES`].
    pub name: String,
    /// `OrgFeatureStatus`: whether this feature is enabled automatically for the
    /// organization's accounts, and for which of them.
    pub auto_enable: String,
    /// The feature's additional configurations, as a request set them. Empty until
    /// one does: GuardDuty documents no additional configuration a feature carries
    /// by default, so nothing is invented here.
    pub additional_configuration: Vec<OrgAdditionalConfigRecord>,
}

/// One `OrgFeatureAdditionalConfiguration`'s auto-enablement, as the store holds
/// it.
#[derive(Debug, Clone)]
pub struct OrgAdditionalConfigRecord {
    /// An `OrgFeatureAdditionalConfiguration` member
    /// (`smithy_query(shape="OrgFeatureAdditionalConfiguration")`), which the
    /// boundary refuses a request outside of before a handler sees it.
    pub name: String,
    /// `OrgFeatureStatus`, as on the feature that carries it.
    pub auto_enable: String,
}

/// The organization configuration of one delegated administrator's detector.
#[derive(Debug, Clone)]
pub struct OrganizationConfiguration {
    /// `AutoEnableMembers`: whether GuardDuty itself is enabled automatically for
    /// the organization's accounts.
    pub auto_enable_members: String,
    /// One record per [`ORG_FEATURES`] member, in that order, so a read reports the
    /// whole set once each and a page token means the same thing on the next call.
    pub features: Vec<OrgFeatureRecord>,
}

impl Default for OrganizationConfiguration {
    /// What an administrator's detector reports before any
    /// `UpdateOrganizationConfiguration`: nothing is enabled automatically, which
    /// is what [`AUTO_ENABLE_NONE`] says for GuardDuty itself and for every
    /// feature. The administrator "must manage GuardDuty for each account in the
    /// organization individually"
    /// (`smithy_query(query="doc",shape="UpdateOrganizationConfigurationRequest",member="AutoEnableOrganizationMembers")`)
    /// until a request says otherwise.
    fn default() -> Self {
        Self {
            auto_enable_members: AUTO_ENABLE_NONE.to_string(),
            features: ORG_FEATURES
                .iter()
                .map(|name| OrgFeatureRecord {
                    name: (*name).to_string(),
                    auto_enable: AUTO_ENABLE_NONE.to_string(),
                    additional_configuration: Vec::new(),
                })
                .collect(),
        }
    }
}

impl OrganizationConfiguration {
    /// Is GuardDuty itself auto-enabled for EVERY account in the organization?
    ///
    /// The administrator that configured this keeps GuardDuty on for each of its
    /// organization's accounts, so the organization — not a single API call — owns
    /// each member relationship: an account has to leave the organization before
    /// its relationship can end or its monitoring stop. Every operation that would
    /// do either reads this before it writes anything, on the administrator's side
    /// of the relationship (see [`organization_managed_members_message`]) and on
    /// the member's own (see [`organization_managed_administrator_message`]).
    pub fn auto_enables_all_members(&self) -> bool {
        self.auto_enable_members == AUTO_ENABLE_ALL
    }

    /// One feature's `OrgFeatureStatus`, or [`AUTO_ENABLE_NONE`] when the
    /// configuration does not carry that feature.
    pub fn feature_auto_enable(&self, name: &str) -> &str {
        self.features
            .iter()
            .find(|feature| feature.name == name)
            .map_or(AUTO_ENABLE_NONE, |feature| feature.auto_enable.as_str())
    }

    /// Set one feature's `OrgFeatureStatus` and hand back the record, so a caller
    /// can write its additional configurations too.
    ///
    /// `None` for a name that is not an [`ORG_FEATURES`] member: the stored set is
    /// exactly that set and stays in its order, which is the invariant
    /// [`Self::features`] states and the page token a read issues depends on, so a
    /// name outside it is IGNORED rather than appended. Nothing can reach here with
    /// one anyway — `OrgFeature` is an enum, so the boundary refuses a value outside
    /// it before a handler runs — and the record for a member the stored list somehow
    /// lacks is inserted at its place in that order.
    pub fn set_feature(&mut self, name: &str, auto_enable: &str) -> Option<&mut OrgFeatureRecord> {
        let rank = |feature: &str| ORG_FEATURES.iter().position(|member| *member == feature);
        let wanted = rank(name)?;
        let position = match self
            .features
            .iter()
            .position(|feature| feature.name == name)
        {
            Some(position) => position,
            None => {
                let position = self
                    .features
                    .iter()
                    .position(|feature| rank(&feature.name).is_none_or(|held| held > wanted))
                    .unwrap_or(self.features.len());
                self.features.insert(
                    position,
                    OrgFeatureRecord {
                        name: name.to_string(),
                        auto_enable: auto_enable.to_string(),
                        additional_configuration: Vec::new(),
                    },
                );
                position
            }
        };
        let feature = &mut self.features[position];
        feature.auto_enable = auto_enable.to_string();
        Some(feature)
    }
}

/// The action `DisassociateMembers` attempts, as the refusal below names it.
pub const DISASSOCIATE_MEMBER_ACTION: &str = "disassociate";

/// The action `StopMonitoringMembers` attempts, as the refusal below names it.
pub const STOP_MONITORING_MEMBER_ACTION: &str = "stop monitoring";

/// The action `DeleteMembers` attempts, as the refusal below names it.
pub const DELETE_MEMBER_ACTION: &str = "delete";

/// The message a call that would end or suspend a member relationship the
/// ORGANIZATION manages is refused with, naming the `action` it attempted — one of
/// [`DISASSOCIATE_MEMBER_ACTION`], [`STOP_MONITORING_MEMBER_ACTION`] and
/// [`DELETE_MEMBER_ACTION`].
///
/// "With `autoEnableOrganizationMembers` configuration for your organization set to
/// `ALL`, you'll receive an error if you attempt to disassociate a member account
/// before removing them from your organization"
/// (`smithy_query(query="doc",shape="DisassociateMembers")`), and the same
/// configuration refuses stopping monitoring
/// (`smithy_query(query="doc",shape="StopMonitoringMembers")`) and disabling
/// GuardDuty for a member (`smithy_query(query="doc",shape="DeleteMembers")`): one
/// precondition over the operations that take a member relationship away, so one
/// message states it and names the remedy the documentation gives.
///
/// The precondition holds for the whole call rather than per account: a member
/// record says which accounts this administrator holds, and nothing records which
/// of them joined through the organization rather than through an invitation, so
/// every member of an administrator that auto-enables all of its organization's
/// accounts is one of them. A later change that records how a member was added can
/// narrow it to those accounts.
pub fn organization_managed_members_message(action: &str) -> String {
    format!(
        "The request is rejected because the current account cannot {action} the given member account ID while {AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER} is set to {AUTO_ENABLE_ALL} for the organization. Remove the account from the organization first."
    )
}

/// The message a MEMBER account's own request to leave the administrator
/// relationship the ORGANIZATION manages is refused with.
///
/// The same precondition [`organization_managed_members_message`] states, worded
/// for the side of the relationship that names no member account — the caller is
/// the member: "with `autoEnableOrganizationMembers` configuration for your
/// organization set to `ALL`, you'll receive an error if you attempt to disable
/// GuardDuty in a member account"
/// (`smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")`). It
/// names the same remedy the documentation gives for the administrator-side calls,
/// because it is the same one: the account leaves the organization first.
pub fn organization_managed_administrator_message() -> String {
    format!(
        "The request is rejected because the current account cannot disassociate from its administrator account while {AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER} is set to {AUTO_ENABLE_ALL} for the organization. Remove the account from the organization first."
    )
}

/// Refuse an organization `Features` list the operation documents as
/// contradictory, with the message the refusal carries.
///
/// Every value in the list is an enum member the boundary already checked, so what
/// is left is the rule that spans two entries, which
/// [`refuse_runtime_monitoring_overlap`] holds for a detector's list and an
/// organization's alike.
///
/// The rule is judged on the auto-enablement each feature ENDS UP with, not on what
/// the request mentions: the request's value where it states one, and `configuration`'s
/// stored value otherwise (a list that omits a feature, or names it without an
/// `AutoEnable`, leaves that switch alone — see
/// [`apply_org_feature_configurations`]). So enabling one of the two while the other
/// is already stored enabled is refused, and naming one of them without a status
/// while enabling the other is not.
pub fn validate_org_feature_configurations(
    configuration: &OrganizationConfiguration,
    features: Option<&Vec<crate::ops::types::OrganizationFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    let enabled = |wanted: &str| {
        // The LAST entry for a name is the one that lands, as applying the list
        // writes them in order.
        let requested = features
            .iter()
            .rfind(|feature| feature.name.as_deref() == Some(wanted))
            .and_then(|feature| feature.auto_enable.as_deref());
        requested.unwrap_or_else(|| configuration.feature_auto_enable(wanted)) != AUTO_ENABLE_NONE
    };
    refuse_runtime_monitoring_overlap(enabled)
}

/// Write a `Features` list onto an organization configuration. A feature the list
/// does not name keeps the auto-enablement it already had, the way
/// [`apply_feature_configurations`] leaves the detector's own switches — and so
/// does a feature the list names WITHOUT an `AutoEnable`, which is an optional
/// member (`smithy_query(shape="OrganizationFeatureConfiguration")`): an entry that
/// states no auto-enablement states nothing to change about it, and may still carry
/// the additional configurations below.
pub fn apply_org_feature_configurations(
    configuration: &mut OrganizationConfiguration,
    features: Option<&Vec<crate::ops::types::OrganizationFeatureConfiguration>>,
) {
    let Some(features) = features else { return };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        let auto_enable = feature
            .auto_enable
            .as_deref()
            .unwrap_or_else(|| configuration.feature_auto_enable(name))
            .to_string();
        // `None` for a name no `OrgFeature` carries, which the boundary refuses
        // before this runs: such an entry writes nothing at all.
        let Some(record) = configuration.set_feature(name, &auto_enable) else {
            continue;
        };
        if let Some(additional) = feature.additional_configuration.as_ref() {
            record.additional_configuration = additional
                .iter()
                .filter_map(|configuration| {
                    configuration
                        .name
                        .as_deref()
                        .map(|name| OrgAdditionalConfigRecord {
                            name: name.to_string(),
                            auto_enable: configuration
                                .auto_enable
                                .as_deref()
                                .unwrap_or(AUTO_ENABLE_NONE)
                                .to_string(),
                        })
                })
                .collect();
        }
    }
}

/// Write the deprecated organization `DataSources` view onto an organization
/// configuration.
///
/// `DataSources` is "deprecated, use Features instead"
/// (`smithy_query(operation="UpdateOrganizationConfiguration")`), so each node is
/// stored as the `OrgFeature` that governs it — one source of truth, read back
/// through either view, exactly as [`apply_data_sources`] stores the detector's own
/// data sources. The boolean becomes an `OrgFeatureStatus` through
/// [`org_status_for_flag`].
pub fn apply_org_data_sources(
    configuration: &mut OrganizationConfiguration,
    data_sources: Option<&crate::ops::types::OrganizationDataSourceConfigurations>,
) {
    let Some(data_sources) = data_sources else {
        return;
    };
    if let Some(s3) = data_sources.s3_logs.as_ref() {
        configuration.set_feature(FEATURE_S3_DATA_EVENTS, org_status_for_flag(s3.auto_enable));
    }
    if let Some(kubernetes) = data_sources.kubernetes.as_ref() {
        configuration.set_feature(
            FEATURE_EKS_AUDIT_LOGS,
            org_status_for_flag(kubernetes.audit_logs.auto_enable),
        );
    }
    if let Some(auto_enable) = data_sources
        .malware_protection
        .as_ref()
        .and_then(|malware| malware.scan_ec2_instance_with_findings.as_ref())
        .and_then(|scan| scan.ebs_volumes.as_ref())
        .and_then(|volumes| volumes.auto_enable)
    {
        configuration.set_feature(
            FEATURE_EBS_MALWARE_PROTECTION,
            org_status_for_flag(auto_enable),
        );
    }
}

// ---------------------------------------------------------------------------
// Reporting malware scans
//
// `ListMalwareScans` and `DescribeMalwareScans` both report a SET of the
// [`MalwareScan`] records, narrowed by a filter criteria structure and ordered by
// a `SortCriteria`, so the criterion lookup, the filter and the sort live here —
// once — beside the record they read, and each operation builds only its own
// modelled output.
//
// The two operations differ ONLY in their criterion-key enum
// (`ListMalwareScansCriterionKey` against `CriterionKey`) and in the shape of the
// criteria structure that carries them, so the rules that do not depend on either
// — the condition comparison ([`filter_condition_matches`]), the two refusal rules
// ([`validate_scan_criteria`]) and the conjunction over entries
// ([`scan_criteria_match`]) — take the entries as `(key, condition)` pairs plus
// the key list, and each operation supplies its own key lookup.
// ---------------------------------------------------------------------------

/// Every `ListMalwareScansCriterionKey` member a filter may name: the crate's
/// MODEL-DERIVED list, so a key the model gains cannot be missed here.
pub const MALWARE_SCAN_CRITERION_KEYS: &[&str] =
    crate::ops::types::shared_types::LIST_MALWARE_SCANS_CRITERION_KEY_VALUES;

/// The criterion key whose value is an INSTANT, and so the only one an ordering
/// condition can compare: `FilterCondition.GreaterThan` / `LessThan` are `long`
/// bounds (`smithy_query(shape="FilterCondition")`), which every other key's
/// value — an ARN, an id, an enum member — has no numeric form for.
///
/// It is the temporal member of BOTH criterion enums — `SCAN_START_TIME` is a
/// member of `ListMalwareScansCriterionKey` and of `CriterionKey`
/// (`smithy_query(shape="CriterionKey")`) — so one constant serves both
/// scan-reporting operations.
pub const MALWARE_SCAN_TEMPORAL_KEY: &str = "SCAN_START_TIME";

/// Resolve one scan value by its `ListMalwareScansCriterionKey`, or `None` when
/// the key is not a member of that enum, or the scan has no value for it.
pub fn malware_scan_criterion_value(scan: &MalwareScan, key: &str) -> Option<Attribute> {
    let value = match key {
        "RESOURCE_ARN" => Attribute::Text(scan.resource_arn.clone()),
        "SCAN_ID" => Attribute::Text(scan.id.clone()),
        // Every scan this emulator holds was started by this account through this
        // emulator, so the account a filter compares is the emulator's own.
        "ACCOUNT_ID" => Attribute::Text(::engine::emulator_account_id().to_string()),
        "GUARDDUTY_FINDING_ID" => {
            // `TriggerDetails.GuardDutyFindingId` names the finding that triggered
            // a GuardDuty-INITIATED scan. Every scan here was asked for through
            // `StartMalwareScan` ([`MALWARE_SCAN_TYPE_ON_DEMAND`]), so none has a
            // triggering finding, and an absent member has no value to compare: no
            // condition on this key matches such a scan, not even one naming the
            // empty string.
            return None;
        }
        "RESOURCE_TYPE" => Attribute::Text(scan.resource_type.clone()),
        MALWARE_SCAN_TEMPORAL_KEY => Attribute::Number(scan.started_at_millis as f64),
        "SCAN_STATUS" => Attribute::Text(scan.status.clone()),
        "SCAN_TYPE" => Attribute::Text(scan.scan_type.clone()),
        _ => return None,
    };
    Some(value)
}

/// Does `value` — the value a criterion key resolves to, or `None` when the record
/// has none — satisfy one `FilterCondition`?
///
/// `EqualsValue` compares the value's text. An instant has two textual forms a
/// client could reasonably send — its ISO-8601 rendering and its epoch
/// milliseconds — and either matches, because the model fixes neither.
///
/// A condition that populates nothing constrains nothing, on EVERY key: it selects
/// every record, including one whose value for that key is absent. Only a condition
/// that does constrain can exclude a record the key has no value for.
///
/// `FilterCondition` is the ONE condition shape both scan-reporting operations
/// filter through — `ListMalwareScans`'s `ListMalwareScansFilterCriterion` and
/// `DescribeMalwareScans`'s `FilterCriterion` both target it
/// (`smithy_query(shape="FilterCriterion")`) — so the comparison lives here once
/// and each operation supplies only its own key lookup.
pub fn filter_condition_matches(
    value: Option<Attribute>,
    condition: &crate::ops::types::FilterCondition,
) -> bool {
    let constrains = condition.equals_value.is_some()
        || condition.greater_than.is_some()
        || condition.less_than.is_some();
    if !constrains {
        return true;
    }
    let Some(value) = value else {
        return false;
    };
    if let Some(expected) = condition.equals_value.as_deref() {
        let matched = match value.as_number() {
            Some(millis) => {
                iso8601(millis as i64) == expected || value.as_text() == expected
            }
            None => value.as_text() == expected,
        };
        if !matched {
            return false;
        }
    }
    for (bound, greater) in [(condition.greater_than, true), (condition.less_than, false)] {
        let Some(bound) = bound else { continue };
        let Some(number) = value.as_number() else {
            return false;
        };
        let holds = if greater {
            number > bound as f64
        } else {
            number < bound as f64
        };
        if !holds {
            return false;
        }
    }
    true
}

/// Does `scan` satisfy `criteria`? An absent or empty criteria structure matches
/// every scan.
///
/// The model caps the criterion list at one entry (len 0..1,
/// `smithy_query(shape="ListMalwareScansFilterCriteria")`), so the boundary
/// refuses a longer one before a handler runs; an entry with no key or no
/// condition constrains nothing.
pub fn malware_scan_matches(
    scan: &MalwareScan,
    criteria: Option<&crate::ops::types::ListMalwareScansFilterCriteria>,
) -> bool {
    scan_criteria_match(list_malware_scans_criterion_pairs(criteria), |key| {
        malware_scan_criterion_value(scan, key)
    })
}

/// Does a record satisfy EVERY entry of a filter criteria structure?
///
/// `criterion` is the structure's entries as `(key, condition)` pairs, each
/// optional: an entry naming no key, or carrying no condition, constrains nothing.
/// `value` resolves one of the record's values by criterion key — the one thing
/// that differs between the two scan-reporting operations, each of which has a
/// criterion enum of its own.
pub fn scan_criteria_match<'a>(
    criterion: impl IntoIterator<
        Item = (
            Option<&'a str>,
            Option<&'a crate::ops::types::FilterCondition>,
        ),
    >,
    value: impl Fn(&str) -> Option<Attribute>,
) -> bool {
    for (key, condition) in criterion {
        let (Some(key), Some(condition)) = (key, condition) else {
            continue;
        };
        if !filter_condition_matches(value(key), condition) {
            return false;
        }
    }
    true
}

/// A `ListMalwareScansFilterCriteria`'s entries as the pairs the shared filter
/// rules read.
fn list_malware_scans_criterion_pairs(
    criteria: Option<&crate::ops::types::ListMalwareScansFilterCriteria>,
) -> impl Iterator<
    Item = (
        Option<&str>,
        Option<&crate::ops::types::FilterCondition>,
    ),
> {
    criteria
        .and_then(|c| c.list_malware_scans_filter_criterion.as_ref())
        .into_iter()
        .flatten()
        .map(|entry| {
            (
                entry.list_malware_scans_criterion_key.as_deref(),
                entry.filter_condition.as_ref(),
            )
        })
}

/// Refuse a `ListMalwareScansFilterCriteria` this service cannot evaluate, with
/// the message the refusal carries.
///
/// Checked BEFORE any scan is looked at, so whether a criterion is refused does
/// not depend on how many scans happen to be stored: an account with none must
/// answer an unevaluable criterion exactly as a populated one does. Two rules: the
/// key must be a `ListMalwareScansCriterionKey` member, and an ordering bound
/// applies only to the one key whose value is an instant.
pub fn validate_malware_scan_criteria(
    criteria: Option<&crate::ops::types::ListMalwareScansFilterCriteria>,
) -> Result<(), String> {
    validate_scan_criteria(
        list_malware_scans_criterion_pairs(criteria),
        MALWARE_SCAN_CRITERION_KEYS,
    )
}

/// Refuse a filter criteria structure this service cannot evaluate, with the
/// message the refusal carries.
///
/// `criterion` is the structure's entries as `(key, condition)` pairs and `keys`
/// the members the operation's criterion enum declares. Two rules, the same for
/// every operation that filters scans: the key must be one of `keys`, and an
/// ordering bound applies only to [`MALWARE_SCAN_TEMPORAL_KEY`], the one key whose
/// value is an instant.
pub fn validate_scan_criteria<'a>(
    criterion: impl IntoIterator<
        Item = (
            Option<&'a str>,
            Option<&'a crate::ops::types::FilterCondition>,
        ),
    >,
    keys: &[&str],
) -> Result<(), String> {
    for (key, condition) in criterion {
        let Some(key) = key else {
            continue;
        };
        if !keys.contains(&key) {
            return Err(format!(
                "filterCriteria: '{key}' is not a malware scan criterion key. Supported keys are {}.",
                keys.join(", ")
            ));
        }
        let orders =
            condition.is_some_and(|c| c.greater_than.is_some() || c.less_than.is_some());
        if orders && key != MALWARE_SCAN_TEMPORAL_KEY {
            return Err(format!(
                "filterCriteria: greaterThan and lessThan compare instants, so they apply only to {MALWARE_SCAN_TEMPORAL_KEY}, not to '{key}'."
            ));
        }
    }
    Ok(())
}

/// The scan attributes this service can sort on, as the `MalwareScan` shape's own
/// `jsonName`s name them (`smithy_query(shape="MalwareScan")`) — the members a
/// `ListMalwareScans` response carries are the ones a client can ask it to order
/// by.
pub const MALWARE_SCAN_ATTRIBUTES: [&str; 8] = [
    "resourceArn",
    "resourceType",
    "scanCompletedAt",
    "scanId",
    "scanResultStatus",
    "scanStartedAt",
    "scanStatus",
    "scanType",
];

/// The attribute a `ListMalwareScans` sort orders by when the request names none:
/// when the scan started, which with [`DESCENDING_BY_DEFAULT`] puts the newest
/// scan first.
pub const MALWARE_SCAN_DEFAULT_SORT_ATTRIBUTE: &str = "scanStartedAt";

/// A scan sort that names no explicit `OrderBy` runs newest-first, like
/// [`sort_findings`] — the order both scan-reporting operations report an
/// unordered request in, so they cannot disagree about "unordered".
pub const DESCENDING_BY_DEFAULT: bool = true;

/// Resolve one scan attribute by its `MalwareScan` member name, or `None` when
/// this service does not model that name.
pub fn malware_scan_attribute(scan: &MalwareScan, path: &str) -> Option<Attribute> {
    let value = match path {
        "resourceArn" => Attribute::Text(scan.resource_arn.clone()),
        "resourceType" => Attribute::Text(scan.resource_type.clone()),
        "scanCompletedAt" => Attribute::Number(scan.completed_at_millis.unwrap_or(0) as f64),
        "scanId" => Attribute::Text(scan.id.clone()),
        // Absent until a scan produces a result, and ordering by an absent value
        // is ordering by the empty string.
        "scanResultStatus" => Attribute::Text(String::new()),
        "scanStartedAt" => Attribute::Number(scan.started_at_millis as f64),
        "scanStatus" => Attribute::Text(scan.status.clone()),
        "scanType" => Attribute::Text(scan.scan_type.clone()),
        _ => return None,
    };
    Some(value)
}

/// Order `scans` for `ListMalwareScans`.
///
/// With no `SortCriteria` the newest scan comes first, with the scan id breaking
/// ties so the order is total and a page token stays meaningful.
/// `Err(attribute)` means this service does not model that sort attribute.
pub fn sort_malware_scans(
    scans: &mut [&MalwareScan],
    sort: Option<&crate::ops::types::SortCriteria>,
) -> Result<(), String> {
    let attribute = sort
        .and_then(|s| s.attribute_name.as_deref())
        .filter(|a| !a.is_empty());
    let descending = match sort.and_then(|s| s.order_by.as_deref()) {
        Some(ORDER_BY_ASCENDING) => false,
        Some(ORDER_BY_DESCENDING) => true,
        // No explicit order: newest-first for the default attribute, ascending for
        // an attribute the caller named.
        _ => attribute.is_none() && DESCENDING_BY_DEFAULT,
    };
    let attribute = attribute.unwrap_or(MALWARE_SCAN_DEFAULT_SORT_ATTRIBUTE);
    if !MALWARE_SCAN_ATTRIBUTES.contains(&attribute) {
        return Err(attribute.to_string());
    }

    scans.sort_by(|a, b| {
        compare_by_attribute(
            *a,
            *b,
            descending,
            |scan| malware_scan_attribute(scan, attribute),
            |scan| scan.id.clone(),
        )
    });
    Ok(())
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

/// Project one stored `ScanCriterion` back onto the wire shape.
///
/// `ScanResourceCriteria` carries BOTH its criteria on every read: a detector with
/// no criterion of that kind reports the criterion as an empty map rather than
/// leaving the member out, so a client that reads one to add a condition — and
/// round-trips the read into `UpdateMalwareScanSettings` — finds the map it is
/// about to write into already there.
pub fn scan_criterion_shape(
    record: &ScanCriterionRecord,
) -> Option<std::collections::HashMap<String, crate::ops::types::ScanCondition>> {
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

/// The message a read or an association of a rule the catalogue does not publish
/// answers with.
///
/// A rule is addressed by its `RuleId`, which is the caller's own request value and
/// the only thing that identifies which rule was missing, so GuardDuty echoes it
/// after a fixed prefix: a caller resolving a batch of rule ids can tell WHICH one
/// the catalogue does not carry.
///
/// `RuleId` carries `@pattern` `^[a-z0-9]+(-[a-z0-9]+)*$` and `@length` 1..100
/// (`smithy_query(operation="GetCustomDetectionRule")`), both refused at the request
/// boundary, so the value interpolated here is a short lower-case identifier and
/// never arbitrary caller text.
pub fn custom_detection_rule_not_found_message(rule_id: &str) -> String {
    format!("Rule not found: {rule_id}")
}

/// The FIXED resource path every custom-detection-rule ARN opens with: the
/// resource type the ARN's own shape names (`DetectionRuleArn`), qualified by the
/// `custom` family the rule belongs to, so a custom rule's ARN is never the ARN of
/// a rule of another family.
///
/// One constant for the rule and for the associations hanging off it
/// ([`custom_detection_rule_association_arn`]), so no operation can report a path
/// another one does not.
const CUSTOM_DETECTION_RULE_ARN_PATH: &str = "detection-rule/custom";

/// The ARN of the custom detection rule `rule_id` as `region`'s endpoint reports
/// it.
///
/// Derived, never stored, exactly as [`malware_protection_plan_arn`] is: the rule's
/// own id and the Region serving the read are the only variable parts, so a read
/// cannot report an ARN that disagrees with the id it was asked about. The resource
/// path is [`CUSTOM_DETECTION_RULE_ARN_PATH`] followed by the rule's own id.
pub fn custom_detection_rule_arn(region: &str, rule_id: &str) -> String {
    ::engine::emulator_arn(
        GUARDDUTY_ARN_SERVICE,
        region,
        &format!("{CUSTOM_DETECTION_RULE_ARN_PATH}/{rule_id}"),
    )
}

// ---------------------------------------------------------------------------
// Custom detection rule associations and organization configurations
//
// A rule is published by GuardDuty; what an ACCOUNT owns is its association with
// one, and — for an organization's delegated administrator — the organization
// configuration that says which member accounts the rule applies to. Both are
// identified by the `(RuleId, Mode)` pair they are created for.
// ---------------------------------------------------------------------------

/// The wire name of the `Mode` member — `jsonName` `mode` on the body-bound
/// operations and the query parameter `mode` on the rest
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,GetCustomDetectionRuleOrgConfiguration")`).
pub const MODE_MEMBER: &str = "mode";

/// The wire name of the `AssociationId` member — the URI label
/// `AssociationId`, rendered `associationId` in a response body
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation")`).
pub const ASSOCIATION_ID_MEMBER: &str = "associationId";

/// `DetectionRuleConfigurationStatus`: the organization configuration is in
/// effect. One of `ACTIVE | PROCESSING | FAILED`
/// (`smithy_query(shape="DetectionRuleConfigurationStatus")`).
pub const DETECTION_RULE_CONFIGURATION_ACTIVE: &str = "ACTIVE";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_CONFIGURATION_STATUS_VALUES,
    DETECTION_RULE_CONFIGURATION_ACTIVE
));

/// `DetectionRuleConfigurationStatus`: the organization configuration has been
/// accepted and is being rolled out to the organization's accounts. See
/// [`DETECTION_RULE_CONFIGURATION_ACTIVE`] for the enum.
pub const DETECTION_RULE_CONFIGURATION_PROCESSING: &str = "PROCESSING";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_CONFIGURATION_STATUS_VALUES,
    DETECTION_RULE_CONFIGURATION_PROCESSING
));

/// How long an organization configuration stays
/// [`DETECTION_RULE_CONFIGURATION_PROCESSING`] before the CLOCK owes it
/// [`DETECTION_RULE_CONFIGURATION_ACTIVE`].
///
/// GuardDuty documents no duration for rolling a rule configuration out to an
/// organization, so this emulator CHOOSES one — the same span
/// [`IP_SET_TRANSITION_MILLIS`] uses, and non-zero for the same reason: a caller
/// that reads the configuration back immediately after creating it still observes
/// the transient `PROCESSING` status.
pub const ORG_CONFIGURATION_TRANSITION_MILLIS: i64 = 500;

/// How many Regions this store will hold custom-detection-rule associations and
/// organization configurations for.
///
/// Per Region the records are bounded by construction — one association per
/// published rule, and one organization configuration per published rule per
/// `AssociationMode` member — but the Region is read from the request's
/// credential scope ([`request_region`]), so the caller chooses the partition key:
/// without a ceiling on the number of partitions a caller signing for invented
/// Region names could make the process retain a full set per distinct Region
/// string. Emulator-chosen, like [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`], and
/// enforced before either create writes
/// ([`ComAmazonawsGuarddutyGuarddutyapiserviceState::detection_rule_regions_exhausted`]).
pub const MAX_DETECTION_RULE_REGIONS: usize = 50;

/// The message a create that would partition the custom-detection-rule stores
/// into one Region more than [`MAX_DETECTION_RULE_REGIONS`] is refused with.
pub const DETECTION_RULE_REGION_LIMIT_MESSAGE: &str =
    "The request is rejected because the maximum number of custom detection rule associations has been reached.";

/// The message a read, an update or a delete of an association the account does not
/// hold answers with.
///
/// An association is addressed by its `AssociationId`, which is the caller's own
/// request value and the only thing that says WHICH association was missing, so
/// GuardDuty echoes it after a fixed prefix — the same shape it answers a missing
/// rule with ([`custom_detection_rule_not_found_message`]).
///
/// `AssociationId` carries `@pattern` `^[a-zA-Z0-9_-]{1,64}$` and `@length` 1..64
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation")`), both refused at
/// the request boundary, so the value interpolated here is a short identifier and
/// never arbitrary caller text.
pub fn detection_rule_association_not_found_message(association_id: &str) -> String {
    format!("Association not found: {association_id}")
}

/// The message a second association for a rule the account has already associated
/// is refused with — the `ConflictException` the create models. Creating is not
/// updating: `UpdateCustomDetectionRuleAssociation` is the operation that changes
/// an existing association's mode, so the refusal holds whatever mode either call
/// names.
///
/// The conflict is about ONE rule, named by the `RuleId` the request carried, so
/// GuardDuty echoes that id after a fixed prefix: a caller associating a batch of
/// rules can tell which of them was already associated. The id is bounded by the
/// member's own pattern and length, as it is for
/// [`custom_detection_rule_not_found_message`].
pub fn detection_rule_association_conflict_message(rule_id: &str) -> String {
    format!("Association already exists for rule {rule_id}")
}

/// The message a read of an organization configuration the account has not
/// created answers with.
pub fn detection_rule_org_configuration_not_found_message() -> String {
    format!(
        "The request is rejected because the input {RULE_ID_MEMBER} and {MODE_MEMBER} do not refer to an organization configuration of the current account."
    )
}

/// The message a second organization configuration for a `(RuleId, Mode)` pair
/// the account has already configured is refused with — the `ConflictException`
/// the create models.
pub fn detection_rule_org_configuration_conflict_message() -> String {
    format!(
        "The request is rejected because the current account already has an organization configuration for the given {RULE_ID_MEMBER} and {MODE_MEMBER}."
    )
}

/// The message an update of a rule the account holds no organization
/// configuration for answers with.
///
/// It names only `RuleId`, because that is what the update identifies the
/// configuration by: `Mode` is one of the values the update SETS, not a selector
/// (`smithy_query(query="doc",shape="UpdateCustomDetectionRuleOrgConfiguration")`
/// — "including the mode and include/exclude account lists").
pub fn detection_rule_org_configuration_rule_not_found_message() -> String {
    format!(
        "The request is rejected because the input {RULE_ID_MEMBER} does not refer to a custom detection rule with an organization configuration of the current account."
    )
}

/// The wire names of the two account lists an organization configuration's scope is
/// carried by (`jsonName` `includeAccountIds` / `excludeAccountIds` —
/// `smithy_query(shape="DetectionRuleOrgConfiguration")`), so a refusal that names
/// the list it was handed spells it the way the request did.
pub const INCLUDE_ACCOUNT_IDS_MEMBER: &str = "includeAccountIds";
/// See [`INCLUDE_ACCOUNT_IDS_MEMBER`].
pub const EXCLUDE_ACCOUNT_IDS_MEMBER: &str = "excludeAccountIds";

/// The message a request naming both `IncludeAccountIds` and `ExcludeAccountIds`
/// is refused with: the two are "mutually exclusive"
/// (`smithy_query(query="doc",shape="DetectionRuleOrgConfiguration",member="IncludeAccountIds")`).
pub const ORG_CONFIGURATION_ACCOUNT_SCOPE_MESSAGE: &str =
    "The request is rejected because includeAccountIds and excludeAccountIds are mutually exclusive.";

/// The ARN of the association `association_id` of rule `rule_id`, as `region`'s
/// endpoint reports it.
///
/// Derived, never stored, exactly as [`custom_detection_rule_arn`] is: the ids and
/// the Region serving the read are the only variable parts, so a read cannot
/// report an ARN that disagrees with the ids it was asked about. The resource path
/// EXTENDS the rule's own — it is built from [`custom_detection_rule_arn`] rather
/// than spelled again — because an association belongs to a rule.
pub fn custom_detection_rule_association_arn(
    region: &str,
    rule_id: &str,
    association_id: &str,
) -> String {
    format!(
        "{}/association/{association_id}",
        custom_detection_rule_arn(region, rule_id)
    )
}

/// `AssociationMode`: the association evaluates the rule without producing
/// findings — one of `LIVE | DRY_RUN`
/// (`smithy_query(shape="AssociationMode")`).
pub const ASSOCIATION_MODE_DRY_RUN: &str = "DRY_RUN";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ASSOCIATION_MODE_VALUES,
    ASSOCIATION_MODE_DRY_RUN
));

/// How long after it enters [`ASSOCIATION_MODE_DRY_RUN`] an association stops
/// evaluating its rule — fourteen days, the dry-run trial window GuardDuty gives
/// an association it does not keep evaluating for ever.
///
/// Not a wait a caller asks for and not a status the clock moves: it is the
/// instant `ExpiresAt` reports, so a caller can tell when a temporary association
/// stops and schedule its renewal.
pub const DRY_RUN_ASSOCIATION_LIFETIME_MILLIS: i64 = 14 * 24 * 60 * 60 * 1_000;

/// When an association that entered `mode` at `since_millis` expires, or `None`
/// for a mode that does not expire.
///
/// `ExpiresAt` is "the timestamp when the association expires"
/// (`smithy_query(shape="AssociationDetail")`) and only a DRY_RUN association has
/// one: a `LIVE` association evaluates its rule until the account deletes it, so
/// there is no instant to report for it and the optional member stays absent.
/// Stored on the record ([`DetectionRuleAssociationRecord::expires_at_millis`])
/// rather than recomputed per read, so every read of one association reports the
/// same instant.
pub fn dry_run_association_expiry(mode: &str, since_millis: i64) -> Option<i64> {
    (mode == ASSOCIATION_MODE_DRY_RUN)
        .then(|| since_millis.saturating_add(DRY_RUN_ASSOCIATION_LIFETIME_MILLIS))
}

/// One custom detection rule association as the store holds it.
///
/// Timestamps are epoch milliseconds, the single source for the instants anything
/// reports.
#[derive(Debug, Clone)]
pub struct DetectionRuleAssociationRecord {
    /// The minted `AssociationId`: `len 1..64`, `pattern ^[a-zA-Z0-9_-]{1,64}$`
    /// (`smithy_query(shape="AssociationDetail")`), which the 32 hexadecimal
    /// characters [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] mints
    /// satisfy.
    pub association_id: String,
    /// The Region whose endpoint made this association, and the only one that
    /// reports it.
    pub region: String,
    /// The `RuleId` of the published rule this association binds.
    pub rule_id: String,
    /// `AssociationDetail.AccountId` — "the Amazon Web Services account ID
    /// associated with this rule association", which is the account the emulator
    /// serves and NOT caller input.
    pub account_id: String,
    /// `AssociationMode`: `LIVE` or `DRY_RUN`.
    pub mode: String,
    /// The tags of the association resource.
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the create carried, so a replay answers with this same
    /// association instead of making a second one.
    pub client_token: Option<String>,
    /// When this association stops evaluating its rule, for a mode that expires —
    /// [`dry_run_association_expiry`] of the instant it entered its current mode.
    /// `None` for a mode that does not expire.
    pub expires_at_millis: Option<i64>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

/// One custom detection rule organization configuration as the store holds it.
#[derive(Debug, Clone)]
pub struct DetectionRuleOrgConfigurationRecord {
    /// The Region whose endpoint created this configuration, and the only one that
    /// reports it.
    pub region: String,
    /// The `RuleId` of the published rule this configuration is for.
    pub rule_id: String,
    /// `AssociationMode`: `LIVE` or `DRY_RUN`.
    pub mode: String,
    /// The member accounts the configuration includes, empty when it names none.
    pub include_account_ids: Vec<String>,
    /// The member accounts the configuration excludes, empty when it names none.
    /// Mutually exclusive with [`Self::include_account_ids`], which the create
    /// refuses before it writes.
    pub exclude_account_ids: Vec<String>,
    /// When the clock owes this configuration
    /// [`DETECTION_RULE_CONFIGURATION_ACTIVE`]. Absent once nothing is owed.
    pub status_due_millis: Option<i64>,
    /// The `ClientToken` the create carried.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl DetectionRuleOrgConfigurationRecord {
    /// This configuration's `Status` at `now`.
    ///
    /// DERIVED from the instant the create recorded rather than stored: the move
    /// out of [`DETECTION_RULE_CONFIGURATION_PROCESSING`] was owed at
    /// [`Self::status_due_millis`] whether or not anybody called, so a read
    /// reports it without writing anything — which is what keeps a report from
    /// changing what a later caller sees.
    pub fn status(&self, now: i64) -> &'static str {
        match self.status_due_millis {
            Some(due) if now < due => DETECTION_RULE_CONFIGURATION_PROCESSING,
            _ => DETECTION_RULE_CONFIGURATION_ACTIVE,
        }
    }
}

/// The wire `AssociationDetail` for a stored association.
///
/// One builder for the create that mints an association and the read that reports
/// it, so the two cannot disagree — the same reason [`member_view`] is shared. The
/// `Arn` is DERIVED from the record's own ids and `region`, never stored.
///
/// `ExpiresAt` is the shape's only optional member — "the timestamp when the
/// association expires" (`smithy_query(shape="AssociationDetail")`) — and is read
/// off the record, so it is reported for an association whose mode expires
/// ([`dry_run_association_expiry`]) and absent for one that does not.
pub fn detection_rule_association_view(
    record: &DetectionRuleAssociationRecord,
) -> crate::ops::types::AssociationDetail {
    crate::ops::types::AssociationDetail {
        association_id: record.association_id.clone(),
        arn: custom_detection_rule_association_arn(
            &record.region,
            &record.rule_id,
            &record.association_id,
        ),
        rule_id: record.rule_id.clone(),
        account_id: record.account_id.clone(),
        mode: record.mode.clone(),
        created_at: ::aws_smithy_types::DateTime::from_millis(record.created_at_millis),
        updated_at: ::aws_smithy_types::DateTime::from_millis(record.updated_at_millis),
        expires_at: record
            .expires_at_millis
            .map(::aws_smithy_types::DateTime::from_millis),
    }
}

/// The wire `DetectionRuleOrgConfiguration` for a stored organization
/// configuration, as of `now`.
///
/// Shared by the create and the read for the same reason
/// [`detection_rule_association_view`] is. `Status` comes from
/// [`DetectionRuleOrgConfigurationRecord::status`], so both report the same
/// derivation of the clock; `StatusReason` is "the reason for the current
/// configuration status" and a configuration that is proceeding normally has
/// none, so it stays absent. `ExpiresAt` is absent for the same reason an
/// association's is.
pub fn detection_rule_org_configuration_view(
    record: &DetectionRuleOrgConfigurationRecord,
    now: i64,
) -> crate::ops::types::DetectionRuleOrgConfiguration {
    crate::ops::types::DetectionRuleOrgConfiguration {
        rule_id: record.rule_id.clone(),
        mode: record.mode.clone(),
        status: record.status(now).to_string(),
        status_reason: None,
        include_account_ids: record.include_account_ids.clone(),
        exclude_account_ids: record.exclude_account_ids.clone(),
        created_at: ::aws_smithy_types::DateTime::from_millis(record.created_at_millis),
        updated_at: ::aws_smithy_types::DateTime::from_millis(record.updated_at_millis),
        expires_at: None,
    }
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
    /// Every published rule, in `RuleId` order — the total order a page token names
    /// a position in.
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
/// The ids are the ones GuardDuty's own catalogue carries where a response has
/// exposed them, and this emulator's elsewhere: an operation handed a `ruleId`
/// outside this table — a `CreateCustomDetectionRuleAssociation`, say — answers the
/// modelled "no such rule" rather than inventing a rule to match it, which is the
/// correct answer for a rule this service does not publish. Nothing outside this
/// table names a rule.
const PUBLISHED_CUSTOM_DETECTION_RULES: &[PublishedRule] = &[
    PublishedRule {
        rule_id: "admin-policy-attached-to-role",
        name: "Administrator policy attached to an IAM role",
        description: "Detects a policy granting administrator permissions being attached to an IAM role, which lets every principal that can assume the role control the account.",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Privilege Escalation",
        technique: "T1098.003 Account Manipulation: Additional Cloud Roles",
        service: "iam",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'iam.amazonaws.com' AND eventName IN ('AttachRolePolicy', 'PutRolePolicy') AND requestParameters.policyArn = 'arn:aws:iam::aws:policy/AdministratorAccess'",
        runtime_platforms: &[],
    },
    PublishedRule {
        rule_id: "admin-policy-attached-to-user",
        name: "Administrator policy attached to an IAM user",
        description: "Detects a policy granting administrator permissions being attached to an IAM user, which hands that user full control of the account.",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Privilege Escalation",
        technique: "T1098.003 Account Manipulation: Additional Cloud Roles",
        service: "iam",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'iam.amazonaws.com' AND eventName IN ('AttachUserPolicy', 'PutUserPolicy') AND requestParameters.policyArn = 'arn:aws:iam::aws:policy/AdministratorAccess'",
        runtime_platforms: &[],
    },
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

/// The account one Region's organization has designated as its GuardDuty
/// delegated administrator.
///
/// The record carries only what a read reports plus the instant a removal falls
/// due: `AdminAccount` has an account id and a status and nothing else
/// (`smithy_query(shape="AdminAccount")`), and the status is a FUNCTION of the
/// pending removal ([`Self::status`]) rather than a field a caller can set to a
/// third value.
#[derive(Debug, Clone)]
pub struct OrganizationAdminAccountRecord {
    /// The designated account, as the designating request named it.
    pub admin_account_id: String,
    /// When a removal has been accepted, the instant the CLOCK owes it — until
    /// then the record reports the transient status, and from then on it is gone.
    /// `None` while no removal is pending.
    pub disable_due_millis: Option<i64>,
}

impl OrganizationAdminAccountRecord {
    /// `AdminAccount.AdminStatus` — "whether the account is enabled as the
    /// delegated administrator" (`smithy_query(shape="AdminAccount")`): the
    /// transient status while a removal is pending, [`ENABLED`] otherwise.
    pub fn status(&self) -> &'static str {
        if self.disable_due_millis.is_some() {
            ADMIN_DISABLE_IN_PROGRESS
        } else {
            ENABLED
        }
    }

    /// Has the removal this record carries fallen due by `now`?
    ///
    /// `false` while no removal is pending, and `false` for one still inside its
    /// span: that record is reported with the transient status instead.
    pub fn disable_settled(&self, now: i64) -> bool {
        self.disable_due_millis.is_some_and(|due| now >= due)
    }

    /// Accept a removal of this delegated administrator, due
    /// [`ADMIN_DISABLE_SPAN_MILLIS`] after `now`.
    ///
    /// Idempotent: a second removal of the same administrator leaves the instant
    /// the first one made due where it was, so repeating the call cannot postpone
    /// the removal for ever.
    pub fn begin_disable(&mut self, now: i64) {
        if self.disable_due_millis.is_none() {
            self.disable_due_millis = Some(now.saturating_add(ADMIN_DISABLE_SPAN_MILLIS));
        }
    }

    /// Turn around a removal still in flight: the record keeps the account it
    /// holds, reports [`ENABLED`] again and the clock owes nothing. `true` when a
    /// removal was pending, `false` when there was nothing to turn around.
    ///
    /// The other half of the rule [`EntitySetRecord::set_activation`] and
    /// [`ThreatIntelSetRecord::set_activation`] implement: asking a transitional
    /// record for the status it has already settled on, or is already
    /// transitioning to, owes nothing and answers `false`, while asking for the
    /// status it is transitioning AWAY from replaces whatever was owed, so a
    /// request that reverses a transition still running turns it around instead of
    /// being refused or queueing behind it.
    pub fn cancel_disable(&mut self) -> bool {
        self.disable_due_millis.take().is_some()
    }
}

/// The modelled `AdminAccount` a read reports for one delegated administrator.
///
/// Both members are optional and both are facts the record holds, so the
/// projection fills them and lives here once.
pub fn organization_admin_account_view(
    record: &OrganizationAdminAccountRecord,
) -> crate::ops::types::list_organization_admin_accounts_types::AdminAccount {
    crate::ops::types::list_organization_admin_accounts_types::AdminAccount {
        admin_account_id: Some(record.admin_account_id.clone()),
        admin_status: Some(record.status().to_string()),
    }
}

/// `AdminStatus.DISABLE_IN_PROGRESS`: the status a delegated administrator whose
/// removal has been accepted reports until the removal falls due. See
/// [`ENABLED`] for the other member of the enum.
pub const ADMIN_DISABLE_IN_PROGRESS: &str = "DISABLE_IN_PROGRESS";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ADMIN_STATUS_VALUES,
    ADMIN_DISABLE_IN_PROGRESS
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ADMIN_STATUS_VALUES,
    ENABLED
));

/// How long a delegated administrator whose removal was accepted stays
/// [`ADMIN_DISABLE_IN_PROGRESS`] before the CLOCK owes its removal.
///
/// AWS documents no duration for removing a delegated administrator, so this span
/// is CHOSEN — the same one [`ENTITY_SET_TRANSITION_MILLIS`] uses, and non-zero
/// for the same reason: a caller that lists the administrators straight after
/// asking for the removal still observes the transient status.
pub const ADMIN_DISABLE_SPAN_MILLIS: i64 = ENTITY_SET_TRANSITION_MILLIS;

/// How many Regions this store will hold a delegated administrator for.
///
/// Per Region the store holds at most one record, but the Region is read from the
/// request's credential scope ([`request_region`]), so without a ceiling on the
/// number of partitions a caller signing for invented Region names could make the
/// process retain a record per distinct Region string. Emulator-chosen, like
/// [`MAX_DETECTION_RULE_REGIONS`], and enforced before the designation writes
/// ([`ComAmazonawsGuarddutyGuarddutyapiserviceState::organization_admin_regions_exhausted`]).
pub const MAX_ORGANIZATION_ADMIN_REGIONS: usize = MAX_DETECTION_RULE_REGIONS;

/// The wire name of the `AdminAccountId` member, as both the designation and the
/// removal carry it (`smithy_query(operation="EnableOrganizationAdminAccount")`
/// renders the `jsonName` `adminAccountId`). It is what a rejection names.
pub const ADMIN_ACCOUNT_ID_MEMBER: &str = "adminAccountId";

/// Refusal of a designation made while the Region's organization already has a
/// delegated administrator.
///
/// A read reports `len 0..1` administrators
/// (`smithy_query(operation="ListOrganizationAdminAccounts")`), so there is room
/// for one: changing it means removing the current one first. The message states
/// the rule and quotes neither the caller's account id nor the stored one.
pub const ORGANIZATION_ADMIN_ALREADY_DESIGNATED_MESSAGE: &str = "The request is rejected because the organization already has a GuardDuty delegated administrator. Remove the current delegated administrator before designating another account.";

/// Refusal of a removal asked for while the Region's organization has no
/// delegated administrator at all.
pub const NO_ORGANIZATION_ADMIN_MESSAGE: &str =
    "The request is rejected because the organization has no GuardDuty delegated administrator.";

/// Refusal of a removal naming an account that is not the delegated administrator
/// the organization has.
pub const NOT_THE_ORGANIZATION_ADMIN_MESSAGE: &str = "The request is rejected because the given account ID is not the organization's GuardDuty delegated administrator.";

/// Refusal of a designation that would partition the delegated-administrator
/// store into one Region more than it holds
/// ([`MAX_ORGANIZATION_ADMIN_REGIONS`]). States the rule without quoting the
/// caller's Region back, like [`DETECTOR_REGION_LIMIT_MESSAGE`].
pub const ORGANIZATION_ADMIN_REGION_LIMIT_MESSAGE: &str = "The request is rejected because this emulator will not hold a GuardDuty delegated administrator for any further Regions.";

// ---------------------------------------------------------------------------
// Resource tagging
//
// A tagging operation names its resource by ARN rather than by id: `TagResource`,
// `UntagResource` and `ListTagsForResource` all carry a `GuardDutyArn`
// `ResourceArn` whose pattern confines it to the detector resource path
// (`smithy_query(operation="UntagResource")`). The parse and the tag-map lookup
// live here, once, so the operations that add, remove and report tags cannot
// disagree about which resource an ARN names or where its tags are kept.
// ---------------------------------------------------------------------------

/// The `Tags` member a read of a taggable resource reports, from the tag map the
/// resource's record holds.
///
/// Every `Get`/`Describe` in this service projects its resource's tags through
/// here, so one rule answers all of them: the member is ALWAYS reported, and a
/// resource nothing tagged reports an EMPTY map rather than no member at all —
/// which is what GuardDuty answers for an untagged resource (the recorded
/// `GetDetector` response of a detector created without tags carries an empty
/// `tags` map). A client can therefore read the map on any resource of this
/// service without a null guard.
///
/// The `len 1..200` the model puts on `TagMap`
/// (`smithy_query(query="shape", shape="TagMap")`) bounds what a REQUEST may
/// carry — which is why a create or `TagResource` sending an empty map is
/// refused at the modelled boundary — and is not a floor on what a read of a
/// resource reports.
pub fn reported_tags(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// A detector sub-resource that carries tags of its own.
///
/// One variant per collection hanging off [`Detector`] whose record has a tag
/// map; the detector itself is the ARN that carries no sub-resource path at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedSubResource {
    Filter,
    IpSet,
    ThreatIntelSet,
    TrustedEntitySet,
    ThreatEntitySet,
    PublishingDestination,
}

/// The ARN resource-path segment that names each sub-resource, which is the
/// segment that resource's own REST URI uses: `GetFilter` is bound to
/// `/detector/{DetectorId}/filter/{FilterName}`, `GetIPSet` to
/// `.../ipset/{IpSetId}`, `GetThreatIntelSet` to
/// `.../threatintelset/{ThreatIntelSetId}`, `GetTrustedEntitySet` to
/// `.../trustedentityset/{TrustedEntitySetId}`, `GetThreatEntitySet` to
/// `.../threatentityset/{ThreatEntitySetId}` and
/// `DescribePublishingDestination` to
/// `.../publishingDestination/{DestinationId}`
/// (`smithy_query(operation="GetFilter,GetIPSet,GetThreatIntelSet,GetTrustedEntitySet,GetThreatEntitySet,DescribePublishingDestination")`).
pub const TAGGED_SUB_RESOURCE_SEGMENTS: [(&str, TaggedSubResource); 6] = [
    ("filter", TaggedSubResource::Filter),
    ("ipset", TaggedSubResource::IpSet),
    ("threatintelset", TaggedSubResource::ThreatIntelSet),
    ("trustedentityset", TaggedSubResource::TrustedEntitySet),
    ("threatentityset", TaggedSubResource::ThreatEntitySet),
    (
        "publishingDestination",
        TaggedSubResource::PublishingDestination,
    ),
];

/// The resource a tagging operation's `ResourceArn` names, as the ARN spells it
/// out: the scope it claims plus the detector and — for a sub-resource — which
/// one of the detector's collections holds it, under which id.
///
/// Form only. Nothing here is resolved against the store, and the claimed scope
/// is not checked either: both are the job of
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tagged_resource_tags_mut`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    /// The partition the ARN claims.
    pub partition: String,
    /// The Region the ARN claims.
    pub region: String,
    /// The account the ARN claims.
    pub account_id: String,
    /// The `DetectorId` the resource path names.
    pub detector_id: String,
    /// The sub-resource the path continues into and the id that names it within
    /// the detector, or `None` when the ARN names the detector itself.
    pub sub_resource: Option<(TaggedSubResource, String)>,
}

/// The resource `arn` names, when `arn` is the ARN of a GuardDuty resource that
/// carries tags.
///
/// `None` when it is not: a resource path that is not a detector's, or one whose
/// type segment is none of [`TAGGED_SUB_RESOURCE_SEGMENTS`] — the Malware
/// Protection plan and custom-detection-rule ARNs
/// ([`malware_protection_plan_arn`], [`custom_detection_rule_arn`]) are of that
/// kind, and are not reachable through `/tags/{ResourceArn}` at all because the
/// member's pattern admits only the detector path.
pub fn parse_tagged_resource_arn(arn: &str) -> Option<TaggedResource> {
    let [_, partition, service, region, account_id, resource] = arn_fields(arn)?;
    if service != GUARDDUTY_ARN_SERVICE {
        return None;
    }
    let segments: Vec<&str> = resource.split('/').collect();
    let (detector_id, sub_resource) = match segments.as_slice() {
        ["detector", detector_id] if !detector_id.is_empty() => ((*detector_id).to_string(), None),
        ["detector", detector_id, kind, id] if !detector_id.is_empty() && !id.is_empty() => {
            let sub_resource = TAGGED_SUB_RESOURCE_SEGMENTS
                .iter()
                .find(|(segment, _)| segment == kind)
                .map(|(_, sub_resource)| *sub_resource)?;
            (
                (*detector_id).to_string(),
                Some((sub_resource, (*id).to_string())),
            )
        }
        _ => return None,
    };
    Some(TaggedResource {
        partition: partition.to_string(),
        region: region.to_string(),
        account_id: account_id.to_string(),
        detector_id,
        sub_resource,
    })
}

/// Classify a `ResourceArn` this endpoint resolves no taggable resource for, the
/// way [`minted_id_rejection`] classifies an id: an ARN that names no resource
/// this service tags is a parameter failure, a well-formed one this
/// account/Region does not hold an ownership failure. The class comes from the
/// REQUEST, and neither message quotes the ARN back.
pub fn resource_arn_rejection(arn: &str) -> String {
    if parse_tagged_resource_arn(arn).is_some() {
        not_owned_message(RESOURCE_ARN_MEMBER)
    } else {
        invalid_parameter_message(RESOURCE_ARN_MEMBER)
    }
}

/// The most tags one resource carries — the PUBLISHED quota, not a figure this
/// emulator chose: "Tagging is currently supported for detectors, finding filters,
/// IP sets, threat intel sets, and publishing destination, with a limit of 50 tags
/// per resource" (`smithy_query(query="doc", shape="ListTagsForResource")`).
///
/// It is a different quantity from the `len 1..200` on `TagMap`
/// (`smithy_query(shape="TagMap")`), which bounds how many pairs ONE REQUEST's map
/// may carry: a request inside that bound can still ask for more tags than a
/// resource may hold, and is refused here rather than at the modelled boundary.
/// The limit is stated for the resource and not for a path to it, so it holds
/// however the tags arrive — through `TagResource` or through the `Tags` member of
/// a create. The sentence predates the newer classes that carry a `TagMap` of
/// their own (the entity sets, the Malware Protection plan, the custom
/// detection-rule association), and the figure is applied to those too: one
/// ceiling for every resource of this service, which is also what the single
/// rejection message can say.
///
/// It also BOUNDS the store: a tag key is REQUEST-chosen, so without a ceiling a
/// caller adding fresh keys to one resource could grow its map without limit — the
/// hazard [`MAX_CLIENT_TOKENS`] guards for request-chosen token keys.
pub const TAGS_PER_RESOURCE: usize = 50;

/// The message a write that would leave a resource carrying more than
/// [`TAGS_PER_RESOURCE`] tags is refused with. Like every other rejection here it
/// names the limit and the resource it bounds, never the caller's keys — see
/// [`publishing_destination_limit_message`]. The resource is named as "the
/// resource" because the limit reaches every class of them and is the same for
/// each.
pub fn tag_limit_message() -> String {
    format!(
        "The request is rejected because the resource has reached the limit of {TAGS_PER_RESOURCE} tags."
    )
}

/// Refuse a write that would leave ONE resource carrying more than
/// [`TAGS_PER_RESOURCE`] tags, with [`tag_limit_message`]; `None` when it fits.
///
/// `carried` is the map the resource holds now and `requested` the keys the
/// request carries: a key the resource already carries is OVERWRITTEN rather than
/// appended, so it grows the resource by nothing. Decided before anything is
/// written, so a refused request persists none of the request's tags — not even
/// the ones that would have fitted.
pub fn tag_count_rejection<'keys>(
    carried: &BTreeMap<String, String>,
    requested: impl IntoIterator<Item = &'keys str>,
) -> Option<String> {
    let added = requested
        .into_iter()
        .filter(|key| !carried.contains_key(*key))
        .count();
    (carried.len() + added > TAGS_PER_RESOURCE).then(tag_limit_message)
}

/// The same rule for a CREATE: the resource does not exist yet, so the whole
/// `TagMap` the request carries is what it would be created carrying. A create
/// that names no `Tags` member at all carries nothing and is never refused.
pub fn created_tag_count_rejection(tags: Option<&HashMap<String, String>>) -> Option<String> {
    tag_count_rejection(
        &BTreeMap::new(),
        tags.into_iter().flatten().map(|(key, _)| key.as_str()),
    )
}

impl ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// The tag map of the resource `arn` names, as `region`'s endpoint holds it,
    /// for an operation that only REPORTS tags.
    ///
    /// Resolution is [`Self::tagged_resource_tags_mut`]'s — same ARN form, same
    /// scope check, same records — except that this one keeps resolving a
    /// sub-resource whose delete has been accepted, because such a record is
    /// RETAINED to be reported and its own `Get` still reports it, so reporting
    /// its tags is consistent while writing them is not. This one borrows the
    /// store immutably, so reporting tags cannot move anything a later caller
    /// reads.
    pub fn tagged_resource_tags(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&BTreeMap<String, String>> {
        let resource = parse_tagged_resource_arn(arn)?;
        if resource.partition != PARTITION
            || resource.region != region
            || resource.account_id != ::engine::emulator_account_id()
        {
            return None;
        }
        let detector = self.detector_in(region, &resource.detector_id)?;
        let Some((sub_resource, id)) = &resource.sub_resource else {
            return Some(&detector.tags);
        };
        match sub_resource {
            TaggedSubResource::Filter => detector.filters.get(id).map(|f| &f.tags),
            TaggedSubResource::IpSet => detector.ip_sets.get(id).map(|s| &s.tags),
            TaggedSubResource::ThreatIntelSet => {
                detector.threat_intel_sets.get(id).map(|s| &s.tags)
            }
            TaggedSubResource::TrustedEntitySet => {
                detector.trusted_entity_sets.get(id).map(|s| &s.tags)
            }
            TaggedSubResource::ThreatEntitySet => {
                detector.threat_entity_sets.get(id).map(|s| &s.tags)
            }
            TaggedSubResource::PublishingDestination => {
                detector.publishing_destinations.get(id).map(|d| &d.tags)
            }
        }
    }

    /// The tag map of the resource `arn` names, as `region`'s endpoint holds it,
    /// for an operation that WRITES tags.
    ///
    /// `None` unless the ARN names a resource this endpoint serves: the scope it
    /// claims must be this emulator's partition and account
    /// ([`::engine::emulator_arn`] mints every ARN here under them) and the
    /// Region serving the request, because a detector and everything hanging off
    /// it are regional — "you must create a detector in each Region where you
    /// enable the service" (`smithy_query(query="doc",shape="CreateDetector")`) —
    /// and the record must exist.
    ///
    /// Tags are kept on the resource's own record, so the map this hands back is
    /// the one its read reports: nothing can tag a resource in a place its
    /// `Get`/`Describe` does not look.
    ///
    /// A sub-resource whose delete has already been ACCEPTED resolves to `None`
    /// here, so it is refused exactly like an id the detector never held. A
    /// record in that state is retained only to be REPORTED — it is named by no
    /// listing, occupies none of the detector's slots and no update may move it
    /// ([`IpSetRecord::is_deleting`], [`ThreatIntelSetRecord::is_deleting`],
    /// [`EntitySetRecord::is_deleting`]) — so a write against it is a write
    /// against a resource that is out of use. The REPORTING seam
    /// ([`Self::tagged_resource_tags`]) keeps resolving it, because its own
    /// `Get` still reports it.
    pub fn tagged_resource_tags_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        let resource = parse_tagged_resource_arn(arn)?;
        if resource.partition != PARTITION
            || resource.region != region
            || resource.account_id != ::engine::emulator_account_id()
        {
            return None;
        }
        let detector = self.detector_in_mut(region, &resource.detector_id)?;
        let Some((sub_resource, id)) = &resource.sub_resource else {
            return Some(&mut detector.tags);
        };
        match sub_resource {
            // A filter and a publishing destination carry no deleting status at
            // all: their delete DROPS the record, so the id resolving at all is
            // already the whole liveness check.
            TaggedSubResource::Filter => detector.filters.get_mut(id).map(|f| &mut f.tags),
            TaggedSubResource::IpSet => detector
                .ip_sets
                .get_mut(id)
                .filter(|s| !s.is_deleting())
                .map(|s| &mut s.tags),
            TaggedSubResource::ThreatIntelSet => detector
                .threat_intel_sets
                .get_mut(id)
                .filter(|s| !s.is_deleting())
                .map(|s| &mut s.tags),
            TaggedSubResource::TrustedEntitySet => detector
                .trusted_entity_sets
                .get_mut(id)
                .filter(|s| !s.is_deleting())
                .map(|s| &mut s.tags),
            TaggedSubResource::ThreatEntitySet => detector
                .threat_entity_sets
                .get_mut(id)
                .filter(|s| !s.is_deleting())
                .map(|s| &mut s.tags),
            TaggedSubResource::PublishingDestination => detector
                .publishing_destinations
                .get_mut(id)
                .map(|d| &mut d.tags),
        }
    }
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
