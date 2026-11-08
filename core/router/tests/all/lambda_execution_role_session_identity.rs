// SPDX-License-Identifier: CC0-1.0

//! WHO a call a running function makes speaks for, as the ACCOUNT answers it.
//!
//! RULE: a session credential minted for an IAM role is recorded in the account's identity
//! store as the session OF that role, so every service that authorizes a signed request
//! resolves the same principal from the request's access key id and judges the call against
//! the policies in force on that role. There are two readings of that question in this
//! emulator — the token service's own session registry, which `sts:GetCallerIdentity` answers
//! from and which a service like Amazon S3 resolves a caller through, and the account's
//! identity record, which EventBridge's publication gate and IAM's own authorizer read — and a
//! mint has to leave BOTH, or one credential speaks for a role at one front door and for
//! nobody at another, which is a call nothing can refuse.
//!
//! Three crates have to agree for that to hold — Lambda mints the credential a function's own
//! SDK calls are signed with, the token service vends the session, and IAM holds the record a
//! service resolves a temporary credential's principal through — so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The tables' rows differ along what the rule generalises over: WHICH role the function runs
//! as, which function the session was opened for (the session belongs to the execution
//! environment, so two functions of one role hold two), whether the account holds a record of
//! the role at all — the case where no session is minted, so nothing is recorded either — and
//! WHEN the next call is made, which is what decides whether the environment presents the
//! credential it already held or a re-minted one.
//!
//! The suite drives the seam every run path builds its SDK access from rather than running a
//! handler: what a RUNNING handler's call is then refused is the EventBridge crate's
//! `handler_execution_role_publication.rs`, and the table of policy shapes a publication is
//! decided by is the Lambda crate's `handler_eventbridge_publication.rs`.

use std::time::{Duration, SystemTime};

use emulator_server::CredentialStore;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::execution_role_session::execution_role_identity;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

/// The instant the rows mint at. A fixed instant, because what decides a record's life is the
/// CLOCK: the expiry it carries is read against the same instant the mint was made at.
const MINTED_AT: Duration = Duration::from_secs(1_800_000_000);

/// How long a session the mint vends lasts, and how much of that life has to be left for the
/// environment to present it again. Source: `sts:AssumeRole`'s documented default session
/// duration ("`DurationSeconds` … Default: 3600 seconds") and the longest `Timeout` a function
/// may carry (900 seconds), which is the span a run could outlast a credential by.
const SESSION_SECONDS: u64 = 3600;
const REFRESH_MARGIN: u64 = 900;

/// A role of this account, held where IAM holds the account's roles: the record
/// `iam:CreateRole` writes, planted through IAM's OWN state type and key rule rather than
/// through a second copy of it.
fn held_role(engine: &engine::InMemoryEngine, role_name: &str) -> String {
    let arn = format!(
        "arn:aws:iam::{}:role/{role_name}",
        engine::emulator_account_id()
    );
    let role = iam::ops::types::Role {
        path: "/".to_string(),
        role_name: role_name.to_string(),
        role_id: format!("AROASEEDED{:0>11}", role_name.len()),
        arn: arn.clone(),
        create_date: aws_smithy_types::DateTime::from_secs(0),
        assume_role_policy_document: None,
        description: None,
        max_session_duration: None,
        permissions_boundary: None,
        tags: None,
        role_last_used: None,
        source_role_template: None,
    };
    let store = iam::state::service_state(engine).expect("the engine carries an IAM store");
    store
        .lock()
        .roles
        .insert(iam::state::name_key(role_name), role);
    arn
}

/// The ARN of a role this account does NOT hold — well formed, so nothing but the absent
/// record decides the row.
fn unheld_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/never-created",
        engine::emulator_account_id()
    )
}

/// The assumed-role session ARN a call made by an invocation of `function_name` running as
/// `role_name` speaks for: the session a function's calls are made under is named for the
/// FUNCTION, which is how a refusal says which function was refused.
fn session_arn(role_name: &str, function_name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role_name}/{function_name}",
        engine::emulator_account_id()
    )
}

/// The record the account holds for the temporary credential `access_key_id` at `now`, or
/// `None` when it holds none — read through IAM's own accessor, which is what a service's
/// authorization gate resolves a principal through.
fn account_record(
    engine: &engine::InMemoryEngine,
    access_key_id: &str,
    now: SystemTime,
) -> Option<iam::state::RoleSessionRecord> {
    let store = iam::state::service_state(engine).expect("the engine carries an IAM store");
    let held = store
        .lock()
        .role_session(access_key_id, aws_smithy_types::DateTime::from(now))
        .cloned();
    held
}

/// RULE: the credential a function's own calls are signed with is recorded in the account's
/// identity store as a session of the function's execution role, and the token service's own
/// registry names the same session — so both readings of the access key id reach that role.
#[test]
fn a_minted_execution_role_session_is_recorded_as_a_session_of_its_role() {
    let engine = engine::InMemoryEngine::new();
    let store = CredentialStore::default();
    let now = SystemTime::UNIX_EPOCH + MINTED_AT;
    let reader = held_role(&engine, "lambda-record-reader");
    let writer = held_role(&engine, "lambda-record-writer");
    let unheld = unheld_role_arn();

    // (case, the execution role, the function the session is opened for, whether a session is
    // minted at all)
    let rows: [(&str, &str, &str, bool); 4] = [
        ("one function of one role", &reader, "reader", true),
        (
            "another function of the SAME role, whose session is its own",
            &reader,
            "another-reader",
            true,
        ),
        ("a function of another role", &writer, "writer", true),
        (
            "a role the account holds no record of, so no session is minted",
            &unheld,
            "unknown-role-runner",
            false,
        ),
    ];

    let mut minted_keys: Vec<String> = Vec::new();
    for (case, role_arn, function_name, minted) in rows {
        let identity = execution_role_identity(&engine, &store, role_arn, function_name, now);
        assert_eq!(
            identity.is_some(),
            minted,
            "case {case:?}: expected a session to be minted={minted}, got {identity:?}"
        );
        let Some(identity) = identity else {
            continue;
        };
        let access_key_id = identity.access_key_id.clone();
        let expected_session = session_arn(
            role_arn
                .rsplit('/')
                .next()
                .expect("a role ARN carries its name"),
            function_name,
        );

        // Reading one: the account's identity record, which is what a service's authorization
        // gate resolves an access key id through — so what it names is the role a call made
        // with this credential is judged against.
        let record = account_record(&engine, &access_key_id, now).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the account to hold a role session for the minted \
                 credential, so a call made with it is judged against {role_arn}"
            )
        });
        assert_eq!(
            record.role_arn, role_arn,
            "case {case:?}: expected the record to place {role_arn} behind the credential, got \
             {:?}",
            record.role_arn
        );
        assert_eq!(
            record.session_arn, expected_session,
            "case {case:?}: expected the record to name the session {expected_session}, got {:?}",
            record.session_arn
        );

        // Reading two: the token service's own registry, which is what reports an identity.
        let registry = sts::service_state(&engine).expect("the engine carries a session registry");
        let session = registry.lock().session(&access_key_id).cloned();
        let session = session.unwrap_or_else(|| {
            panic!("case {case:?}: expected the token service to hold the minted session")
        });
        assert_eq!(
            session.identity.arn, expected_session,
            "case {case:?}: expected both readings to name one session, got {:?} and {:?}",
            session.identity.arn, record.session_arn
        );
        assert_eq!(
            session.role_arn.as_deref(),
            Some(role_arn),
            "case {case:?}: expected the session to be of {role_arn}, got {:?}",
            session.role_arn
        );

        // A record of its own per execution environment: no row's credential is another's.
        assert!(
            !minted_keys.contains(&access_key_id),
            "case {case:?}: expected a credential of its own, got one already minted"
        );
        minted_keys.push(access_key_id);
    }
    assert_eq!(
        minted_keys.len(),
        3,
        "expected one credential per row that mints a session, got {}",
        minted_keys.len()
    );
}

/// RULE, over the CLOCK: the record follows every credential an execution environment
/// presents, so an environment whose session has been re-minted is still resolvable to its
/// role — a long-lived environment's calls do not stop being judged on its policies.
///
/// The rows differ along WHEN the next call is made: at the same instant, inside the session's
/// usable span, and past it, which is the only thing that decides whether the credential is
/// the one already held or a fresh one.
#[test]
fn the_record_follows_the_credential_the_environment_presents() {
    let engine = engine::InMemoryEngine::new();
    let store = CredentialStore::default();
    let minted_at = SystemTime::UNIX_EPOCH + MINTED_AT;
    let role_name = "lambda-record-refresher";
    let role_arn = held_role(&engine, role_name);
    let function_name = "refresher";
    let key_at = |now| {
        execution_role_identity(&engine, &store, &role_arn, function_name, now)
            .map(|identity| identity.access_key_id)
    };
    let first = key_at(minted_at).expect("a session is minted for a held role");
    let usable = Duration::from_secs(SESSION_SECONDS - REFRESH_MARGIN);
    let rows: [(&str, Duration, bool); 3] = [
        ("the same instant", Duration::ZERO, true),
        ("inside the usable span", usable - Duration::from_secs(1), true),
        ("past it", usable + Duration::from_secs(1), false),
    ];
    for (case, elapsed, held) in rows {
        let now = minted_at + elapsed;
        let presented = key_at(now).expect("a session is still minted");
        assert_eq!(
            presented == first,
            held,
            "case {case:?}: expected the held credential to be presented again={held}, got \
             {presented:?} against {first:?}"
        );
        // Whichever credential the environment now presents, the account resolves it to the
        // execution role — which is what keeps the calls of a refreshed environment judged on
        // that role's policies.
        let record = account_record(&engine, &presented, now).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the account to resolve the credential the environment \
                 presents to {role_arn}"
            )
        });
        assert_eq!(
            record.role_arn, role_arn,
            "case {case:?}: expected the record to place {role_arn} behind the credential, got \
             {:?}",
            record.role_arn
        );
        assert_eq!(
            record.session_arn,
            session_arn(role_name, function_name),
            "case {case:?}: expected the record to name the function's own session, got {:?}",
            record.session_arn
        );
    }
}
