// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplify-amplify service.
//!
//! The persisted record for an app IS the modelled [`App`] structure, and the
//! persisted record for a branch IS the modelled [`Branch`] structure. Keeping one
//! copy of each resource — rather than a parallel struct with the same ~30 members
//! plus a conversion — is what makes `GetApp` after `CreateApp` return the created
//! record field-for-field: a reader clones what the writer stored, so the two cannot
//! drift.
//!
//! Branches live INSIDE their owning app, so `DeleteApp` removes them transitively
//! and `ListBranches` can never reach another app's branches.

use std::collections::BTreeMap;

use crate::ops::types::{App, Branch};

/// One Amplify app as this service persists it: the modelled record it hands back,
/// plus the branches created on it, keyed by branch name.
///
/// The maps are `BTreeMap`s, not `HashMap`s, because both list operations are
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `nextToken` a page returns is the key of the first record it did NOT return, so
/// the next call resumes at that key.
#[derive(Debug, Clone)]
pub struct AmplifyApp {
    /// The app record, exactly as every reader returns it.
    pub app: App,
    /// The branches of this app, keyed by `branchName`.
    pub branches: BTreeMap<String, Branch>,
}

impl AmplifyApp {
    /// A newly created app with no branches.
    pub fn new(app: App) -> Self {
        Self {
            app,
            branches: BTreeMap::new(),
        }
    }
}

/// Per-service state for com-amazonaws-amplify-amplify. A handler reaches it through [`service_state`].
///
/// The apps map is NOT partitioned by region: an app's ARN records the region of the
/// request that created it, but any later call reaches every app in the store
/// whatever region it is signed for. Partitioning the control plane by region is a
/// separate change; nothing here claims it.
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifyAmplifyState {
    /// Every app that exists, keyed by its minted `appId`.
    pub apps: BTreeMap<String, AmplifyApp>,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_app_id`].
    pub next_app_id_seq: u64,
}

impl ComAmazonawsAmplifyAmplifyState {
    /// Mint an `appId` no app in state is using.
    ///
    /// `AppId` is modelled `length 1..20, pattern ^d[a-z0-9]+$`
    /// (`smithy_query(query="shape",shape="App")`), so the id is a literal `d`
    /// followed by 13 base-36 digits — 14 characters, inside the bound, and inside
    /// the pattern's alphabet.
    ///
    /// The 13 digits come from a monotonic counter mixed with the wall clock through
    /// the SplitMix64 finaliser, so ids do not expose a guessable sequence, and the
    /// loop re-mints on the (astronomically unlikely) collision, so two `CreateApp`
    /// calls can never be handed the same id.
    pub fn mint_app_id(&mut self) -> String {
        loop {
            self.next_app_id_seq = self.next_app_id_seq.wrapping_add(1);
            let clock = ::std::time::SystemTime::now()
                .duration_since(::std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
            let mixed = splitmix64(self.next_app_id_seq ^ clock.rotate_left(17));
            let candidate = format!("d{}", base36_digits(mixed));
            if !self.apps.contains_key(&candidate) {
                return candidate;
            }
        }
    }
}

/// The SplitMix64 finaliser: an invertible bit mixer, so distinct inputs give
/// distinct outputs and a counter does not show through the minted id.
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly 13 base-36 digits over `[0-9a-z]`, zero-padded on the left.
///
/// 36^13 > `u64::MAX`, so 13 digits represent every `u64` without truncation.
fn base36_digits(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = [b'0'; 13];
    for slot in out.iter_mut().rev() {
        let digit = usize::try_from(value % 36).unwrap_or(0);
        *slot = DIGITS[digit];
        value /= 36;
    }
    out.iter().map(|byte| char::from(*byte)).collect()
}

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own shared
/// AWS config names, so an unsigned call and a default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the one
/// in its SigV4 credential scope, so that is what an ARN minted while serving it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Now, truncated to milliseconds.
///
/// restJson1 writes a timestamp as epoch seconds with a fractional part, i.e. as a
/// JSON number: an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond, so a nanosecond-precision instant would not survive the round trip
/// and `createTime` would differ between the create response and a later read.
/// Milliseconds round-trip exactly, which keeps a stored timestamp stable across reads.
pub fn now() -> ::aws_smithy_types::DateTime {
    let millis = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The page size to serve for a `@paginated` operation whose `maxResults` member is
/// `max_results` and whose modelled `range` maximum is `modelled_max`.
///
/// `maxResults` is modelled with `default: 0`, so an absent member arrives as `0`:
/// that is "the caller set no bound", which the operation serves with its modelled
/// maximum rather than with an empty page. A caller-supplied bound is already inside
/// the modelled range by the time a handler runs (the boundary validator refuses
/// anything else); clamping again costs nothing and keeps this total.
pub fn page_size(max_results: i32, modelled_max: usize) -> usize {
    match usize::try_from(max_results) {
        Ok(0) | Err(_) => modelled_max,
        Ok(bound) => bound.min(modelled_max),
    }
}

/// One page of an ordered `(key, record)` sequence, plus the `nextToken` to continue
/// from — `None` when the page reached the end.
///
/// The token is the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts
/// at the first key at or after the token. Resuming is therefore well defined even
/// when the record the token names was deleted between the two calls: the scan simply
/// continues at the next key.
pub fn page<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if next_token.is_some_and(|token| key.as_str() < token) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(key));
        }
        records.push(record);
    }
    (records, None)
}

/// The `message` an Amplify `NotFoundException` carries for a resource the request
/// addressed and this store does not hold.
///
/// The text is built FROM THE REQUEST and is a complete sentence: the kind of entity,
/// the identifier the request used for it, and `not found` closed by the
/// sentence-terminating period. Every 404 in this crate is formatted through this one
/// function, so `GetApp`, `DeleteApp`, `GetBranch`, `DeleteBranch`, `CreateBranch` and
/// `ListBranches` cannot drift from one another — and an identifier is NEVER inspected
/// here, so the message is the same construction whatever the request named.
pub fn not_found_message(entity: &str, identifier: &str) -> String {
    format!("{entity} {identifier} not found.")
}

/// The entity name [`not_found_message`] uses for an app addressed by `appId`.
pub const APP_ENTITY: &str = "App";

/// The entity name [`not_found_message`] uses for a branch addressed by `branchName`.
pub const BRANCH_ENTITY: &str = "Branch";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifyAmplifyState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifyAmplifyState>)
}
