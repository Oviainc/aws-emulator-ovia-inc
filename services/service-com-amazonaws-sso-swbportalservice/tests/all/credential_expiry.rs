// SPDX-License-Identifier: CC0-1.0

//! The end of a minted role credential: the `expiration` a `GetRoleCredentials` response
//! advertises is the instant that credential is spent, so at that instant the portal
//! forgets it — in its own store and in the emulator's credential store, which is what
//! stops the emulator accepting it for signing.
//!
//! The rule under test is the clock's, not the caller's: an expiration is reached by time
//! passing, so nothing about the request that happens to run next decides whether a
//! credential ends. The rows therefore differ along everything that could have keyed it
//! wrongly — which operation runs next, which session asks it (the credential's own,
//! another one, or none at all), and how far past the expiration the clock has gone —
//! and in every row the due credential is gone and the credentials still inside their own
//! duration (the same session's other role, and another session's credential for the very
//! same account and role) are untouched.
//!
//! The second test covers the arithmetic every expiry here is stamped with: a sum the
//! platform's clock type cannot represent saturates rather than panicking.

use http::{Response, StatusCode};
use service_com_amazonaws_sso_swbportalservice::state::{self, ROLE_SESSION_DURATION};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::portal_session::{body_json, body_text, error_code, Portal};

/// The sign-in session whose credential is made due.
const HOLDER: &str = "tok-expiring";
/// A second, unrelated sign-in session. Its own credential must outlive the holder's, and
/// a request IT makes still materialises the holder's expiry.
const BYSTANDER: &str = "tok-other";
/// A token naming no sign-in session at all: its request is refused, and the expiry is
/// still due.
const UNKNOWN: &str = "tok-unknown";

/// The (account, role) pair whose credential is made due. Assigned in both accounts the
/// sessions below are signed in to, from `portal_session.rs`'s fixture.
const DUE: (&str, &str) = ("111111111111", "AdministratorAccess");
/// A second pair, whose credential stays inside its own duration throughout.
const STILL_VALID: (&str, &str) = ("444444444444", "ReadOnly");

/// The accounts both sessions below are assigned to.
const ASSIGNED: &[&str] = &[DUE.0, STILL_VALID.0];

/// The portal request that runs after the expiration has passed, carrying the token it
/// presents. One variant per operation this service serves.
#[derive(Debug, Clone, Copy)]
enum Next {
    ListAccounts(&'static str),
    ListAccountRoles(&'static str),
    GetAnotherRoleCredential(&'static str),
    Logout(&'static str),
}

/// The stored expiration a row leaves the due credential with — the state the clock
/// itself reaches once that instant has passed.
#[derive(Debug, Clone, Copy)]
enum Due {
    /// The start of the clock's epoch: as long past as an expiration can be.
    Epoch,
    /// This instant exactly: an expiration that is reached, not merely overshot.
    Now,
    /// That many milliseconds before this instant.
    Ago(i64),
}

impl Due {
    fn expiration_millis(self, now_millis: i64) -> i64 {
        match self {
            Due::Epoch => 0,
            Due::Now => now_millis,
            Due::Ago(millis) => now_millis - millis,
        }
    }
}

/// How a role credential is keyed in state: the token that asked for it, the account and
/// the role.
fn credential_key(token: &str, (account_id, role_name): (&str, &str)) -> (String, String, String) {
    (
        token.to_string(),
        account_id.to_string(),
        role_name.to_string(),
    )
}

/// The subject `Portal::sign_in` seeds a session with.
fn subject_of(token: &str) -> String {
    format!("{token}@example.com")
}

/// Ask for the credentials of one (account, role) and answer the access key id and the
/// expiration the response advertised.
async fn fetch_credential(
    portal: &Portal,
    token: &str,
    (account_id, role_name): (&str, &str),
    case: &str,
) -> (String, i64) {
    let response = portal
        .get(
            &format!("/federation/credentials?account_id={account_id}&role_name={role_name}"),
            Some(token),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "case {case:?}: fetching the credentials of {account_id}/{role_name} as {token:?} \
         must succeed, got {} {:?}",
        response.status(),
        body_text(&response)
    );
    let credentials = body_json(&response)["roleCredentials"].clone();
    let access_key_id = credentials["accessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let expiration = credentials["expiration"].as_i64().unwrap_or_default();
    assert!(
        !access_key_id.is_empty(),
        "case {case:?}: the response must carry an access key id, got {credentials:?}"
    );
    (access_key_id, expiration)
}

/// Run the row's next request.
async fn run_next(portal: &Portal, next: Next) -> Response<bytes::Bytes> {
    match next {
        Next::ListAccounts(token) => portal.get("/assignment/accounts", Some(token)).await,
        Next::ListAccountRoles(token) => {
            portal
                .get(
                    &format!("/assignment/roles?account_id={}", STILL_VALID.0),
                    Some(token),
                )
                .await
        }
        Next::GetAnotherRoleCredential(token) => {
            portal
                .get(
                    &format!(
                        "/federation/credentials?account_id={}&role_name={}",
                        STILL_VALID.0, STILL_VALID.1
                    ),
                    Some(token),
                )
                .await
        }
        Next::Logout(token) => portal.logout(Some(token)).await,
    }
}

/// RULE: a minted role credential ends at the expiration its response advertised — the
/// portal forgets it and the emulator stops accepting it — whether or not anyone ever
/// asks for that account and role again, while every credential still inside its own
/// duration is left alone.
#[tokio::test]
async fn a_due_role_credential_ends_without_being_asked_for_again() {
    // Rows: which operation runs next, which session asks it, and how far past the
    // expiration the clock has gone.
    let rows: &[(&str, Due, Next, StatusCode)] = &[
        (
            "the same session lists its accounts",
            Due::Ago(1),
            Next::ListAccounts(HOLDER),
            StatusCode::OK,
        ),
        (
            "another session lists its accounts",
            Due::Epoch,
            Next::ListAccounts(BYSTANDER),
            StatusCode::OK,
        ),
        (
            "the same session lists one account's roles",
            Due::Now,
            Next::ListAccountRoles(HOLDER),
            StatusCode::OK,
        ),
        (
            "another session lists one account's roles",
            Due::Ago(60_000),
            Next::ListAccountRoles(BYSTANDER),
            StatusCode::OK,
        ),
        (
            "the same session asks for a different role's credential",
            Due::Ago(1),
            Next::GetAnotherRoleCredential(HOLDER),
            StatusCode::OK,
        ),
        (
            "another session logs out",
            Due::Ago(i64::try_from(ROLE_SESSION_DURATION.as_millis()).unwrap_or(i64::MAX)),
            Next::Logout(BYSTANDER),
            StatusCode::OK,
        ),
        (
            "a token naming no session is refused",
            Due::Ago(1),
            Next::ListAccounts(UNKNOWN),
            StatusCode::UNAUTHORIZED,
        ),
    ];

    for (case, due, next, expected_status) in rows {
        let portal = Portal::new();
        portal.sign_in(HOLDER, ASSIGNED);
        portal.sign_in(BYSTANDER, ASSIGNED);

        // Three credentials: the one that falls due, one of the same session's for
        // another role, and another session's for the very same account and role.
        let (due_key_id, advertised) = fetch_credential(&portal, HOLDER, DUE, case).await;
        let (kept_key_id, kept_expiration) =
            fetch_credential(&portal, HOLDER, STILL_VALID, case).await;
        let (bystander_key_id, _) = fetch_credential(&portal, BYSTANDER, DUE, case).await;
        assert!(
            advertised > state::epoch_millis(SystemTime::now()),
            "case {case:?}: the credential must start out valid, got expiration {advertised}"
        );

        // Put the clock past that expiration: this is the state time itself reaches one
        // role session duration after the credential was minted.
        let now_millis = state::epoch_millis(SystemTime::now());
        {
            let store = portal.state();
            let mut store = store.lock();
            let record = store
                .issued_credentials
                .get_mut(&credential_key(HOLDER, DUE))
                .unwrap_or_else(|| panic!("case {case:?}: the minted credential must be stored"));
            record.expiration_millis = due.expiration_millis(now_millis);
        }

        // Nobody asks for that account and role again: some other request arrives.
        let response = run_next(&portal, *next).await;
        assert_eq!(
            response.status(),
            *expected_status,
            "case {case:?}: {next:?} must answer {expected_status}, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        if *expected_status == StatusCode::UNAUTHORIZED {
            assert_eq!(
                error_code(&response),
                "UnauthorizedException",
                "case {case:?}: {next:?} must be refused as unauthorized, got {:?}",
                body_text(&response)
            );
        }

        // The guard lives in a block of its own, so nothing below is awaited while this
        // test holds the store the handlers need.
        {
            let now = SystemTime::now();
            let store = portal.state();
            let store = store.lock();
            assert!(
                !store
                    .issued_credentials
                    .contains_key(&credential_key(HOLDER, DUE)),
                "case {case:?}: the due credential must be forgotten, still holding {:?}",
                store.issued_credentials.get(&credential_key(HOLDER, DUE))
            );
            assert!(
                store.signed_in_subject(&due_key_id, now).is_none(),
                "case {case:?}: the due access key must speak for nobody, got {:?}",
                store.signed_in_subject(&due_key_id, now)
            );

            // Everything still inside its own duration is untouched.
            let kept = store
                .issued_credentials
                .get(&credential_key(HOLDER, STILL_VALID))
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a credential inside its own duration must be kept")
                });
            assert_eq!(
                (kept.access_key_id.as_str(), kept.expiration_millis),
                (kept_key_id.as_str(), kept_expiration),
                "case {case:?}: the kept credential must be unchanged, got {kept:?}"
            );
            assert_eq!(
                store.signed_in_subject(&kept_key_id, now),
                Some(subject_of(HOLDER).as_str()),
                "case {case:?}: the kept access key must still speak for its session"
            );
            let bystander = store
                .issued_credentials
                .get(&credential_key(BYSTANDER, DUE))
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: another session's credential for the same role must be \
                         kept"
                    )
                });
            assert_eq!(
                bystander.access_key_id, bystander_key_id,
                "case {case:?}: another session's credential must be unchanged, got {bystander:?}"
            );
        }

        // Asking again mints a replacement: a different access key with a fresh lifetime,
        // never the credential that was already spent.
        let before = state::epoch_millis(SystemTime::now());
        let (fresh_key_id, fresh_expiration) = fetch_credential(&portal, HOLDER, DUE, case).await;
        let lifetime = i64::try_from(ROLE_SESSION_DURATION.as_millis()).unwrap_or(i64::MAX);
        assert_ne!(
            fresh_key_id, due_key_id,
            "case {case:?}: the replacement must not be the spent credential, got \
             {fresh_key_id:?}"
        );
        assert!(
            fresh_expiration >= before + lifetime
                && fresh_expiration <= state::epoch_millis(SystemTime::now()) + lifetime,
            "case {case:?}: the replacement must expire one role session duration \
             ({lifetime} ms) out, got {fresh_expiration}"
        );
    }
}

/// RULE: every expiry this service stamps is computed by saturating addition, so a sum the
/// platform's clock type cannot represent yields the furthest instant it can hold instead
/// of panicking — and a representable sum is exactly the sum.
#[test]
fn an_expiry_saturates_instead_of_overflowing() {
    // Rows span the instant stamped from (the epoch, now) and the duration added (none, a
    // role session, a session ttl far longer than any clock can hold, and the longest
    // duration there is).
    let rows: &[(&str, SystemTime, Duration)] = &[
        ("the epoch plus a role session", UNIX_EPOCH, ROLE_SESSION_DURATION),
        ("now plus a role session", SystemTime::now(), ROLE_SESSION_DURATION),
        ("now plus nothing at all", SystemTime::now(), Duration::ZERO),
        (
            "now plus a century",
            SystemTime::now(),
            Duration::from_secs(100 * 365 * 24 * 60 * 60),
        ),
        ("now plus half the longest duration", SystemTime::now(), Duration::MAX / 2),
        ("now plus the longest duration there is", SystemTime::now(), Duration::MAX),
        ("the epoch plus the longest duration there is", UNIX_EPOCH, Duration::MAX),
    ];

    for (case, at, duration) in rows {
        let stamped = state::expiry_after(*at, *duration);
        match at.checked_add(*duration) {
            Some(exact) => assert_eq!(
                stamped, exact,
                "case {case:?}: a representable sum must be the sum itself, got {stamped:?}"
            ),
            None => {
                assert!(
                    stamped >= *at,
                    "case {case:?}: a saturated expiry must not fall before the instant it \
                     was stamped at, got {stamped:?}"
                );
                assert!(
                    stamped > SystemTime::now(),
                    "case {case:?}: a saturated expiry must read as not yet expired, got \
                     {stamped:?}"
                );
            }
        }
        // Whatever the row, the stamped instant is one the modeled millisecond field can
        // carry, and never earlier than where it was stamped from.
        assert!(
            state::epoch_millis(stamped) >= state::epoch_millis(*at),
            "case {case:?}: the stamped expiry must be at or after {at:?}, got {stamped:?}"
        );
    }
}
