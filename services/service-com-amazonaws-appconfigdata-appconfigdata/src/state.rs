// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appconfigdata-appconfigdata service.
//!
//! AppConfigData is a two-operation polling data plane, so the only thing it owns is
//! one map: [`ComAmazonawsAppconfigdataAppconfigdataState::sessions`], every live
//! configuration session keyed by the opaque token the client currently holds.
//! `StartConfigurationSession` inserts one; each `GetLatestConfiguration` consumes the
//! token it was called with and inserts the rotated successor, so the token a client
//! holds is always resolvable until it is used or expires.
//!
//! Everything else a poll answers with — which applications, environments and
//! configuration profiles exist, which configuration version is deployed into an
//! environment, and the bytes of that version — belongs to the AppConfig CONTROL
//! PLANE, and is read out of that service's own store through the one shared engine
//! rather than copied into a second map here ([`resolve_triple`],
//! [`served_configuration`]). A session therefore resolves against the same records
//! AppConfig's own operations report, and a configuration deployed through
//! `StartDeployment` is the one a poll serves.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use ::service_com_amazonaws_appconfig_amazonappconfig::state::{
    ComAmazonawsAppconfigAmazonappconfigState as AppConfigState, Deployment, DEPLOYMENT_COMPLETE,
};

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
    /// The configuration version this client has already received, as
    /// [`ServedConfiguration::version`] names it; `None` before its first poll, so the
    /// first poll always returns content.
    pub last_served_version: Option<String>,
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
}

/// Per-service state for com-amazonaws-appconfigdata-appconfigdata. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppconfigdataAppconfigdataState {
    /// Live sessions, keyed by the configuration token that resolves to them.
    pub sessions: HashMap<String, Session>,
    /// How many tokens this store has minted; part of the token material, so no two
    /// tokens are ever derived from the same input.
    tokens_minted: u64,
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

// =============================================================================
// The AppConfig control plane, read through the one shared engine
// =============================================================================

/// The modelled `ResourceType` values this service reports a missing resource as, from
/// `RESOURCE_TYPE_VALUES` in `crate::ops::types`.
pub const RESOURCE_TYPE_APPLICATION: &str = "Application";
/// See [`RESOURCE_TYPE_APPLICATION`].
pub const RESOURCE_TYPE_ENVIRONMENT: &str = "Environment";
/// See [`RESOURCE_TYPE_APPLICATION`].
pub const RESOURCE_TYPE_CONFIGURATION_PROFILE: &str = "ConfigurationProfile";
/// See [`RESOURCE_TYPE_APPLICATION`].
pub const RESOURCE_TYPE_CONFIGURATION: &str = "Configuration";

/// The `StartConfigurationSession` input members that name each level of the
/// hierarchy, which the modelled `ReferencedBy` map is keyed by.
pub const APPLICATION_IDENTIFIER_MEMBER: &str = "ApplicationIdentifier";
/// See [`APPLICATION_IDENTIFIER_MEMBER`].
pub const ENVIRONMENT_IDENTIFIER_MEMBER: &str = "EnvironmentIdentifier";
/// See [`APPLICATION_IDENTIFIER_MEMBER`].
pub const CONFIGURATION_PROFILE_IDENTIFIER_MEMBER: &str = "ConfigurationProfileIdentifier";

/// A level of the AppConfig hierarchy a request named and the control plane does not
/// hold, carrying everything the modelled `ResourceNotFoundException` reports.
#[derive(Debug, Clone)]
pub struct UnresolvedResource {
    /// The modelled `ResourceType` value for the level that is missing.
    pub resource_type: &'static str,
    /// The input member that named it, and the identifier it carried — the modelled
    /// `ReferencedBy` is "a map indicating which parameters in the REQUEST reference
    /// the resource that was not found", so it is filled only where the request
    /// itself named the resource. A poll names only its token, so a poll leaves it
    /// absent.
    pub referenced_by: Option<(&'static str, String)>,
    /// What the refusal says, built from the request rather than stored as prose.
    pub message: String,
}

impl UnresolvedResource {
    /// A level of the hierarchy the request named by `member` and the control plane
    /// does not hold.
    ///
    /// The message is the service's OWN fixed text for that level — the modelled
    /// `ResourceType` value followed by "not found" — and carries nothing from the
    /// request: WHICH identifier failed to resolve travels in `ReferencedBy`, which is
    /// the member a client reads it from.
    fn named(resource_type: &'static str, member: &'static str, identifier: &str) -> Self {
        Self {
            resource_type,
            referenced_by: Some((member, identifier.to_string())),
            message: format!("{resource_type} not found"),
        }
    }

    /// The same missing level, reported for a call whose own input did not name it:
    /// the identifiers came from the session the token resolved to, so there is no
    /// request parameter to key `ReferencedBy` by.
    fn unreferenced(self) -> Self {
        Self {
            referenced_by: None,
            ..self
        }
    }

    /// No configuration is deployed for a triple that does resolve, or the version the
    /// deployment names is not in AppConfig's hosted configuration store.
    fn configuration(message: String) -> Self {
        Self {
            resource_type: RESOURCE_TYPE_CONFIGURATION,
            referenced_by: None,
            message,
        }
    }
}

/// The configuration AppConfig currently serves for one (application, environment,
/// configuration profile) triple.
#[derive(Debug, Clone)]
pub struct ServedConfiguration {
    /// The configuration content, byte for byte as the hosted version holds it.
    pub content: Vec<u8>,
    /// The MIME type the hosted version declares, reported as `ContentType`.
    pub content_type: String,
    /// The hosted version's label, when it carries one.
    pub version_label: Option<String>,
    /// Which configuration version this is, as [`Session::last_served_version`]
    /// records it: the hosted version number the in-force deployment names. Comparing
    /// it with what the session was last served is what makes "the client already has
    /// the latest version" a comparison of two stored values.
    pub version: String,
}

/// The AppConfig control plane's own store, reached through the one shared engine.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one),
/// which a handler answers with the same modelled server error it uses for its own
/// store being unreachable.
fn app_config_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<AppConfigState>> {
    ::service_com_amazonaws_appconfig_amazonappconfig::state::service_state(engine)
}

/// Resolve a session's three identifiers against the AppConfig control plane, as
/// `StartConfigurationSession` must before it mints a session.
///
/// Each identifier is "the ID or the name" of its level, so each resolves either way,
/// and the levels are resolved outermost first: an environment or a profile is a child
/// of the application, so a request naming an application that does not exist is
/// refused on the application rather than on its children.
///
/// A one-directional, guard-free read of the AppConfig store: it takes that store's
/// lock, holds no other while it does, and hands back owned values — so it is called
/// with NO guard of this service's own store held.
pub fn resolve_triple<Eng: Send + Sync + 'static>(
    engine: &Eng,
    application_identifier: &str,
    environment_identifier: &str,
    configuration_profile_identifier: &str,
) -> Option<Result<(), UnresolvedResource>> {
    let state = app_config_state(engine)?;
    let store = state.lock();
    Some(
        resolve_in(
            &store,
            application_identifier,
            environment_identifier,
            configuration_profile_identifier,
        )
        .map(|_| ()),
    )
}

/// The configuration a session over this triple polls, at `now`.
///
/// The configuration in force for a configuration profile is the one the most recent
/// deployment into that environment deployed, counting only deployments whose rollout
/// has begun and has not been undone — a deployment a caller stopped is rolling back
/// to, or has restored, the configuration that preceded it, so its own version is not
/// what the data plane serves.
///
/// Reporting-only: the environment is brought up to whatever state the CLOCK already
/// owes it on a COPY, so a poll never writes to the control plane.
pub fn served_configuration<Eng: Send + Sync + 'static>(
    engine: &Eng,
    application_identifier: &str,
    environment_identifier: &str,
    configuration_profile_identifier: &str,
    now: SystemTime,
) -> Option<Result<ServedConfiguration, UnresolvedResource>> {
    let state = app_config_state(engine)?;
    let store = state.lock();
    let resolved = match resolve_in(
        &store,
        application_identifier,
        environment_identifier,
        configuration_profile_identifier,
    ) {
        // The poll's own input named none of these levels — see
        // `UnresolvedResource::referenced_by`.
        Err(unresolved) => return Some(Err(unresolved.unreferenced())),
        Ok(resolved) => resolved,
    };
    // Every id came out of the resolution above, so each lookup holds.
    let application = store.application(&resolved.application_id)?;
    let profile = application.profiles.get(&resolved.configuration_profile_id)?;
    let environment = application.environments.get(&resolved.environment_id)?;

    // A COPY is materialised: a deployment whose rollout window and bake time have
    // both elapsed is `COMPLETE` whether or not anybody has read it, and bringing the
    // copy up to date leaves the stored record untouched.
    let mut environment = environment.clone();
    environment.materialize(now);
    let in_force = environment
        .deployments
        .values()
        .filter(|deployment| {
            deployment.configuration_profile_id == resolved.configuration_profile_id
                && configuration_is_in_force(deployment)
        })
        .max_by_key(|deployment| deployment.deployment_number);

    let Some(deployment) = in_force else {
        return Some(Err(UnresolvedResource::configuration(format!(
            "No deployed configuration found for application '{application_identifier}', \
             environment '{environment_identifier}' and configuration profile \
             '{configuration_profile_identifier}'."
        ))));
    };
    let Some(version) = profile.resolve_hosted_version(&deployment.configuration_version) else {
        return Some(Err(UnresolvedResource::configuration(format!(
            "Configuration version '{version}' of configuration profile \
             '{configuration_profile_identifier}' is not held in the AppConfig hosted \
             configuration store.",
            version = deployment.configuration_version,
        ))));
    };
    Some(Ok(ServedConfiguration {
        content: version.content.clone(),
        content_type: version.content_type.clone(),
        version_label: version.version_label.clone(),
        version: version.version_number.to_string(),
    }))
}

/// Whether the configuration a deployment deployed is the one AppConfig is serving: it
/// is from the moment the rollout begins (`DEPLOYING`, then `BAKING`, then `COMPLETE`),
/// and stops being so once a caller's stop starts rolling it back.
fn configuration_is_in_force(deployment: &Deployment) -> bool {
    deployment.is_in_flight() || deployment.state == DEPLOYMENT_COMPLETE
}

/// The ids a triple resolves to inside an already-locked AppConfig store.
struct ResolvedTriple {
    application_id: String,
    environment_id: String,
    configuration_profile_id: String,
}

/// Resolve the three identifiers against `store`, outermost level first.
fn resolve_in(
    store: &AppConfigState,
    application_identifier: &str,
    environment_identifier: &str,
    configuration_profile_identifier: &str,
) -> Result<ResolvedTriple, UnresolvedResource> {
    let application = store.resolve_application(application_identifier).ok_or_else(|| {
        UnresolvedResource::named(
            RESOURCE_TYPE_APPLICATION,
            APPLICATION_IDENTIFIER_MEMBER,
            application_identifier,
        )
    })?;
    let environment = application
        .resolve_environment(environment_identifier)
        .ok_or_else(|| {
            UnresolvedResource::named(
                RESOURCE_TYPE_ENVIRONMENT,
                ENVIRONMENT_IDENTIFIER_MEMBER,
                environment_identifier,
            )
        })?;
    let profile = application
        .resolve_profile(configuration_profile_identifier)
        .ok_or_else(|| {
            UnresolvedResource::named(
                RESOURCE_TYPE_CONFIGURATION_PROFILE,
                CONFIGURATION_PROFILE_IDENTIFIER_MEMBER,
                configuration_profile_identifier,
            )
        })?;
    Ok(ResolvedTriple {
        application_id: application.id.clone(),
        environment_id: environment.id.clone(),
        configuration_profile_id: profile.id.clone(),
    })
}
