// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-shield-awsshield-20160616 service.
//!
//! Shield Advanced's control plane is two resources: the account's *subscription*
//! (there is at most one) and the *protections* it grants, each covering exactly one
//! protected Amazon Web Services resource. Every value a handler answers with is
//! derived from the records kept here, so a write made by one request is what the next
//! request reads.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ApplicationLayerAutomaticResponseConfiguration, Limit, Protection,
    ProtectionGroupArbitraryPatternLimits, ProtectionGroupLimits, ProtectionGroupPatternTypeLimits,
    ProtectionLimits, Subscription, SubscriptionLimits, Tag, PROTECTED_RESOURCE_TYPE_VALUES,
};

/// The `SubscriptionState` an account with a live Shield Advanced subscription is in.
pub const SUBSCRIPTION_STATE_ACTIVE: &str = "ACTIVE";
/// The `SubscriptionState` an account without a live subscription is in.
pub const SUBSCRIPTION_STATE_INACTIVE: &str = "INACTIVE";

/// The `AutoRenew` value a freshly created subscription carries — the model's own
/// wording on `Subscription$AutoRenew`: "When you initally create a subscription,
/// `AutoRenew` is set to `ENABLED`."
pub const AUTO_RENEW_ENABLED: &str = "ENABLED";

/// The `ProactiveEngagementStatus` a freshly created subscription carries: proactive
/// engagement is off until the account asks for it (`EnableProactiveEngagement` /
/// `AssociateProactiveEngagementDetails`).
pub const PROACTIVE_ENGAGEMENT_STATUS_DISABLED: &str = "DISABLED";

/// The length of a Shield Advanced subscription, which is also the commitment the
/// account takes on (`Subscription$TimeCommitmentInSeconds`: "The length, in seconds,
/// of the Shield Advanced subscription for the account").
///
/// DOCUMENTED: AWS Shield Advanced is sold with a one-year subscription commitment, so
/// the commitment is one year of seconds and the subscription's `EndTime` is its
/// `StartTime` plus this span.
pub const SUBSCRIPTION_TIME_COMMITMENT: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The `resourceType` this service reports for a protection in an error body.
pub const PROTECTION_RESOURCE_TYPE: &str = "protection";
/// The `resourceType` this service reports for the account's subscription in an error
/// body.
pub const SUBSCRIPTION_RESOURCE_TYPE: &str = "subscription";

/// The `Type` a `LimitsExceededException` names when the account is out of protections.
pub const PROTECTION_LIMIT_TYPE: &str = "protection";

/// The most protections one account may hold at a time; a create beyond it is refused
/// with the modelled `LimitsExceededException` (HTTP 400).
///
/// ENFORCED ACCOUNT-WIDE, across every protected resource type. It is also what
/// `DescribeSubscription` reports as each type's `Max`, because the modelled
/// `Subscription$Limits` / `ProtectionLimits$ProtectedResourceTypeLimits` express a
/// ceiling per type and no single type can hold more protections than the account does:
/// the per-type figure is this account-wide bound, not a separate per-type allowance.
///
/// CHOSEN: AWS quotas the protections an account may hold but the figure is not in the
/// model. The bound is also what keeps this store finite — a record is released only by
/// `DeleteProtection`.
pub const MAX_PROTECTIONS_PER_ACCOUNT: i64 = 1000;

/// The most protection groups one account may hold at a time, as
/// `SubscriptionLimits$ProtectionGroupLimits` reports it. CHOSEN, like
/// [`MAX_PROTECTIONS_PER_ACCOUNT`].
pub const MAX_PROTECTION_GROUPS: i64 = 1000;

/// The most members an `ARBITRARY`-pattern protection group may name, as
/// `ProtectionGroupPatternTypeLimits` reports it. CHOSEN, like
/// [`MAX_PROTECTIONS_PER_ACCOUNT`].
pub const MAX_ARBITRARY_PATTERN_MEMBERS: i64 = 100;

/// The page size `ListProtections` uses when the caller names no `MaxResults` — the
/// model's own wording on `ListProtectionsRequest$MaxResults`: "The default setting
/// is 20."
pub const DEFAULT_LIST_PROTECTIONS_MAX_RESULTS: i32 = 20;

/// How many ids a mint tries before giving up, so no request can spin here. A candidate
/// is a version-4 UUID, so the first one is free in every realistic run.
const MINT_ATTEMPTS: usize = 8;

/// A fresh `ProtectionId`: a version-4 UUID in canonical hyphenated form (`8-4-4-4-12`
/// lowercase hex with the version and variant nibbles set), which is exactly the 36
/// characters `ProtectionId` (len 36..36, pattern `^[a-zA-Z0-9\-]*$`) allows.
///
/// The random bits come from the standard library's [`RandomState`], whose SipHash keys
/// std seeds from the host's randomness once per thread and advances on every `new()`,
/// so two draws in one process do not agree and two processes do not share a sequence.
/// A `uuid` crate dependency would be the obvious source, but it is the one thing this
/// fix cannot afford: a new dependency edits the workspace `Cargo.lock`, the single file
/// every concurrent change to this repository rewrites, and that overlap is what refused
/// this fix's merge. The value is an opaque name — never a credential or a capability —
/// and [`ComAmazonawsShieldAwsshield20160616State::mint_protection_id`] re-draws if the
/// store somehow already holds it.
fn new_uuid() -> String {
    let mut bytes = [0u8; 16];
    for half in 0..2 {
        let draw = RandomState::new().build_hasher().finish().to_be_bytes();
        bytes[half * 8..(half + 1) * 8].copy_from_slice(&draw);
    }
    // Version 4 ("random") and the RFC 4122 variant, so the value is a well-formed v4
    // rather than 16 bytes that merely look like one.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let digits = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// This service's ARN for a protection. Shield is a global service, so the Region
/// segment of every ARN it issues is empty.
pub fn protection_arn(protection_id: &str) -> String {
    ::engine::emulator_arn("shield", "", &format!("protection/{protection_id}"))
}

/// This service's ARN for the account's subscription. See [`protection_arn`] for the
/// empty Region segment.
pub fn subscription_arn(subscription_id: &str) -> String {
    ::engine::emulator_arn("shield", "", &format!("subscription/{subscription_id}"))
}

/// The `ProtectedResourceType` of the resource an ARN names, or `None` when the ARN is
/// not in one of the layouts `CreateProtectionRequest$ResourceArn` documents.
///
/// The mapping is exactly that member's documented list of formats:
///
/// * `arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/app/<name>/<id>`
///   — `APPLICATION_LOAD_BALANCER`
/// * `arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/<name>`
///   — `CLASSIC_LOAD_BALANCER`
/// * `arn:aws:cloudfront::<account>:distribution/<id>` — `CLOUDFRONT_DISTRIBUTION`
/// * `arn:aws:globalaccelerator::<account>:accelerator/<id>` — `GLOBAL_ACCELERATOR`
/// * `arn:aws:route53:::hostedzone/<id>` — `ROUTE_53_HOSTED_ZONE`
/// * `arn:aws:ec2:<region>:<account>:eip-allocation/<id>` — `ELASTIC_IP_ALLOCATION`
///
/// Every value this answers is one of [`PROTECTED_RESOURCE_TYPE_VALUES`]. An ARN in no
/// documented layout is stored with no resource type rather than refused: the emulator
/// does not hold the protected resource, so it cannot tell a resource type it has never
/// heard of from one AWS added after this model was published.
pub fn protected_resource_type(resource_arn: &str) -> Option<&'static str> {
    // `arn:aws:<service>:<region>:<account>:<resource>` — the resource portion may
    // itself contain `:`, so it is the sixth field and everything after it.
    let mut fields = resource_arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };

    match service {
        "elasticloadbalancing" => {
            let load_balancer = resource.strip_prefix("loadbalancer/")?;
            if let Some(application) = load_balancer.strip_prefix("app/") {
                // `app/<name>/<id>`: an Application Load Balancer.
                (!application.is_empty()).then_some("APPLICATION_LOAD_BALANCER")
            } else if !load_balancer.is_empty() && !load_balancer.contains('/') {
                // `<name>`: a Classic Load Balancer. A `net/` or `gwy/` load balancer
                // is neither, and is not in the documented list.
                Some("CLASSIC_LOAD_BALANCER")
            } else {
                None
            }
        }
        "cloudfront" => named_resource(resource, "distribution/").map(|_| "CLOUDFRONT_DISTRIBUTION"),
        "globalaccelerator" => named_resource(resource, "accelerator/").map(|_| "GLOBAL_ACCELERATOR"),
        "route53" => named_resource(resource, "hostedzone/").map(|_| "ROUTE_53_HOSTED_ZONE"),
        "ec2" => named_resource(resource, "eip-allocation/").map(|_| "ELASTIC_IP_ALLOCATION"),
        _ => None,
    }
}

/// The identifier after `prefix` in an ARN's resource portion, when the portion is that
/// prefix followed by a non-empty identifier.
fn named_resource<'a>(resource: &'a str, prefix: &str) -> Option<&'a str> {
    let id = resource.strip_prefix(prefix)?;
    (!id.is_empty()).then_some(id)
}

/// How many protections of each protected resource type the subscription allows, as
/// `Subscription$Limits` and `ProtectionLimits$ProtectedResourceTypeLimits` report it.
///
/// The types are exactly the modelled `ProtectedResourceType` values, so the list
/// follows the model rather than a hand-kept copy of it.
pub fn protected_resource_type_limits() -> Vec<Limit> {
    PROTECTED_RESOURCE_TYPE_VALUES
        .iter()
        .map(|resource_type| Limit {
            r#type: Some((*resource_type).to_string()),
            max: MAX_PROTECTIONS_PER_ACCOUNT,
        })
        .collect()
}

/// The `SubscriptionLimits` every subscription of this emulator grants.
pub fn subscription_limits() -> SubscriptionLimits {
    SubscriptionLimits {
        protection_limits: ProtectionLimits {
            protected_resource_type_limits: protected_resource_type_limits(),
        },
        protection_group_limits: ProtectionGroupLimits {
            max_protection_groups: MAX_PROTECTION_GROUPS,
            pattern_type_limits: ProtectionGroupPatternTypeLimits {
                arbitrary_pattern_limits: ProtectionGroupArbitraryPatternLimits {
                    max_members: MAX_ARBITRARY_PATTERN_MEMBERS,
                },
            },
        },
    }
}

/// The account's Shield Advanced subscription, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct SubscriptionRecord {
    pub subscription_arn: String,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
    pub time_commitment_in_seconds: i64,
    pub auto_renew: String,
    pub proactive_engagement_status: String,
}

impl SubscriptionRecord {
    /// The subscription an account takes on by calling `CreateSubscription` at `now`.
    pub fn activated_at(now: SystemTime) -> Self {
        Self {
            subscription_arn: subscription_arn(&new_uuid()),
            start_time: now,
            end_time: now + SUBSCRIPTION_TIME_COMMITMENT,
            time_commitment_in_seconds: SUBSCRIPTION_TIME_COMMITMENT.as_secs() as i64,
            auto_renew: AUTO_RENEW_ENABLED.to_string(),
            proactive_engagement_status: PROACTIVE_ENGAGEMENT_STATUS_DISABLED.to_string(),
        }
    }

    /// Whether the subscription still covers the account at `now`: its commitment runs
    /// from `StartTime` until `EndTime`.
    pub fn is_active_at(&self, now: SystemTime) -> bool {
        now < self.end_time
    }

    /// This record as the modelled `Subscription` a read answers with.
    pub fn to_modeled(&self) -> Subscription {
        Subscription {
            start_time: Some(DateTime::from(self.start_time)),
            end_time: Some(DateTime::from(self.end_time)),
            time_commitment_in_seconds: self.time_commitment_in_seconds,
            auto_renew: Some(self.auto_renew.clone()),
            limits: Some(protected_resource_type_limits()),
            proactive_engagement_status: Some(self.proactive_engagement_status.clone()),
            subscription_limits: subscription_limits(),
            subscription_arn: Some(self.subscription_arn.clone()),
        }
    }
}

/// One protection — a single protected resource — as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ProtectionEntry {
    pub id: String,
    pub name: String,
    pub resource_arn: String,
    pub protection_arn: String,
    /// The `ProtectedResourceType` [`protected_resource_type`] derives from
    /// `resource_arn`, or `None` for an ARN in no documented layout.
    pub resource_type: Option<String>,
    pub health_check_ids: Vec<String>,
    pub application_layer_automatic_response:
        Option<ApplicationLayerAutomaticResponseConfiguration>,
    /// The tags `CreateProtection` was given. Stored for the tagging operations
    /// (`ListTagsForResource`, `TagResource`, `UntagResource`), which are still
    /// scaffolded, so no implemented operation reads this yet.
    pub tags: Vec<Tag>,
    /// When the protection was created. Stored for the same reason as `tags`: no member
    /// of the modelled `Protection` carries it, so no implemented read reports it.
    pub created_at: SystemTime,
    /// Position in the account's creation order: what `ListProtections` pages over, so
    /// its order is stable and a token keeps meaning the same place after a delete.
    pub sequence: u64,
}

impl ProtectionEntry {
    /// This record as the modelled `Protection` a read answers with.
    pub fn to_modeled(&self) -> Protection {
        Protection {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            health_check_ids: Some(self.health_check_ids.clone()),
            protection_arn: Some(self.protection_arn.clone()),
            application_layer_automatic_response_configuration: self
                .application_layer_automatic_response
                .clone(),
        }
    }
}

/// Per-service state for com-amazonaws-shield-awsshield-20160616. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsShieldAwsshield20160616State {
    /// The account's Shield Advanced subscription: `None` until `CreateSubscription`.
    pub subscription: Option<SubscriptionRecord>,
    /// Every live protection, keyed by its `ProtectionId` — the primary index.
    pub protections: HashMap<String, ProtectionEntry>,
    /// `ResourceArn` -> `ProtectionId`: the uniqueness index that refuses a second
    /// protection for one resource and resolves a describe-by-ARN.
    pub protections_by_resource_arn: HashMap<String, String>,
    /// The creation order counter behind [`ProtectionEntry::sequence`].
    next_sequence: u64,
}

impl ComAmazonawsShieldAwsshield20160616State {
    /// The `SubscriptionState` the account is in at `now`.
    pub fn subscription_state_at(&self, now: SystemTime) -> &'static str {
        match self.subscription.as_ref() {
            Some(subscription) if subscription.is_active_at(now) => SUBSCRIPTION_STATE_ACTIVE,
            _ => SUBSCRIPTION_STATE_INACTIVE,
        }
    }

    /// A `ProtectionId` no stored protection holds, or `None` when [`MINT_ATTEMPTS`]
    /// candidates were all taken.
    pub fn mint_protection_id(&self) -> Option<String> {
        (0..MINT_ATTEMPTS)
            .map(|_| new_uuid())
            .find(|candidate| !self.protections.contains_key(candidate))
    }

    /// Store a protection in creation order and index it by the ARN it protects.
    pub fn insert_protection(&mut self, mut entry: ProtectionEntry) {
        entry.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.protections_by_resource_arn
            .insert(entry.resource_arn.clone(), entry.id.clone());
        self.protections.insert(entry.id.clone(), entry);
    }

    /// The protection with this `ProtectionId`.
    pub fn protection_by_id(&self, protection_id: &str) -> Option<&ProtectionEntry> {
        self.protections.get(protection_id)
    }

    /// The protection covering this `ResourceArn`.
    pub fn protection_by_resource_arn(&self, resource_arn: &str) -> Option<&ProtectionEntry> {
        let protection_id = self.protections_by_resource_arn.get(resource_arn)?;
        self.protections.get(protection_id)
    }

    /// Forget a protection (and its resource-ARN index entry), as `DeleteProtection`
    /// does.
    pub fn remove_protection(&mut self, protection_id: &str) -> Option<ProtectionEntry> {
        let entry = self.protections.remove(protection_id)?;
        self.protections_by_resource_arn.remove(&entry.resource_arn);
        Some(entry)
    }

    /// Every stored protection in creation order — the stable order `ListProtections`
    /// pages over.
    pub fn protections_in_creation_order(&self) -> Vec<&ProtectionEntry> {
        let mut ordered: Vec<&ProtectionEntry> = self.protections.values().collect();
        ordered.sort_by_key(|entry| entry.sequence);
        ordered
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsShieldAwsshield20160616State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsShieldAwsshield20160616State>)
}
