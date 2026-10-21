// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-notifications-notifications service.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

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

/// Per-service state for com-amazonaws-notifications-notifications. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNotificationsNotificationsState {
    /// arn -> configuration.
    pub configurations: BTreeMap<String, NotificationConfigurationRecord>,
    /// How many configurations this store has ever minted. Doubles as the next
    /// record's [`NotificationConfigurationRecord::sequence`] and as part of the
    /// uniqueness seed of a new arn, so an id is never reused after a delete.
    pub configurations_minted: u64,
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
