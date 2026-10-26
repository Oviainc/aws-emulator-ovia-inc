// SPDX-License-Identifier: CC0-1.0

//! One account, two services: the access key `iam:CreateAccessKey` mints for a user is a
//! credential that speaks for THAT user, and every service that resolves a caller from the
//! presented access key id reports that same user.
//!
//! The rule both halves implement: the principal of a request is the IAM user that holds
//! the access key id the request presents — its `UserId` and its `Arn` exactly as IAM
//! holds them — and a key id the account holds no record of is the ambient identity. So
//! `sts:GetCallerIdentity` under a freshly minted pair answers that user's ARN,
//! `iam:GetUser` with no `UserName` answers the same user, and a session vended under that
//! key keeps it. The resolution is read from IAM's own access-key registry, which is why
//! only a cross-service suite can pin it: the IAM crate holds the registry and the STS
//! crate reads it, and a crate-local suite can seed one side's state but never drive
//! `CreateUser`/`CreateAccessKey` end to end into the other's answer.
//!
//! The rows vary what the rule generalises over: the user's name (its character set and
//! case) and the path its ARN embeds, how many keys one user holds (one key, and two keys
//! of one user, which must resolve to one identity), and how many users the account holds
//! (no two of which may resolve to one ARN). Beside the table, the boundaries: a key id
//! nothing minted resolves to the ambient identity rather than to the last user created,
//! and a key `iam:DeleteAccessKey` has withdrawn stops resolving to its user while that
//! user's remaining key still does.
//!
//! The role-side counterpart — that STS reports the `RoleId` IAM holds — is
//! `iam_role_id_agrees_with_sts.rs`; cross-service routing for the `iam` signing name is
//! `iam_signing_name_routing.rs`.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The form encoder that builds an awsQuery body, and the reader that pulls one element's
// text out of the XML answer, are this test crate's own (`query_wire`), written once there.
use crate::query_wire::{form_encode, opt_element};

/// The access key id an administrative call presents: the account holds no record of it,
/// so it is the ambient caller rather than any user this suite creates.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The name of the ambient IAM user the emulator publishes its own long-term key for.
const AMBIENT_USER_NAME: &str = "emulator";

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and
/// `access_key_id`. The signature is never checked: this router runs with request
/// authentication off, and the access key id is what resolves the caller.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The emulator with IAM and STS mounted on ONE engine, as `core/router/src/lib.rs`
/// mounts them: the registry `iam:CreateAccessKey` writes is the one STS resolves a
/// presented key against.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call against `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM action, presenting `access_key_id`.
async fn iam(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        access_key_id,
        "iam",
        "2010-05-08",
        action,
        parameters,
    )
    .await
}

/// One STS action, presenting `access_key_id`.
async fn sts(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        access_key_id,
        "sts",
        "2011-06-15",
        action,
        parameters,
    )
    .await
}

/// The member, or a panic naming the case and the whole body.
fn member(case: &str, body: &str, name: &str) -> String {
    opt_element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// One member of an answer that CARRIES CREDENTIAL MATERIAL (a minted secret access key,
/// a session token). Nothing here quotes the body — not even on a failure — so a secret a
/// handler handed back never reaches the test output; a failure names the member it looked
/// for and the case instead.
fn minted_member(case: &str, body: &str, name: &str) -> String {
    opt_element(body, name).unwrap_or_else(|| {
        panic!("case {case:?}: the minted-credential answer carries no <{name}> element")
    })
}

/// The identity a presented key resolves to, as `GetCallerIdentity` reports it:
/// `(UserId, Account, Arn)`.
async fn caller_identity(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
) -> (String, String, String) {
    let (status, body) = sts(router, access_key_id, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCallerIdentity, got {status} {body}"
    );
    (
        member(case, &body, "UserId"),
        member(case, &body, "Account"),
        member(case, &body, "Arn"),
    )
}

/// Create one IAM user and return its `UserId` and `Arn`.
async fn create_user(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    path: &str,
) -> (String, String) {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        "CreateUser",
        &[
            ("UserName", user_name.to_string()),
            ("Path", path.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateUser, got {status} {body}"
    );
    (member(case, &body, "UserId"), member(case, &body, "Arn"))
}

/// Mint one access key for `user_name` and return its `AccessKeyId` with the
/// `SecretAccessKey` the call handed back.
async fn create_access_key(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
) -> (String, String) {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        "CreateAccessKey",
        &[("UserName", user_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAccessKey, got status {status}"
    );
    let minted_for = minted_member(case, &body, "UserName");
    assert_eq!(
        minted_for, user_name,
        "case {case:?}: the minted key must belong to the user it was asked for, got \
         {minted_for:?}"
    );
    (
        minted_member(case, &body, "AccessKeyId"),
        minted_member(case, &body, "SecretAccessKey"),
    )
}

/// A request presenting the access key `CreateAccessKey` minted for a user is that user's
/// request: STS reports the user's own `UserId`/`Arn`, IAM resolves the nameless `GetUser`
/// to the same user, and a session vended under the key keeps that identity — while a key
/// the account holds no record of is the ambient identity.
#[tokio::test]
async fn an_access_key_minted_for_a_user_resolves_to_that_user() {
    let router = build_router();
    let account = engine::emulator_account_id().to_string();

    // (case, the user name, the path its ARN embeds, how many keys the user holds)
    let rows: [(&str, &str, &str, usize); 5] = [
        ("a user at the default path", "u-1", "/", 1),
        ("a user under a path", "pathed-user", "/engineering/", 1),
        ("a user under a deep path", "deep-user", "/engineering/data/", 1),
        ("a mixed-case name", "Carol.Smith", "/", 1),
        // `userNameType`'s pattern `^[\w+=,.@-]+$` admits this punctuation, and IAM lets
        // one user hold more than one key: both keys are the same principal.
        ("a punctuated name holding two keys", "d_e-f+g=h,i.j@k", "/", 2),
    ];

    let mut seen_arns: Vec<String> = Vec::new();
    for (case, user_name, path, key_count) in rows {
        let (user_id, user_arn) = create_user(&router, case, user_name, path).await;
        assert_eq!(
            user_arn,
            format!("arn:aws:iam::{account}:user{path}{user_name}"),
            "case {case:?}: expected the user's ARN to embed its path and name, got {user_arn}"
        );

        for index in 0..key_count {
            let case = format!("{case} (key {})", index + 1);
            let (access_key_id, secret_access_key) =
                create_access_key(&router, &case, user_name).await;
            assert!(
                !access_key_id.is_empty() && !secret_access_key.is_empty(),
                "case {case:?}: CreateAccessKey must hand back a usable pair, got \
                 id {access_key_id:?}"
            );

            // The rule: the presented key id selects the principal, and it is the user
            // the key was minted for — not a built-in one, and not the key's own id.
            let identity = caller_identity(&router, &case, &access_key_id).await;
            assert_eq!(
                identity,
                (user_id.clone(), account.clone(), user_arn.clone()),
                "case {case:?}: expected the minted key to speak for {user_name:?}"
            );

            // IAM resolves the nameless read through the same registry: "If you do not
            // specify a user name, IAM determines the user name implicitly based on the
            // Amazon Web Services access key ID signing the request."
            let (status, body) = iam(&router, &access_key_id, "GetUser", &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the nameless GetUser, got {status} {body}"
            );
            assert_eq!(
                (
                    member(&case, &body, "UserId"),
                    member(&case, &body, "Arn")
                ),
                (user_id.clone(), user_arn.clone()),
                "case {case:?}: the nameless GetUser must answer the key's own user, got {body}"
            );

            // A session vended under that key carries the identity forward, so the
            // principal survives the credential the caller goes on to use.
            let (status, body) = sts(&router, &access_key_id, "GetSessionToken", &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetSessionToken, got status {status}"
            );
            let session_key = minted_member(&case, &body, "AccessKeyId");
            let (_, _, session_arn) =
                caller_identity(&router, &format!("{case}: a session of the key"), &session_key)
                    .await;
            assert_eq!(
                session_arn, user_arn,
                "case {case:?}: a session vended under the user's key must keep that user's \
                 ARN, got {session_arn}"
            );
        }

        assert!(
            !seen_arns.contains(&user_arn),
            "case {case:?}: two users must never resolve to one ARN ({user_arn})"
        );
        seen_arns.push(user_arn);
    }

    // A key id nothing minted is the ambient identity — not the user created last, which
    // is what an emulator that ignores the presented key would answer.
    let unheld = "AKIANEVERMINTEDKEY01";
    let (_, ambient_account, ambient_arn) = caller_identity(&router, "an unheld key", unheld).await;
    assert_eq!(
        (ambient_account, ambient_arn),
        (
            account.clone(),
            format!("arn:aws:iam::{account}:user/{AMBIENT_USER_NAME}")
        ),
        "case 'an unheld key': expected the ambient identity for a key the account does not hold"
    );
}

/// Resolution reads IAM as it stands NOW: the key `DeleteAccessKey` withdrew no longer
/// speaks for its user, while that user's remaining key still does.
#[tokio::test]
async fn a_withdrawn_access_key_no_longer_speaks_for_its_user() {
    let router = build_router();
    let account = engine::emulator_account_id().to_string();
    let user_name = "two-key-user";
    let (user_id, user_arn) = create_user(&router, "two keys", user_name, "/").await;

    let (first, _) = create_access_key(&router, "the first key", user_name).await;
    let (second, _) = create_access_key(&router, "the second key", user_name).await;
    for (case, key) in [("the first key", &first), ("the second key", &second)] {
        assert_eq!(
            caller_identity(&router, case, key).await,
            (user_id.clone(), account.clone(), user_arn.clone()),
            "case {case:?}: both of a user's keys speak for that one user"
        );
    }

    let (status, body) = iam(
        &router,
        AMBIENT_KEY,
        "DeleteAccessKey",
        &[
            ("UserName", user_name.to_string()),
            ("AccessKeyId", first.clone()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'the delete': expected HTTP 200 from DeleteAccessKey, got {status} {body}"
    );

    let (_, _, withdrawn_arn) = caller_identity(&router, "the withdrawn key", &first).await;
    assert_eq!(
        withdrawn_arn,
        format!("arn:aws:iam::{account}:user/{AMBIENT_USER_NAME}"),
        "case 'the withdrawn key': a key the account no longer holds must not speak for its \
         former user, got {withdrawn_arn}"
    );
    assert_eq!(
        caller_identity(&router, "the surviving key", &second).await,
        (user_id, account, user_arn),
        "case 'the surviving key': the user's other key must keep speaking for it"
    );
}
