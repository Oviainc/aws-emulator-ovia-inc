// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-observabilityadmin-observabilityadmin service.
//!
//! The service's primary resource is the **telemetry rule**. A rule is created in
//! the region the request was signed for — its *home* region — and, when the rule
//! carries a multi-region scope (`Regions` or `AllRegions`), the service fans it out
//! into every other region the scope names. Both the original and each fan-out
//! replica are stored here as one record apiece, keyed by `(region, RuleName)`:
//! `RuleName` is unique per region on AWS, so the region is part of the key and a
//! read only ever resolves a rule that lives in the region the caller signed for.
//!
//! A replica does not exist the instant the create returns: its record carries the
//! epoch-millisecond instant at which the fan-out the create started comes due, and
//! every read treats a record whose instant has not arrived as not yet there. That
//! is also where the `PENDING` -> `ACTIVE` replication status a `GetTelemetryRule`
//! reports comes from, so no operation ever has to sleep and no read has to write.
//!
//! The service's other rule resource is the **organization centralization rule**,
//! held in [`ComAmazonawsObservabilityadminObservabilityadminState::centralization_rules`]
//! and keyed by `(CreatedRegion, RuleName)` for the same reason: the rule is created
//! in the region the request was signed for, and
//! `ListCentralizationRulesForOrganization`'s `AllRegions` flag is what decides
//! whether a list spans the other regions' rules or only that one. A centralization
//! rule is not fanned out — it names its source regions and its destination in its
//! own body — but it does carry the instant its provisioning comes due, which is
//! where the `Provisioning` -> `Healthy` `RuleHealth` a read reports comes from.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    CentralizationRule, CentralizationRuleSummary, RegionStatus, TelemetryRule,
    TelemetryRuleSummary, ACTION_VALUES, DELETION_PROTECTION_VALUES, DESTINATION_TYPE_VALUES,
    ENCRYPTED_LOG_GROUP_STRATEGY_VALUES, ENCRYPTION_CONFLICT_RESOLUTION_STRATEGY_VALUES,
    ENCRYPTION_SCOPE_VALUES, ENCRYPTION_STRATEGY_VALUES, FILTER_BEHAVIOR_VALUES,
    FILTER_REQUIREMENT_VALUES, LOG_TYPE_VALUES, MSK_ENHANCED_MONITORING_LEVEL_VALUES,
    OUTPUT_FORMAT_VALUES, RESOURCE_TYPE_VALUES, TAG_CONFLICT_RESOLUTION_STRATEGY_VALUES,
    TELEMETRY_SOURCE_TYPE_VALUES, TELEMETRY_TYPE_VALUES, W_A_F_LOG_TYPE_VALUES,
};

/// The ARN service namespace for this service (`arnNamespace` / sigv4 name
/// `observabilityadmin`).
pub const ARN_SERVICE: &str = "observabilityadmin";

/// The resource portion a telemetry-rule ARN carries before its name, so an ARN
/// reads `arn:aws:observabilityadmin:<region>:<account>:telemetry-rule/<RuleName>`.
pub const RULE_ARN_RESOURCE_PREFIX: &str = "telemetry-rule/";

/// The resource portion an organization centralization rule's ARN carries before its
/// name, so an ARN reads
/// `arn:aws:observabilityadmin:<region>:<account>:centralization-rule/<RuleName>`.
pub const CENTRALIZATION_RULE_ARN_RESOURCE_PREFIX: &str = "centralization-rule/";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most telemetry rules a caller may create.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no rule quota.
/// Only caller-created (home) rules count against it; [`MAX_STORED_RULE_RECORDS`] is
/// the cap that covers the fan-out replicas too. A create past either cap is refused
/// with the modelled `ServiceQuotaExceededException`.
pub const MAX_TELEMETRY_RULES: usize = 500;

/// The most records — caller-created rules AND fan-out replicas together — this
/// emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. It is the cap that dominates the only
/// growth site a request can drive: a create or update fans the rule out into at most
/// [`MAX_REPLICATION_TARGETS`] spoke regions, so this bounds the PRODUCT of rules and
/// regions, which `MAX_TELEMETRY_RULES` alone does not. A create or update that would
/// take the store past it is refused with the modelled
/// `ServiceQuotaExceededException` before a single replica is allocated.
pub const MAX_STORED_RULE_RECORDS: usize = 8_000;

/// The most spoke regions one rule may be fanned out into.
///
/// Emulator resource guard, NOT an AWS quota: the model gives `Rule.Regions` a
/// minimum length and no maximum, so this is where the maximum lives. Every named
/// region must also be a region of the home region's own partition
/// ([`replication_rejection`]), which already bounds a valid list by the size of that
/// partition; this constant is the independent clamp, so the fan-out is bounded even
/// if a future caller of [`replication_targets`] forgets to validate first.
pub const MAX_REPLICATION_TARGETS: usize = 50;

/// How long after a create or update the fan-out into a spoke region comes due.
///
/// A CHOSEN span, not an AWS-documented duration: AWS reports a freshly replicated
/// rule as `PENDING` in its spoke regions and only later as `ACTIVE`, and this is
/// the span this emulator takes to get there. It is deliberately non-zero, so a
/// caller that reads straight after the create still observes `PENDING`.
pub const REPLICATION_DELAY_MS: i64 = 400;

/// Replication status of a rule in a spoke region the fan-out has not reached yet.
///
/// From `RegionStatus$Status`' own documentation: "For telemetry rules, valid values
/// include `PENDING`, `ACTIVE`, and `FAILED`."
pub const REGION_STATUS_PENDING: &str = "PENDING";

/// Replication status of a rule the fan-out has created in a spoke region.
pub const REGION_STATUS_ACTIVE: &str = "ACTIVE";

/// Replication status of a spoke region the fan-out could not create the rule in.
pub const REGION_STATUS_FAILED: &str = "FAILED";

/// The most organization centralization rules this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no centralization
/// rule quota. A create past it is refused with the modelled
/// `ServiceQuotaExceededException` before a record is allocated. A centralization rule
/// is stored exactly once (it is never fanned out), so this one cap bounds the store.
pub const MAX_CENTRALIZATION_RULES: usize = 500;

/// How long after a create or an update an organization centralization rule's
/// provisioning comes due.
///
/// A CHOSEN span, not an AWS-documented duration: `ContextGraphStatus`' own
/// documentation says a status "Returns `Provisioning` while the context graph is
/// being set up, `Healthy` once it is active", and this is the span this emulator
/// takes to get there. It is deliberately non-zero, so a caller that reads straight
/// after the create still observes `Provisioning`.
pub const CENTRALIZATION_PROVISIONING_DELAY_MS: i64 = 400;

/// The health of a centralization rule whose provisioning has not come due, from
/// `RuleHealth`'s own value set (`Healthy`, `Unhealthy`, `Provisioning`).
pub const RULE_HEALTH_PROVISIONING: &str = "Provisioning";

/// The health of a centralization rule this emulator has finished provisioning.
pub const RULE_HEALTH_HEALTHY: &str = "Healthy";

/// The tag-propagation health of a rule whose destination configures tag propagation:
/// `TagPropagationStatus$Healthy`, which that member documents as "the most recent
/// tag-propagation attempt succeeded".
pub const TAG_PROPAGATION_STATUS_HEALTHY: &str = "Healthy";

/// A stored rule's key: the region it lives in, then its name. `RuleName` is unique
/// per region, and ordering by region then name is what gives `ListTelemetryRules` a
/// stable, name-ordered page sequence within one region.
pub type RuleKey = (String, String);

/// One stored telemetry rule: either the rule a caller created in its home region,
/// or one of the fan-out replicas that rule's multi-region scope asked for.
#[derive(Debug, Clone, Default)]
pub struct StoredTelemetryRule {
    pub rule_name: String,
    /// The region this record lives in — the home region for a caller-created rule,
    /// the spoke region for a replica.
    pub region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:telemetry-rule/<RuleName>`.
    pub rule_arn: String,
    /// The rule body exactly as the caller supplied it.
    ///
    /// Shared with this rule's fan-out replicas rather than copied into each of
    /// them: the body is caller-sized, and one request must not cost its size times
    /// the number of regions it names.
    pub rule: ::std::sync::Arc<TelemetryRule>,
    /// `CreatedTimeStamp`, in epoch milliseconds. For a replica this is the instant
    /// the fan-out comes due, which is also when the record starts existing.
    pub created_timestamp_ms: i64,
    /// `LastUpdateTimeStamp`, in epoch milliseconds.
    pub last_update_timestamp_ms: i64,
    pub home_region: String,
    /// Whether this record is a fan-out replica rather than the rule the caller
    /// created. Reported as `IsReplicated`, whose own documentation defines it as
    /// "whether this telemetry rule is a replica that was created in this region
    /// through multi-region fan-out from the home region".
    pub is_replica: bool,
    /// The spoke regions this rule's scope asks for. Empty on a replica and on a
    /// rule with no multi-region scope.
    pub replication_targets: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

impl StoredTelemetryRule {
    /// This record's store key.
    pub fn key(&self) -> RuleKey {
        (self.region.clone(), self.rule_name.clone())
    }

    /// Whether this record exists yet at `now_ms`. Always true for a caller-created
    /// rule; false for a replica whose fan-out has not come due.
    pub fn exists_at(&self, now_ms: i64) -> bool {
        self.created_timestamp_ms <= now_ms
    }

    /// This record as a `TelemetryRuleSummary`, the shape `ListTelemetryRules`
    /// reports one rule as.
    pub fn summary(&self) -> TelemetryRuleSummary {
        TelemetryRuleSummary {
            rule_name: Some(self.rule_name.clone()),
            rule_arn: Some(self.rule_arn.clone()),
            created_time_stamp: Some(self.created_timestamp_ms),
            last_update_time_stamp: Some(self.last_update_timestamp_ms),
            resource_type: self.rule.resource_type.clone(),
            telemetry_type: Some(self.rule.telemetry_type.clone()),
            telemetry_source_types: self.rule.telemetry_source_types.clone(),
        }
    }
}

/// One stored organization centralization rule.
///
/// The rule body is held behind an `Arc` for the same reason the telemetry rule's is:
/// it is caller-sized, and an update must not cost a second copy of it while a reader
/// still holds the old one.
#[derive(Debug, Clone, Default)]
pub struct StoredCentralizationRule {
    pub rule_name: String,
    /// `CreatedRegion`: "the Amazon Web Services region where the organization
    /// centralization rule was created", which is the region the create was signed
    /// for and the region field of this rule's ARN.
    pub created_region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:centralization-rule/<RuleName>`.
    pub rule_arn: String,
    /// The rule body exactly as the caller supplied it.
    pub rule: ::std::sync::Arc<CentralizationRule>,
    /// `CreatorAccountId`: "the Amazon Web Services Account that created the
    /// organization centralization rule".
    pub creator_account_id: String,
    /// `CreatedTimeStamp`, in epoch milliseconds.
    pub created_timestamp_ms: i64,
    /// `LastUpdateTimeStamp`, in epoch milliseconds.
    pub last_update_timestamp_ms: i64,
    /// The epoch-millisecond instant at which provisioning the CURRENT rule body
    /// comes due. Until it arrives every read reports `Provisioning`; after it, the
    /// rule is `Healthy`. A create and an update each set it, so no operation sleeps
    /// and no read has to write.
    pub provisioned_at_ms: i64,
    pub tags: BTreeMap<String, String>,
}

impl StoredCentralizationRule {
    /// This record's store key.
    pub fn key(&self) -> RuleKey {
        (self.created_region.clone(), self.rule_name.clone())
    }

    /// `RuleHealth` at `now_ms`: `Provisioning` until this rule's provisioning comes
    /// due, `Healthy` from then on.
    pub fn rule_health(&self, now_ms: i64) -> String {
        if now_ms < self.provisioned_at_ms {
            RULE_HEALTH_PROVISIONING.to_string()
        } else {
            RULE_HEALTH_HEALTHY.to_string()
        }
    }

    /// `ContextGraphStatus` at `now_ms`, or `None` when the member does not apply:
    /// it is "the status of context graph centralization for this rule", so it is
    /// reported only for a rule whose source asks for context graph centralization.
    /// It follows the same `Provisioning` -> `Healthy` span as [`Self::rule_health`],
    /// which is what that member documents: `Provisioning` "while the context graph
    /// is being set up", `Healthy` "once it is active".
    pub fn context_graph_status(&self, now_ms: i64) -> Option<String> {
        self.rule
            .source
            .source_context_graph_configuration
            .as_ref()
            .map(|_| self.rule_health(now_ms))
    }

    /// `TagPropagationStatus`, or `None` when the member does not apply: it is "the
    /// health status of tag propagation for this rule", so it is reported only for a
    /// rule whose destination configures tag propagation. Nothing in this emulator
    /// can fail such an attempt, so the status is the `Healthy` that member defines
    /// as "the most recent tag-propagation attempt succeeded".
    pub fn tag_propagation_status(&self) -> Option<String> {
        self.rule
            .destination
            .destination_logs_configuration
            .as_ref()
            .and_then(|logs| logs.tag_propagation_configuration.as_ref())
            .map(|_| TAG_PROPAGATION_STATUS_HEALTHY.to_string())
    }

    /// The destination account this rule centralizes into: the account its
    /// destination names, or — when it names none — the account that created it,
    /// since the data then stays in the creator's own account.
    pub fn destination_account_id(&self) -> String {
        self.rule
            .destination
            .account
            .clone()
            .unwrap_or_else(|| self.creator_account_id.clone())
    }

    /// This record as a `CentralizationRuleSummary`, the shape
    /// `ListCentralizationRulesForOrganization` reports one rule as.
    ///
    /// `FailureReason` and `TagPropagationFailureReason` are left unset: each is the
    /// reason an attempt failed, and nothing in this emulator fails one.
    pub fn summary(&self, now_ms: i64) -> CentralizationRuleSummary {
        CentralizationRuleSummary {
            rule_name: Some(self.rule_name.clone()),
            rule_arn: Some(self.rule_arn.clone()),
            creator_account_id: Some(self.creator_account_id.clone()),
            created_time_stamp: Some(self.created_timestamp_ms),
            created_region: Some(self.created_region.clone()),
            last_update_time_stamp: Some(self.last_update_timestamp_ms),
            rule_health: Some(self.rule_health(now_ms)),
            failure_reason: None,
            tag_propagation_status: self.tag_propagation_status(),
            tag_propagation_failure_reason: None,
            context_graph_status: self.context_graph_status(now_ms),
            destination_account_id: Some(self.destination_account_id()),
            destination_region: Some(self.rule.destination.region.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-observabilityadmin-observabilityadmin. A
/// handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsObservabilityadminObservabilityadminState {
    /// Every stored rule, keyed by `(region, RuleName)`.
    pub telemetry_rules: BTreeMap<RuleKey, StoredTelemetryRule>,
    /// Rule ARN -> store key, so a `RuleIdentifier` resolves as an ARN as well as
    /// as a bare name.
    pub arn_index: BTreeMap<String, RuleKey>,
    /// Every organization centralization rule, keyed by `(CreatedRegion, RuleName)`.
    /// Ordering by region then name is what gives
    /// `ListCentralizationRulesForOrganization` a stable, name-ordered page sequence
    /// within one region and a stable region-then-name one across all of them.
    pub centralization_rules: BTreeMap<RuleKey, StoredCentralizationRule>,
    /// Centralization rule ARN -> store key, so a `RuleIdentifier` resolves as an
    /// ARN as well as as a bare name.
    pub centralization_arn_index: BTreeMap<String, RuleKey>,
}

impl ComAmazonawsObservabilityadminObservabilityadminState {
    /// The key of the rule `identifier` names for a caller signed for `region`,
    /// accepting both forms `RuleIdentifier`'s documentation allows: the bare rule
    /// name, or the full rule ARN.
    ///
    /// An ARN must match a stored ARN in EVERY field and name a rule in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the rule that shares its name.
    pub fn resolve_key(&self, region: &str, identifier: &str) -> Option<RuleKey> {
        if identifier.starts_with("arn:") {
            return self
                .arn_index
                .get(identifier)
                .filter(|(rule_region, _)| rule_region == region)
                .cloned();
        }
        let key = (region.to_string(), identifier.to_string());
        self.telemetry_rules.contains_key(&key).then_some(key)
    }

    /// The rule `identifier` names in `region`, as it exists at `now_ms`.
    pub fn rule(
        &self,
        region: &str,
        identifier: &str,
        now_ms: i64,
    ) -> Option<&StoredTelemetryRule> {
        let key = self.resolve_key(region, identifier)?;
        self.telemetry_rules
            .get(&key)
            .filter(|record| record.exists_at(now_ms))
    }

    /// Whether `name` is already in use in `region` — by a stored rule or by a
    /// replica whose fan-out has not come due, since both occupy the name.
    pub fn name_taken(&self, region: &str, name: &str) -> bool {
        self.telemetry_rules
            .contains_key(&(region.to_string(), name.to_string()))
    }

    /// How many rules callers have created, which is what [`MAX_TELEMETRY_RULES`]
    /// bounds; fan-out replicas are not counted.
    pub fn home_rule_count(&self) -> usize {
        self.telemetry_rules
            .values()
            .filter(|record| !record.is_replica)
            .count()
    }

    /// How many records the store holds in total — caller-created rules and fan-out
    /// replicas alike — which is what [`MAX_STORED_RULE_RECORDS`] bounds.
    pub fn record_count(&self) -> usize {
        self.telemetry_rules.len()
    }

    /// Whether storing `records` more would take the store past
    /// [`MAX_STORED_RULE_RECORDS`]. A write path asks this BEFORE it allocates, so
    /// the refusal costs nothing.
    pub fn would_exceed_store(&self, records: usize) -> bool {
        self.record_count().saturating_add(records) > MAX_STORED_RULE_RECORDS
    }

    /// Store a record and index it by ARN.
    ///
    /// The key is `(region, RuleName)`, so a caller must have established that the
    /// name is free in that region first (`name_taken`) — every write path here does
    /// — because inserting over an occupied key would replace that record.
    pub fn insert(&mut self, record: StoredTelemetryRule) {
        self.arn_index.insert(record.rule_arn.clone(), record.key());
        self.telemetry_rules.insert(record.key(), record);
    }

    /// Remove one record, with its ARN index entry.
    fn remove(&mut self, key: &RuleKey) -> Option<StoredTelemetryRule> {
        let record = self.telemetry_rules.remove(key)?;
        self.arn_index.remove(&record.rule_arn);
        Some(record)
    }

    /// The replica of the rule `home` owns in `region`, if the fan-out put one
    /// there. A record in that region that belongs to somebody else — a rule
    /// created directly in it, or a replica of another home region's rule of the
    /// same name — is NOT this rule's replica.
    fn replica_of<'a>(
        &'a self,
        home: &StoredTelemetryRule,
        region: &str,
    ) -> Option<&'a StoredTelemetryRule> {
        self.telemetry_rules
            .get(&(region.to_string(), home.rule_name.clone()))
            .filter(|candidate| candidate.is_replica && candidate.home_region == home.home_region)
    }

    /// The per-spoke-region replication status of `record`, or `None` when the
    /// member does not apply: `RegionStatuses`' own documentation says it "is only
    /// present for rules created with multi-region scope", and a replica was not —
    /// it IS the fan-out of one.
    pub fn region_statuses(
        &self,
        record: &StoredTelemetryRule,
        now_ms: i64,
    ) -> Option<Vec<RegionStatus>> {
        if record.is_replica || !has_multi_region_scope(&record.rule) {
            return None;
        }
        Some(
            record
                .replication_targets
                .iter()
                .map(|region| match self.replica_of(record, region) {
                    // The fan-out reached this region, and `RuleArn` is reported
                    // because that member "is populated when the rule has been
                    // successfully created in the spoke region (status is ACTIVE)".
                    Some(replica) if replica.exists_at(now_ms) => RegionStatus {
                        region: Some(region.clone()),
                        status: Some(REGION_STATUS_ACTIVE.to_string()),
                        failure_reason: None,
                        rule_arn: Some(replica.rule_arn.clone()),
                    },
                    Some(_) => RegionStatus {
                        region: Some(region.clone()),
                        status: Some(REGION_STATUS_PENDING.to_string()),
                        failure_reason: None,
                        rule_arn: None,
                    },
                    // Nothing of ours is there and nothing ever will be: the name
                    // was already in use in that region when the fan-out ran.
                    None => RegionStatus {
                        region: Some(region.clone()),
                        status: Some(REGION_STATUS_FAILED.to_string()),
                        failure_reason: Some(format!(
                            "a telemetry rule named '{}' already exists in {region}",
                            record.rule_name
                        )),
                        rule_arn: None,
                    },
                })
                .collect(),
        )
    }

    /// Fan `home`'s multi-region scope out: create a replica in every spoke region
    /// the scope names and whose name is free, leaving the ones already taken to be
    /// reported as `FAILED`. Each new replica starts existing `REPLICATION_DELAY_MS`
    /// after `now_ms`, so until then the spoke region reports `PENDING`.
    ///
    /// A replica is a small record: it SHARES its parent's rule body (an `Arc`,
    /// never a copy) and carries no tags of its own, since the tags belong to the
    /// resource the home rule's ARN names. One request therefore cannot cost its
    /// body size times the number of regions it named.
    fn replicate(&mut self, home: &StoredTelemetryRule, now_ms: i64) {
        let due = now_ms.saturating_add(REPLICATION_DELAY_MS);
        for region in home
            .replication_targets
            .iter()
            .take(MAX_REPLICATION_TARGETS)
        {
            if self.name_taken(region, &home.rule_name) {
                continue;
            }
            self.insert(StoredTelemetryRule {
                rule_name: home.rule_name.clone(),
                region: region.clone(),
                rule_arn: rule_arn(region, &home.rule_name),
                rule: ::std::sync::Arc::clone(&home.rule),
                created_timestamp_ms: due,
                last_update_timestamp_ms: due,
                home_region: home.home_region.clone(),
                is_replica: true,
                replication_targets: Vec::new(),
                tags: BTreeMap::new(),
            });
        }
    }

    /// Store a newly created rule and start the fan-out its scope asks for.
    pub fn create_rule(&mut self, record: StoredTelemetryRule, now_ms: i64) {
        self.replicate(&record, now_ms);
        self.insert(record);
    }

    /// Replace the body of the rule at `key`, bumping `LastUpdateTimeStamp` and
    /// leaving `CreatedTimeStamp` and the tags where they were, then bring its
    /// replicas in line with the new scope: a spoke region the new scope drops loses
    /// its replica, one it adds gains a `PENDING` one, and one it keeps gets the new
    /// body. Answers the rule's ARN.
    pub fn replace_rule(
        &mut self,
        key: &RuleKey,
        rule: TelemetryRule,
        now_ms: i64,
    ) -> Option<String> {
        let home = self.telemetry_rules.get_mut(key)?;
        // Only the rule in its home region owns a fan-out. Every caller refuses a
        // replica before reaching here; this is the invariant stated in one place, so
        // a spoke-region update can never start a fan-out of its own.
        if home.is_replica {
            return None;
        }
        let previous_targets = std::mem::take(&mut home.replication_targets);
        home.rule = ::std::sync::Arc::new(rule);
        home.last_update_timestamp_ms = now_ms;
        home.replication_targets = replication_targets(&home.rule, &home.region);
        let home = home.clone();

        for region in &previous_targets {
            if home.replication_targets.contains(region) {
                continue;
            }
            if self.replica_of(&home, region).is_some() {
                self.remove(&(region.clone(), home.rule_name.clone()));
            }
        }
        let kept: Vec<String> = home
            .replication_targets
            .iter()
            .filter(|region| previous_targets.contains(region))
            .cloned()
            .collect();
        for region in kept {
            let replica_key = (region, home.rule_name.clone());
            let is_ours = self
                .telemetry_rules
                .get(&replica_key)
                .is_some_and(|replica| {
                    replica.is_replica && replica.home_region == home.home_region
                });
            if is_ours {
                if let Some(replica) = self.telemetry_rules.get_mut(&replica_key) {
                    replica.rule = ::std::sync::Arc::clone(&home.rule);
                    // A replica that is still pending starts existing at its own due
                    // instant, so its last update can never predate that.
                    replica.last_update_timestamp_ms = now_ms.max(replica.created_timestamp_ms);
                }
            }
        }
        let added = StoredTelemetryRule {
            replication_targets: home
                .replication_targets
                .iter()
                .filter(|region| !previous_targets.contains(region))
                .cloned()
                .collect(),
            ..home.clone()
        };
        self.replicate(&added, now_ms);
        Some(home.rule_arn)
    }

    /// Remove the rule at `key` together with every replica it owns. Answers the
    /// removed rule, or `None` when nothing was stored there.
    pub fn remove_rule_tree(&mut self, key: &RuleKey) -> Option<StoredTelemetryRule> {
        let home = self.telemetry_rules.get(key)?.clone();
        let replicas: Vec<RuleKey> = home
            .replication_targets
            .iter()
            .filter(|region| self.replica_of(&home, region).is_some())
            .map(|region| (region.clone(), home.rule_name.clone()))
            .collect();
        for replica in &replicas {
            self.remove(replica);
        }
        self.remove(key)
    }

    /// The key of the centralization rule `identifier` names for a caller signed for
    /// `region`, accepting both forms `RuleIdentifier` allows: the bare rule name, or
    /// the full rule ARN.
    ///
    /// An ARN must match a stored ARN in EVERY field and name a rule created in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the rule that shares its name.
    pub fn resolve_centralization_key(&self, region: &str, identifier: &str) -> Option<RuleKey> {
        if identifier.starts_with("arn:") {
            return self
                .centralization_arn_index
                .get(identifier)
                .filter(|(rule_region, _)| rule_region == region)
                .cloned();
        }
        let key = (region.to_string(), identifier.to_string());
        self.centralization_rules.contains_key(&key).then_some(key)
    }

    /// The centralization rule `identifier` names in `region`.
    pub fn centralization_rule(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&StoredCentralizationRule> {
        let key = self.resolve_centralization_key(region, identifier)?;
        self.centralization_rules.get(&key)
    }

    /// Whether a centralization rule named `name` already exists in `region`.
    pub fn centralization_name_taken(&self, region: &str, name: &str) -> bool {
        self.centralization_rules
            .contains_key(&(region.to_string(), name.to_string()))
    }

    /// How many centralization rules the store holds, which is what
    /// [`MAX_CENTRALIZATION_RULES`] bounds.
    pub fn centralization_rule_count(&self) -> usize {
        self.centralization_rules.len()
    }

    /// Store a newly created centralization rule and index it by ARN.
    ///
    /// The key is `(CreatedRegion, RuleName)`, so a caller must have established that
    /// the name is free in that region first ([`Self::centralization_name_taken`]) —
    /// every write path here does — because inserting over an occupied key would
    /// replace that record.
    pub fn insert_centralization_rule(&mut self, record: StoredCentralizationRule) {
        self.centralization_arn_index
            .insert(record.rule_arn.clone(), record.key());
        self.centralization_rules.insert(record.key(), record);
    }

    /// Replace the body of the centralization rule at `key`, bumping
    /// `LastUpdateTimeStamp` and restarting its provisioning, and leaving
    /// `CreatedTimeStamp`, `CreatorAccountId`, `CreatedRegion` and the tags where they
    /// were. Answers the rule's ARN, or `None` when nothing is stored at `key`.
    pub fn replace_centralization_rule(
        &mut self,
        key: &RuleKey,
        rule: CentralizationRule,
        now_ms: i64,
    ) -> Option<String> {
        let record = self.centralization_rules.get_mut(key)?;
        record.rule = ::std::sync::Arc::new(rule);
        record.last_update_timestamp_ms = now_ms;
        record.provisioned_at_ms = now_ms.saturating_add(CENTRALIZATION_PROVISIONING_DELAY_MS);
        Some(record.rule_arn.clone())
    }

    /// Remove the centralization rule at `key`, with its ARN index entry. Answers the
    /// removed rule, or `None` when nothing was stored there.
    pub fn remove_centralization_rule(
        &mut self,
        key: &RuleKey,
    ) -> Option<StoredCentralizationRule> {
        let record = self.centralization_rules.remove(key)?;
        self.centralization_arn_index.remove(&record.rule_arn);
        Some(record)
    }
}

/// Whether `rule` asks to be replicated beyond its home region.
///
/// `Regions` and `AllRegions` are the two forms of multi-region scope the model
/// carries; a rule with neither lives only in its home region.
pub fn has_multi_region_scope(rule: &TelemetryRule) -> bool {
    rule.all_regions == Some(true)
        || rule
            .regions
            .as_ref()
            .is_some_and(|regions| !regions.is_empty())
}

/// The spoke regions `rule` asks to be replicated into from `home_region`.
///
/// `Regions` names them outright; `AllRegions` means "all Amazon Web Services
/// Regions where Amazon CloudWatch Observability Admin is available in the current
/// partition", which this emulator reads as every region of the home region's
/// partition. The home region itself is never a spoke of its own rule, and the
/// result is de-duplicated and ordered so two calls fan out identically.
///
/// The set is CLAMPED to [`MAX_REPLICATION_TARGETS`] and to the regions of the home
/// region's partition, so the number of replicas a request can ask for is decided
/// here, by the server, and not by the length of the list it sent:
/// [`replication_rejection`] refuses such a list outright, and this clamp holds even
/// if a caller of this function has not run that check.
pub fn replication_targets(rule: &TelemetryRule, home_region: &str) -> Vec<String> {
    let partition = partition_regions(home_region);
    let listed: BTreeSet<String> = if rule.all_regions == Some(true) {
        partition
    } else {
        rule.regions
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|region| partition.contains(*region))
            .cloned()
            .collect()
    };
    listed
        .into_iter()
        .filter(|region| region != home_region)
        .take(MAX_REPLICATION_TARGETS)
        .collect()
}

/// Every AWS region of `region`'s own partition.
///
/// The catalog is `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the
/// wire may carry"), partitioned by [`partition_of`].
fn partition_regions(region: &str) -> BTreeSet<String> {
    let partition = partition_of(region);
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .iter()
        .filter(|candidate| partition_of(candidate) == partition)
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// The AWS partition a region belongs to, by its region-code prefix.
///
/// The prefixes and partition names are AWS's own (`aws`, `aws-cn`, `aws-us-gov` and
/// the isolated partitions); the longer `us-isob-` / `us-isof-` prefixes are matched
/// before `us-iso-` so each lands in its own partition.
fn partition_of(region: &str) -> &'static str {
    const PARTITION_PREFIXES: &[(&str, &str)] = &[
        ("cn-", "aws-cn"),
        ("us-gov-", "aws-us-gov"),
        ("us-isob-", "aws-iso-b"),
        ("us-isof-", "aws-iso-f"),
        ("us-iso-", "aws-iso"),
        ("eu-isoe-", "aws-iso-e"),
    ];
    PARTITION_PREFIXES
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
        .map_or("aws", |(_, partition)| *partition)
}

/// Why `rule` is not a rule this service will store, or `None` when it is fine.
///
/// Two rules the model states but cannot express as a constraint, so the centralized
/// walk lets both through and this is where they live. Every operation that stores a
/// `TelemetryRule` applies this one function, each rendering the refusal as its own
/// modelled `ValidationException`:
///
/// 1. every member targeting a modelled enum carries one of that enum's values —
///    the generated `*_VALUES` catalogs are exactly what
///    `smithy_query(query="shape",shape="<Enum>")` reports;
/// 2. `Regions` and `AllRegions` are, in each member's own words, "mutually
///    exclusive", and a named region must be a region of the home region's own
///    partition, within [`MAX_REPLICATION_TARGETS`] of them — that text decides how
///    many records one request stores and lands in the region field of each
///    replica's ARN.
pub fn telemetry_rule_rejection(rule: &TelemetryRule, home_region: &str) -> Option<String> {
    enum_rejection(
        "Rule.TelemetryType",
        Some(rule.telemetry_type.as_str()),
        TELEMETRY_TYPE_VALUES,
    )
    .or_else(|| {
        enum_rejection(
            "Rule.ResourceType",
            rule.resource_type.as_deref(),
            RESOURCE_TYPE_VALUES,
        )
    })
    .or_else(|| {
        list_enum_rejection(
            "Rule.TelemetrySourceTypes",
            rule.telemetry_source_types.as_deref(),
            TELEMETRY_SOURCE_TYPE_VALUES,
        )
    })
    .or_else(|| destination_rejection(rule))
    .or_else(|| replication_rejection(rule, home_region))
}

/// Why a member that targets a modelled enum is unacceptable. A member the request
/// omits is one the model admits, so it is not refused here.
fn enum_rejection(member: &str, value: Option<&str>, catalog: &[&str]) -> Option<String> {
    let supplied = value?;
    if catalog.contains(&supplied) {
        return None;
    }
    Some(format!(
        "{member} must be one of [{}], got '{supplied}'",
        catalog.join(", ")
    ))
}

/// [`enum_rejection`] for every entry of a list whose member targets an enum.
fn list_enum_rejection(member: &str, values: Option<&[String]>, catalog: &[&str]) -> Option<String> {
    values?.iter().enumerate().find_map(|(index, value)| {
        enum_rejection(&format!("{member}[{index}]"), Some(value.as_str()), catalog)
    })
}

/// The enum members carried under `Rule.DestinationConfiguration`, down to the WAF
/// logging filter's own conditions.
fn destination_rejection(rule: &TelemetryRule) -> Option<String> {
    let destination = rule.destination_configuration.as_ref()?;
    enum_rejection(
        "Rule.DestinationConfiguration.DestinationType",
        destination.destination_type.as_deref(),
        DESTINATION_TYPE_VALUES,
    )
    .or_else(|| {
        enum_rejection(
            "Rule.DestinationConfiguration.ELBLoadBalancerLoggingParameters.OutputFormat",
            destination
                .elb_load_balancer_logging_parameters
                .as_ref()
                .and_then(|parameters| parameters.output_format.as_deref()),
            OUTPUT_FORMAT_VALUES,
        )
    })
    .or_else(|| {
        list_enum_rejection(
            "Rule.DestinationConfiguration.LogDeliveryParameters.LogTypes",
            destination
                .log_delivery_parameters
                .as_ref()
                .and_then(|parameters| parameters.log_types.as_deref()),
            LOG_TYPE_VALUES,
        )
    })
    .or_else(|| {
        enum_rejection(
            "Rule.DestinationConfiguration.MskMonitoringParameters.EnhancedMonitoring",
            destination
                .msk_monitoring_parameters
                .as_ref()
                .and_then(|parameters| parameters.enhanced_monitoring.as_deref()),
            MSK_ENHANCED_MONITORING_LEVEL_VALUES,
        )
    })
    .or_else(|| {
        let waf = destination.waf_logging_parameters.as_ref()?;
        enum_rejection(
            "Rule.DestinationConfiguration.WAFLoggingParameters.LogType",
            waf.log_type.as_deref(),
            W_A_F_LOG_TYPE_VALUES,
        )
        .or_else(|| {
            let filter = waf.logging_filter.as_ref()?;
            enum_rejection(
                "Rule.DestinationConfiguration.WAFLoggingParameters.LoggingFilter.DefaultBehavior",
                filter.default_behavior.as_deref(),
                FILTER_BEHAVIOR_VALUES,
            )
            .or_else(|| {
                filter
                    .filters
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .find_map(|(index, entry)| {
                        let at = format!(
                            "Rule.DestinationConfiguration.WAFLoggingParameters.LoggingFilter.Filters[{index}]"
                        );
                        enum_rejection(
                            &format!("{at}.Behavior"),
                            entry.behavior.as_deref(),
                            FILTER_BEHAVIOR_VALUES,
                        )
                        .or_else(|| {
                            enum_rejection(
                                &format!("{at}.Requirement"),
                                entry.requirement.as_deref(),
                                FILTER_REQUIREMENT_VALUES,
                            )
                        })
                        .or_else(|| {
                            entry
                                .conditions
                                .as_deref()
                                .unwrap_or_default()
                                .iter()
                                .enumerate()
                                .find_map(|(nth, condition)| {
                                    enum_rejection(
                                        &format!("{at}.Conditions[{nth}].ActionCondition.Action"),
                                        condition
                                            .action_condition
                                            .as_ref()
                                            .and_then(|action| action.action.as_deref()),
                                        ACTION_VALUES,
                                    )
                                })
                        })
                    })
            })
        })
    })
}

/// Why `rule`'s multi-region scope is unacceptable, or `None` when it is fine.
///
/// The two forms of the scope are mutually exclusive, a named list may be no longer
/// than [`MAX_REPLICATION_TARGETS`], and each region it names must be a region of
/// the home region's own partition. The partition catalog is
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may
/// carry"), split by [`partition_of`]: a region outside it is refused rather than
/// stored, because the number of regions the list names is the number of records
/// the request stores, and because a replica's ARN carries that region in a field
/// the modelled `ResourceArn` pattern admits only `[a-z0-9-]+` in — and this
/// emulator mints ARNs in the `aws` partition only.
fn replication_rejection(rule: &TelemetryRule, home_region: &str) -> Option<String> {
    let named = rule.regions.as_deref().unwrap_or_default();
    if rule.all_regions == Some(true) && !named.is_empty() {
        return Some(
            "Rule.Regions and Rule.AllRegions are mutually exclusive; supply one of them"
                .to_string(),
        );
    }
    if named.len() > MAX_REPLICATION_TARGETS {
        return Some(format!(
            "Rule.Regions names {} regions, more than the {MAX_REPLICATION_TARGETS} this \
             service replicates a rule into",
            named.len()
        ));
    }
    let partition = partition_regions(home_region);
    named
        .iter()
        .enumerate()
        .find(|(_, region)| !partition.contains(*region))
        .map(|(index, region)| {
            format!(
                "Rule.Regions[{index}] must be an Amazon Web Services Region of the \
                 {} partition, got '{region}'",
                partition_of(home_region)
            )
        })
}

/// The ARN of the telemetry rule `name` in `region`.
pub fn rule_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RULE_ARN_RESOURCE_PREFIX}{name}"),
    )
}

/// The ARN of the organization centralization rule `name` created in `region`.
pub fn centralization_rule_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CENTRALIZATION_RULE_ARN_RESOURCE_PREFIX}{name}"),
    )
}

/// Why `rule` is not an organization centralization rule this service will store, or
/// `None` when it is fine.
///
/// The rules the model states but cannot express as a constraint, so the centralized
/// walk lets them through and this is where they live. Every operation that stores a
/// `CentralizationRule` applies this one function, each rendering the refusal as its
/// own modelled `ValidationException`:
///
/// 1. every member targeting a modelled enum carries one of that enum's values — the
///    generated `*_VALUES` catalogs are exactly what
///    `smithy_query(query="shape",shape="<Enum>")` reports;
/// 2. every member targeting `Region` names an Amazon Web Services Region of the
///    caller's own partition — the source regions the rule centralizes FROM, the
///    destination it centralizes INTO and each backup region — because a region is
///    the field a later read reports as `DestinationRegion` and the field an ARN
///    carries, and this emulator mints ARNs in the `aws` partition only.
pub fn centralization_rule_rejection(rule: &CentralizationRule, home_region: &str) -> Option<String> {
    let partition = partition_regions(home_region);
    rule.source
        .regions
        .iter()
        .enumerate()
        .find_map(|(index, region)| {
            region_rejection(
                &format!("Rule.Source.Regions[{index}]"),
                Some(region.as_str()),
                &partition,
                home_region,
            )
        })
        .or_else(|| {
            region_rejection(
                "Rule.Destination.Region",
                Some(rule.destination.region.as_str()),
                &partition,
                home_region,
            )
        })
        .or_else(|| source_logs_rejection(rule))
        .or_else(|| destination_logs_rejection(rule, &partition, home_region))
        .or_else(|| destination_metrics_rejection(rule, &partition, home_region))
}

/// Why a member that targets `Region` is unacceptable: it has to name a region of the
/// caller's own partition. A member the request omits is one the model admits, so it
/// is not refused here.
fn region_rejection(
    member: &str,
    value: Option<&str>,
    partition: &BTreeSet<String>,
    home_region: &str,
) -> Option<String> {
    let supplied = value?;
    if partition.contains(supplied) {
        return None;
    }
    Some(format!(
        "{member} must be an Amazon Web Services Region of the {} partition, got '{supplied}'",
        partition_of(home_region)
    ))
}

/// The enum members carried under `Rule.Source`.
fn source_logs_rejection(rule: &CentralizationRule) -> Option<String> {
    let logs = rule.source.source_logs_configuration.as_ref()?;
    enum_rejection(
        "Rule.Source.SourceLogsConfiguration.EncryptedLogGroupStrategy",
        Some(logs.encrypted_log_group_strategy.as_str()),
        ENCRYPTED_LOG_GROUP_STRATEGY_VALUES,
    )
}

/// The enum and `Region` members carried under
/// `Rule.Destination.DestinationLogsConfiguration`.
fn destination_logs_rejection(
    rule: &CentralizationRule,
    partition: &BTreeSet<String>,
    home_region: &str,
) -> Option<String> {
    let logs = rule
        .destination
        .destination_logs_configuration
        .as_ref()?;
    enum_rejection(
        "Rule.Destination.DestinationLogsConfiguration.LogGroupDeletionProtection",
        logs.log_group_deletion_protection.as_deref(),
        DELETION_PROTECTION_VALUES,
    )
    .or_else(|| {
        let encryption = logs.logs_encryption_configuration.as_ref()?;
        enum_rejection(
            "Rule.Destination.DestinationLogsConfiguration.LogsEncryptionConfiguration.\
             EncryptionStrategy",
            Some(encryption.encryption_strategy.as_str()),
            ENCRYPTION_STRATEGY_VALUES,
        )
        .or_else(|| {
            enum_rejection(
                "Rule.Destination.DestinationLogsConfiguration.LogsEncryptionConfiguration.\
                 EncryptionConflictResolutionStrategy",
                encryption.encryption_conflict_resolution_strategy.as_deref(),
                ENCRYPTION_CONFLICT_RESOLUTION_STRATEGY_VALUES,
            )
        })
        .or_else(|| {
            enum_rejection(
                "Rule.Destination.DestinationLogsConfiguration.LogsEncryptionConfiguration.\
                 EncryptionScope",
                encryption.encryption_scope.as_deref(),
                ENCRYPTION_SCOPE_VALUES,
            )
        })
    })
    .or_else(|| {
        enum_rejection(
            "Rule.Destination.DestinationLogsConfiguration.TagPropagationConfiguration.\
             TagConflictResolutionStrategy",
            logs.tag_propagation_configuration
                .as_ref()
                .and_then(|tags| tags.tag_conflict_resolution_strategy.as_deref()),
            TAG_CONFLICT_RESOLUTION_STRATEGY_VALUES,
        )
    })
    .or_else(|| {
        region_rejection(
            "Rule.Destination.DestinationLogsConfiguration.BackupConfiguration.Region",
            logs.backup_configuration
                .as_ref()
                .map(|backup| backup.region.as_str()),
            partition,
            home_region,
        )
    })
}

/// The `Region` member carried under
/// `Rule.Destination.DestinationMetricsConfiguration`.
fn destination_metrics_rejection(
    rule: &CentralizationRule,
    partition: &BTreeSet<String>,
    home_region: &str,
) -> Option<String> {
    let metrics = rule
        .destination
        .destination_metrics_configuration
        .as_ref()?;
    region_rejection(
        "Rule.Destination.DestinationMetricsConfiguration.BackupConfiguration.Region",
        metrics
            .backup_configuration
            .as_ref()
            .map(|backup| backup.region.as_str()),
        partition,
        home_region,
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only `[a-z0-9-]+` in that field, so anything else is
/// refused here and the default is used instead: a generated ARN can never carry
/// text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, in epoch milliseconds — the unit the modelled `CreatedTimeStamp` /
/// `LastUpdateTimeStamp` longs carry.
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsObservabilityadminObservabilityadminState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsObservabilityadminObservabilityadminState>)
}
