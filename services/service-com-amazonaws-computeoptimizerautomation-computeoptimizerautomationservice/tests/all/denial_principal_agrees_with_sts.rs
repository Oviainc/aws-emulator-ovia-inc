// SPDX-License-Identifier: CC0-1.0

//! WHICH principal this service's authorization refusal names: the identity the
//! presented credential speaks for.
//!
//! [`crate::unauthorized_resource_message`] asserts the SHAPE of that refusal — the
//! principal, the IAM action and the resource string, over a table of operations and
//! resource forms. This suite varies the one input that decides the principal: the
//! credential the request is signed with. The refusal is built from the same records
//! `sts:GetCallerIdentity` answers from (the STS session registry in the shared engine,
//! else the identity this emulator publishes), so a caller that reads its own identity
//! back from STS sees the principal it was refused as.
//!
//! The rows differ along what that rule generalises over: a key no session was minted
//! under (the ambient identity), a live session credential (whose principal is the
//! session's own, not the account's), a second session of a different role (so one
//! session's principal is not the other's), and a session the clock has already expired
//! (which speaks for nobody, so the refusal falls back to the published identity rather
//! than naming the role the credential once carried).
//!
//! Sessions are seeded into the STS store this engine holds — the registry an
//! `sts:AssumeRole` call writes — because this test binary mounts this service alone;
//! the cross-service half, that STS's own `GetCallerIdentity` reports the same principal
//! for the same key, is
//! `core/router/tests/all/compute_optimizer_automation_denial_principal_agrees_with_sts.rs`.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::automation_rule_lifecycle::{
    assert_refused, call_as, emulator_with_engine, AMBIENT_ACCESS_KEY_ID,
};
use crate::error_payloads::error_message;
use crate::unauthorized_resource_message::{expected_message, request_for};

/// How long a seeded session lasts. Any span puts its expiry in the future; the expired
/// row below is seeded with a `created_at`/`expires_at` pair already behind the clock.
const SESSION_DURATION: Duration = Duration::from_secs(3600);

/// The ARN and the `UserId` STS reports for an assumed-role session of `role_name` under
/// `session_name`, built the way STS builds them, so what this suite seeds is a record
/// STS itself could have written.
fn assumed_role_identity(role_name: &str, session_name: &str) -> sts_state::Identity {
    let account = engine::EMULATOR_ACCOUNT_ID;
    let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
    sts_state::Identity {
        user_id: format!(
            "{}:{session_name}",
            sts_state::unique_id("AROA", &role_arn)
        ),
        account: account.to_string(),
        arn: format!("arn:aws:sts::{account}:assumed-role/{role_name}/{session_name}"),
    }
}

/// One assumed-role session of `role_name`, keyed by `access_key_id`, already `age`
/// behind the clock — so `age` past [`SESSION_DURATION`] is a credential that has
/// expired.
fn session(
    access_key_id: &str,
    role_name: &str,
    session_name: &str,
    age: Duration,
) -> sts_state::Session {
    let created_at = SystemTime::now() - age;
    let account = engine::EMULATOR_ACCOUNT_ID;
    sts_state::Session {
        access_key_id: access_key_id.to_string(),
        created_at,
        expires_at: created_at + SESSION_DURATION,
        duration_seconds: i32::try_from(SESSION_DURATION.as_secs()).unwrap_or(i32::MAX),
        identity: assumed_role_identity(role_name, session_name),
        kind: sts_state::SessionKind::AssumedRole,
        source_identity: None,
        tags: Vec::new(),
        transitive_tag_keys: Vec::new(),
        packed_policy_size: None,
        external_id: None,
        role_arn: Some(format!("arn:aws:iam::{account}:role/{role_name}")),
        revoked: false,
    }
}

/// The refusal names the principal of the credential that signed the request: the
/// session's own for a live session, and the published identity for a key that names no
/// session and for one whose session has expired.
#[tokio::test]
async fn the_refusal_names_the_principal_the_credential_speaks_for() {
    let (router, engine) = emulator_with_engine();
    let ambient = sts_state::ambient_identity().arn;

    let live = session("ASIAEXAMPLELIVE00001", "automation-reconciler", "reconcile", Duration::ZERO);
    let other = session("ASIAEXAMPLEOTHER0001", "automation-auditor", "audit", Duration::ZERO);
    // Seeded one hour past its own expiry: the credential is gone as far as the clock is
    // concerned, which is the state a caller holding a stale session is in.
    let expired = session(
        "ASIAEXAMPLEEXPIRED01",
        "automation-reconciler",
        "stale",
        SESSION_DURATION + Duration::from_secs(3600),
    );

    let sts = sts_state::service_state(&*engine).expect("the engine bears the STS store");
    for seeded in [&live, &other, &expired] {
        sts.lock()
            .insert(seeded.clone())
            .expect("the session registry has room");
    }

    // (case, the access key id presented, the principal it speaks for)
    let credentials = [
        (
            "a key naming no session",
            AMBIENT_ACCESS_KEY_ID,
            ambient.clone(),
        ),
        (
            "a live assumed-role session",
            live.access_key_id.as_str(),
            live.identity.arn.clone(),
        ),
        (
            "a session of another role",
            other.access_key_id.as_str(),
            other.identity.arn.clone(),
        ),
        (
            "a session the clock has expired",
            expired.access_key_id.as_str(),
            ambient,
        ),
    ];
    // Two operations and two resource forms beside the credentials, so the principal is
    // the only thing that moves between the rows of one credential.
    let operations = ["GetAutomationRule", "ListTagsForResource"];
    let resources = [
        "not-an-arn",
        "arn:aws:compute-optimizer::1234:automation-rule/42d39f82e52fa35c",
    ];

    for (credential, access_key_id, principal) in &credentials {
        for operation in operations {
            for resource in resources {
                let case = format!("{credential}: {operation} on {resource}");
                let answer = call_as(
                    &router,
                    access_key_id,
                    operation,
                    &request_for(operation, resource),
                )
                .await;
                assert_refused(answer.clone(), 403, "AccessDeniedException", &case);
                let stated = error_message(&answer.1);
                assert_eq!(
                    stated,
                    expected_message(principal, operation, resource),
                    "case {case:?}: the refusal must name the principal the credential speaks \
                     for ({principal:?}), got {stated:?}"
                );
            }
        }
    }

    // The live session and the ambient key are refused in two different names, so the
    // rows above are not one constant sentence asserted four times.
    assert_ne!(
        live.identity.arn,
        sts_state::ambient_identity().arn,
        "a session credential must not speak for the published identity: got {:?} for both",
        live.identity.arn
    );
}
