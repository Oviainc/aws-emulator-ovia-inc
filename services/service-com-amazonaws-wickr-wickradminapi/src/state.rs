// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wickr-wickradminapi service.
//!
//! The network is this service's primary resource: users and security groups are
//! nested under one, which is why both are keyed by `networkId` here and why
//! deleting a network drops its whole subtree.
//!
//! Besides the store itself this module holds the few helpers more than one
//! operation needs — id minting, the opaque pagination token codec, the clock,
//! and the two content-derived user identifiers — so the per-operation files in
//! `ops/` share one implementation instead of three copies.

use std::collections::BTreeMap;

/// The first `networkId` a store mints.
///
/// `NetworkId` is `^[0-9]{8}$` (exactly 8 digits), so ids run from 10000000 to
/// 99999999: starting at the low end of the 8-digit range keeps every minted id
/// inside the pattern without relying on leading zeros.
const FIRST_NETWORK_ID: u32 = 10_000_000;

/// The last `networkId` the 8-digit `NetworkId` pattern can express.
const LAST_NETWORK_ID: u32 = 99_999_999;

/// The largest `userId` the modelled `UserId` (len 1..10, `^[0-9]+$`) can express.
const LAST_USER_ID: u64 = 9_999_999_999;

/// `networkName` bound from the `CreateNetworkRequest$networkName` documentation:
/// "Must be between 1 and 20 characters."
pub const NETWORK_NAME_MAX_CHARS: usize = 20;

/// The id of the security group every new network is seeded with.
///
/// A network has to have at least one security group for `BatchCreateUser` to
/// reference, and a real Wickr network is created with its default group already
/// present.
pub const DEFAULT_SECURITY_GROUP_ID: &str = "0";

/// The name of that seeded group.
pub const DEFAULT_SECURITY_GROUP_NAME: &str = "Default";

/// `UserStatus.PENDING` (1): "The user has been invited to the network but has
/// not yet accepted their invitation." The other value `UserStatus` declares,
/// ACTIVE (2), is set by the operation that accepts an invitation, which this
/// slice does not implement, so nothing here reports it.
pub const USER_STATUS_PENDING: i32 = 1;

/// The most users one `BatchCreateUser` call may carry.
///
/// `BatchCreateUserRequest$users` carries no `@length`, so without a cap the work
/// one request does is chosen entirely by the caller. This is the emulator's own
/// server-side batch limit, not a value the model states; a larger batch is
/// refused with the modelled `ValidationError` naming `users`, the way AWS batch
/// APIs refuse an over-long batch rather than accepting it.
pub const MAX_BATCH_USERS: usize = 100;

/// The most items one page of a listing may carry.
///
/// Neither `maxResults` member carries a `@range`, so this is the emulator's own
/// server-side page cap: a request asking for more (or for none at all) is served
/// one page of at most this many items plus a `nextToken`, so no single request
/// can make a listing serialize the whole store.
pub const MAX_PAGE_SIZE: usize = 100;

/// `Network$migrationState` 0: the model documents "0 (not started), 1 (in
/// progress), or 2 (completed)" for the SSO redirect URI migration wizard, and a
/// network nobody has run the wizard on has not started it.
pub const MIGRATION_STATE_NOT_STARTED: i32 = 0;

/// `Network$standing` for a network that exists and is usable.
///
/// The model documents `standing` as "the current standing or status of the
/// network" without enumerating values, so this emulator reports ONE stable
/// value for every live network rather than inventing transitions it cannot
/// ground.
pub const NETWORK_STANDING_GOOD: i32 = 1;

/// Length of the premium free trial, in days.
///
/// AWS documents the Wickr premium free trial as a 30-day trial (AWS Wickr
/// Administration Guide, "Premium free trial"), so a network created with
/// `enablePremiumFreeTrial` expires its trial 30 days after it was created.
pub const PREMIUM_FREE_TRIAL_DAYS: i64 = 30;

/// Seconds in a day, for the free-trial arithmetic above.
const SECONDS_PER_DAY: i64 = 86_400;

/// The region an ARN carries when the request is unsigned and so names none.
const DEFAULT_REGION: &str = "us-east-1";

/// One stored network — every member of the modelled `Network` shape, so
/// `GetNetwork` and `ListNetworks` read the same record rather than each
/// recomputing half of it.
#[derive(Debug, Clone)]
pub struct NetworkRecord {
    pub network_id: String,
    pub network_name: String,
    pub access_level: String,
    pub aws_account_id: String,
    pub network_arn: String,
    pub standing: i32,
    pub free_trial_expiration: Option<String>,
    pub migration_state: i32,
    pub encryption_key_arn: Option<String>,
}

/// One stored user of one network.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub user_id: String,
    pub username: String,
    pub uname: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub security_group_ids: Vec<String>,
    pub is_admin: bool,
    pub suspended: bool,
    pub status: i32,
    pub invite_code: String,
    /// Invite-code lifetime in days, as the request gave it; `None` means the
    /// code does not expire.
    pub invite_code_ttl: Option<i32>,
    pub code_validation: Option<bool>,
    /// When the user was created, in epoch seconds — the base the invite-code
    /// expiry is measured from.
    pub created_at: i64,
}

impl UserRecord {
    /// Whether this user's invite code has expired *as of `now`*.
    ///
    /// Derived from the stored creation instant and TTL on every read, so no
    /// reporting operation has to write to keep it current.
    pub fn invite_expired_at(&self, now: i64) -> bool {
        self.invite_code_ttl
            .is_some_and(|ttl| now >= self.created_at.saturating_add(i64::from(ttl) * SECONDS_PER_DAY))
    }
}

/// One stored security group of one network.
#[derive(Debug, Clone)]
pub struct SecurityGroupRecord {
    pub group_id: String,
    pub name: String,
    pub is_default: bool,
}

/// Per-service state for com-amazonaws-wickr-wickradminapi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWickrWickradminapiState {
    /// Networks by `networkId`. `BTreeMap`, so a listing is in id order without
    /// the store having to remember an insertion sequence.
    pub networks: BTreeMap<String, NetworkRecord>,
    /// Users by `networkId`, then by `userId`.
    pub users: BTreeMap<String, BTreeMap<String, UserRecord>>,
    /// Security groups by `networkId`, then by group id.
    pub security_groups: BTreeMap<String, BTreeMap<String, SecurityGroupRecord>>,
    /// `userId` by `(networkId, username)`, so checking that a username is free
    /// is a lookup rather than a scan of the network's users. Maintained only by
    /// [`Self::insert_user`] and [`Self::remove_network`], which are the only
    /// writers of `users`, so it cannot drift from it.
    username_index: BTreeMap<(String, String), String>,
    /// How many network ids this store has minted. Monotonic and never reset, so
    /// a deleted network's id is never handed out again.
    minted_networks: u32,
    /// How many user ids this store has minted, with the same guarantee.
    minted_users: u64,
}

impl ComAmazonawsWickrWickradminapiState {
    /// Mint the next unused `networkId`, or `None` once the 8-digit id space is
    /// exhausted.
    pub fn mint_network_id(&mut self) -> Option<String> {
        let next = FIRST_NETWORK_ID.checked_add(self.minted_networks)?;
        if next > LAST_NETWORK_ID {
            return None;
        }
        self.minted_networks += 1;
        Some(next.to_string())
    }

    /// Mint the next unused `userId`, or `None` once the id space is exhausted.
    pub fn mint_user_id(&mut self) -> Option<String> {
        let next = self.minted_users.checked_add(1)?;
        if next > LAST_USER_ID {
            return None;
        }
        self.minted_users = next;
        Some(next.to_string())
    }

    /// The users of `network_id`, in `userId` order.
    pub fn users_of(&self, network_id: &str) -> Vec<UserRecord> {
        self.users
            .get(network_id)
            .map(|users| users.values().cloned().collect())
            .unwrap_or_default()
    }

    /// The user of `network_id` holding `username`, if one does.
    ///
    /// O(log n) through the username index, so deciding a batch of items costs
    /// the batch size times a lookup rather than the batch size times the number
    /// of users already stored.
    pub fn user_by_username(&self, network_id: &str, username: &str) -> Option<&UserRecord> {
        let user_id = self
            .username_index
            .get(&(network_id.to_string(), username.to_string()))?;
        self.users.get(network_id)?.get(user_id)
    }

    /// Store `record` under `network_id`, keeping the username index with it.
    pub fn insert_user(&mut self, network_id: &str, record: UserRecord) {
        self.username_index.insert(
            (network_id.to_string(), record.username.clone()),
            record.user_id.clone(),
        );
        self.users
            .entry(network_id.to_string())
            .or_default()
            .insert(record.user_id.clone(), record);
    }

    /// Remove `network_id` and everything stored under it, answering how many
    /// users went with it, or `None` when there was no such network.
    ///
    /// Users and security groups exist only inside a network, so they go with it:
    /// leaving them would let a later reader see the subtree of a network that is
    /// gone.
    pub fn remove_network(&mut self, network_id: &str) -> Option<usize> {
        self.networks.remove(network_id)?;
        let removed_users = self.users.remove(network_id).unwrap_or_default();
        self.security_groups.remove(network_id);
        self.username_index
            .retain(|(network, _), _| network != network_id);
        Some(removed_users.len())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWickrWickradminapiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWickrWickradminapiState>)
}

/// The region the caller signed for, or [`DEFAULT_REGION`] when the request
/// carries no SigV4 credential scope to read one from.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Seconds since the Unix epoch.
pub fn now_epoch_secs() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
}

/// `secs` as an RFC 3339 date-time, the form this emulator reports every
/// timestamp the model types as a string in.
pub fn format_epoch_secs(secs: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The instant a premium free trial started at `created_at` expires, formatted
/// as [`format_epoch_secs`] does.
pub fn free_trial_expiration(created_at: i64) -> String {
    format_epoch_secs(created_at.saturating_add(PREMIUM_FREE_TRIAL_DAYS * SECONDS_PER_DAY))
}

/// The `uname` a user is known by: the SHA-256 of the network id and username,
/// hex encoded.
///
/// `User$uname` is "the unique identifier for the user" and is what
/// `BatchLookupUserUname` resolves, so it has to be stable for the life of the
/// user and distinct between users. Deriving it from the two values that
/// identify the user gives both without a per-call random value.
pub fn derive_uname(network_id: &str, username: &str) -> String {
    use ::sha2::Digest as _;
    ::hex::encode(::sha2::Sha256::digest(
        format!("{network_id}/{username}").as_bytes(),
    ))
}

/// The invite code minted for a user created without one.
///
/// `BatchCreateUserRequestItem$inviteCode` documents "If not provided, one will
/// be generated automatically". The code is derived from the network and the
/// minted user id, so it is unique per user and unchanged on every later read.
pub fn derive_invite_code(network_id: &str, user_id: &str) -> String {
    use ::sha2::Digest as _;
    let digest = ::sha2::Sha256::digest(format!("invite/{network_id}/{user_id}").as_bytes());
    ::hex::encode(&digest[..6]).to_uppercase()
}

/// Encode the index a page resumes at as an opaque continuation token.
///
/// Opaque to the caller by construction: the only thing a client can do with it
/// is hand it back, which is exactly the `@paginated` contract.
pub fn encode_page_token(offset: usize) -> String {
    ::hex::encode(offset.to_string())
}

/// Why a listing's paging arguments were refused. Each listing turns this into
/// its own modelled `ValidationError`, which is the only part that differs
/// between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingProblem {
    /// `maxResults` was below one.
    MaxResults(i32),
    /// `nextToken` was not a token this service issued.
    NextToken,
}

impl PagingProblem {
    /// The input member the refusal names.
    pub fn field(&self) -> &'static str {
        match self {
            Self::MaxResults(_) => "maxResults",
            Self::NextToken => "nextToken",
        }
    }

    /// Why that member was refused, built from the request.
    pub fn reason(&self) -> String {
        match self {
            Self::MaxResults(max) => format!("maxResults must be at least 1, got {max}"),
            Self::NextToken => {
                "nextToken is not a continuation token this service issued".to_string()
            }
        }
    }
}

/// Validate a listing's paging arguments and answer the item index to resume at.
///
/// Shared by every paginated listing so they refuse the same arguments for the
/// same reasons.
pub fn page_start(
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<usize, PagingProblem> {
    if let Some(max) = max_results {
        if max < 1 {
            return Err(PagingProblem::MaxResults(max));
        }
    }
    match next_token {
        Some(token) => decode_page_token(token).ok_or(PagingProblem::NextToken),
        None => Ok(0),
    }
}

/// Decode a token [`encode_page_token`] produced, or `None` when it was not one.
fn decode_page_token(token: &str) -> Option<usize> {
    let raw = ::hex::decode(token).ok()?;
    ::std::str::from_utf8(&raw).ok()?.parse::<usize>().ok()
}

/// One page of `items` starting at `start`, plus the token for the page after it
/// (`None` when this page reaches the end).
///
/// `max_results` is the page size the caller asked for; it is clamped to
/// [`MAX_PAGE_SIZE`], which is also the page size used when the caller asked for
/// none, so the size of a response is bounded by the server rather than by the
/// store. Whatever the page size, following `nextToken` still walks every item
/// exactly once.
pub fn page<T: Clone>(
    items: &[T],
    start: usize,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    let requested = match max_results {
        Some(max) => usize::try_from(max).unwrap_or(0),
        None => MAX_PAGE_SIZE,
    };
    let limit = requested.min(MAX_PAGE_SIZE);
    let end = start.saturating_add(limit).min(items.len());
    let slice = items.get(start..end).unwrap_or_default().to_vec();
    let next = (end < items.len()).then(|| encode_page_token(end));
    (slice, next)
}

/// The sort directions `SortDirection` accepts, as the model declares them.
pub const SORT_DIRECTION_DESC: &str = "DESC";

/// `User$type` for a human account: the member documents "the descriptive type
/// of the user account (e.g., 'user')".
const USER_TYPE_USER: &str = "user";

/// The `userId` a failed batch item carries when it was refused before an id was
/// minted.
///
/// `BatchUserErrorResponseItem$userId` is `@required`, so an item that never got
/// an id still has to name one. Minted ids start at 1, so `0` can never collide
/// with a real user.
pub const UNASSIGNED_USER_ID: &str = "0";

/// One stored user as the modelled `User` shape, as of the instant `now`.
///
/// Shared by every operation that reports a user so they cannot disagree about
/// which members a user carries.
pub fn user_view(record: &UserRecord, now: i64) -> crate::ops::types::User {
    crate::ops::types::User {
        user_id: Some(record.user_id.clone()),
        first_name: record.first_name.clone(),
        last_name: record.last_name.clone(),
        username: Some(record.username.clone()),
        security_groups: Some(record.security_group_ids.clone()),
        is_admin: Some(record.is_admin),
        suspended: Some(record.suspended),
        status: Some(record.status),
        // Nothing in this slice enables OTP or records a failed challenge, so
        // both report the state a freshly created user is in.
        otp_enabled: Some(false),
        challenge_failures: Some(0),
        is_invite_expired: Some(record.invite_expired_at(now)),
        is_user: Some(true),
        r#type: Some(USER_TYPE_USER.to_string()),
        invite_code: Some(record.invite_code.clone()),
        code_validation: record.code_validation,
        uname: Some(record.uname.clone()),
        // `scimId` is "currently not used"; `cell` and `countryCode` are only set
        // by the operations that collect a phone number, none of which this slice
        // implements, so they are reported absent rather than invented.
        scim_id: None,
        cell: None,
        country_code: None,
    }
}
