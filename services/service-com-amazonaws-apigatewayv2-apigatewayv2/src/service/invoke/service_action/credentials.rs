// SPDX-License-Identifier: CC0-1.0

//! The CREDENTIAL a first-class service-action integration's call is made with: a SESSION
//! of the role its `CredentialsArn` names.
//!
//! "Specifies the credentials required for the integration, if any. For AWS integrations,
//! three options are available. To specify an IAM Role for API Gateway to assume, use the
//! role's Amazon Resource Name (ARN)" (`Integration$CredentialsArn`). So the call an
//! `AWS_PROXY` subtype integration issues is a call made BY THAT ROLE, and the service
//! serving it judges it against the policies in force on the role and against its own
//! resource policy, exactly as it judges any other signed request. That is the whole point
//! of the member: two routes whose integrations name roles carrying different grants are
//! refused different things, and an integration whose role does not hold the action's
//! permission performs nothing.
//!
//! Where that credential comes from:
//!
//! * the role is resolved against the IAM store of the SAME shared engine, which is where
//!   the account's roles live, so the session speaks for the very record `iam:CreateRole`
//!   wrote;
//! * the session is minted by the TOKEN SERVICE's own mint
//!   ([`sts_store::mint_role_session`]), so every service that authorizes a request resolves
//!   it the same way and the credential is one the emulator's own request verifier accepts.
//!   Nothing here constructs an access key id or a secret;
//! * its identity is the assumed-role session AWS reports for such a call —
//!   `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, under the
//!   session name API Gateway opens an integration role's session with
//!   ([`SESSION_NAME`]) — which is the principal a refusal the integrated service answers
//!   names, so a client reads back the role its own integration was performed as;
//! * it belongs to the ROLE rather than to one request: one session is held per role and
//!   re-minted once the clock has brought it close enough to its expiry that a call could
//!   outlast it ([`REFRESH_MARGIN`]). The one it replaces is given up in the credential
//!   store, so a request presenting it no longer verifies, and in the token service's
//!   registry, so it names no identity either.
//!
//! Where nothing can be SHOWN about the role — the integration names none, this emulator
//! holds no IAM record of the one it names (a role fronting an api is routinely provisioned
//! outside the client that creates the integration), or there is no token service to mint a
//! session with — no session is minted and the call is signed with the credential the
//! in-process callback hands this handler, which is what every other in-process call between
//! two emulated services presents. Refusing on a role this emulator knows nothing about
//! would deny calls AWS permits.
//!
//! The credential never reaches the caller: it signs the one in-process call and appears in
//! no response, log or header this api answers with. Nor does the CALLER's own credential
//! travel to the action — the integration's role is what the call speaks for.
//!
//! The IAM and STS lookups stay in this crate rather than in `service-common`, for the
//! reason [`crate::service::authorization`] states: hoisting them would make every service
//! depending on that crate link the IAM and STS service crates.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::emulator_server::CredentialStore;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

/// The session name API Gateway opens an integration role's session under, which is what a
/// refusal the integrated service answers names as the principal: a call made for one of this
/// gateway's integrations reports
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/BackplaneAssumeRoleSession`. It
/// names the GATEWAY's own assumption rather than any one api, which is why one session is
/// held per role.
const SESSION_NAME: &str = "BackplaneAssumeRoleSession";

/// How long a minted session lasts.
///
/// `sts:AssumeRole` documents 3600 seconds as the default duration of a role session
/// ("`DurationSeconds` … Default: 3600 seconds"), and a session is never minted for longer
/// than the role itself admits (`Role$MaxSessionDuration`).
const SESSION_SECONDS: i32 = 3600;

/// The range `Role$MaxSessionDuration` may carry: "the maximum session duration (in
/// seconds) … Minimum value of 3600. Maximum value of 43200" (`UpdateRole`'s own
/// documentation of the member). A stored value outside it is not a maximum AWS could have
/// recorded, so it caps nothing here and the default duration stands.
const MAX_SESSION_DURATION_RANGE: ::std::ops::RangeInclusive<i32> = 3600..=43_200;

/// How much of a held session's life has to be left for it to be used again.
///
/// The longest an integration may take is its own `TimeoutInMillis`, whose window ends at
/// [`crate::state::MAX_INTEGRATION_TIMEOUT_MILLIS`], so a session with at least that much
/// left cannot expire in the middle of a call that starts now; one with less is re-minted
/// first. Stated as a span and not as a count of calls, because what makes a credential
/// unusable is the CLOCK.
const REFRESH_MARGIN: Duration =
    Duration::from_millis(crate::state::MAX_INTEGRATION_TIMEOUT_MILLIS.unsigned_abs() as u64);

/// The credential one in-process call is signed with.
///
/// Its `Debug` form carries no field: a secret that cannot be formatted cannot reach a log.
#[derive(Clone)]
pub struct SigningIdentity {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl ::std::fmt::Debug for SigningIdentity {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.write_str("SigningIdentity")
    }
}

/// The sessions this emulator holds for the roles its integrations name, keyed by the role
/// each was opened for.
///
/// Engine-scoped rather than a field of this service's own store, so minting one takes the
/// token service's lock and this small one and never this service's state guard. LOCK ORDER:
/// this store first, then the token service's registry and the credential store, and last
/// the IAM store the mint records the session in — the order [`integration_identity`] takes
/// them in, and the only order anything may.
#[derive(Debug, Default)]
pub struct IntegrationRoleSessions {
    /// One held session per role ARN.
    held: BTreeMap<String, Held>,
}

/// One held session: what a call is signed with, and when it stops being valid.
#[derive(Debug, Clone)]
struct Held {
    identity: SigningIdentity,
    /// The instant the token service minted it to last until.
    expires_at: SystemTime,
}

/// The sessions store in the SAME shared engine, or `None` where the engine carries none.
fn sessions_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IntegrationRoleSessions>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<IntegrationRoleSessions>)
}

/// The credential the action of an integration whose `CredentialsArn` is `credentials_arn` is
/// called with.
///
/// `None` when the call is to be signed with the credential the callback hands this handler
/// — every case the module docs name.
pub fn integration_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    credentials_arn: Option<&str>,
) -> Option<SigningIdentity> {
    let credentials_arn = credentials_arn.map(str::trim).filter(|arn| !arn.is_empty())?;
    let role = held_role(engine, credentials_arn)?;
    role_session(engine, ctx.credential_store(), &role, SystemTime::now())
}

/// The credential a session of `role` is signed with, minting the session if this emulator
/// holds none that is still good for a whole call.
fn role_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    role: &ResolvedRole,
    now: SystemTime,
) -> Option<SigningIdentity> {
    let sessions = sessions_state(engine)?;
    let key = role.arn.clone();
    let mut sessions = sessions.lock();
    // A session the clock has already expired is held by nobody: it is given up here, so a
    // credential no api presents does not stay in either store for the life of the process.
    let expired: Vec<String> = sessions
        .held
        .iter()
        .filter(|(_, held)| held.expires_at <= now)
        .map(|(key, _)| key.clone())
        .collect();
    for stale in expired {
        if let Some(held) = sessions.held.remove(&stale) {
            forget(engine, store, &held.identity.access_key_id);
        }
    }
    // A held session is used again only while it has a whole call's worth of life left.
    let still_good = sessions
        .held
        .get(&key)
        .filter(|held| held.expires_at > now + REFRESH_MARGIN);
    if let Some(held) = still_good {
        return Some(held.identity.clone());
    }
    let minted = mint(engine, store, role, now)?;
    let identity = minted.identity.clone();
    // The session this one replaces is forgotten where it was held, so a credential nothing
    // presents any more stops being valid.
    if let Some(replaced) = sessions.held.insert(key, minted) {
        forget(engine, store, &replaced.identity.access_key_id);
    }
    Some(identity)
}

/// What the IAM store holds about one integration role, as a session of it is minted from.
struct ResolvedRole {
    /// The role's ARN, exactly as IAM holds it — the spelling a policy names the role by.
    arn: String,
    /// Its name, which the session's own ARN carries.
    name: String,
    /// Its unique id, which the session's `AssumedRoleId` is built from.
    id: String,
    /// The account whose IAM holds it, read off its ARN.
    account: String,
    /// The partition that ARN names, so a session of a role of another partition is named in
    /// that partition.
    partition: String,
    /// "The maximum session duration (in seconds) that you want to set for the specified
    /// role" (`Role$MaxSessionDuration`), which caps the session minted here.
    max_session_duration: i32,
}

/// The role `role_arn` names, when the IAM store of this engine holds one under it.
///
/// `None` when nothing is known about the role: no IAM store on this engine, a value that
/// names no IAM role, or an ARN this account's IAM holds no role under.
fn held_role<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: &str) -> Option<ResolvedRole> {
    let parsed = ::service_common::arn::parse(role_arn)?;
    let iam = iam_store::service_state(engine)?;
    let held = iam.lock();
    let role = held.roles.values().find(|role| role.arn == role_arn)?;
    Some(ResolvedRole {
        arn: role.arn.clone(),
        name: role.role_name.clone(),
        id: role.role_id.clone(),
        account: parsed.account.to_string(),
        partition: parsed.partition.to_string(),
        max_session_duration: role
            .max_session_duration
            .filter(|duration| MAX_SESSION_DURATION_RANGE.contains(duration))
            .unwrap_or(SESSION_SECONDS),
    })
}

/// Mint one session of `role`, through the token service's own mint.
fn mint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    role: &ResolvedRole,
    now: SystemTime,
) -> Option<Held> {
    let registry = sts_store::service_state(engine)?;
    let identity = sts_store::Identity {
        user_id: format!("{}:{SESSION_NAME}", role.id),
        account: role.account.clone(),
        arn: format!(
            "arn:{}:sts::{}:assumed-role/{}/{SESSION_NAME}",
            role.partition, role.account, role.name
        ),
    };
    let duration_seconds = SESSION_SECONDS.min(role.max_session_duration);
    let minted = sts_store::mint_role_session(
        engine,
        store,
        &registry,
        now,
        sts_store::NewSession {
            role_arn: Some(role.arn.clone()),
            ..sts_store::NewSession::new(
                identity,
                sts_store::SessionKind::AssumedRole,
                duration_seconds,
            )
        },
    )
    .ok()?;
    Some(Held {
        identity: SigningIdentity {
            access_key_id: minted.credentials.access_key_id,
            secret_access_key: minted.credentials.secret_access_key,
            session_token: Some(minted.credentials.session_token),
        },
        expires_at: minted.session.expires_at,
    })
}

/// Forget a session this module minted, in both stores that held it: the credential store
/// (so a request presenting it no longer verifies) and the token service's own registry (so
/// it names no identity either).
fn forget<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    access_key_id: &str,
) {
    sts_store::forget_audited_session(engine, access_key_id);
    match sts_store::service_state(engine) {
        Some(registry) => registry.lock().discard(access_key_id, store),
        None => {
            let _ = store.delete(access_key_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A role held where IAM holds the account's roles: the record `iam:CreateRole` writes,
    /// planted through IAM's OWN state type and key rule rather than through a second copy
    /// of it.
    fn held(engine: &::engine::InMemoryEngine, role_name: &str) -> String {
        let arn = format!(
            "arn:aws:iam::{}:role/{role_name}",
            ::engine::emulator_account_id()
        );
        let role = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        };
        let iam = iam_store::service_state(engine).expect("the engine carries an IAM store");
        iam.lock()
            .roles
            .insert(iam_store::name_key(role_name), role);
        arn
    }

    /// RULE: the credential belongs to the ROLE — it is used again while it has a whole
    /// call's worth of life left, and re-minted once the CLOCK has brought it nearer than
    /// that to its expiry, with the one it replaces left valid nowhere.
    ///
    /// The rows differ along WHEN the next call is made: at the same instant, inside the
    /// session's usable span, and past it.
    #[test]
    fn a_held_session_is_used_again_until_the_clock_brings_it_near_its_expiry() {
        let engine = ::engine::InMemoryEngine::new();
        let store = CredentialStore::default();
        let role_arn = held(&engine, "apigatewayv2-integration-probe");
        let minted_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let identity = |now| {
            let role = held_role(&engine, &role_arn).expect("the role is held");
            role_session(&engine, &store, &role, now).map(|held| held.access_key_id)
        };
        let first = identity(minted_at).expect("a session is minted for a held role");
        let usable =
            Duration::from_secs(u64::try_from(SESSION_SECONDS).expect("a positive duration"))
                - REFRESH_MARGIN;
        let rows: [(&str, Duration, bool); 3] = [
            ("the same instant", Duration::ZERO, true),
            ("inside the usable span", usable - Duration::from_secs(1), true),
            ("past it", usable + Duration::from_secs(1), false),
        ];
        for (case, elapsed, same) in rows {
            let later = identity(minted_at + elapsed).expect("a session is still minted");
            assert_eq!(
                later == first,
                same,
                "case {case:?}: expected the held credential to be reused={same}, got {later:?} \
                 against {first:?}"
            );
            if same {
                continue;
            }
            let registry =
                sts_store::service_state(&engine).expect("the engine carries a session registry");
            assert!(
                registry.lock().session(&first).is_none(),
                "case {case:?}: expected the replaced session {first:?} to be forgotten"
            );
        }
        let sessions = sessions_state(&engine).expect("the engine carries the session store");
        let count = sessions.lock().held.len();
        assert_eq!(
            count, 1,
            "expected one held session for the one role, got {count}"
        );
    }

    /// RULE: nothing can be shown about a role this emulator holds no record of, so no
    /// session is minted for it and the call is signed with the callback's own credential.
    #[test]
    fn a_role_no_record_is_held_for_mints_no_session() {
        let engine = ::engine::InMemoryEngine::new();
        let rows: &[(&str, &str)] = &[
            (
                "a role ARN this account holds no role under",
                "arn:aws:iam::000000000000:role/nothing-holds-this",
            ),
            ("a value that is no ARN at all", "not-an-arn"),
        ];
        for (case, arn) in rows {
            let resolved = held_role(&engine, arn).is_some();
            assert!(
                !resolved,
                "case {case:?}: expected no role to resolve for {arn:?}"
            );
        }
    }
}
