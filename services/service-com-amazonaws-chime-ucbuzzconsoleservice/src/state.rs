// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chime-ucbuzzconsoleservice service.
//!
//! The Amazon Chime account is this service's root resource — rooms, bots, users and
//! phone-number associations all hang off `/accounts/{AccountId}` — so the accounts and
//! users this module stores are the records every other operation resolves against.
//! Resources are stored as the MODELED shapes (`crate::ops::types::{Account, User}`), so
//! a reader hands back exactly what the writer persisted rather than a second,
//! drifting copy of the same shape.

use std::collections::BTreeMap;

use crate::ops::types::{Account, User};

/// The account type `CreateAccount` mints. The operation's own doc says "Only Team
/// account types are currently supported for this action"; `Team` is one of the values
/// the `com.amazonaws.chime#AccountType` enum declares (Team, EnterpriseDirectory,
/// EnterpriseLWA, EnterpriseOIDC).
pub const ACCOUNT_TYPE_TEAM: &str = "Team";

/// A live account's status, from the `com.amazonaws.chime#AccountStatus` enum
/// (Suspended, Active). A created account is returned ready, so this is terminal: no
/// later transition is owed.
pub const ACCOUNT_STATUS_ACTIVE: &str = "Active";

/// The license a new account defaults to, from the `com.amazonaws.chime#License` enum
/// (Basic, Plus, Pro, ProTrial).
pub const DEFAULT_LICENSE: &str = "Pro";

/// The licenses an account supports, from the same `com.amazonaws.chime#License` enum.
/// `ProTrial` is excluded: it is the trial grant, not a license an account can assign.
pub const SUPPORTED_LICENSES: [&str; 3] = ["Basic", "Plus", "Pro"];

/// The user type `CreateUser` assumes when the request names none, from the
/// `com.amazonaws.chime#UserType` enum (PrivateUser, SharedDevice).
pub const USER_TYPE_PRIVATE_USER: &str = "PrivateUser";

/// The registration status of a user created directly on an account (as opposed to one
/// invited through `InviteUsers`, which is what moves `UserInvitationStatus`). From the
/// `com.amazonaws.chime#RegistrationStatus` enum (Unregistered, Registered, Suspended).
pub const REGISTRATION_STATUS_REGISTERED: &str = "Registered";

/// Per-service state for com-amazonaws-chime-ucbuzzconsoleservice. A handler reaches it through [`service_state`].
///
/// Both maps are `BTreeMap`s because the list operations page over them: a paginated
/// list needs a total order on the keys so a `NextToken` can name where a page ended
/// and the next page can resume after it without storing a per-request snapshot.
#[derive(Debug, Default)]
pub struct ComAmazonawsChimeUcbuzzconsoleserviceState {
    /// Every Amazon Chime account, keyed by its generated `AccountId`.
    pub accounts: BTreeMap<String, Account>,
    /// Every user, keyed by `(AccountId, UserId)` — so one account's users are a
    /// contiguous range, which is what `ListUsers` scans and `DeleteAccount` removes.
    pub users: BTreeMap<(String, String), User>,
    /// How many accounts have been minted; the source of the next `AccountId`.
    account_seq: u64,
    /// How many users have been minted; the source of the next `UserId` and PIN.
    user_seq: u64,
}

impl ComAmazonawsChimeUcbuzzconsoleserviceState {
    /// Mint the next `AccountId`. Unique and stable: the caller stores the account under
    /// it in the same lock scope it reports it in.
    pub fn next_account_id(&mut self) -> String {
        self.account_seq = self.account_seq.saturating_add(1);
        opaque_id(self.account_seq ^ ACCOUNT_ID_SALT)
    }

    /// Mint the next `UserId` together with that user's personal meeting PIN — one
    /// sequence bump, so the two can never disagree about which user they belong to.
    pub fn next_user_identity(&mut self) -> (String, String) {
        self.user_seq = self.user_seq.saturating_add(1);
        let user_id = opaque_id(self.user_seq ^ USER_ID_SALT);
        // A Chime personal meeting PIN is a 10-digit number; keeping it in
        // `[1_000_000_000, …)` keeps every PIN exactly ten digits and distinct.
        let pin = format!("{:010}", 1_000_000_000_u64.saturating_add(self.user_seq));
        (user_id, pin)
    }

    /// The users stored under `account_id`, in `UserId` order.
    pub fn users_of<'a>(&'a self, account_id: &'a str) -> impl Iterator<Item = &'a User> + 'a {
        self.users
            .range((account_id.to_string(), String::new())..)
            .take_while(move |((acct, _), _)| acct == account_id)
            .map(|(_, user)| user)
    }
}

/// Keeps account ids and user ids from colliding even though both are minted from a
/// small counter through the same mixer.
const ACCOUNT_ID_SALT: u64 = 0x4163_636F_756E_7400;
/// The user-id counterpart of [`ACCOUNT_ID_SALT`].
const USER_ID_SALT: u64 = 0x5573_6572_4964_0000;

/// A server-generated opaque id for `seq`.
///
/// SplitMix64's finalizer is a bijection on `u64`, so distinct sequence numbers always
/// produce distinct ids while the id itself does not read as a counter. It depends only
/// on the store's own sequence — never on anything in the request.
fn opaque_id(seq: u64) -> String {
    let mut z = seq.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    format!("{z:016x}")
}

/// One page of a paginated list operation.
pub struct Page<T> {
    /// The resources on this page, in key order.
    pub items: Vec<T>,
    /// Where the next page resumes, or `None` on the last page.
    pub next_token: Option<String>,
}

/// The page size a list operation uses when the request names no `MaxResults`: both
/// `ListAccounts` and `ListUsers` document that member as "The maximum number of results
/// to return in a single call. Defaults to 100."
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// [`DEFAULT_MAX_RESULTS`] as a page length.
const DEFAULT_PAGE_SIZE: usize = 100;

/// How many accounts this emulator's store will hold.
///
/// An EMULATOR capacity guard, not a published AWS quota: the store is in memory, so
/// without a ceiling a caller could grow it until the process dies. It is far above any
/// realistic test or demo, so it changes no legitimate caller's behaviour, and the
/// refusal at the ceiling is the service's own `ResourceLimitExceededException` ("The
/// request exceeds the resource limit", HTTP 400).
pub const MAX_ACCOUNTS: usize = 1_000;

/// How many users this emulator's store will hold under one account. The same emulator
/// capacity guard as [`MAX_ACCOUNTS`], and what bounds the per-account scans
/// ([`ComAmazonawsChimeUcbuzzconsoleserviceState::users_of`]) that `CreateUser` and the
/// list operations walk.
pub const MAX_USERS_PER_ACCOUNT: usize = 1_000;

/// The longest `NextToken` this service will even look at.
///
/// A token it issues is the hex of one store key (32 characters for an account id, the
/// same for a user id) and the model puts no `@length` on `NextToken`, so without this a
/// caller could hand over a megabyte-long token and have it decoded. Anything longer is
/// a token this service did not issue, which the list operations already refuse with
/// `BadRequestException`.
const MAX_PAGE_TOKEN_CHARS: usize = 256;

/// Cut one page out of `selected` — the resources a list operation matched, paired with
/// their store keys and in key order.
///
/// `after` is the decoded `NextToken`: the page resumes strictly after that key, so a
/// resource is reported exactly once across the pages of a walk. The page holds at most
/// `max_results` resources and carries a `NextToken` only when resources remain beyond
/// it — the last page carries none.
pub fn page_of<T>(selected: Vec<(String, T)>, after: Option<&str>, max_results: i32) -> Page<T> {
    let mut remaining: Vec<(String, T)> = match after {
        Some(after) => selected
            .into_iter()
            .filter(|(key, _)| key.as_str() > after)
            .collect(),
        None => selected,
    };
    // `MaxResults` is modelled @range(1..200), so the boundary refuses a non-positive
    // value before any handler runs; a caller of this helper whose page size is not
    // modelled gets the documented default rather than an empty success.
    let limit = usize::try_from(max_results)
        .ok()
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_PAGE_SIZE);
    let truncated = limit < remaining.len();
    remaining.truncate(limit);
    let next_token = if truncated {
        remaining.last().map(|(key, _)| encode_page_token(key))
    } else {
        None
    };
    Page {
        items: remaining.into_iter().map(|(_, item)| item).collect(),
        next_token,
    }
}

/// Encode the key a page ended on as an opaque `NextToken`.
///
/// Hex rather than the bare key so the token is opaque to a client (which must only
/// hand it back, never parse it) and can carry any byte a key holds.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key)
}

/// Decode a `NextToken` produced by [`encode_page_token`], or `None` when the client
/// sent something this service never issued — including anything longer than
/// [`MAX_PAGE_TOKEN_CHARS`], which is refused before a byte is allocated for it.
pub fn decode_page_token(token: &str) -> Option<String> {
    if token.len() > MAX_PAGE_TOKEN_CHARS {
        return None;
    }
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimeUcbuzzconsoleserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimeUcbuzzconsoleserviceState>)
}
