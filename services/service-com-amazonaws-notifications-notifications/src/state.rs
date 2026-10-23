// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-notifications-notifications service.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// `ACTIVE`, the status a freshly created `NotificationConfiguration` carries.
///
/// One of `NotificationConfigurationStatus`'s four modelled values
/// (`crate::ops::types::NOTIFICATION_CONFIGURATION_STATUS_VALUES`). It is a
/// terminal status — the model has no `CREATING` — so a configuration this
/// emulator mints is readable as `ACTIVE` immediately and stays there until a
/// caller changes it.
pub const CONFIGURATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `ACCOUNT`, the subtype of a configuration an account created for itself, as
/// opposed to `ADMIN_MANAGED` (one an organization's management account owns).
/// Both are modelled values of `NotificationConfigurationSubtype`
/// (`crate::ops::types::NOTIFICATION_CONFIGURATION_SUBTYPE_VALUES`).
pub const CONFIGURATION_SUBTYPE_ACCOUNT: &str = "ACCOUNT";

/// The alphabet and width of the id segment of a configuration arn, from the
/// `NotificationConfigurationArn` pattern the model declares:
/// `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$`.
const CONFIGURATION_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const CONFIGURATION_ID_LEN: usize = 27;

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
    /// `status`, one of `NotificationConfigurationStatus`'s values.
    pub status: String,
    /// `subtype`, one of `NotificationConfigurationSubtype`'s values.
    pub subtype: Option<String>,
    /// `creationTime`, set once when the record is inserted and never rewritten.
    pub creation_time: ::aws_smithy_types::DateTime,
    /// `tags`, as the create call supplied them (empty when it supplied none).
    pub tags: BTreeMap<String, String>,
    /// The sources of the EventRules this configuration carries — what
    /// `ListNotificationConfigurations`'s `eventRuleSource` filter matches a
    /// configuration on. Empty on every record today: `CreateEventRule` is
    /// still a 501 stub, and it is the operation that will write here.
    pub event_rule_sources: BTreeSet<String>,
    /// The arns of the channels associated with this configuration — what
    /// `ListNotificationConfigurations`'s `channelArn` filter matches a
    /// configuration on. Empty on every record today: `AssociateChannel` is
    /// still a 501 stub, and it is the operation that will write here.
    pub channel_arns: BTreeSet<String>,
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

/// Per-service state for com-amazonaws-notifications-notifications. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNotificationsNotificationsState {
    /// arn -> configuration.
    pub configurations: BTreeMap<String, NotificationConfigurationRecord>,
    /// How many configurations this store has ever minted. Doubles as the next
    /// record's [`NotificationConfigurationRecord::sequence`] and as part of the
    /// uniqueness seed of a new arn, so an id is never reused after a delete.
    pub configurations_minted: u64,
    /// notificationHubRegion -> hub. One entry per region, as AWS's "maximum of
    /// one NotificationHub per Region" requires.
    pub hubs: BTreeMap<String, NotificationHubRecord>,
    /// How many hubs this store has ever registered. Doubles as the next
    /// record's [`NotificationHubRecord::sequence`], so a listing's order and its
    /// page tokens survive a deregister and a re-register of the same region.
    pub hubs_registered: u64,
    /// Service Trust Enablement between User Notifications and Amazon Web
    /// Services Organizations for this account. One record per account, holding
    /// `DISABLED` until an enable asks for trust.
    pub organization_access: OrganizationAccessRecord,
}

impl ComAmazonawsNotificationsNotificationsState {
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

        let mut digest = Sha256::new();
        digest.update(process_seed().to_le_bytes());
        digest.update(sequence.to_le_bytes());
        digest.update(nanos_since_epoch().to_le_bytes());
        let bytes = digest.finalize();

        let id: String = bytes
            .iter()
            .take(CONFIGURATION_ID_LEN)
            .map(|byte| {
                let index = usize::from(*byte) % CONFIGURATION_ID_ALPHABET.len();
                char::from(CONFIGURATION_ID_ALPHABET[index])
            })
            .collect();

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
