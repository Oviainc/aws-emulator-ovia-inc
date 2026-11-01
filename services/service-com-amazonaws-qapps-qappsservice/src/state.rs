// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-qapps-qappsservice service, plus the small
//! helpers every Q Apps handler needs (id minting, ARNs, the caller principal,
//! pagination tokens and the card/definition conversions).
//!
//! The helpers live here rather than in one operation's file because several
//! operations read and write the SAME records: `src/ops/<op>.rs` owns an
//! operation's behaviour, and this module owns the store those operations share.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AppDefinition, AppDefinitionInput, Card, CardInput, CardStatus, CardValue, Category,
    FileUploadCard, FormInputCard, QPluginCard, QQueryCard, SessionSharingConfiguration,
    TextInputCard,
};

/// The version of the app-definition schema this emulator renders
/// `AppDefinition.appDefinitionVersion` with. The member is documented as "the
/// version of the app definition schema or specification" — a property of the
/// shape of the definition, not of the app's own `appVersion`, so it does not
/// move when an app is updated.
pub const APP_DEFINITION_VERSION: &str = "1.0";

/// The `AppStatus` of a stored Q App. The wire spellings are the model's
/// `AppStatus` enum values (`DELETED | DRAFT | PUBLISHED`); see
/// `crate::ops::types::APP_STATUS_VALUES`, which this crate's tests pin these
/// against. `Deleted` is the model's value for an app that is gone: this
/// emulator removes the record on `DeleteQApp` rather than keeping it in that
/// status, so no stored app carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppStatus {
    Draft,
    Published,
    Deleted,
}

impl AppStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Published => "PUBLISHED",
            Self::Deleted => "DELETED",
        }
    }
}

/// The `ExecutionStatus` of a session or of one card in it. The wire spellings
/// are the model's `ExecutionStatus` enum values (`COMPLETED | ERROR |
/// IN_PROGRESS | WAITING`); see `crate::ops::types::EXECUTION_STATUS_VALUES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Completed,
    Error,
    InProgress,
    Waiting,
}

impl ExecutionStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Completed => "COMPLETED",
            Self::Error => "ERROR",
            Self::InProgress => "IN_PROGRESS",
            Self::Waiting => "WAITING",
        }
    }
}

/// One persisted Q App, as `CreateQApp` minted it and `UpdateQApp` revises it.
#[derive(Debug, Clone)]
pub struct QApp {
    pub app_id: String,
    pub app_arn: String,
    pub title: String,
    pub description: Option<String>,
    pub initial_prompt: Option<String>,
    pub app_version: i32,
    pub status: AppStatus,
    pub created_at: DateTime,
    pub created_by: String,
    pub updated_at: DateTime,
    pub updated_by: String,
    pub required_capabilities: Vec<String>,
    /// The definition at `app_version`, with the cards exactly as submitted
    /// (plus the dependencies `CreateQApp`/`UpdateQApp` computed). Shared with
    /// the entry for `app_version` in `versions` rather than copied.
    pub app_definition: Arc<AppDefinition>,
    /// Every retained version of the definition, keyed by the `appVersion` it
    /// belongs to, so `GetQApp(appVersion=N)` can answer the definition of
    /// version N. Versions that share a definition share ONE copy of it, so an
    /// `UpdateQApp` that does not supply a definition costs a pointer.
    pub versions: BTreeMap<i32, Arc<AppDefinition>>,
    pub tags: HashMap<String, String>,
}

/// One persisted Q App session, as `StartQAppSession` seeded it.
#[derive(Debug, Clone)]
pub struct QAppSession {
    pub session_id: String,
    pub session_arn: String,
    pub session_name: Option<String>,
    pub app_id: String,
    pub app_version: i32,
    pub status: ExecutionStatus,
    /// Keyed by the card ids of the app definition this session runs.
    pub card_status: HashMap<String, CardStatus>,
    pub card_values: Vec<CardValue>,
    pub sharing_configuration: SessionSharingConfiguration,
    pub owner: String,
    pub tags: HashMap<String, String>,
    pub created_at: DateTime,
}

/// The `LibraryItemStatus` of a published library item. The wire spellings are
/// the model's `LibraryItemStatus` enum values (`PUBLISHED | DISABLED`); see
/// `crate::ops::types::LIBRARY_ITEM_STATUS_VALUES`, which this crate's tests pin
/// these against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryItemStatus {
    Published,
    Disabled,
}

impl LibraryItemStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Published => "PUBLISHED",
            Self::Disabled => "DISABLED",
        }
    }
}

/// One category of an instance's library catalog — the records
/// `BatchCreateCategory` will mint and `ListCategories` page once those
/// scaffolds are implemented.
///
/// A library item names the categories it is filed under BY ID, and the model's
/// `Category` carries a required `title`, so the catalog is the only place a
/// category's title and colour live. `appCount` is NOT stored: it is how many
/// published library items are filed under the category, which
/// [`ComAmazonawsQappsQappsserviceState::item_categories`] derives from the
/// items themselves.
#[derive(Debug, Clone)]
pub struct LibraryCategory {
    pub id: String,
    pub title: String,
    pub color: Option<String>,
}

/// One published library item, as `CreateLibraryItem` minted it: the snapshot of
/// a Q App that the instance's library offers, plus the reviews its users have
/// associated with it.
#[derive(Debug, Clone)]
pub struct LibraryItem {
    pub library_item_id: String,
    pub app_id: String,
    pub app_version: i32,
    /// The ids of the categories this item is filed under, in the order the
    /// publisher named them. Every id is a category of the SAME instance's
    /// catalog: `CreateLibraryItem` refuses one the catalog does not hold.
    pub categories: Vec<String>,
    pub status: LibraryItemStatus,
    pub created_at: DateTime,
    pub created_by: String,
    pub updated_at: Option<DateTime>,
    pub updated_by: Option<String>,
    /// The principals that have a review associated with this item — one entry
    /// per user, because a review is "associated with the user submitting the
    /// request". `ratingCount` is this set's SIZE, so a user who submits twice
    /// is still counted once.
    ///
    /// Both dimensions of this set are BOUNDED, because both are caller-driven:
    /// at most [`MAX_REVIEWS_PER_LIBRARY_ITEM`] entries, each at most
    /// [`MAX_PRINCIPAL_LENGTH`] bytes ([`is_storable_principal`] refuses a
    /// longer one), so repeated requests cannot grow the store without limit.
    pub raters: BTreeSet<String>,
    pub is_verified: bool,
}

impl LibraryItem {
    /// The `ratingCount` this item reports: how many distinct users have a
    /// review associated with it.
    pub fn rating_count(&self) -> i32 {
        i32::try_from(self.raters.len()).unwrap_or(i32::MAX)
    }

    /// Whether `principal` has a review associated with this item. That is
    /// `isRatedByUser` — a statement about the CALLER, so it is decided per read
    /// rather than stored.
    pub fn is_rated_by(&self, principal: &str) -> bool {
        self.raters.contains(principal)
    }
}

/// Per-service state for com-amazonaws-qapps-qappsservice. A handler reaches it through [`service_state`].
///
/// Every map is keyed by the `instance-id` request header first: an Amazon Q
/// Business application environment instance is the tenant boundary of this
/// API, and every operation takes that header as required input.
#[derive(Debug, Default)]
pub struct ComAmazonawsQappsQappsserviceState {
    /// `(instanceId, appId) -> app`.
    pub apps: HashMap<(String, String), QApp>,
    /// Creation order of `(instanceId, appId)`, so `ListQApps` pages
    /// deterministically.
    pub app_order: Vec<(String, String)>,
    /// `(instanceId, sessionId) -> session`.
    pub sessions: HashMap<(String, String), QAppSession>,
    /// `(instanceId, appId) -> sessionIds`, so deleting an app can drop the
    /// sessions that ran it.
    pub sessions_by_app: HashMap<(String, String), Vec<String>>,
    /// `(instanceId, libraryItemId) -> library item`.
    pub library_items: HashMap<(String, String), LibraryItem>,
    /// `(instanceId, categoryId) -> category`: the instance's library catalog,
    /// which `BatchCreateCategory` will fill and `BatchDeleteCategory` empty. A
    /// library item is filed under categories FROM this catalog.
    ///
    /// Nothing in this crate writes it yet (both of those operations are still
    /// scaffolds), so no request can grow it; [`MAX_CATEGORIES_PER_INSTANCE`]
    /// and [`MAX_CATEGORIES_TOTAL`] are the bounds the implementation that fills
    /// it charges, both key components being caller-chosen.
    pub library_categories: HashMap<(String, String), LibraryCategory>,
}

/// How many Q Apps one instance may hold at once. A guardrail on this
/// emulator's memory, answered with the `ServiceQuotaExceededException`
/// CreateQApp models (HTTP 402) so a client sees a quota refusal rather than an
/// emulator that grows without limit. Charged against every entry the store
/// holds for that instance (a delete removes the entry, so it frees room).
pub const MAX_APPS_PER_INSTANCE: usize = 250;

/// How many Q Apps this emulator holds at once across EVERY instance. The
/// model's `InstanceId` is an unconstrained string, so the number of distinct
/// instances a caller can name is unbounded and the per-instance guardrail
/// alone would not bound `apps`/`app_order`: this total is the bound that does,
/// answered with the same quota refusal.
pub const MAX_APPS_TOTAL: usize = 1_000;

/// How many sessions one Q App may hold at once. Same guardrail, answered with
/// the `ServiceQuotaExceededException` StartQAppSession models (HTTP 402).
pub const MAX_SESSIONS_PER_APP: usize = 250;

/// How many sessions this emulator holds at once across every instance and
/// app — the bound on `sessions`/`sessions_by_app`, for the same reason
/// [`MAX_APPS_TOTAL`] bounds the apps: both key components are caller-chosen.
pub const MAX_SESSIONS_TOTAL: usize = 1_000;

/// The page size `ListQApps` uses when the request names no `limit`. The
/// model's `limit` is `range(min: 1, max: 100)`, so this is the largest page a
/// caller could ask for: one call never returns more apps than the API lets one
/// ask for, and the rest are reached with the `nextToken`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// How many definition versions one Q App retains. `UpdateQApp` mints a new
/// version on every call without creating a new resource, so the oldest
/// retained definition is dropped past this many; `GetQApp` answers
/// `ResourceNotFoundException` for a version no longer retained, exactly as it
/// does for one that never existed.
///
/// With [`MAX_APPS_TOTAL`] this is what bounds the BYTES the store holds for
/// definitions: at most `MAX_APPS_TOTAL * MAX_RETAINED_VERSIONS` definitions,
/// each itself bounded by the model (`CardList` length max 20, `Prompt` length
/// max 50000), and versions that share a definition hold one copy of it.
pub const MAX_RETAINED_VERSIONS: usize = 10;

/// How many library items one instance's library may hold at once. The same
/// guardrail on this emulator's memory as [`MAX_APPS_PER_INSTANCE`], answered
/// with the `ServiceQuotaExceededException` CreateLibraryItem models (HTTP 402).
pub const MAX_LIBRARY_ITEMS_PER_INSTANCE: usize = 250;

/// How many library items this emulator holds at once across every instance.
/// `InstanceId` is an unconstrained string, so the per-instance guardrail alone
/// would not bound `library_items`: this total is what does, for the same reason
/// [`MAX_APPS_TOTAL`] bounds the apps.
pub const MAX_LIBRARY_ITEMS_TOTAL: usize = 1_000;

/// How many distinct users' reviews one library item retains.
///
/// A review's identity is the caller's principal, which nothing constrains, so
/// without this bound repeated `AssociateLibraryItemReview` calls under
/// different credentials would grow a library item's rater set without limit.
/// Charged under the same store lock as the insert and answered with the
/// `ServiceQuotaExceededException` the operation models (HTTP 402), so a client
/// sees a quota refusal rather than an emulator that grows.
pub const MAX_REVIEWS_PER_LIBRARY_ITEM: usize = 100;

/// How many tags one resource of this service holds at once.
///
/// "You can add up to 50 tags to each AWS resource" (AWS tagging limits, which
/// this API's `Tags` map does not restate): the model bounds each KEY (1..128) and
/// each VALUE (0..256) but not how many entries a caller may submit, so this is
/// what bounds the tags a record carries. Charged by `TagResource` against the
/// tags the resource would hold after the request, so repeated calls cannot grow a
/// record without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest principal this service stores against a record.
///
/// [`caller_principal`] reads the access key id out of the request's
/// `Authorization` header, which is not length-constrained; an AWS access key id
/// is at most 128 characters, so a longer value is not one. The bound is what
/// makes each entry of a rater set a fixed size rather than a header-sized one.
pub const MAX_PRINCIPAL_LENGTH: usize = 128;

/// How many categories one instance's library catalog may hold at once — the
/// per-instance bound on `library_categories`, whose records carry
/// caller-supplied titles and colours.
///
/// NOT charged by anything yet, and nothing in this crate writes the catalog
/// today: `BatchCreateCategory`, the operation that fills it, is still a
/// scaffold answering 501, so no request can grow the map. The bound is declared
/// here, beside the ones the apps, the sessions, the library items and the
/// reviews carry, so that implementation charges it against
/// [`ComAmazonawsQappsQappsserviceState::library_category_count`] under the same
/// store lock as its insert, answering with the `ServiceQuotaExceededException`
/// it models.
pub const MAX_CATEGORIES_PER_INSTANCE: usize = 100;

/// How many categories this emulator holds at once across every instance. The
/// catalog is keyed by `(instanceId, categoryId)` and `InstanceId` is an
/// unconstrained string, so this is the total that will bound
/// `library_categories`, for the same reason [`MAX_APPS_TOTAL`] bounds the apps.
/// To be charged against
/// [`ComAmazonawsQappsQappsserviceState::total_library_category_count`] by the
/// same `BatchCreateCategory` implementation; see
/// [`MAX_CATEGORIES_PER_INSTANCE`].
pub const MAX_CATEGORIES_TOTAL: usize = 1_000;

impl ComAmazonawsQappsQappsserviceState {
    /// The app `app_id` of `instance_id`, if the store holds one.
    ///
    /// `DeleteQApp` REMOVES the record outright — this emulator keeps no
    /// tombstone — so an entry in `apps` is a live app, and every count below
    /// charges exactly what the store holds.
    pub fn app(&self, instance_id: &str, app_id: &str) -> Option<&QApp> {
        self.apps
            .get(&(instance_id.to_string(), app_id.to_string()))
    }

    /// How many apps `instance_id` holds, for the quota check: every entry the
    /// store holds for that instance, so nothing escapes the quota.
    pub fn app_count(&self, instance_id: &str) -> usize {
        self.apps
            .keys()
            .filter(|(instance, _)| instance == instance_id)
            .count()
    }

    /// The tags held against the resource `resource` names, or `None` when this
    /// store holds no such resource.
    ///
    /// One reader for the whole tagging surface: `CreateQApp` and
    /// `StartQAppSession` store the tags their caller submitted on the record
    /// itself, so the tags of a resource are the record's and nothing keeps a
    /// second copy. WHERE a resource lives is [`TaggedResource::store_key`]'s
    /// answer, shared with the revising reader below so the two cannot drift: a
    /// session ARN names the app it runs, so a session id read under the WRONG app
    /// names no resource — the same answer as an id the store never held.
    pub fn resource_tags(&self, resource: &TaggedResource) -> Option<&HashMap<String, String>> {
        let key = resource.store_key();
        match resource {
            TaggedResource::App { .. } => self.apps.get(&key).map(|app| &app.tags),
            TaggedResource::Session { app_id, .. } => self
                .sessions
                .get(&key)
                .filter(|session| &session.app_id == app_id)
                .map(|session| &session.tags),
        }
    }

    /// The same tags, for a request that revises them (`TagResource`,
    /// `UntagResource`). `None` is the same "no such resource" answer
    /// [`Self::resource_tags`] gives, decided from the same key and the same
    /// session-to-app check.
    pub fn resource_tags_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut HashMap<String, String>> {
        let key = resource.store_key();
        match resource {
            TaggedResource::App { .. } => self.apps.get_mut(&key).map(|app| &mut app.tags),
            TaggedResource::Session { app_id, .. } => self
                .sessions
                .get_mut(&key)
                .filter(|session| &session.app_id == app_id)
                .map(|session| &mut session.tags),
        }
    }

    /// How many apps this emulator holds across every instance, for the total
    /// quota check: the whole map, which is also what `app_order` indexes.
    pub fn total_app_count(&self) -> usize {
        self.apps.len()
    }

    /// How many sessions this emulator holds across every instance, for the
    /// total quota check.
    pub fn total_session_count(&self) -> usize {
        self.sessions.len()
    }

    /// The library item `library_item_id` of `instance_id`, if the store holds
    /// one. The instance is part of the key, so an item published in one
    /// instance is invisible from another.
    pub fn library_item(&self, instance_id: &str, library_item_id: &str) -> Option<&LibraryItem> {
        self.library_items
            .get(&(instance_id.to_string(), library_item_id.to_string()))
    }

    /// The same record, for an operation that revises it (associating a review,
    /// say).
    pub fn library_item_mut(
        &mut self,
        instance_id: &str,
        library_item_id: &str,
    ) -> Option<&mut LibraryItem> {
        self.library_items
            .get_mut(&(instance_id.to_string(), library_item_id.to_string()))
    }

    /// How many library items `instance_id` holds, for the quota check.
    pub fn library_item_count(&self, instance_id: &str) -> usize {
        self.library_items
            .keys()
            .filter(|(instance, _)| instance == instance_id)
            .count()
    }

    /// How many library items this emulator holds across every instance, for
    /// the total quota check.
    pub fn total_library_item_count(&self) -> usize {
        self.library_items.len()
    }

    /// The category `category_id` of `instance_id`'s catalog, if it holds one.
    pub fn library_category(
        &self,
        instance_id: &str,
        category_id: &str,
    ) -> Option<&LibraryCategory> {
        self.library_categories
            .get(&(instance_id.to_string(), category_id.to_string()))
    }

    /// How many categories `instance_id`'s catalog holds, for the quota check
    /// the operation that FILLS the catalog charges against
    /// [`MAX_CATEGORIES_PER_INSTANCE`].
    pub fn library_category_count(&self, instance_id: &str) -> usize {
        self.library_categories
            .keys()
            .filter(|(instance, _)| instance == instance_id)
            .count()
    }

    /// How many categories this emulator holds across every instance, for the
    /// total quota check against [`MAX_CATEGORIES_TOTAL`].
    pub fn total_library_category_count(&self) -> usize {
        self.library_categories.len()
    }

    /// The `categories` a library item reports: one `Category` per stored id, in
    /// the order the publisher named them, titled and coloured from the
    /// instance's catalog.
    ///
    /// `appCount` is documented as "the number of published Amazon Q Apps
    /// associated with a category", so it is DERIVED here rather than stored:
    /// publishing, disabling or deleting an item moves it. One pass over the
    /// instance's items counts every category of this item at once, so the work
    /// is linear in the store rather than linear per reported category.
    ///
    /// An id the catalog no longer holds (its category was deleted after the
    /// item was filed) is left out rather than reported with an invented title:
    /// a deleted category files nothing.
    pub fn item_categories(&self, instance_id: &str, item: &LibraryItem) -> Vec<Category> {
        let mut app_counts: BTreeMap<&str, i32> = item
            .categories
            .iter()
            .map(|category_id| (category_id.as_str(), 0))
            .collect();
        for ((instance, _), stored) in &self.library_items {
            if instance != instance_id || stored.status != LibraryItemStatus::Published {
                continue;
            }
            for category_id in &stored.categories {
                if let Some(count) = app_counts.get_mut(category_id.as_str()) {
                    *count = count.saturating_add(1);
                }
            }
        }

        item.categories
            .iter()
            .filter_map(|category_id| {
                let category = self.library_category(instance_id, category_id)?;
                Some(Category {
                    id: category.id.clone(),
                    title: category.title.clone(),
                    color: category.color.clone(),
                    app_count: Some(
                        app_counts
                            .get(category_id.as_str())
                            .copied()
                            .unwrap_or_default(),
                    ),
                })
            })
            .collect()
    }
}

/// Mint a fresh id of the service's `UUID` shape.
///
/// A version-4 UUID: the model constrains every `UUID` member to
/// `^[\da-f]{8}-[\da-f]{4}-[45][\da-f]{3}-[89ABab][\da-f]{3}-[\da-f]{12}$`,
/// which is the hyphenated lowercase rendering of exactly that — 32 hex digits
/// with the version nibble `4` and the variant nibble in `8..=b`.
///
/// The 122 free bits come from the OS-seeded, per-process
/// [`std::collections::hash_map::RandomState`] hasher over a process-wide
/// sequence number and the clock, so two ids minted in one tick differ and an id
/// is not guessable from another one. This is the same way the b2bi service in
/// this workspace mints its resource ids, and it keeps this crate's dependency
/// set exactly the one the scaffold generated.
pub fn mint_uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Distinguishes two ids minted inside one clock tick.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    /// Odd 64-bit constant (the golden-ratio mix) so the two halves of the
    /// digest see different inputs.
    const MIX: u64 = 0x9e37_79b9_7f4a_7c15;

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as u64);

    let seed = std::collections::hash_map::RandomState::new();
    let mut high = seed.build_hasher();
    high.write_u64(sequence);
    high.write_u64(nanos);
    let mut low = seed.build_hasher();
    low.write_u64(nanos);
    low.write_u64(sequence.wrapping_mul(MIX));
    let digits = format!("{:016x}{:016x}", high.finish(), low.finish());

    // The variant nibble is one of `8 9 a b`, chosen by the two low bits of the
    // digit this layout drops, so it carries entropy rather than a constant.
    let variant = match digits.as_bytes()[16] {
        b'0' | b'4' | b'8' | b'c' => '8',
        b'1' | b'5' | b'9' | b'd' => '9',
        b'2' | b'6' | b'a' | b'e' => 'a',
        _ => 'b',
    };
    format!(
        "{}-{}-4{}-{}{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[13..16],
        variant,
        &digits[17..20],
        &digits[20..32]
    )
}

/// Whether `value` matches the model's `UUID` pattern
/// (`^[\da-f]{8}-[\da-f]{4}-[45][\da-f]{3}-[89ABab][\da-f]{3}-[\da-f]{12}$`).
///
/// `StartQAppSession` accepts a caller-supplied `sessionId` as a plain string,
/// but every operation that reads a session back takes `UUID`, so an id outside
/// that pattern could never be read again.
pub fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let [g0, g1, g2, g3, g4] = groups[..] else {
        return false;
    };
    let hex_lower = |s: &str, len: usize| {
        s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    hex_lower(g0, 8)
        && hex_lower(g1, 4)
        && g2.len() == 4
        && matches!(g2.as_bytes()[0], b'4' | b'5')
        && hex_lower(&g2[1..], 3)
        && g3.len() == 4
        && matches!(g3.as_bytes()[0], b'8' | b'9' | b'A' | b'B' | b'a' | b'b')
        && hex_lower(&g3[1..], 3)
        && hex_lower(g4, 12)
}

/// Now, as the model's `QAppsTimestamp`.
pub fn now() -> DateTime {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// The principal this request is signed by, for `createdBy`/`updatedBy` and for
/// a session's owner. Derived from the SigV4 access key id, so the same caller
/// is the same principal on every call.
pub fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::engine::extract_akid(&ctx.parts().headers)
}

/// Whether `principal` is an access key id this service can attribute a stored
/// record to: one no longer than [`MAX_PRINCIPAL_LENGTH`]. A longer value cannot
/// be an AWS access key id at all, and storing it would put a caller-sized
/// string in the store.
pub fn is_storable_principal(principal: &str) -> bool {
    !principal.is_empty() && principal.len() <= MAX_PRINCIPAL_LENGTH
}

/// The region this request was signed for; `us-east-1` when it carries no
/// credential scope.
pub fn caller_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| "us-east-1".to_string(), |scope| scope.region)
}

/// The ARN of a Q App: `application/<instanceId>/qapp/<appId>` in the `qapps`
/// namespace the model declares.
pub fn app_arn(region: &str, instance_id: &str, app_id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("application/{instance_id}/qapp/{app_id}"),
    )
}

/// The ARN of a Q App session, which hangs off the app it runs.
pub fn session_arn(region: &str, instance_id: &str, app_id: &str, session_id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("application/{instance_id}/qapp/{app_id}/session/{session_id}"),
    )
}

/// This service's namespace: the `arnNamespace` of its `awsService` trait
/// (`smithy_query(query="service_info")`), which is both the vendor field of every
/// ARN it mints and the prefix of every IAM action it is authorized by.
pub const SIGNING_NAME: &str = "qapps";

/// A Q Apps resource that carries tags, as the ARN a tagging request names
/// addresses it.
///
/// The two forms are the ones [`app_arn`] and [`session_arn`] build, which are the
/// resources this service mints: a Q App, and a session of one. The instance is
/// part of both, because an instance is this API's tenant boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    App {
        instance_id: String,
        app_id: String,
    },
    Session {
        instance_id: String,
        app_id: String,
        session_id: String,
    },
}

impl TaggedResource {
    /// The `resourceType` a refusal about this resource names, as the records
    /// `ResourceNotFoundException` already carries elsewhere in this service spell
    /// them.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::App { .. } => "qapp",
            Self::Session { .. } => "session",
        }
    }

    /// The `resourceId` a refusal about this resource names: the id of the resource
    /// itself, not of the instance holding it.
    pub fn resource_id(&self) -> &str {
        match self {
            Self::App { app_id, .. } => app_id,
            Self::Session { session_id, .. } => session_id,
        }
    }

    /// The `(instanceId, resourceId)` key the store holds this resource under —
    /// `apps` for a Q App and `sessions` for a session, both keyed the same way,
    /// since an instance is this API's tenant boundary.
    ///
    /// Stated once so the reading and the revising tag accessors cannot look in two
    /// different places.
    fn store_key(&self) -> (String, String) {
        match self {
            Self::App {
                instance_id,
                app_id,
            } => (instance_id.clone(), app_id.clone()),
            Self::Session {
                instance_id,
                session_id,
                ..
            } => (instance_id.clone(), session_id.clone()),
        }
    }
}

/// The Q Apps resource the ARN `value` names, or `None` when it names none.
///
/// Read through the ONE ARN reader this tree has
/// ([`::service_common::arn::parse`]) rather than by splitting the value again, and
/// then against the resource forms this service's own ARN builders produce:
/// `application/<instanceId>/qapp/<appId>` and that followed by
/// `/session/<sessionId>`. A value that is not an ARN, one of another vendor, and
/// one whose resource field is any other shape all name no resource here.
///
/// The ACCOUNT and the REGION are deliberately not judged here: who may address an
/// ARN is the authorization question (`crate::service::authorization`), and which
/// region an id lives in is not part of this store's keys.
pub fn tagged_resource(value: &str) -> Option<TaggedResource> {
    let arn = ::service_common::arn::parse(value)?;
    if arn.vendor != SIGNING_NAME {
        return None;
    }
    let segments: Vec<&str> = arn.resource.split('/').collect();
    match segments[..] {
        ["application", instance_id, "qapp", app_id]
            if !instance_id.is_empty() && !app_id.is_empty() =>
        {
            Some(TaggedResource::App {
                instance_id: instance_id.to_string(),
                app_id: app_id.to_string(),
            })
        }
        ["application", instance_id, "qapp", app_id, "session", session_id]
            if !instance_id.is_empty() && !app_id.is_empty() && !session_id.is_empty() =>
        {
            Some(TaggedResource::Session {
                instance_id: instance_id.to_string(),
                app_id: app_id.to_string(),
                session_id: session_id.to_string(),
            })
        }
        _ => None,
    }
}

/// The opaque `nextToken` marking "resume after this app". Hex-encoded so the
/// token is opaque to the caller and every byte of it is validated on the way
/// back in.
pub fn encode_page_token(instance_id: &str, last_app_id: &str) -> String {
    hex::encode(format!("v1:{instance_id}:{last_app_id}"))
}

/// The app id a `nextToken` resumes after, or `None` when the token is not one
/// this service issued for `instance_id`.
pub fn decode_page_token(instance_id: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let rest = decoded.strip_prefix("v1:")?;
    let (token_instance, last_app_id) = rest.split_once(':')?;
    (token_instance == instance_id && !last_app_id.is_empty()).then(|| last_app_id.to_string())
}

/// The id of a submitted card, whichever variant it is.
pub fn card_input_id(card: &CardInput) -> &str {
    match card {
        CardInput::textInput(c) => &c.id,
        CardInput::qQuery(c) => &c.id,
        CardInput::qPlugin(c) => &c.id,
        CardInput::fileUpload(c) => &c.id,
        CardInput::formInput(c) => &c.id,
    }
}

/// The prompt a submitted card carries, for the cards that have one.
fn card_input_prompt(card: &CardInput) -> Option<&str> {
    match card {
        CardInput::qQuery(c) => Some(&c.prompt),
        CardInput::qPlugin(c) => Some(&c.prompt),
        CardInput::textInput(_) | CardInput::fileUpload(_) | CardInput::formInput(_) => None,
    }
}

/// The id of a stored card, whichever variant it is.
pub fn card_id(card: &Card) -> &str {
    match card {
        Card::textInput(c) => &c.id,
        Card::qQuery(c) => &c.id,
        Card::qPlugin(c) => &c.id,
        Card::fileUpload(c) => &c.id,
        Card::formInput(c) => &c.id,
    }
}

/// Build the stored `AppDefinition` from a submitted `AppDefinitionInput`.
///
/// `CreateQApp` is documented to "calculate the dependencies between the cards
/// by inspecting the references in the prompts", so each card's `dependencies`
/// are the ids of the OTHER cards of the same definition that its prompt
/// references, in definition order. A card with no prompt references nothing.
pub fn definition_from_input(input: &AppDefinitionInput, can_edit: bool) -> AppDefinition {
    let ids: Vec<String> = input
        .cards
        .iter()
        .map(|card| card_input_id(card).to_string())
        .collect();

    let cards = input
        .cards
        .iter()
        .map(|card| {
            let own_id = card_input_id(card).to_string();
            let prompt = card_input_prompt(card).unwrap_or_default();
            let dependencies: Vec<String> = ids
                .iter()
                .filter(|id| **id != own_id && prompt.contains(id.as_str()))
                .cloned()
                .collect();
            card_from_input(card, dependencies)
        })
        .collect();

    AppDefinition {
        app_definition_version: APP_DEFINITION_VERSION.to_string(),
        cards,
        can_edit: Some(can_edit),
    }
}

/// One submitted card, as the stored card of the same kind.
fn card_from_input(card: &CardInput, dependencies: Vec<String>) -> Card {
    match card {
        CardInput::textInput(c) => Card::textInput(TextInputCard {
            id: c.id.clone(),
            title: c.title.clone(),
            dependencies,
            r#type: c.r#type.clone(),
            placeholder: c.placeholder.clone(),
            default_value: c.default_value.clone(),
        }),
        CardInput::qQuery(c) => Card::qQuery(QQueryCard {
            id: c.id.clone(),
            title: c.title.clone(),
            dependencies,
            r#type: c.r#type.clone(),
            prompt: c.prompt.clone(),
            output_source: c.output_source.clone(),
            attribute_filter: c.attribute_filter.clone(),
            memory_references: None,
        }),
        CardInput::qPlugin(c) => Card::qPlugin(QPluginCard {
            id: c.id.clone(),
            title: c.title.clone(),
            dependencies,
            r#type: c.r#type.clone(),
            prompt: c.prompt.clone(),
            // The submitted card carries only the plugin's id; `pluginType` is
            // a property of that plugin, which this service does not host, so
            // the stored card reports the model's `CUSTOM` plugin type.
            plugin_type: "CUSTOM".to_string(),
            plugin_id: c.plugin_id.clone(),
            action_identifier: c.action_identifier.clone(),
        }),
        CardInput::fileUpload(c) => Card::fileUpload(FileUploadCard {
            id: c.id.clone(),
            title: c.title.clone(),
            dependencies,
            r#type: c.r#type.clone(),
            filename: c.filename.clone(),
            file_id: c.file_id.clone(),
            allow_override: c.allow_override,
        }),
        CardInput::formInput(c) => Card::formInput(FormInputCard {
            id: c.id.clone(),
            title: c.title.clone(),
            dependencies,
            r#type: c.r#type.clone(),
            metadata: c.metadata.clone(),
            compute_mode: c.compute_mode.clone(),
        }),
    }
}

/// The capabilities a definition's cards require, from the model's
/// `AppRequiredCapability` enum (`CreatorMode | FileUpload | PluginMode |
/// RetrievalMode`): a file-upload card needs `FileUpload`, a plugin card needs
/// `PluginMode`, and a query card needs `RetrievalMode` or `CreatorMode`
/// according to its `CardOutputSource` (`approved-sources` or `llm`).
pub fn required_capabilities(definition: &AppDefinition) -> Vec<String> {
    let mut capabilities: Vec<String> = Vec::new();
    let mut require = |capability: &str| {
        if !capabilities.iter().any(|held| held == capability) {
            capabilities.push(capability.to_string());
        }
    };
    for card in &definition.cards {
        match card {
            Card::fileUpload(_) => require("FileUpload"),
            Card::qPlugin(_) => require("PluginMode"),
            Card::qQuery(c) => {
                if c.output_source == "approved-sources" {
                    require("RetrievalMode");
                } else {
                    require("CreatorMode");
                }
            }
            Card::textInput(_) | Card::formInput(_) => {}
        }
    }
    capabilities.sort();
    capabilities
}

/// The `cardStatus` map a session starts with: one entry per card of the app's
/// definition, carrying the value `initialValues` supplied for that card.
///
/// A card nobody supplied a value for is `WAITING`: its value is whatever the
/// session later produces for it, and this emulator runs no model, so it
/// reports no value rather than inventing one.
pub fn seed_card_status(
    definition: &AppDefinition,
    initial_values: &[CardValue],
) -> HashMap<String, CardStatus> {
    definition
        .cards
        .iter()
        .map(|card| {
            let id = card_id(card).to_string();
            let supplied = initial_values.iter().find(|value| value.card_id == id);
            let status = match supplied {
                Some(value) => CardStatus {
                    current_state: ExecutionStatus::Completed.as_wire().to_string(),
                    current_value: value.value.clone(),
                    submissions: None,
                },
                None => CardStatus {
                    current_state: ExecutionStatus::Waiting.as_wire().to_string(),
                    current_value: String::new(),
                    submissions: None,
                },
            };
            (id, status)
        })
        .collect()
}

/// A session's own status: `COMPLETED` once every card of its definition holds
/// a value, `WAITING` while any card is still waiting for one.
///
/// A session of a definition with NO cards has nothing that could produce a
/// value, so it is reported `WAITING` rather than complete: the empty map is
/// stated explicitly here instead of riding on `all()`'s vacuous truth.
pub fn session_status(card_status: &HashMap<String, CardStatus>) -> ExecutionStatus {
    if card_status.is_empty() {
        return ExecutionStatus::Waiting;
    }
    if card_status
        .values()
        .all(|status| status.current_state == ExecutionStatus::Completed.as_wire())
    {
        ExecutionStatus::Completed
    } else {
        ExecutionStatus::Waiting
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsQappsQappsserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsQappsQappsserviceState>)
}
