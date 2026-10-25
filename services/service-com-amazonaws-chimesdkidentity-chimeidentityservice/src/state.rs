// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkidentity-chimeidentityservice service.
//!
//! The `AppInstance` is this service's root resource: every other identity resource
//! (users, bots, admins, endpoints, retention settings) hangs off exactly one of them.
//! State is therefore a single `BTreeMap` of app instances keyed by ARN, with each
//! instance owning its users, plus two index maps:
//!
//! * `users_by_arn` — a user ARN to its owning instance ARN, because
//!   `DescribeAppInstanceUser` / `DeleteAppInstanceUser` are keyed only by the user ARN.
//! * `idempotency` — a create operation's `ClientRequestToken` to the ARN that token
//!   already minted, so repeating one token never mints a second resource.
//!
//! `BTreeMap` (not `HashMap`) everywhere: list operations page in key order, so the
//! iteration order has to be stable for a `NextToken` to be resumable.
//!
//! Two things this service reads are NOT in the struct below, because another Chime
//! service reads or writes them too and the engine keys state by type: the app-instance
//! ADMIN directory ([`app_instance_admins`]) and the family's ARN-keyed TAG directory
//! ([`chime_tags`]).

use std::collections::{BTreeMap, BTreeSet};
use std::hash::BuildHasher as _;
use std::ops::Bound;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::ExpirationSettings;

/// The region used for a minted ARN when the request carries no SigV4 credential
/// scope to read one from (an unsigned or presigned request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The SigV4 signing name of the Chime SDK services, and so the service segment of
/// every ARN this emulator mints: the model's `aws.auth#sigv4` name is `chime`.
pub const ARN_SERVICE: &str = "chime";

/// The largest page any list operation in this service returns, from the model's
/// `MaxResults` range (`min 1, max 50`) on `ListAppInstances` / `ListAppInstanceUsers`.
/// Also the page size used when the caller supplies no `MaxResults`.
pub const MAX_PAGE_SIZE: i32 = 50;

/// The version marker a `NextToken` this service issues carries, so a token from
/// anywhere else is recognised as not ours.
const TOKEN_VERSION: &str = "v1";

/// How many `AppInstance`s this emulator holds for one account, and how many
/// `AppInstanceUser`s one `AppInstance` holds.
///
/// These are the EMULATOR's capacity bounds, not a claim about any published AWS
/// quota: the store lives in process memory, so the resource count has to be bounded
/// somewhere or a create loop grows it without limit. A create past the bound is
/// refused with the modelled `ResourceLimitExceededException` ("The request exceeds
/// the resource limit"), which both create operations declare, so a client sees a
/// modelled quota refusal rather than an emulator that swells.
pub const MAX_APP_INSTANCES: usize = 100;

/// See [`MAX_APP_INSTANCES`].
pub const MAX_APP_INSTANCE_USERS: usize = 500;

/// The ceiling on
/// [`idempotency`](ComAmazonawsChimesdkidentityChimeidentityserviceState::idempotency).
///
/// That map's size is already one entry per LIVE resource, which the two caps above
/// bound: an entry is written only where a create actually minted a new resource
/// (`insert_user` refuses to overwrite a live ARN, and an instance ARN is a fresh
/// uuid), and it is released again when that resource is deleted. This constant is
/// the structural ceiling that keeps the map bounded even if a future create path
/// records a token without minting anything: past it, a token is not recorded.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_APP_INSTANCES * (1 + MAX_APP_INSTANCE_USERS);

/// A persisted `AppInstanceUser`.
#[derive(Debug, Clone)]
pub struct AppInstanceUserRecord {
    pub arn: String,
    pub app_instance_arn: String,
    pub user_id: String,
    pub name: String,
    pub metadata: Option<String>,
    pub created_timestamp: SystemTime,
    /// When this user was last written. A user is stamped when it is CREATED (AWS
    /// reports a last-updated instant on a user that has never been updated, equal to
    /// its created instant), and re-stamped by every operation that changes it — unlike
    /// an [`AppInstanceRecord`], whose stamp appears only on a real update.
    pub last_updated_timestamp: SystemTime,
    pub expiration_settings: Option<ExpirationSettings>,
}

/// A persisted `AppInstance` and everything it owns.
#[derive(Debug, Clone)]
pub struct AppInstanceRecord {
    pub arn: String,
    pub name: String,
    pub metadata: Option<String>,
    pub created_timestamp: SystemTime,
    /// When this instance was last CHANGED, or `None` while it still carries exactly
    /// what created it.
    ///
    /// On an `AppInstance` a last-updated instant reports an UPDATE, not a creation: an
    /// instance nobody has updated has none to report, so the optional
    /// `AppInstance$LastUpdatedTimestamp` member ("The time an AppInstance was last
    /// updated") is left out of the response until an operation changes the instance.
    pub last_updated_timestamp: Option<SystemTime>,
    /// This instance's users, keyed by `AppInstanceUserArn`.
    pub users: BTreeMap<String, AppInstanceUserRecord>,
}

/// What [`ComAmazonawsChimesdkidentityChimeidentityserviceState::insert_user`] did, so
/// the handler maps each outcome to the error the model declares for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertUser {
    /// The user was stored under its parent.
    Stored,
    /// The named `AppInstanceArn` does not exist; nothing was written.
    NoSuchAppInstance,
    /// A user with this ARN already exists; nothing was written. A stored user is
    /// never overwritten, so one live resource can never collect a second record — nor
    /// a second idempotency entry.
    AlreadyExists,
    /// The parent already holds [`MAX_APP_INSTANCE_USERS`] users; nothing was written.
    AtCapacity,
}

/// Per-service state for com-amazonaws-chimesdkidentity-chimeidentityservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkidentityChimeidentityserviceState {
    /// Every app instance, keyed by `AppInstanceArn`.
    pub app_instances: BTreeMap<String, AppInstanceRecord>,
    /// `AppInstanceUserArn` -> owning `AppInstanceArn`.
    pub users_by_arn: BTreeMap<String, String>,
    /// `<operation>:<ClientRequestToken>` -> the ARN that token minted.
    pub idempotency: BTreeMap<String, String>,
}

impl ComAmazonawsChimesdkidentityChimeidentityserviceState {
    /// The user with this ARN, resolved through the `users_by_arn` index in one hop.
    pub fn user(&self, user_arn: &str) -> Option<&AppInstanceUserRecord> {
        let instance_arn = self.users_by_arn.get(user_arn)?;
        self.app_instances.get(instance_arn)?.users.get(user_arn)
    }

    /// The user with this ARN, for a caller that is about to CHANGE it; resolved
    /// through the same one-hop index as [`user`](Self::user).
    pub fn user_mut(&mut self, user_arn: &str) -> Option<&mut AppInstanceUserRecord> {
        let instance_arn = self.users_by_arn.get(user_arn)?.clone();
        self.app_instances
            .get_mut(&instance_arn)?
            .users
            .get_mut(user_arn)
    }

    /// Store a user under its parent instance and index it.
    ///
    /// The parent lookup here is the ONLY one a caller needs: an absent parent is
    /// reported as [`InsertUser::NoSuchAppInstance`] and a full one as
    /// [`InsertUser::AtCapacity`], and in neither case is anything written.
    pub fn insert_user(&mut self, record: AppInstanceUserRecord) -> InsertUser {
        let Some(instance) = self.app_instances.get_mut(&record.app_instance_arn) else {
            return InsertUser::NoSuchAppInstance;
        };
        if instance.users.contains_key(&record.arn) {
            return InsertUser::AlreadyExists;
        }
        if instance.users.len() >= MAX_APP_INSTANCE_USERS {
            return InsertUser::AtCapacity;
        }
        self.users_by_arn
            .insert(record.arn.clone(), record.app_instance_arn.clone());
        instance.users.insert(record.arn.clone(), record);
        InsertUser::Stored
    }

    /// Remove a user from its parent instance and from the index.
    pub fn remove_user(&mut self, user_arn: &str) -> Option<AppInstanceUserRecord> {
        let instance_arn = self.users_by_arn.remove(user_arn)?;
        let removed = self
            .app_instances
            .get_mut(&instance_arn)
            .and_then(|instance| instance.users.remove(user_arn))?;
        self.forget_idempotency_for(&[user_arn]);
        Some(removed)
    }

    /// Remove an app instance and cascade to everything it owns: its users go with it,
    /// so a later `DescribeAppInstanceUser` on one of them resolves to nothing. The
    /// idempotency tokens that minted the instance and its users are released too.
    pub fn remove_app_instance(&mut self, arn: &str) -> Option<AppInstanceRecord> {
        let instance = self.app_instances.remove(arn)?;
        self.users_by_arn
            .retain(|_, owning_arn| owning_arn.as_str() != arn);
        let mut gone: BTreeSet<&str> = instance.users.keys().map(String::as_str).collect();
        gone.insert(arn);
        self.idempotency
            .retain(|_, minted| !gone.contains(minted.as_str()));
        Some(instance)
    }

    /// Drop the idempotency entries that minted these now-deleted ARNs.
    ///
    /// A token is released with the resource it minted, so the map holds at most one
    /// entry per live resource rather than one per create ever made.
    fn forget_idempotency_for(&mut self, arns: &[&str]) {
        self.idempotency
            .retain(|_, minted| !arns.contains(&minted.as_str()));
    }

    /// True when `arn` still names a live resource of this service.
    pub fn resource_exists(&self, arn: &str) -> bool {
        self.app_instances.contains_key(arn) || self.users_by_arn.contains_key(arn)
    }

    /// Whether a tagging call may address `arn`.
    ///
    /// The ARN has to name a resource of this family at all — one in the Chime ARN
    /// namespace ([`ARN_SERVICE`]), read through the one parse this workspace states for a
    /// caller-supplied ARN — so a value that is no ARN, or another vendor's, names nothing
    /// to tag. A resource THIS service mints ([`names_identity_resource`]) then has to
    /// exist for its tags to be written or read: this store is what says whether it does.
    /// A resource of a SIBLING Chime service cannot be checked from here — its store is
    /// the sibling's, and this service only serves the family's shared tagging routes for
    /// it — so such an ARN is taken at its word rather than refused as absent.
    pub fn taggable(&self, arn: &str) -> bool {
        ::service_common::arn::is_resource_arn_of(arn, ARN_SERVICE)
            && (!names_identity_resource(arn) || self.resource_exists(arn))
    }

    /// The ARN a previous call of `operation` already minted for `token`, if that
    /// resource is still live.
    ///
    /// A token whose resource has since been deleted is forgotten here, so the token
    /// can mint a new resource rather than replaying an ARN that resolves to nothing.
    pub fn idempotent_arn(&mut self, operation: &str, token: &str) -> Option<String> {
        let key = idempotency_key(operation, token);
        let recorded = self.idempotency.get(&key)?.clone();
        if self.resource_exists(&recorded) {
            return Some(recorded);
        }
        self.idempotency.remove(&key);
        None
    }

    /// Record that `operation` minted `arn` for `token`.
    ///
    /// Answers `false`, recording nothing, once the map holds
    /// [`MAX_IDEMPOTENCY_TOKENS`] entries: the resource caps bind first, so this is
    /// unreachable through the operations as written, and it is here so the map stays
    /// bounded whatever a caller does. The resource itself is created either way; only
    /// the token's replay is given up.
    pub fn record_idempotency(&mut self, operation: &str, token: &str, arn: &str) -> bool {
        let key = idempotency_key(operation, token);
        if self.idempotency.len() >= MAX_IDEMPOTENCY_TOKENS && !self.idempotency.contains_key(&key) {
            return false;
        }
        self.idempotency.insert(key, arn.to_string());
        true
    }
}

/// The key an idempotency token is stored under. Prefixed with the operation name so
/// `CreateAppInstance` and `CreateAppInstanceUser` can be called with the same token
/// (they are different requests) without either replaying the other's ARN.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkidentityChimeidentityserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkidentityChimeidentityserviceState>)
}

/// The SHARED app-instance ADMIN directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// An `AppInstanceAdmin` is promoted here and the role it grants is read by Chime SDK
/// Messaging, which reserves an app instance's private-channel listing and its message
/// deletion for an administrator. The engine keys state by TYPE, so the promotions live
/// in ONE directory under one shared type
/// ([`::service_common::chime_app_instance_admins::AppInstanceAdminDirectory`]) rather
/// than in a copy inside [`ComAmazonawsChimesdkidentityChimeidentityserviceState`],
/// which the messaging service could not see.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn app_instance_admins<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::chime_app_instance_admins::AppInstanceAdminDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::chime_app_instance_admins::AppInstanceAdminDirectory,
            >,
        )
}

/// The SHARED Chime TAG directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A resource's tags are keyed by ARN and live in ONE directory
/// ([`::service_common::chime_tags::ChimeTagDirectory`]) rather than on the resource
/// records of this state struct, because the family's generic tagging routes are served
/// by one member for every member's resource: a copy inside
/// [`ComAmazonawsChimesdkidentityChimeidentityserviceState`] would be invisible to a
/// sibling serving a tagging call at its own endpoint.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn chime_tags<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::chime_tags::ChimeTagDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::chime_tags::ChimeTagDirectory>)
}

/// Whether `arn` names a resource THIS service mints, so that this service's store is
/// what says whether it exists.
///
/// The ARNs this service mints are the app instance itself
/// (`arn:…:chime:…:app-instance/<id>`) and its identity children, which hang off it:
/// `…/user/<id>` ([`app_instance_user_arn`]) and `…/bot/<id>`. A sibling's resource hangs
/// off the same app instance under a segment of its own (Chime SDK Messaging's
/// `…/channel/<id>`), and every other family member's resources carry a different resource
/// type altogether, so neither reads as this service's.
pub fn names_identity_resource(arn: &str) -> bool {
    let Some(resource) = arn.splitn(6, ':').nth(5) else {
        return false;
    };
    let tokens: Vec<&str> = resource.split('/').collect();
    match tokens.as_slice() {
        ["app-instance", id] => !id.is_empty(),
        ["app-instance", id, child, child_id] => {
            !id.is_empty() && !child_id.is_empty() && matches!(*child, "user" | "bot")
        }
        _ => false,
    }
}

/// The region to mint an ARN in: the one the caller signed for, else
/// [`DEFAULT_REGION`]. Read from the SigV4 credential scope, the same signal the
/// router routes on, so an ARN is in the region the client addressed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh `AppInstanceArn`: `arn:aws:chime:<region>:<account>:app-instance/<uuid>`.
///
/// The account segment comes from `engine::emulator_arn`, which is the single source
/// of the emulator's account id.
pub fn new_app_instance_arn(region: &str) -> String {
    let resource = format!("app-instance/{}", new_uuid_v4());
    ::engine::emulator_arn(ARN_SERVICE, region, &resource)
}

/// A fresh version-4 UUID in the lowercase hyphenated form an AWS ARN carries.
///
/// The 16 bytes are random: `sha2` over the std hasher's per-instance random keys
/// (`RandomState` is seeded from the OS) plus the monotonic clock, with the version
/// and variant nibbles then set as RFC 4122 section 4.4 specifies. Nothing here is
/// derived from the request, so two creates never collide and a minted id is not
/// guessable from one a caller has seen. Only the two nibbles and the hyphenation are
/// written by hand; the randomness and the hex-coding come from maintained crates this
/// workspace already carries.
fn new_uuid_v4() -> String {
    use sha2::Digest as _;

    let entropy = ::std::collections::hash_map::RandomState::new();
    let mut hasher = sha2::Sha256::new();
    hasher.update(entropy.hash_one(0u64).to_be_bytes());
    hasher.update(entropy.hash_one(u64::MAX).to_be_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
            .to_be_bytes(),
    );

    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version 4 in the high nibble of octet 6, variant 0b10 in the two high bits of
    // octet 8 — what makes the value a well-formed v4 UUID rather than 16 loose bytes.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// An `AppInstanceUserArn` is its instance's ARN with `/user/<AppInstanceUserId>`
/// appended, so a user ARN carries its parent and the id the caller chose.
pub fn app_instance_user_arn(app_instance_arn: &str, app_instance_user_id: &str) -> String {
    format!("{app_instance_arn}/user/{app_instance_user_id}")
}

/// A stored instant as the wire timestamp the model declares (`Timestamp`).
pub fn wire_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Now, as epoch MILLISECONDS — the resolution the shared app-instance admin directory
/// records a promotion at, so both services that read it agree on the instant without
/// either having to agree on a Rust clock type.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// An epoch-milliseconds instant as the wire timestamp the model declares.
pub fn wire_timestamp_of_millis(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// One page of a listing, plus the token that resumes after it.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Encode a state key as an opaque `NextToken`, scoped to the listing that issued it.
///
/// The encoded form carries a version marker and the listing's `scope` beside the key,
/// so a token from one listing (or from another emulator version) is recognisably not
/// a token the listing being resumed issued, rather than silently resuming at a key
/// that collection never held. Hex-coded rather than base64: the token travels back in
/// a query string (`?next-token=...`), and hex needs no escaping there, while base64's
/// `+` would arrive as a space.
pub fn encode_page_token(scope: &str, key: &str) -> String {
    // `\n` separates the fields: a scope is an operation name or an ARN and a key is
    // an ARN, so neither can contain one.
    ::hex::encode(format!("{TOKEN_VERSION}\n{scope}\n{key}"))
}

/// Decode a `NextToken` issued for `scope` back into the state key it resumes after;
/// `None` when the token is not one this listing issued.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let raw = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(raw).ok()?;
    let mut fields = decoded.splitn(3, '\n');
    let (Some(TOKEN_VERSION), Some(token_scope), Some(key)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    (token_scope == scope).then(|| key.to_string())
}

/// One page of a key-ordered map: the entries after `after`, at most `max_results`
/// (bounded by [`MAX_PAGE_SIZE`]) of them, each projected to a response shape.
///
/// The page starts AT the cursor — `BTreeMap::range` seeks to it in O(log n) rather
/// than re-scanning the consumed prefix — so walking a whole collection costs one
/// pass in total, not one pass per page. The returned `next_token` is `Some` only
/// when entries remain, so the final page omits it, which is how a paginating client
/// knows to stop. `scope` identifies the listing in the token it issues; see
/// [`encode_page_token`].
pub fn paginate<'a, V, T>(
    entries: &'a BTreeMap<String, V>,
    max_results: Option<i32>,
    after: Option<&str>,
    scope: &str,
    project: impl Fn(&'a V) -> T,
) -> Page<T> {
    // Defence in depth, not validation: the boundary validator has already refused a
    // `MaxResults` outside the model's `range(min: 1, max: 50)` by the time a handler
    // runs, so this clamp is a no-op on every request that reaches here. It is kept so
    // that no page can ever be materialised larger than the modelled maximum.
    debug_assert!(
        max_results.is_none_or(|requested| (1..=MAX_PAGE_SIZE).contains(&requested)),
        "MaxResults {max_results:?} should have been refused at the boundary"
    );
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let mut items = Vec::new();
    let mut last_key: Option<&str> = None;
    let mut truncated = false;

    let start = match after {
        Some(cursor) => Bound::Excluded(cursor),
        None => Bound::Unbounded,
    };
    for (key, value) in entries.range::<str, _>((start, Bound::Unbounded)) {
        if items.len() == limit {
            truncated = true;
            break;
        }
        items.push(project(value));
        last_key = Some(key.as_str());
    }

    Page {
        items,
        next_token: truncated
            .then(|| last_key.map(|key| encode_page_token(scope, key)))
            .flatten(),
    }
}
