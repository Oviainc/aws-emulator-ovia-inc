// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appconfigdata-appconfigdata service.
//!
//! AppConfigData is a two-operation polling data plane, so its whole state is two
//! maps:
//!
//! * [`ComAmazonawsAppconfigdataAppconfigdataState::sessions`] — every live
//!   configuration session, keyed by the opaque token the client currently holds.
//!   `StartConfigurationSession` inserts one; each `GetLatestConfiguration` consumes
//!   the token it was called with and inserts the rotated successor, so the token a
//!   client holds is always resolvable until it is used or expires.
//! * [`ComAmazonawsAppconfigdataAppconfigdataState::configurations`] — the
//!   configuration deployed for an (application, environment, configuration profile)
//!   triple, with a monotonic `version` that is bumped whenever the deployed bytes,
//!   content type or version label change. A session remembers the version it was
//!   last served, which is what makes "the client already has the latest version"
//!   a comparison of two stored values rather than a guess.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// How long a configuration token stays usable after it is minted.
///
/// The model documents the lifetime on both `ConfigurationToken` and
/// `NextPollConfigurationToken`: "To support long poll use cases, the token is valid
/// for up to 24 hours. If a `GetLatestConfiguration` call uses an expired token, the
/// system returns `BadRequestException`." A documented duration keeps its documented
/// value.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The poll interval reported to a session that asked for no minimum of its own.
///
/// `RequiredMinimumPollIntervalInSeconds` is optional, and
/// `NextPollIntervalInSeconds` is required, so a session that set no minimum still
/// has to be told how long to wait. AppConfig's polling guidance is one poll a
/// minute (the model's own example for the minimum is 60 seconds), so that is the
/// interval this emulator reports when the session named none. A session that DID
/// name a minimum is told exactly its own minimum, because it may not poll faster
/// than that.
pub const DEFAULT_POLL_INTERVAL_IN_SECONDS: i32 = 60;

/// Upper bound on the number of entries [`ComAmazonawsAppconfigdataAppconfigdataState::sessions`]
/// holds.
///
/// Every poll stores the successor token's session, so a client polling in a loop
/// would otherwise grow the map for as long as the process lives. Expiry alone does
/// not bound it — a token lives up to [`TOKEN_LIFETIME`] — so the store also keeps a
/// hard cap, past which the entries that can no longer serve a poll are dropped
/// first (consumed ones, then the ones nearest their expiry). A real client holds one
/// token at a time, so the cap is reached only by a request loop, and the token it
/// is holding is the last thing evicted.
pub const MAX_SESSIONS: usize = 4_096;

/// How long a consumed token's entry is kept after the poll that used it.
///
/// It is kept only so a client that retries the token it just used is told `Expired`
/// rather than `Corrupted`; both are the same modelled `BadRequestException` with the
/// same 400, so reclaiming the entry afterwards changes no status and no error code.
pub const CONSUMED_TOKEN_RETENTION: Duration = Duration::from_secs(5 * 60);

/// The triple that identifies one deployed configuration: application, environment
/// and configuration profile, exactly as `StartConfigurationSession` names them.
pub type ConfigurationKey = (String, String, String);

/// When a token minted at `now` stops being usable.
///
/// `SystemTime + Duration` panics on an unrepresentable result, so a grossly
/// misconfigured host clock must not unwind through a handler: it falls back to
/// `now`, which makes the token expire immediately (a modelled `BadRequestException`)
/// rather than aborting the request.
fn expiry(now: SystemTime) -> SystemTime {
    now.checked_add(TOKEN_LIFETIME).unwrap_or(now)
}

/// One configuration session: the triple it polls, the poll floor it asked for, and
/// how far through the configuration's version history the client has been served.
///
/// A session is stored once per token: rotating a token moves the (updated) session
/// to a new key and marks the old entry consumed, so a replayed token is
/// distinguishable from one that was never issued.
#[derive(Debug, Clone)]
pub struct Session {
    /// `ApplicationIdentifier` as the session was started with.
    pub application_identifier: String,
    /// `EnvironmentIdentifier` as the session was started with.
    pub environment_identifier: String,
    /// `ConfigurationProfileIdentifier` as the session was started with.
    pub configuration_profile_identifier: String,
    /// `RequiredMinimumPollIntervalInSeconds`, when the session set one.
    pub required_minimum_poll_interval_in_seconds: Option<i32>,
    /// The [`DeployedConfiguration::version`] this client has already received;
    /// `None` before its first poll, so the first poll always returns content.
    pub last_served_version: Option<u64>,
    /// When the session itself was started.
    pub created_at: SystemTime,
    /// When THIS token stops being usable ([`TOKEN_LIFETIME`] after it was minted).
    pub expires_at: SystemTime,
    /// When this session was last polled, which is what the session's own minimum
    /// poll interval is measured from. `None` until the first poll.
    pub last_polled_at: Option<SystemTime>,
    /// When the token this entry is keyed by was used for a poll, if it was. A token
    /// is valid for exactly one call, so an entry with a consumption instant is
    /// refused — and reclaimed [`CONSUMED_TOKEN_RETENTION`] later.
    pub consumed_at: Option<SystemTime>,
}

impl Session {
    /// Whether the token this entry is keyed by has already served a poll.
    pub fn is_consumed(&self) -> bool {
        self.consumed_at.is_some()
    }

    /// When this entry was last touched: the poll that minted its token, or — for a
    /// session's first token, which no poll has used yet — when the session started.
    /// The order the store reclaims by when it is at its cap.
    pub fn last_touched_at(&self) -> SystemTime {
        self.last_polled_at.unwrap_or(self.created_at)
    }

    /// The configuration this session polls.
    pub fn configuration_key(&self) -> ConfigurationKey {
        (
            self.application_identifier.clone(),
            self.environment_identifier.clone(),
            self.configuration_profile_identifier.clone(),
        )
    }
}

/// The configuration currently deployed for one triple.
#[derive(Debug, Clone)]
pub struct DeployedConfiguration {
    /// The configuration content a poll returns as the `Configuration` payload.
    pub content: Vec<u8>,
    /// The MIME type describing `content`, returned as the `ContentType` header.
    pub content_type: String,
    /// The hosted-configuration-version label, when the deployment has one.
    pub version_label: Option<String>,
    /// Monotonic version of this deployment, bumped on every change. A session
    /// compares it against [`Session::last_served_version`] to decide whether the
    /// client already has the latest version.
    pub version: u64,
}

/// Per-service state for com-amazonaws-appconfigdata-appconfigdata. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppconfigdataAppconfigdataState {
    /// Live sessions, keyed by the configuration token that resolves to them.
    pub sessions: HashMap<String, Session>,
    /// The configuration deployed per (application, environment, profile) triple.
    pub configurations: HashMap<ConfigurationKey, DeployedConfiguration>,
    /// How many tokens this store has minted; part of the token material, so no two
    /// tokens are ever derived from the same input.
    tokens_minted: u64,
    /// How many configuration changes this store has recorded; the source of
    /// [`DeployedConfiguration::version`], so a version is monotonic across every
    /// triple.
    deployments_recorded: u64,
}

impl ComAmazonawsAppconfigdataAppconfigdataState {
    /// Start a session over `(application, environment, profile)` and return the
    /// token that resolves to it.
    pub fn start_session(
        &mut self,
        application_identifier: &str,
        environment_identifier: &str,
        configuration_profile_identifier: &str,
        required_minimum_poll_interval_in_seconds: Option<i32>,
        now: SystemTime,
    ) -> String {
        let session = Session {
            application_identifier: application_identifier.to_string(),
            environment_identifier: environment_identifier.to_string(),
            configuration_profile_identifier: configuration_profile_identifier.to_string(),
            required_minimum_poll_interval_in_seconds,
            last_served_version: None,
            created_at: now,
            expires_at: expiry(now),
            last_polled_at: None,
            consumed_at: None,
        };
        self.store_session(session, now)
    }

    /// Store `session` under a freshly minted token and return that token.
    ///
    /// Used both to start a session and to rotate one: the caller updates the
    /// session it read (its served version, its poll instant) and stores it back,
    /// which is what gives the client a new single-use token pointing at the
    /// progressed session. Storing also reclaims the entries this one supersedes,
    /// so repeated polling cannot grow the store without limit.
    pub fn store_session(&mut self, mut session: Session, now: SystemTime) -> String {
        self.reclaim_sessions(now);
        let token = self.mint_token(now);
        session.expires_at = expiry(now);
        session.consumed_at = None;
        self.sessions.insert(token.clone(), session);
        token
    }

    /// The session `token` resolves to, if any.
    pub fn session(&self, token: &str) -> Option<&Session> {
        self.sessions.get(token)
    }

    /// Mark `token` used at `now`, so a second call with it is not taken for a fresh
    /// poll.
    pub fn consume_token(&mut self, token: &str, now: SystemTime) {
        if let Some(session) = self.sessions.get_mut(token) {
            session.consumed_at = Some(now);
        }
    }

    /// Drop the session entries that can no longer serve a poll, and hold the store
    /// to [`MAX_SESSIONS`].
    ///
    /// Three classes go, in order: an entry past its own expiry (its token is refused
    /// either way), a consumed entry older than [`CONSUMED_TOKEN_RETENTION`], and —
    /// only if the store is still at the cap — the LEAST RECENTLY TOUCHED entries:
    /// consumed ones first, then the ones whose token was minted or last polled
    /// longest ago. So under a mint flood an idle session is evicted before an active
    /// one, and the entry the current call is about to store always survives it; a
    /// client that holds one token at a time never reaches the cap at all.
    fn reclaim_sessions(&mut self, now: SystemTime) {
        self.sessions.retain(|_, session| {
            if now >= session.expires_at {
                return false;
            }
            session.consumed_at.is_none_or(|consumed_at| {
                consumed_at
                    .checked_add(CONSUMED_TOKEN_RETENTION)
                    .is_none_or(|until| now < until)
            })
        });
        // One slot for the entry about to be stored.
        if self.sessions.len() < MAX_SESSIONS {
            return;
        }
        let excess = self.sessions.len() + 1 - MAX_SESSIONS;
        let mut candidates: Vec<(bool, SystemTime, String)> = self
            .sessions
            .iter()
            .map(|(token, session)| {
                (!session.is_consumed(), session.last_touched_at(), token.clone())
            })
            .collect();
        // `!is_consumed` sorts the consumed entries first, then the oldest activity;
        // the token breaks a tie so eviction does not depend on hash order.
        candidates.sort();
        for (_, _, token) in candidates.into_iter().take(excess) {
            self.sessions.remove(&token);
        }
    }

    /// The configuration deployed for `key`, if one is.
    pub fn deployed_configuration(&self, key: &ConfigurationKey) -> Option<&DeployedConfiguration> {
        self.configurations.get(key)
    }

    /// Record the configuration deployed for a triple and return its version.
    ///
    /// The version is bumped only when something a poll would return actually
    /// differs, so redeploying identical content does not make every session's next
    /// poll re-download it.
    pub fn deploy_configuration(
        &mut self,
        application_identifier: &str,
        environment_identifier: &str,
        configuration_profile_identifier: &str,
        content: impl Into<Vec<u8>>,
        content_type: &str,
        version_label: Option<&str>,
    ) -> u64 {
        let key = (
            application_identifier.to_string(),
            environment_identifier.to_string(),
            configuration_profile_identifier.to_string(),
        );
        let content = content.into();
        let version_label = version_label.map(str::to_string);
        if let Some(current) = self.configurations.get(&key) {
            if current.content == content
                && current.content_type == content_type
                && current.version_label == version_label
            {
                return current.version;
            }
        }
        self.deployments_recorded += 1;
        let version = self.deployments_recorded;
        self.configurations.insert(
            key,
            DeployedConfiguration {
                content,
                content_type: content_type.to_string(),
                version_label,
                version,
            },
        );
        version
    }

    /// Mint an opaque token: the SHA-256 of this store's mint counter and the
    /// current instant, hex-encoded.
    ///
    /// It carries none of the request's values — the session's identifiers are
    /// looked up THROUGH the token, never recovered from it — and the counter makes
    /// two tokens minted for identical input differ. 64 hex characters satisfy the
    /// model's `^\S{1,8192}$` token pattern.
    fn mint_token(&mut self, now: SystemTime) -> String {
        let nanos = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        loop {
            self.tokens_minted += 1;
            let mut hasher = Sha256::new();
            hasher.update(self.tokens_minted.to_be_bytes());
            hasher.update(nanos.to_be_bytes());
            let token = hex::encode(hasher.finalize());
            // A fresh token must not collide with a live one: resolving a token has
            // to name exactly one session.
            if !self.sessions.contains_key(&token) {
                return token;
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppconfigdataAppconfigdataState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppconfigdataAppconfigdataState>)
}
