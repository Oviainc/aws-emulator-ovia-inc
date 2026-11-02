// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-notifications-notifications service.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// `ACTIVE`, the `NotificationConfigurationStatus` of a configuration every one
/// of whose `EventRule`s is `ACTIVE`: "All of the EventRules are in ACTIVE
/// Status."
///
/// One of `NotificationConfigurationStatus`'s four modelled values
/// (`crate::ops::types::NOTIFICATION_CONFIGURATION_STATUS_VALUES`). The status is
/// DERIVED from the rules a configuration owns rather than stored on it — see
/// [`ComAmazonawsNotificationsNotificationsState::configuration_status`] — so no
/// operation sets it.
pub const CONFIGURATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `PARTIALLY_ACTIVE`, the status of a configuration some of whose `EventRule`s
/// are `ACTIVE` and some are not: "Some EventRules are in ACTIVE Status and some
/// are INACTIVE." See [`CONFIGURATION_STATUS_ACTIVE`].
pub const CONFIGURATION_STATUS_PARTIALLY_ACTIVE: &str = "PARTIALLY_ACTIVE";

/// `INACTIVE`, the status of a configuration none of whose `EventRule`s is
/// `ACTIVE`: "All of the EventRules are in INACTIVE Status." It is therefore also
/// the status of a configuration that owns NO EventRule at all — the state a
/// freshly created one is in, since `CreateNotificationConfiguration` creates no
/// rule — because no rule of it is ACTIVE. See [`CONFIGURATION_STATUS_ACTIVE`].
pub const CONFIGURATION_STATUS_INACTIVE: &str = "INACTIVE";

/// `ACCOUNT`, the subtype of a configuration an account created for itself, as
/// opposed to `ADMIN_MANAGED` (one an organization's management account owns).
/// Both are modelled values of `NotificationConfigurationSubtype`
/// (`crate::ops::types::NOTIFICATION_CONFIGURATION_SUBTYPE_VALUES`).
pub const CONFIGURATION_SUBTYPE_ACCOUNT: &str = "ACCOUNT";

/// The alphabet and width of an id segment of this service's arns, from the
/// patterns the model declares for them — `NotificationConfigurationArn`
/// (`^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$`)
/// and `EventRuleArn`, which appends `/rule/[a-z0-9]{27}` to it. Both id
/// segments are drawn from the same alphabet at the same width, so one pair of
/// constants serves both.
const ARN_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const ARN_ID_LEN: usize = 27;

/// The most NotificationConfigurations this emulator holds at once.
///
/// `configurations` grows by one record per `CreateNotificationConfiguration`
/// request — each carrying a caller-supplied tag map — and shrinks only on an
/// explicit delete, so without a ceiling its entry count, and the per-request
/// ordering `ListNotificationConfigurations` computes over it, are set by how
/// many requests a caller makes. AWS bounds the same thing with a per-account
/// service quota and refuses a create past it; the quota's value is adjustable
/// and is not in the model, so this number is the EMULATOR's own bound rather
/// than a claim about AWS's, while the refusal it produces is the error the
/// model declares for exactly this case: `ServiceQuotaExceededException`
/// (HTTP 402, "Request would cause a service quota to be exceeded").
pub const MAX_NOTIFICATION_CONFIGURATIONS: usize = 500;

/// One persisted `NotificationConfiguration`.
///
/// This is the single record every configuration operation reads and writes:
/// `CreateNotificationConfiguration` inserts it, `GetNotificationConfiguration`
/// and `ListNotificationConfigurations` report it,
/// `UpdateNotificationConfiguration` mutates it in place, and
/// `DeleteNotificationConfiguration` removes it. `tags` is persisted here (not
/// in a second map beside it) so `TagResource`/`UntagResource`/
/// `ListTagsForResource` can be implemented against the same record.
///
/// `status` is NOT a field here: the model defines every reportable
/// `NotificationConfigurationStatus` in terms of the configuration's `EventRule`
/// set, so it is derived from the stored rules by
/// [`ComAmazonawsNotificationsNotificationsState::configuration_status`] rather
/// than carried as a second copy of them.
#[derive(Debug, Clone)]
pub struct NotificationConfigurationRecord {
    /// The minted arn — the single key for lookup, update and delete.
    pub arn: String,
    /// `name`, from create and mutable through update.
    pub name: String,
    /// `description`, from create and mutable through update.
    pub description: String,
    /// `aggregationDuration` (`LONG` | `NONE` | `SHORT`), absent when the
    /// create call omitted it and no update has set it.
    pub aggregation_duration: Option<String>,
    /// `subtype`, one of `NotificationConfigurationSubtype`'s values.
    pub subtype: Option<String>,
    /// `creationTime`, set once when the record is inserted and never rewritten.
    pub creation_time: ::aws_smithy_types::DateTime,
    /// `tags`, as the create call supplied them (empty when it supplied none)
    /// and as `TagResource` and `UntagResource` have since changed them. This
    /// is the set `ListTagsForResource` reports.
    pub tags: BTreeMap<String, String>,
    /// The sources of the EventRules this configuration carries — what
    /// `ListNotificationConfigurations`'s `eventRuleSource` filter matches a
    /// configuration on. An INDEX, derived from the `event_rules` the store
    /// holds for this configuration by
    /// [`ComAmazonawsNotificationsNotificationsState::refresh_event_rule_sources`],
    /// never written by hand: the rules are the one stored copy.
    pub event_rule_sources: BTreeSet<String>,
    /// The channels associated with this configuration, each mapped to the
    /// association sequence it was given — what `ListChannels` reports and pages
    /// through, and what `ListNotificationConfigurations`'s `channelArn` filter
    /// matches a configuration on. `AssociateChannel` inserts an entry and
    /// `DisassociateChannel` removes one; the sequence is kept on a repeat of an
    /// association that already holds, so an idempotent re-association never
    /// reorders a listing.
    pub channel_arns: BTreeMap<String, u64>,
    /// Creation order within this store. `ListNotificationConfigurations`
    /// reports configurations in this order and pages through it, so a listing
    /// is stable and a page token keeps meaning across inserts and deletes.
    pub sequence: u64,
}

/// The most tags one resource carries at once.
///
/// This is the `max` of the modelled `TagMap`'s `@length` constraint
/// (`0..200`) — the shape both `TagResource`'s `tags` input and
/// `ListTagsForResource`'s `tags` output are typed as. A tag write that left
/// more than this many tags on a record would make the later read exceed the
/// bound its own model declares, so a write past it is refused instead. The
/// number comes from the model, not from this emulator.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

impl NotificationConfigurationRecord {
    /// How many tags this record would carry after merging `keys` into it,
    /// counted WITHOUT writing anything.
    ///
    /// Tagging adds or overwrites the keys it is given and leaves every other
    /// key alone, so the resulting count is the stored count plus the incoming
    /// keys this record does not already carry. A write is all-or-nothing, so
    /// the count has to be known before the first insert.
    pub fn tag_count_after_merge<'a, I: IntoIterator<Item = &'a str>>(&self, keys: I) -> usize {
        self.tags.len()
            + keys
                .into_iter()
                .filter(|key| !self.tags.contains_key(*key))
                .count()
    }

    /// Merge `tags` into this record's tag set: each supplied key is inserted
    /// or overwritten, and a key the request does not mention keeps the value
    /// it had. Repeating the same merge is therefore a no-op, which is what
    /// `TagResource`'s `@idempotent` trait requires.
    pub fn merge_tags<I: IntoIterator<Item = (String, String)>>(&mut self, tags: I) {
        for (key, value) in tags {
            self.tags.insert(key, value);
        }
    }

    /// Remove `keys` from this record's tag set, leaving every key they do not
    /// name alone.
    ///
    /// A key this record does not carry is already in the state the request
    /// asks for, so it is not an error and nothing else is removed in its
    /// place: repeating the same removal is a no-op, which is what
    /// `UntagResource`'s `@idempotent` trait requires.
    pub fn remove_tags<'a, I: IntoIterator<Item = &'a str>>(&mut self, keys: I) {
        for key in keys {
            self.tags.remove(key);
        }
    }
}

/// `CREATING`, the per-Region status a freshly created `EventRule` carries.
///
/// One of `EventRuleStatus`'s five modelled values
/// (`crate::ops::types::EVENT_RULE_STATUS_VALUES`), documented in the model as
/// "This EventRule is being created. Only GET/LIST calls can be executed." — so
/// it is transitional, and the clock owes it [`EVENT_RULE_TRANSITION_SPAN`]
/// later.
pub const EVENT_RULE_STATUS_CREATING: &str = "CREATING";

/// `ACTIVE`, the per-Region status an `EventRule` settles in: "EventRule is
/// processing events. Any call can be executed." Terminal — nothing moves a rule
/// out of it but a caller.
pub const EVENT_RULE_STATUS_ACTIVE: &str = "ACTIVE";

/// `UPDATING`, the per-Region status an `EventRule` carries while an update of
/// it is being applied.
///
/// Another of `EventRuleStatus`'s five modelled values, documented in the model
/// as "This EventRule is being updated. Only GET/LIST calls can be executed." —
/// transitional in exactly the way [`EVENT_RULE_STATUS_CREATING`] is, so the
/// clock owes it [`EVENT_RULE_TRANSITION_SPAN`] later.
pub const EVENT_RULE_STATUS_UPDATING: &str = "UPDATING";

/// The `reason` reported beside a `CREATING` Region of an `EventRule`.
///
/// `EventRuleStatusReason` is an unconstrained string for which the model
/// declares no values, so this sentence is this emulator's own rendering of the
/// documentation the model carries for the matching `EventRuleStatus` value:
/// the status is the contract, this prose is not.
pub const EVENT_RULE_REASON_CREATING: &str = "The EventRule is being created.";

/// The `reason` reported beside an `ACTIVE` Region of an `EventRule`. See
/// [`EVENT_RULE_REASON_CREATING`].
pub const EVENT_RULE_REASON_ACTIVE: &str = "The EventRule can process events.";

/// The `reason` reported beside an `UPDATING` Region of an `EventRule`. See
/// [`EVENT_RULE_REASON_CREATING`].
pub const EVENT_RULE_REASON_UPDATING: &str = "The EventRule is being updated.";

/// How long an `EventRule`'s Regions stay in a transitional status
/// ([`EVENT_RULE_STATUS_CREATING`] after a create, [`EVENT_RULE_STATUS_UPDATING`]
/// after an update) before the clock makes [`EVENT_RULE_STATUS_ACTIVE`] due.
///
/// AWS publishes no duration for the step, so this span is the EMULATOR's own
/// choice; what matters is that it is never zero — a caller reading straight
/// after the create still observes the `CREATING` the model declares, and the
/// "Only GET/LIST calls can be executed" window it names — and that it is short
/// enough to wait on. Nothing sleeps for it: the due instant is stored and
/// [`ComAmazonawsNotificationsNotificationsState::settle_due_event_rules`]
/// materialises whatever it has already made due.
pub const EVENT_RULE_TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The most `EventRule`s one `NotificationConfiguration` holds at once.
///
/// `event_rules` grows by one record per `CreateEventRule` request that is not a
/// repeat, and shrinks only on an explicit delete, so without a ceiling its
/// entry count is set by how many requests a caller makes. AWS bounds the same
/// thing with an adjustable per-account service quota which is not in the model,
/// so this number is the EMULATOR's own bound rather than a claim about AWS's,
/// while the refusal it produces is the error the model declares for exactly
/// this case: `ServiceQuotaExceededException` (HTTP 402, "Request would cause a
/// service quota to be exceeded").
pub const MAX_EVENT_RULES_PER_CONFIGURATION: usize = 100;

/// The most channels one `NotificationConfiguration` is associated with at
/// once.
///
/// A record's `channel_arns` grows by one entry per `AssociateChannel` request
/// naming a channel it does not already hold, and shrinks only on an explicit
/// `DisassociateChannel`, so without a ceiling its entry count — and the size of
/// every `ListChannels` page computed over it — is set by how many requests a
/// caller makes. AWS bounds the same thing with an adjustable per-account
/// service quota which is not in the model, so this number is the EMULATOR's own
/// bound rather than a claim about AWS's, while the refusal it produces is the
/// error the model declares for exactly this case on `AssociateChannel`:
/// `ServiceQuotaExceededException` (HTTP 402, "Request would cause a service
/// quota to be exceeded").
pub const MAX_CHANNELS_PER_CONFIGURATION: usize = 100;

/// The most Regions one `EventRule` covers.
///
/// The modelled `Regions` list declares `@length(1..)` — a floor and NO ceiling
/// — so without a bound here the size of one stored rule (its `regions` list and
/// its one `EventRuleStatusSummary` per Region) and of every response that
/// reports it are set by how long a list a caller sends. AWS bounds the same
/// thing by what Regions exist: a Region that is not one cannot send events to
/// the rule, and there are fewer than this many. The value is therefore the
/// EMULATOR's own ceiling rather than a claim about AWS's, chosen above the
/// number of Regions AWS has so no faithful request meets it, while the refusal
/// it produces names the member and its bound, which is what the modelled
/// `ValidationException` (HTTP 400) carries in its `fieldList`.
pub const MAX_REGIONS_PER_EVENT_RULE: usize = 50;

/// How this service states a `regions` list longer than
/// [`MAX_REGIONS_PER_EVENT_RULE`]: the `message` of the `ValidationException`
/// (HTTP 400) that refuses it, naming the bound and the length the request
/// asked for.
///
/// Spelled once, beside the bound it states, so every operation taking
/// `regions` refuses an over-long list in the same words rather than one
/// sentence per handler.
pub fn regions_bound_message(asked_for: usize) -> String {
    format!("regions must name at most {MAX_REGIONS_PER_EVENT_RULE} Regions, got {asked_for}")
}

/// What that same refusal states about the `regions` member itself, in the
/// `ValidationExceptionField` the modelled `ValidationException` carries in its
/// `fieldList`. See [`regions_bound_message`].
pub fn regions_bound_field_message() -> String {
    format!("the list names more than the {MAX_REGIONS_PER_EVENT_RULE} Regions an EventRule covers")
}

/// How this service states a create of an `EventRule` for a `(source,
/// eventType)` pair one of the configuration's rules already covers: the
/// `message` of the `ConflictException` (HTTP 409) that refuses it.
///
/// The pair is what identifies a rule, so the sentence states the pair that is
/// already covered and the configuration covering it, in the same lower-case
/// prose this service's other refusals use. Spelled once so the refusal reads
/// the same way wherever the uniqueness of a pair is enforced.
///
/// AWS's own sentence for this refusal is neither in the model (whose
/// `ConflictException` carries only the generic documentation of the shape) nor
/// among the refusals this service has been observed to state, so the wording
/// here is this service's own and an observation of AWS's supersedes it; the
/// variable parts come from the request either way.
pub fn event_rule_pair_conflict_message(
    configuration_arn: &str,
    source: &str,
    event_type: &str,
) -> String {
    format!(
        "an event rule with source {source} and event type {event_type} already exists for \
         notification configuration {configuration_arn}"
    )
}

/// The Regions one `EventRule` covers, out of the list a request asked for: the
/// Regions in the order they were named, without repeats.
///
/// `statusSummaryByRegion` is keyed by Region, so a Region named twice in one
/// request is one Region: keep the first mention and drop the repeat rather than
/// reporting one Region twice in `regions` and once in the status map. Shared by
/// the create and the update path, so both store a rule's Regions the same way.
/// The allocation is clamped to [`MAX_REGIONS_PER_EVENT_RULE`] and the
/// already-seen test is a set lookup, so neither the allocation nor the work is
/// sized by a caller-chosen list length (a caller that asks for a longer list is
/// refused outright, before this runs).
fn distinct_regions(regions: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut ordered: Vec<String> = Vec::with_capacity(regions.len().min(MAX_REGIONS_PER_EVENT_RULE));
    for region in regions.iter().take(MAX_REGIONS_PER_EVENT_RULE) {
        if seen.insert(region.as_str()) {
            ordered.push(region.clone());
        }
    }
    ordered
}

/// One persisted `EventRule`.
///
/// This is the single record every EventRule operation reads and writes:
/// `CreateEventRule` inserts it, `GetEventRule` and `ListEventRules` report it,
/// `UpdateEventRule` mutates it in place, and `DeleteEventRule` removes it. The
/// rule is keyed by its own arn, which nests under the arn of the
/// `NotificationConfiguration` it is associated with.
#[derive(Debug, Clone)]
pub struct EventRuleRecord {
    /// The minted arn — the single key for lookup, update and delete.
    pub arn: String,
    /// `notificationConfigurationArn`, the configuration this rule is
    /// associated with. The rule's own arn nests under it, so this is also its
    /// prefix; it is persisted because every report of the rule carries it.
    pub notification_configuration_arn: String,
    /// `source`, the matched EventBridge event source. Fixed at create: neither
    /// `UpdateEventRule`'s input nor any other operation can change it.
    pub source: String,
    /// `eventType`, the matched EventBridge event type. Fixed at create for the
    /// same reason as [`Self::source`].
    pub event_type: String,
    /// `eventPattern`, the additional filter, or `None` when the create call
    /// supplied none and no update has set one. `GetEventRule` marks the member
    /// required, so an absent pattern is reported as the empty string, which
    /// the modelled `@length(0..4096)` admits.
    pub event_pattern: Option<String>,
    /// `regions`, the Regions that send events to this rule, in the order the
    /// caller gave and without repeats — `statusSummaryByRegion` is keyed by
    /// Region, so a Region named twice in one request is one Region.
    pub regions: Vec<String>,
    /// `creationTime`, set once when the record is inserted and never rewritten.
    pub creation_time: ::aws_smithy_types::DateTime,
    /// `statusSummaryByRegion`, one entry per Region in [`Self::regions`].
    /// Per-Region because the model keys it that way: a rule can be settled in
    /// one Region while another is still coming up.
    pub status_by_region: BTreeMap<String, crate::ops::types::EventRuleStatusSummary>,
    /// When the clock owes this rule its next step, or `None` when every Region
    /// is settled. See [`EVENT_RULE_TRANSITION_SPAN`].
    pub due_at: Option<SystemTime>,
    /// Creation order within this store. `ListEventRules` reports rules in this
    /// order and pages through it, so a listing is stable and a page token keeps
    /// meaning across inserts and deletes.
    pub sequence: u64,
}

impl EventRuleRecord {
    /// This rule's `statusSummaryByRegion`, as the wire map every EventRule
    /// operation returns.
    pub fn status_summary_by_region(
        &self,
    ) -> std::collections::HashMap<String, crate::ops::types::EventRuleStatusSummary> {
        self.status_by_region
            .iter()
            .map(|(region, summary)| (region.clone(), summary.clone()))
            .collect()
    }

    /// Whether this rule is `ACTIVE` as a whole: every Region it carries reports
    /// `EventRuleStatus` `ACTIVE`.
    ///
    /// `statusSummaryByRegion` is per-Region, while the status of the
    /// `NotificationConfiguration` that owns the rule is not, so one answer per
    /// rule is what that derivation needs. A rule with a Region still coming up
    /// is not processing events everywhere it was asked to, so it is not ACTIVE;
    /// a rule carrying no Region at all cannot be processing events either.
    pub fn is_active(&self) -> bool {
        !self.status_by_region.is_empty()
            && self
                .status_by_region
                .values()
                .all(|summary| summary.status == EVENT_RULE_STATUS_ACTIVE)
    }

    /// `eventPattern` as the wire reports it: the stored pattern, or the empty
    /// string when the rule carries none. See [`Self::event_pattern`].
    pub fn reported_event_pattern(&self) -> String {
        self.event_pattern.clone().unwrap_or_default()
    }

    /// The Amazon EventBridge Managed Rule arns associated with this rule.
    ///
    /// ALWAYS EMPTY, and that is the honest answer rather than a placeholder:
    /// the model documents these as rules "created by User Notifications within
    /// your account", and this emulator creates no EventBridge rule, so there is
    /// none to report. `ManagedRuleArns` is a required member with no `@length`
    /// floor, so an empty list satisfies the model; minting an arn for a rule
    /// nothing created would report a resource that does not exist.
    pub fn managed_rules(&self) -> Vec<String> {
        Vec::new()
    }
}

/// `REGISTERING`, the status a freshly registered `NotificationHub` carries.
///
/// One of `NotificationHubStatus`'s four modelled values
/// (`crate::ops::types::NOTIFICATION_HUB_STATUS_VALUES`), documented in the model
/// as "Notification Hub is initializing. Cannot deregister a Notification Hub in
/// this status." — so it is transitional, and the clock owes it
/// [`HUB_TRANSITION_SPAN`] later.
pub const NOTIFICATION_HUB_STATUS_REGISTERING: &str = "REGISTERING";

/// `ACTIVE`, the status a registered hub settles in: "Incoming Notification
/// Events are being replicated to this Notification Hub. A Notification Hub with
/// this status can be deregistered." It is terminal — nothing moves a hub out of
/// it but a deregister.
pub const NOTIFICATION_HUB_STATUS_ACTIVE: &str = "ACTIVE";

/// `DEREGISTERING`, the status a hub carries while it is being removed:
/// "Notification Hub is being deleted. Cannot register a Notification Hub in the
/// same region as one in this status." Transitional too: the record leaves the
/// store [`HUB_TRANSITION_SPAN`] after the deregister call.
pub const NOTIFICATION_HUB_STATUS_DEREGISTERING: &str = "DEREGISTERING";

/// The `reason` reported beside a `REGISTERING` hub.
///
/// `NotificationHubStatusReason` is an unconstrained string for which the model
/// declares no values, so each of these three sentences is this emulator's own
/// rendering of the documentation the model carries for the matching
/// `NotificationHubStatus` member: the status is the contract, this prose is not.
pub const HUB_REASON_REGISTERING: &str = "The NotificationHub is initializing.";

/// The `reason` reported beside an `ACTIVE` hub. See [`HUB_REASON_REGISTERING`].
pub const HUB_REASON_ACTIVE: &str =
    "Incoming Notification Events are being replicated to this NotificationHub.";

/// The `reason` reported beside a `DEREGISTERING` hub. See
/// [`HUB_REASON_REGISTERING`].
pub const HUB_REASON_DEREGISTERING: &str = "The NotificationHub is being deleted.";

/// How long a `NotificationHub` stays in a transitional status before the clock
/// makes the next step due: `REGISTERING` -> `ACTIVE` after a register, and
/// removal from the store after a deregister.
///
/// AWS publishes no duration for either step, so this span is the EMULATOR's own
/// choice; what matters is that it is never zero, so a caller that reads straight
/// after the call still observes the transitional status AWS reports, and that it
/// is short enough for a caller to wait on. Nothing sleeps for it: the due instant
/// is stored and [`ComAmazonawsNotificationsNotificationsState::settle_due_hubs`]
/// materialises whatever it has already made due.
pub const HUB_TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The most `NotificationHub`s this account holds at once: "There is a maximum of
/// one NotificationHub per Region. You can have a maximum of 3 NotificationHub
/// resources at a time.", from `RegisterNotificationHub`'s own documentation in
/// this service's model. A register past it is refused with the error the model
/// declares for exactly that case, `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_NOTIFICATION_HUBS: usize = 3;

/// One persisted `NotificationHub`.
///
/// `RegisterNotificationHub` inserts it, `ListNotificationHubs` reports it, and
/// `DeregisterNotificationHub` moves it to `DEREGISTERING`; the clock removes it
/// from there. A hub is keyed by its region — "a maximum of one NotificationHub
/// per Region" — so there is no minted id.
#[derive(Debug, Clone)]
pub struct NotificationHubRecord {
    /// `notificationHubRegion`, the hub's key.
    pub region: String,
    /// `statusSummary.status`, one of `NotificationHubStatus`'s values.
    pub status: String,
    /// `statusSummary.reason`, the prose that accompanies [`Self::status`].
    pub reason: String,
    /// `creationTime`, set once when the hub is registered and never rewritten.
    pub creation_time: ::aws_smithy_types::DateTime,
    /// `lastActivationTime`, absent until the hub first reaches `ACTIVE` and
    /// stamped with the instant that activation fell due.
    pub last_activation_time: Option<::aws_smithy_types::DateTime>,
    /// When the clock owes this hub its next step, or `None` when the record is
    /// settled. See [`HUB_TRANSITION_SPAN`].
    pub due_at: Option<SystemTime>,
    /// Registration order within this store — the order `ListNotificationHubs`
    /// reports and pages through, so a listing is stable and a page token keeps
    /// meaning across registers and deregisters.
    pub sequence: u64,
}

impl NotificationHubRecord {
    /// This hub's `statusSummary`, as the wire shape every hub operation returns.
    pub fn status_summary(&self) -> crate::ops::types::NotificationHubStatusSummary {
        crate::ops::types::NotificationHubStatusSummary {
            status: self.status.clone(),
            reason: self.reason.clone(),
        }
    }

    /// This hub as the `NotificationHubOverview` a listing reports.
    pub fn overview(&self) -> crate::ops::types::NotificationHubOverview {
        crate::ops::types::NotificationHubOverview {
            notification_hub_region: self.region.clone(),
            status_summary: self.status_summary(),
            creation_time: self.creation_time,
            last_activation_time: self.last_activation_time,
        }
    }
}

/// `DISABLED`, the `AccessStatus` of Service Trust Enablement between User
/// Notifications and Amazon Web Services Organizations in an account where
/// nobody has enabled it.
///
/// One of `AccessStatus`'s four modelled values
/// (`crate::ops::types::ACCESS_STATUS_VALUES`). It is the status a store that
/// has never seen an `EnableNotificationsAccessForOrganization` reports, and the
/// status a `DisableNotificationsAccessForOrganization` returns the account to.
pub const ORGANIZATION_ACCESS_DISABLED: &str = "DISABLED";

/// `PENDING`, the transitional `AccessStatus` an account carries between the
/// `EnableNotificationsAccessForOrganization` that asked for service trust and
/// the instant that trust is in force.
///
/// Enabling trust is an organization-wide step, and `PENDING` is the modelled
/// value that names it in flight — the only operations that touch this status
/// are the enable and the disable, so this is where it arises. The clock, not a
/// read, moves it to [`ORGANIZATION_ACCESS_ENABLED`]
/// [`ORGANIZATION_ACCESS_TRANSITION_SPAN`] later.
pub const ORGANIZATION_ACCESS_PENDING: &str = "PENDING";

/// `ENABLED`, the `AccessStatus` of an account whose service trust is in force.
/// Terminal: nothing moves out of it but a
/// `DisableNotificationsAccessForOrganization`.
pub const ORGANIZATION_ACCESS_ENABLED: &str = "ENABLED";

/// How long service trust stays [`ORGANIZATION_ACCESS_PENDING`] before the clock
/// makes [`ORGANIZATION_ACCESS_ENABLED`] due.
///
/// AWS publishes no duration for the step, so this span is the EMULATOR's own
/// choice; what matters is that it is never zero — a caller reading straight
/// after the enable still observes the `PENDING` the model declares — and that
/// it is short enough to wait on. Nothing sleeps for it: the due instant is
/// stored and
/// [`ComAmazonawsNotificationsNotificationsState::settle_due_organization_access`]
/// materialises it once it has fallen due.
pub const ORGANIZATION_ACCESS_TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The persisted state of Service Trust Enablement between User Notifications
/// and Amazon Web Services Organizations for this account.
///
/// `EnableNotificationsAccessForOrganization` moves it to `PENDING`,
/// `DisableNotificationsAccessForOrganization` returns it to `DISABLED`, and
/// `GetNotificationsAccessForOrganization` reports it. There is exactly one such
/// record per account, so it is a single field on the store rather than a map.
#[derive(Debug, Clone)]
pub struct OrganizationAccessRecord {
    /// `notificationsAccessForOrganization.accessStatus`, one of
    /// `AccessStatus`'s modelled values.
    pub access_status: String,
    /// When the clock owes this record its next step, or `None` when it is
    /// settled. See [`ORGANIZATION_ACCESS_TRANSITION_SPAN`].
    pub due_at: Option<SystemTime>,
}

impl Default for OrganizationAccessRecord {
    /// Service trust is off until an account enables it, so a store that has
    /// never seen an enable reports `DISABLED` and owes the clock nothing.
    fn default() -> Self {
        Self {
            access_status: ORGANIZATION_ACCESS_DISABLED.to_string(),
            due_at: None,
        }
    }
}

impl OrganizationAccessRecord {
    /// This record as the wire shape `GetNotificationsAccessForOrganization`
    /// reports.
    pub fn overview(&self) -> crate::ops::types::NotificationsAccessForOrganization {
        crate::ops::types::NotificationsAccessForOrganization {
            access_status: self.access_status.clone(),
        }
    }
}

/// `ACCOUNT_PRIMARY`, the account's primary contact.
///
/// One of the contact identifiers the `ChannelIdentifier` pattern accepts
/// verbatim (beside `ACCOUNT_ALTERNATE_BILLING`,
/// `ACCOUNT_ALTERNATE_OPERATIONS` and `ACCOUNT_ALTERNATE_SECURITY`, and the arn
/// of a chatbot, console-app or contact channel). Every managed notification
/// configuration this store publishes starts associated with it, so the
/// `channelIdentifier` filter of a managed listing has a channel to select on
/// before any association operation runs. The model does not say which
/// associations an account starts with, so this seed is the EMULATOR's, not a
/// claim about AWS's.
pub const MANAGED_DEFAULT_CHANNEL_IDENTIFIER: &str = "ACCOUNT_PRIMARY";

/// One entry of the catalog [`MANAGED_NOTIFICATION_CATALOG`] publishes.
///
/// `name` and `description` are the entry's OWN published text and NOT a
/// function of the arn: the display name is prose a console renders (so it
/// carries spacing and capitalisation the arn's segments cannot), and the
/// description states which events of that sub-category the configuration
/// covers. Deriving either from the arn's `category`/`sub-category` tokens
/// would publish an arn token where the service publishes display text, so both
/// are carried per entry.
#[derive(Debug, Clone, Copy)]
struct PublishedManagedNotification {
    /// The arn's `category/` segment — the service whose events this managed
    /// configuration delivers.
    category: &'static str,
    /// The arn's `sub-category/` segment — that service's own event-type
    /// category.
    sub_category: &'static str,
    /// `name`, the published display name.
    name: &'static str,
    /// `description`, the published prose description of the covered events.
    description: &'static str,
}

/// The managed notification configurations this store publishes.
///
/// A managed notification configuration is published BY the service: the model
/// declares no create or delete for one, only `GetManagedNotificationConfiguration`
/// and `ListManagedNotificationConfigurations` (plus the operations that
/// associate channels with one), so the catalog is store state from boot rather
/// than something a request inserts.
///
/// The model does not enumerate the catalog, so the published SET is the
/// service's own behaviour rather than something the model states; its SHAPE is
/// modelled — `ManagedNotificationConfigurationOsArn` (see
/// `smithy_query(query="shape",shape="ManagedNotificationConfigurationOsArn")`)
/// is `managed-notification-configuration/category/<category>/sub-category/<subCategory>`
/// with each segment `[a-zA-Z0-9\-]{3,64}`, and `ManagedNotificationConfigurationDescription`
/// bounds a description at 256 characters.
///
/// The set below spans the services AWS publishes managed notifications for,
/// each with one entry per event-type category of that service: `AWS-Health`
/// (whose sub-categories are AWS Health's own event type categories),
/// `AWS-Partner-Central` (whose sub-categories are the AWS Marketplace seller
/// notification categories — product listings, offers and agreements, payments
/// and disbursements, and seller account management) and `AWS-MP-Buyer` (whose
/// sub-categories are the Marketplace BUYER notification categories — the
/// products and solutions subscribed to, private offers extended to the
/// account, pricing changes and agreements). Both sides of Marketplace are
/// published, because a buyer account enumerates the catalog to subscribe to
/// buyer notifications just as a seller account does to seller ones. A category
/// is published for every account, since nothing a caller does creates or
/// removes one — the catalog is the same fixed set for every account, so an
/// entry's arn, name and description are published values rather than
/// per-account or per-run ones.
const MANAGED_NOTIFICATION_CATALOG: &[PublishedManagedNotification] = &[
    PublishedManagedNotification {
        category: "AWS-Health",
        sub_category: "Issue",
        name: "Issues",
        description: "AWS Health events reporting an issue that affects resources, services or regions you use, including the resources involved and the action to take.",
    },
    PublishedManagedNotification {
        category: "AWS-Health",
        sub_category: "AccountNotification",
        name: "Account notifications",
        description: "AWS Health events reporting account and service notifications for this account, including announcements, deprecations and changes that need your attention.",
    },
    PublishedManagedNotification {
        category: "AWS-Health",
        sub_category: "ScheduledChange",
        name: "Scheduled changes",
        description: "AWS Health events reporting a change scheduled against resources you use, including planned lifecycle events, maintenance windows and certificate rotations.",
    },
    PublishedManagedNotification {
        category: "AWS-Health",
        sub_category: "Investigation",
        name: "Investigations",
        description: "AWS Health events reporting an open investigation into possible impact on resources you use, published while the impact is still being confirmed.",
    },
    PublishedManagedNotification {
        category: "AWS-Partner-Central",
        sub_category: "PCProductListings",
        name: "Product listings",
        description: "Marketplace events related to your published AMI, container, and SaaS product catalog, including listing change set status, vulnerability scan findings, open product tasks, and Vendor Insights security profile snapshots.",
    },
    PublishedManagedNotification {
        category: "AWS-Partner-Central",
        sub_category: "PCOffersAndAgreements",
        name: "Offers and Agreements",
        description: "Marketplace events related to private offers, reseller activity, and post-acceptance agreement lifecycle, including offer publishing, acceptance, agreement creation, amendment, cancellation, compliance failures, and ISV entitlement events.",
    },
    PublishedManagedNotification {
        category: "AWS-Partner-Central",
        sub_category: "PCPaymentsAndDisbursements",
        name: "Payments and disbursements",
        description: "Marketplace events with payments and financial implications, including disbursement failures and pauses, payment failures, payment request rejections, billing adjustments on purchase agreements, and invoice submission outcomes.",
    },
    PublishedManagedNotification {
        category: "AWS-Partner-Central",
        sub_category: "PCAccountManagement",
        name: "Account management",
        description: "Marketplace events related to seller account standing, including business and bank account verification status.",
    },
    PublishedManagedNotification {
        category: "AWS-MP-Buyer",
        sub_category: "MPBuyerProducts",
        name: "Products and solutions",
        description: "Notifications about changes to products you've subscribed to, including new versions, instance type updates, visibility changes, region restrictions, and security profile snapshots.",
    },
    PublishedManagedNotification {
        category: "AWS-MP-Buyer",
        sub_category: "MPBuyerPrivateOffers",
        name: "Private offers",
        description: "Notifications about private offers extended to you, including new offers, offer set availability, and channel partner offer lifecycle updates.",
    },
    PublishedManagedNotification {
        category: "AWS-MP-Buyer",
        sub_category: "MPBuyerPricing",
        name: "Pricing changes",
        description: "Notifications about price changes to products you're subscribed to, including hourly, monthly, and usage-based pricing updates.",
    },
    PublishedManagedNotification {
        category: "AWS-MP-Buyer",
        sub_category: "MPBuyerAgreements",
        name: "Agreements",
        // The published text of this one entry is known only as far as this
        // sentence goes: the recording available here states it up to "new
        // agreements" and no further. It stops at the last published word
        // rather than continuing with prose nobody published, so the entry is
        // part of the catalog a buyer account enumerates without carrying
        // invented display text.
        description: "Notifications about your agreements and subscriptions, including new agreements.",
    },
];

/// One published `ManagedNotificationConfiguration`.
///
/// `ListManagedNotificationConfigurations` reports the `arn`/`name`/`description`
/// of each of these as a `ManagedNotificationConfigurationStructure`, and
/// `GetManagedNotificationConfiguration` reports the same record with its
/// `category` and `subCategory` — which are the two segments its arn is built
/// from, so a listed entry and the read of its arn can never disagree.
#[derive(Debug, Clone)]
pub struct ManagedNotificationConfigurationRecord {
    /// The arn, built from `category` and `sub_category` — the single key for
    /// lookup.
    pub arn: String,
    /// `name`, as both operations report it.
    pub name: String,
    /// `description`, as both operations report it.
    pub description: String,
    /// `category`, the arn's `category/` segment.
    pub category: String,
    /// `subCategory`, the arn's `sub-category/` segment.
    pub sub_category: String,
    /// Publication order within this store, counting from 1.
    /// `ListManagedNotificationConfigurations` reports the catalog newest-first
    /// by this order and pages through it, so a listing is stable and a page
    /// token keeps its meaning.
    pub sequence: u64,
    /// The channel identifiers this managed configuration delivers to, each
    /// mapped to the association sequence it was given — what the
    /// `channelIdentifier` filter of a managed listing matches a configuration
    /// on, and what `ListManagedNotificationChannelAssociations` reports once it
    /// is implemented. Seeded with
    /// [`MANAGED_DEFAULT_CHANNEL_IDENTIFIER`]; the associate and disassociate
    /// operations add and remove entries.
    pub channel_identifiers: BTreeMap<String, u64>,
}

/// How many channel-association sequences the seeded catalog consumes: one per
/// published entry, for the [`MANAGED_DEFAULT_CHANNEL_IDENTIFIER`] each starts
/// associated with. Derived from the catalog itself, so it cannot drift when
/// entries are added or removed.
pub const SEEDED_CHANNEL_ASSOCIATIONS: u64 = MANAGED_NOTIFICATION_CATALOG.len() as u64;

/// The catalog as stored: arn -> record, in [`MANAGED_NOTIFICATION_CATALOG`]
/// order. Built once per store so every read sees the same published set
/// without a read ever writing to it.
fn seeded_managed_configurations() -> BTreeMap<String, ManagedNotificationConfigurationRecord> {
    MANAGED_NOTIFICATION_CATALOG
        .iter()
        .enumerate()
        .map(|(index, published)| {
            let PublishedManagedNotification {
                category,
                sub_category,
                name,
                description,
            } = *published;
            // `notifications` arns carry no region (`notifications::<account>:`).
            let arn = ::engine::emulator_arn(
                "notifications",
                "",
                &format!(
                    "managed-notification-configuration/category/{category}/sub-category/{sub_category}"
                ),
            );
            let record = ManagedNotificationConfigurationRecord {
                arn: arn.clone(),
                // The published display name, NOT a rendering of the arn's own
                // segments: the two are independent, so a caller that renders
                // `name` shows display text rather than an arn token.
                name: name.to_string(),
                description: description.to_string(),
                category: category.to_string(),
                sub_category: sub_category.to_string(),
                sequence: index as u64 + 1,
                // The seeded association draws its sequence from the same
                // counter every other association in this store uses (see
                // `channel_associations_minted`), so a later minted association
                // never reuses a seeded sequence and an association listing
                // stays unambiguously ordered.
                channel_identifiers: BTreeMap::from([(
                    MANAGED_DEFAULT_CHANNEL_IDENTIFIER.to_string(),
                    index as u64 + 1,
                )]),
            };
            (arn, record)
        })
        .collect()
}

/// Per-service state for com-amazonaws-notifications-notifications. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsNotificationsNotificationsState {
    /// arn -> configuration.
    pub configurations: BTreeMap<String, NotificationConfigurationRecord>,
    /// How many configurations this store has ever minted. Doubles as the next
    /// record's [`NotificationConfigurationRecord::sequence`] and as part of the
    /// uniqueness seed of a new arn, so an id is never reused after a delete.
    pub configurations_minted: u64,
    /// arn -> EventRule. One flat map keyed by the rule's own arn, which nests
    /// under its configuration's arn, so a rule is reached by the arn every
    /// EventRule operation is given without a second lookup.
    pub event_rules: BTreeMap<String, EventRuleRecord>,
    /// How many EventRules this store has ever minted. Doubles as the next
    /// record's [`EventRuleRecord::sequence`] and as part of the uniqueness seed
    /// of a new arn, so an id is never reused after a delete.
    pub event_rules_minted: u64,
    /// notificationHubRegion -> hub. One entry per region, as AWS's "maximum of
    /// one NotificationHub per Region" requires.
    pub hubs: BTreeMap<String, NotificationHubRecord>,
    /// How many hubs this store has ever registered. Doubles as the next
    /// record's [`NotificationHubRecord::sequence`], so a listing's order and its
    /// page tokens survive a deregister and a re-register of the same region.
    pub hubs_registered: u64,
    /// How many channel associations this store has ever made. Doubles as the
    /// next association's sequence in a record's
    /// [`NotificationConfigurationRecord::channel_arns`], so a `ListChannels`
    /// page token keeps its meaning across associations and disassociations —
    /// including a re-association of a channel that was removed, which lands
    /// after the associations that outlived it. It starts at
    /// [`SEEDED_CHANNEL_ASSOCIATIONS`], the sequences the published catalog's
    /// own associations already hold.
    pub channel_associations_minted: u64,
    /// Service Trust Enablement between User Notifications and Amazon Web
    /// Services Organizations for this account. One record per account, holding
    /// `DISABLED` until an enable asks for trust.
    pub organization_access: OrganizationAccessRecord,
    /// arn -> published ManagedNotificationConfiguration. Seeded from
    /// [`MANAGED_NOTIFICATION_CATALOG`] when the store is created, because the
    /// service publishes these rather than a caller creating them.
    pub managed_configurations: BTreeMap<String, ManagedNotificationConfigurationRecord>,
}

impl Default for ComAmazonawsNotificationsNotificationsState {
    /// A fresh store holds no configuration, rule or hub a caller creates, has
    /// service trust off — and already publishes the managed notification
    /// catalog, which no request mints.
    fn default() -> Self {
        let managed_configurations = seeded_managed_configurations();
        Self {
            configurations: BTreeMap::new(),
            configurations_minted: 0,
            event_rules: BTreeMap::new(),
            event_rules_minted: 0,
            hubs: BTreeMap::new(),
            hubs_registered: 0,
            // The seeded associations of the published catalog already hold
            // these sequences, so the next minted association lands after them.
            channel_associations_minted: SEEDED_CHANNEL_ASSOCIATIONS,
            organization_access: OrganizationAccessRecord::default(),
            managed_configurations,
        }
    }
}

impl ComAmazonawsNotificationsNotificationsState {
    /// Every published managed notification configuration in publication order
    /// — the order `ListManagedNotificationConfigurations` reports (reversed, so
    /// newest first) and pages through.
    pub fn managed_configurations_in_publication_order(
        &self,
    ) -> Vec<&ManagedNotificationConfigurationRecord> {
        let mut records: Vec<&ManagedNotificationConfigurationRecord> =
            self.managed_configurations.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// Whether another configuration fits under
    /// [`MAX_NOTIFICATION_CONFIGURATIONS`]. A create that cannot must be
    /// refused with the modelled quota error, not served by growing the store.
    pub fn has_configuration_capacity(&self) -> bool {
        self.configurations.len() < MAX_NOTIFICATION_CONFIGURATIONS
    }

    /// Mint a fresh configuration arn and reserve its creation sequence.
    ///
    /// The id segment is 27 characters drawn from `[a-z0-9]`, as the
    /// `NotificationConfigurationArn` pattern requires, derived from a SHA-256
    /// over a per-process random seed, this store's mint counter and the current
    /// instant — so two creates never collide, not even with identical input,
    /// and the id is not a function of the request.
    pub fn mint_configuration_arn(&mut self) -> (String, u64) {
        self.configurations_minted += 1;
        let sequence = self.configurations_minted;
        let id = mint_arn_id(sequence);

        // `notifications` arns carry no region (`notifications::<account>:`),
        // which `emulator_arn` renders from an empty region segment.
        let arn = ::engine::emulator_arn("notifications", "", &format!("configuration/{id}"));
        (arn, sequence)
    }

    /// Every stored configuration in creation order — the order
    /// `ListNotificationConfigurations` reports and pages through.
    pub fn configurations_in_creation_order(&self) -> Vec<&NotificationConfigurationRecord> {
        let mut records: Vec<&NotificationConfigurationRecord> =
            self.configurations.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// The `NotificationConfigurationStatus` of `configuration_arn`, DERIVED from
    /// the `EventRule`s the store holds for it.
    ///
    /// The model defines the three reportable values in terms of exactly that set
    /// — `ACTIVE` is "All of the EventRules are in ACTIVE Status",
    /// `PARTIALLY_ACTIVE` is "Some EventRules are in ACTIVE Status and some are
    /// INACTIVE", `INACTIVE` is "All of the EventRules are in INACTIVE Status" —
    /// so the status is a VIEW of the rules rather than a field an operation
    /// writes, and a configuration owning no rule at all has none of them ACTIVE
    /// and so reports `INACTIVE`. (The fourth value, `DELETING`, names a
    /// configuration on its way out, which this store removes outright.)
    ///
    /// A rule counts as ACTIVE when every Region it carries is
    /// ([`EventRuleRecord::is_active`]). Call
    /// [`Self::settle_due_event_rules`] first, or a rule the clock has already
    /// activated still counts as not ACTIVE.
    ///
    /// `INACTIVE` for an arn the store does not hold: a configuration that does
    /// not exist owns no ACTIVE rule either, and every caller establishes that it
    /// exists before reporting its status.
    pub fn configuration_status(&self, configuration_arn: &str) -> String {
        let mut rules = 0_usize;
        let mut active = 0_usize;
        for rule in self.event_rules.values() {
            if rule.notification_configuration_arn != configuration_arn {
                continue;
            }
            rules += 1;
            if rule.is_active() {
                active += 1;
            }
        }
        if active == 0 {
            CONFIGURATION_STATUS_INACTIVE.to_string()
        } else if active == rules {
            CONFIGURATION_STATUS_ACTIVE.to_string()
        } else {
            CONFIGURATION_STATUS_PARTIALLY_ACTIVE.to_string()
        }
    }

    /// Every stored configuration in creation order, each beside the status
    /// [`Self::configuration_status`] derives for it — what
    /// `ListNotificationConfigurations` filters, pages and reports over, computed
    /// once per listing rather than per filter test.
    pub fn configurations_with_status(&self) -> Vec<(&NotificationConfigurationRecord, String)> {
        self.configurations_in_creation_order()
            .into_iter()
            .map(|record| {
                let status = self.configuration_status(&record.arn);
                (record, status)
            })
            .collect()
    }

    /// Associate `channel_arn` with `configuration_arn`, and report whether that
    /// changed the stored record.
    ///
    /// `AssociateChannel` is `@idempotent`: a repeat of an association the
    /// record already holds keeps the sequence the first one was given and
    /// reports `false`, so neither the listing's order nor the mint counter
    /// moves. `false` is also the answer when no configuration carries
    /// `configuration_arn` — the caller establishes that first and refuses with
    /// the modelled not-found error.
    pub fn associate_channel(&mut self, configuration_arn: &str, channel_arn: &str) -> bool {
        let sequence = self.channel_associations_minted + 1;
        let Some(record) = self.configurations.get_mut(configuration_arn) else {
            return false;
        };
        if record.channel_arns.contains_key(channel_arn) {
            return false;
        }
        record
            .channel_arns
            .insert(channel_arn.to_string(), sequence);
        self.channel_associations_minted = sequence;
        true
    }

    /// Remove the association between `channel_arn` and `configuration_arn`, and
    /// report whether the record held one. `false` leaves the store untouched,
    /// which is what lets the caller refuse a disassociation of a channel this
    /// configuration is not associated with.
    pub fn disassociate_channel(&mut self, configuration_arn: &str, channel_arn: &str) -> bool {
        self.configurations
            .get_mut(configuration_arn)
            .is_some_and(|record| record.channel_arns.remove(channel_arn).is_some())
    }

    /// The channels associated with `configuration_arn` in association order,
    /// each with its association sequence — the order `ListChannels` reports and
    /// pages through. Empty for a configuration the store does not hold, which
    /// the caller refuses before it gets here.
    pub fn channels_in_association_order(&self, configuration_arn: &str) -> Vec<(&str, u64)> {
        let Some(record) = self.configurations.get(configuration_arn) else {
            return Vec::new();
        };
        let mut channels: Vec<(&str, u64)> = record
            .channel_arns
            .iter()
            .map(|(arn, sequence)| (arn.as_str(), *sequence))
            .collect();
        channels.sort_by_key(|(_, sequence)| *sequence);
        channels
    }

    /// The `EventRule`s of `configuration_arn` in creation order — the order
    /// `ListEventRules` reverses into "reverse chronological order (newest
    /// first)" and pages through. Empty for a configuration the store does not
    /// hold, which the caller refuses before it gets here.
    pub fn event_rules_in_creation_order(&self, configuration_arn: &str) -> Vec<&EventRuleRecord> {
        let mut rules: Vec<&EventRuleRecord> = self
            .event_rules
            .values()
            .filter(|rule| rule.notification_configuration_arn == configuration_arn)
            .collect();
        rules.sort_by_key(|rule| rule.sequence);
        rules
    }

    /// How many `EventRule`s one configuration holds.
    pub fn event_rule_count(&self, configuration_arn: &str) -> usize {
        self.event_rules
            .values()
            .filter(|rule| rule.notification_configuration_arn == configuration_arn)
            .count()
    }

    /// Whether another `EventRule` fits under
    /// [`MAX_EVENT_RULES_PER_CONFIGURATION`] for this configuration. A create
    /// that cannot must be refused with the modelled quota error, not served by
    /// growing the store.
    pub fn has_event_rule_capacity(&self, configuration_arn: &str) -> bool {
        self.event_rule_count(configuration_arn) < MAX_EVENT_RULES_PER_CONFIGURATION
    }

    /// The `EventRule` of `configuration_arn` that already covers the
    /// `(source, eventType)` pair a create names, if the store holds one.
    ///
    /// One rule per pair per configuration is what identifies a rule: that pair
    /// is what the rule matches events on, so a configuration never holds two
    /// rules for the same one. A create naming a pair a rule already covers is
    /// therefore asking for a rule that exists, which the caller refuses with
    /// the modelled conflict error rather than minting a second rule or
    /// reporting the first; `eventPattern` and `regions` are what such a rule
    /// CARRIES, not what names it, so they are no part of this lookup. The
    /// caller holds this store's lock across the lookup and the insert that
    /// follows it, so two creates for one pair cannot both pass the check.
    pub fn event_rule_covering(
        &self,
        configuration_arn: &str,
        source: &str,
        event_type: &str,
    ) -> Option<&EventRuleRecord> {
        self.event_rules.values().find(|rule| {
            rule.notification_configuration_arn == configuration_arn
                && rule.source == source
                && rule.event_type == event_type
        })
    }

    /// Mint a fresh `EventRule` arn under `configuration_arn` and reserve its
    /// creation sequence.
    ///
    /// `EventRuleArn` is the configuration's own arn followed by
    /// `/rule/[a-z0-9]{27}`, so the rule's arn is built from its parent's rather
    /// than assembled again, and the id segment is minted the same way a
    /// configuration's is (see [`mint_arn_id`]).
    pub fn mint_event_rule_arn(&mut self, configuration_arn: &str) -> (String, u64) {
        self.event_rules_minted += 1;
        let sequence = self.event_rules_minted;
        let id = mint_arn_id(sequence);
        (format!("{configuration_arn}/rule/{id}"), sequence)
    }

    /// Insert an `EventRule` for `configuration_arn` with every Region in
    /// [`EVENT_RULE_STATUS_CREATING`] and its settling due
    /// [`EVENT_RULE_TRANSITION_SPAN`] after `now`, and return its arn.
    ///
    /// The caller has already established that the configuration exists, that
    /// the store has capacity for another rule, and that `regions` is within
    /// [`MAX_REGIONS_PER_EVENT_RULE`]; a longer list is truncated here rather
    /// than trusted, so no allocation is sized by an unbounded request field.
    pub fn insert_event_rule(
        &mut self,
        configuration_arn: &str,
        source: String,
        event_type: String,
        event_pattern: Option<String>,
        regions: &[String],
        now: SystemTime,
    ) -> String {
        let ordered = distinct_regions(regions);
        let creating = crate::ops::types::EventRuleStatusSummary {
            status: EVENT_RULE_STATUS_CREATING.to_string(),
            reason: EVENT_RULE_REASON_CREATING.to_string(),
        };
        let status_by_region = ordered
            .iter()
            .map(|region| (region.clone(), creating.clone()))
            .collect();

        let (arn, sequence) = self.mint_event_rule_arn(configuration_arn);
        let record = EventRuleRecord {
            arn: arn.clone(),
            notification_configuration_arn: configuration_arn.to_string(),
            source,
            event_type,
            event_pattern,
            regions: ordered,
            creation_time: ::aws_smithy_types::DateTime::from(now),
            status_by_region,
            due_at: Some(now + EVENT_RULE_TRANSITION_SPAN),
            sequence,
        };
        self.event_rules.insert(arn.clone(), record);
        self.refresh_event_rule_sources(configuration_arn);
        arn
    }

    /// Apply an `UpdateEventRule` request to the stored rule `arn` and report
    /// the record it leaves, or `None` when the store holds no rule under that
    /// arn.
    ///
    /// An absent member is "leave this as it is", not "clear it": only the
    /// members the request carries are written, and `source`, `eventType`,
    /// `creationTime` and the configuration the rule belongs to are not members
    /// of this operation at all, so they keep the values the record holds. A
    /// request that asks for what the rule already says changes nothing, which
    /// is what the operation's `@idempotent` trait requires; one that does
    /// change the rule moves every Region it now covers into the transitional
    /// [`EVENT_RULE_STATUS_UPDATING`] the model documents for an update, with
    /// its settling due [`EVENT_RULE_TRANSITION_SPAN`] after `now` — the clock,
    /// not a later read, makes it `ACTIVE`.
    ///
    /// The caller has already established that `regions` is within
    /// [`MAX_REGIONS_PER_EVENT_RULE`]; a longer list is truncated by
    /// [`distinct_regions`] rather than trusted, so no allocation is sized by an
    /// unbounded request field.
    pub fn update_event_rule(
        &mut self,
        arn: &str,
        event_pattern: Option<String>,
        regions: Option<&[String]>,
        now: SystemTime,
    ) -> Option<&EventRuleRecord> {
        let rule = self.event_rules.get_mut(arn)?;
        let mut changed = false;

        if let Some(event_pattern) = event_pattern {
            if rule.event_pattern.as_deref() != Some(event_pattern.as_str()) {
                rule.event_pattern = Some(event_pattern);
                changed = true;
            }
        }
        if let Some(regions) = regions {
            let ordered = distinct_regions(regions);
            if ordered != rule.regions {
                rule.regions = ordered;
                changed = true;
            }
        }

        if changed {
            let updating = crate::ops::types::EventRuleStatusSummary {
                status: EVENT_RULE_STATUS_UPDATING.to_string(),
                reason: EVENT_RULE_REASON_UPDATING.to_string(),
            };
            // One entry per Region the rule now covers: a Region the update
            // dropped is no longer sending events to this rule, so it has no
            // status to report, and a Region it added does.
            rule.status_by_region = rule
                .regions
                .iter()
                .map(|region| (region.clone(), updating.clone()))
                .collect();
            rule.due_at = Some(now + EVENT_RULE_TRANSITION_SPAN);
        }

        Some(rule)
    }

    /// Drop every `EventRule` of `configuration_arn` — what deleting the
    /// configuration does to the rules associated with it, so a later create
    /// cannot reach a rule whose configuration is gone.
    pub fn remove_event_rules_of_configuration(&mut self, configuration_arn: &str) {
        self.event_rules
            .retain(|_, rule| rule.notification_configuration_arn != configuration_arn);
    }

    /// Recompute the `event_rule_sources` index on a configuration record from
    /// the `EventRule`s the store holds for it.
    ///
    /// That index is what `ListNotificationConfigurations`'s `eventRuleSource`
    /// filter matches on; deriving it here, from the rules themselves, keeps it
    /// a view of one stored set rather than a second copy of it. Call it
    /// wherever the set of a configuration's rules changes.
    pub fn refresh_event_rule_sources(&mut self, configuration_arn: &str) {
        let sources: BTreeSet<String> = self
            .event_rules
            .values()
            .filter(|rule| rule.notification_configuration_arn == configuration_arn)
            .map(|rule| rule.source.clone())
            .collect();
        if let Some(configuration) = self.configurations.get_mut(configuration_arn) {
            configuration.event_rule_sources = sources;
        }
    }

    /// Materialise every `EventRule` transition the clock has already made due,
    /// as of `now`: a Region in a transitional status (`CREATING` after a
    /// create, `UPDATING` after an update) becomes `ACTIVE`.
    ///
    /// Every EventRule operation calls this first, including the reporting ones:
    /// the step was owed at that instant whether or not anybody called, so
    /// applying it is not a mutation this request chose. Nothing here sleeps or
    /// depends on a request's shape.
    pub fn settle_due_event_rules(&mut self, now: SystemTime) {
        for rule in self.event_rules.values_mut() {
            let Some(due_at) = rule.due_at else {
                continue;
            };
            if due_at > now {
                continue;
            }
            rule.due_at = None;
            for summary in rule.status_by_region.values_mut() {
                if summary.status == EVENT_RULE_STATUS_CREATING
                    || summary.status == EVENT_RULE_STATUS_UPDATING
                {
                    summary.status = EVENT_RULE_STATUS_ACTIVE.to_string();
                    summary.reason = EVENT_RULE_REASON_ACTIVE.to_string();
                }
            }
        }
    }

    /// Materialise every hub transition the clock has already made due, as of
    /// `now`: a `REGISTERING` hub becomes `ACTIVE` (stamped with the instant its
    /// activation fell due), and a `DEREGISTERING` hub leaves the store.
    ///
    /// Every hub operation calls this first, including the reporting ones: the
    /// step was owed at that instant whether or not anybody called, so applying
    /// it is not a mutation this request chose. Nothing here sleeps or depends on
    /// a request's shape.
    pub fn settle_due_hubs(&mut self, now: SystemTime) {
        self.hubs.retain(|_, hub| {
            let Some(due_at) = hub.due_at else {
                return true;
            };
            if due_at > now {
                return true;
            }
            hub.due_at = None;
            match hub.status.as_str() {
                NOTIFICATION_HUB_STATUS_REGISTERING => {
                    hub.status = NOTIFICATION_HUB_STATUS_ACTIVE.to_string();
                    hub.reason = HUB_REASON_ACTIVE.to_string();
                    hub.last_activation_time = Some(::aws_smithy_types::DateTime::from(due_at));
                    true
                }
                // The deregister this hub was holding has fallen due, so the
                // record is gone: a listing stops reporting it.
                NOTIFICATION_HUB_STATUS_DEREGISTERING => false,
                // A settled status owes nothing.
                _ => true,
            }
        });
    }

    /// Whether another hub fits under [`MAX_NOTIFICATION_HUBS`]. A register that
    /// cannot must be refused with the modelled quota error, not served by
    /// growing the store. Call [`Self::settle_due_hubs`] first, or a hub the
    /// clock has already removed still counts against the cap.
    pub fn has_hub_capacity(&self) -> bool {
        self.hubs.len() < MAX_NOTIFICATION_HUBS
    }

    /// Insert a hub for `region` in `REGISTERING`, with its activation due
    /// [`HUB_TRANSITION_SPAN`] after `now`, and return it.
    pub fn register_hub(&mut self, region: &str, now: SystemTime) -> &NotificationHubRecord {
        self.hubs_registered += 1;
        let record = NotificationHubRecord {
            region: region.to_string(),
            status: NOTIFICATION_HUB_STATUS_REGISTERING.to_string(),
            reason: HUB_REASON_REGISTERING.to_string(),
            creation_time: ::aws_smithy_types::DateTime::from(now),
            // A hub has never been active at the instant it is registered; the
            // clock stamps this when `REGISTERING` falls due.
            last_activation_time: None,
            due_at: Some(now + HUB_TRANSITION_SPAN),
            sequence: self.hubs_registered,
        };
        self.hubs.entry(region.to_string()).or_insert(record)
    }

    /// Every stored hub in registration order — the order `ListNotificationHubs`
    /// reports and pages through.
    pub fn hubs_in_registration_order(&self) -> Vec<&NotificationHubRecord> {
        let mut records: Vec<&NotificationHubRecord> = self.hubs.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// Materialise the organization-access transition the clock has already made
    /// due as of `now`: trust that was asked for becomes `ENABLED` once its
    /// [`ORGANIZATION_ACCESS_TRANSITION_SPAN`] has elapsed.
    ///
    /// Every organization-access operation calls this first, including the
    /// reporting one: the step was owed at that instant whether or not anybody
    /// called, so applying it is not a mutation this request chose. Nothing here
    /// sleeps or depends on a request's shape.
    pub fn settle_due_organization_access(&mut self, now: SystemTime) {
        let Some(due_at) = self.organization_access.due_at else {
            return;
        };
        if due_at > now {
            return;
        }
        self.organization_access.due_at = None;
        if self.organization_access.access_status == ORGANIZATION_ACCESS_PENDING {
            self.organization_access.access_status = ORGANIZATION_ACCESS_ENABLED.to_string();
        }
    }

    /// Ask for service trust between User Notifications and Organizations, and
    /// return the stored record.
    ///
    /// The enable is `@idempotent`: an account whose trust is already in force,
    /// or already on its way there, keeps the record it has — so repeating the
    /// call neither restarts the transition nor moves a settled account back to
    /// `PENDING`. Call [`Self::settle_due_organization_access`] first, or an
    /// enablement the clock has already completed still looks pending.
    pub fn enable_organization_access(&mut self, now: SystemTime) -> &OrganizationAccessRecord {
        if self.organization_access.access_status == ORGANIZATION_ACCESS_DISABLED {
            self.organization_access = OrganizationAccessRecord {
                access_status: ORGANIZATION_ACCESS_PENDING.to_string(),
                due_at: Some(now + ORGANIZATION_ACCESS_TRANSITION_SPAN),
            };
        }
        &self.organization_access
    }

    /// Withdraw service trust, and return the stored record.
    ///
    /// `DISABLED` is immediate and terminal — `AccessStatus` models no
    /// transitional value on the way out — and it also cancels an enablement
    /// the clock has not yet made due, so a disable can never be overtaken by
    /// the enable it followed.
    pub fn disable_organization_access(&mut self) -> &OrganizationAccessRecord {
        self.organization_access = OrganizationAccessRecord::default();
        &self.organization_access
    }
}

/// One id segment of an arn this store mints: [`ARN_ID_LEN`] characters drawn
/// from [`ARN_ID_ALPHABET`], derived from a SHA-256 over a per-process random
/// seed, the minting counter the caller passes and the current instant — so two
/// mints never collide, not even for identical input, and the id is not a
/// function of the request.
fn mint_arn_id(sequence: u64) -> String {
    let mut digest = Sha256::new();
    digest.update(process_seed().to_le_bytes());
    digest.update(sequence.to_le_bytes());
    digest.update(nanos_since_epoch().to_le_bytes());
    let bytes = digest.finalize();

    bytes
        .iter()
        .take(ARN_ID_LEN)
        .map(|byte| {
            let index = usize::from(*byte) % ARN_ID_ALPHABET.len();
            char::from(ARN_ID_ALPHABET[index])
        })
        .collect()
}

/// A random value fixed for the life of the process, so configuration ids are
/// unpredictable as well as unique. Drawn from the standard library's
/// randomly-seeded hasher, which needs no extra dependency.
fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(random_u64)
}

/// One random `u64` from the standard library's randomly-seeded hasher. Each
/// `RandomState` draws its own keys, so two calls yield independent values.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_usize(std::process::id() as usize);
    hasher.finish()
}

/// Nanoseconds since the Unix epoch — the time component of a minted id.
fn nanos_since_epoch() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

/// The tag every pagination token this process issues carries, so a token the
/// emulator never minted is refused rather than resuming someone's listing.
/// Process-scoped and random, so verifying a token needs no write on a read
/// path.
///
/// Seeded INDEPENDENTLY of [`process_seed`]: a token carries this tag in the
/// clear, so sharing the seed that mints configuration ids would hand anyone who
/// paged a listing the id seed as well.
pub fn page_token_tag() -> u64 {
    static TAG: OnceLock<u64> = OnceLock::new();
    *TAG.get_or_init(random_u64)
}

/// Encode the resume point of a listing page as a `nextToken`.
///
/// The token is this process's [`page_token_tag`] followed by the cursor the
/// listing resumes from (the creation or registration sequence of the last record
/// on the page). Rendered through the `hex` codec so the token is an opaque
/// fixed-width string of word characters, within the modelled `NextToken` length
/// (1..4096) and pattern (`^[\w+-/=]+$`). The tag is what makes a
/// forged-but-well-formed cursor distinguishable from one this emulator issued.
///
/// Shared by every paginated listing in this service, so one token format is
/// minted and verified in one place.
pub fn encode_page_token(cursor: u64) -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&page_token_tag().to_be_bytes());
    bytes[8..].copy_from_slice(&cursor.to_be_bytes());
    hex::encode(bytes)
}

/// Recover the cursor from a `nextToken` [`encode_page_token`] issued. `None` for
/// a token this process did not issue — wrong length, not hex, or carrying
/// another tag — which a handler refuses as a validation failure rather than
/// silently restarting the listing from the beginning.
pub fn decode_page_token(token: &str) -> Option<u64> {
    let bytes: [u8; 16] = hex::decode(token).ok()?.try_into().ok()?;
    let tag = u64::from_be_bytes(bytes[..8].try_into().ok()?);
    if tag != page_token_tag() {
        return None;
    }
    Some(u64::from_be_bytes(bytes[8..].try_into().ok()?))
}

/// The wall-clock instant a record is stamped with, as the modelled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(std::time::SystemTime::now())
}

/// `notification hub`, the resource label [`not_found_message`] names a
/// `NotificationHub` by.
pub const HUB_RESOURCE_LABEL: &str = "notification hub";

/// `region`, the identifier label [`not_found_message`] names a hub's
/// `notificationHubRegion` by.
pub const HUB_IDENTIFIER_LABEL: &str = "region";

/// `notification configuration`, the resource label [`not_found_message`] names a
/// `NotificationConfiguration` by.
pub const CONFIGURATION_RESOURCE_LABEL: &str = "notification configuration";

/// `event rule`, the resource label [`not_found_message`] names an `EventRule` by.
pub const EVENT_RULE_RESOURCE_LABEL: &str = "event rule";

/// `channel`, the resource label [`not_found_message`] names a `Channel` by —
/// the resource a channel operation looks up in the configuration it names.
pub const CHANNEL_RESOURCE_LABEL: &str = "channel";

/// `managed notification configuration`, the resource label
/// [`not_found_message`] names a `ManagedNotificationConfiguration` by.
pub const MANAGED_CONFIGURATION_RESOURCE_LABEL: &str = "managed notification configuration";

/// `arn`, the identifier label [`not_found_message`] names an arn-keyed resource
/// by — the `arn` member every one of those operations takes.
pub const ARN_IDENTIFIER_LABEL: &str = "arn";

/// How this service states a reference to a resource that does not exist: the
/// `message` of a `ResourceNotFoundException` (HTTP 404).
///
/// ONE template for every not-found path of this service, built from the request:
/// the kind of resource that was looked up, the member that identified it, and
/// the value the request carried there. The recorded refusal of a deregister
/// naming a Region no hub is registered in reads `notification hub with region
/// <notificationHubRegion> not found`, so the resource and the identifier are
/// named in lower case prose and the value is quoted bare.
///
/// The identifier is named by the member's own noun with the resource's name
/// dropped from it, which is what that recording shows: `notificationHubRegion`
/// is stated as `region` beside `notification hub`. By the same reading
/// `notificationConfigurationArn` is stated as `arn` beside `notification
/// configuration`, so an operation naming a configuration in `arn` and one
/// naming it in `notificationConfigurationArn` state the same sentence about the
/// same missing resource.
///
/// Spelled once so the whole family reads the same way rather than one sentence
/// per handler; the labels are the constants above.
pub fn not_found_message(resource: &str, identifier: &str, value: &str) -> String {
    format!("{resource} with {identifier} {value} not found")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNotificationsNotificationsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNotificationsNotificationsState>)
}
