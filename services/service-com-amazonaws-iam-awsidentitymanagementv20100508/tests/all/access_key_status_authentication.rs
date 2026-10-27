// SPDX-License-Identifier: CC0-1.0

//! What an access key's stored `Status` decides about a signed request, end to end
//! over the awsQuery wire.
//!
//! RULE: an access key the account holds authenticates a request only while its stored
//! `Status` is `Active`. `UpdateAccessKey$Status` states what the value means — "Active
//! means that the key can be used for programmatic calls to Amazon Web Services, while
//! Inactive means that the key cannot be used" — so a request presented with a key the
//! account has moved off `Active` is refused as an AUTHENTICATION failure (HTTP 403,
//! error code `InvalidClientTokenId`, the fixed sentence [`UNUSABLE_MESSAGE`]) before
//! the operation it named runs, and moving the key back to `Active` makes it work
//! again. A key id the account holds no record of is not an IAM user's key at all, so
//! this rule says nothing about it and such a request reaches its handler as before.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! `statusType` value assigned (every value the enum declares that is not `Active`),
//! the operation the refused request named — a read that resolves its caller from the
//! signing key, a read that names its subject, an account-wide read, a read that is
//! also authorized against the caller's policies, and a MUTATION — which of the user's
//! two keys is deactivated, and whether the presented key is one the account holds at
//! all.
//!
//! The awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::STATUS_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// The status a key carries from the moment it is minted: `CreateAccessKey` documents
/// "the default status for new keys is Active".
const ACTIVE: &str = "Active";

/// The error code AWS answers a request whose signing credential it will not accept
/// with. It is an authentication outcome, so it is NOT `AccessDenied`: no principal was
/// resolved for the request at all.
const UNUSABLE_CODE: &str = "InvalidClientTokenId";

/// How AWS words that refusal — a fixed sentence with no variable part, so it carries
/// nothing about the credential or the account.
const UNUSABLE_MESSAGE: &str = "The security token included in the request is invalid.";

/// One operation a row presents its key to, and the parameters it carries.
///
/// The set spans the ways an IAM operation reaches state: a read that resolves its
/// caller FROM the signing key (`GetUser` with no `UserName`), a read that names its
/// subject (`ListAccessKeys`), an account-wide read that names nothing
/// (`ListSAMLProviders`), and a MUTATION (`CreateAccessKey`), so a refusal is shown to
/// land ahead of every one of them rather than only ahead of a read.
struct Call {
    /// The `Action` the request names.
    action: &'static str,
    /// Whether the request carries the owning user's name (the operations above that
    /// take one).
    names_user: bool,
}

/// The calls every row presents its key to.
const CALLS: &[Call] = &[
    Call {
        action: "GetUser",
        names_user: false,
    },
    Call {
        action: "ListAccessKeys",
        names_user: true,
    },
    Call {
        action: "ListSAMLProviders",
        names_user: false,
    },
    Call {
        action: "CreateAccessKey",
        names_user: true,
    },
];

/// Create `user` and mint two access keys for it (the published per-user quota),
/// answering their ids.
async fn user_with_two_keys(router: &MultiServiceRouter, case: &str, user: &str) -> Vec<String> {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
    let mut ids = Vec::new();
    for index in 0..2 {
        let (status, created) = call(router, "CreateAccessKey", &[("UserName", user)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: fixture key {index} must be minted, got {status}: {created}"
        );
        ids.push(element(&created, "AccessKeyId"));
    }
    ids
}

/// Assign `status` to one key through a request that names the owning user, so the
/// write never depends on the key under test being usable.
async fn set_status(
    router: &MultiServiceRouter,
    case: &str,
    user: &str,
    access_key_id: &str,
    status: &str,
) {
    let (code, body) = call(
        router,
        "UpdateAccessKey",
        &[
            ("UserName", user),
            ("AccessKeyId", access_key_id),
            ("Status", status),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: UpdateAccessKey(Status={status:?}) must be accepted, got {code}: {body}"
    );
}

/// Present `access_key_id` to `call_under_test` on behalf of `user`.
async fn present(
    router: &MultiServiceRouter,
    access_key_id: &str,
    call_under_test: &Call,
    user: &str,
) -> (u16, String) {
    let params: Vec<(&str, &str)> = if call_under_test.names_user {
        vec![("UserName", user)]
    } else {
        Vec::new()
    };
    call_signed_by(
        router,
        Some(access_key_id),
        call_under_test.action,
        &params,
    )
    .await
}

/// How many access keys the account reports for `user` — the read that shows whether a
/// refused `CreateAccessKey` minted one anyway.
async fn keys_held(router: &MultiServiceRouter, case: &str, user: &str) -> usize {
    let (status, body) = call(router, "ListAccessKeys", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture ListAccessKeys must succeed, got {status}: {body}"
    );
    elements(&body, "AccessKeyId").len()
}

/// RULE: a key the account holds authenticates only while its status is `Active` — so
/// every value `statusType` declares other than `Active` makes the key unusable, on
/// every operation, while its `Active` twin keeps working; the refusal lands before the
/// operation runs, so nothing is read or written for it.
#[tokio::test]
async fn a_key_the_account_moved_off_active_cannot_sign_a_call() {
    // The catalog this test spans, taken from the service's own record of `statusType`'s
    // value set (`smithy_query(query="shape", shape="statusType")`), so the rows cannot
    // drift from the model.
    assert_eq!(
        STATUS_TYPE_VALUES,
        ["Active", "Expired", "Inactive"],
        "statusType's value set moved, so this test no longer spans it: got {STATUS_TYPE_VALUES:?}"
    );
    let unusable: Vec<&str> = STATUS_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| *value != ACTIVE)
        .collect();

    for assigned in &unusable {
        // Rows: the FIRST of the user's two keys deactivated, and then the second, so
        // the twin that stays usable is checked on both sides of the pair.
        for deactivated in 0..2 {
            let case = format!("{assigned} on key {deactivated}");
            let router = router();
            let user = format!("holder{deactivated}");
            let held = user_with_two_keys(&router, &case, &user).await;
            let usable = &held[1 - deactivated];
            set_status(&router, &case, &user, &held[deactivated], assigned).await;

            for call_under_test in CALLS {
                let action = call_under_test.action;
                let before = keys_held(&router, &case, &user).await;

                // The deactivated key: refused, with the authentication code and
                // sentence, whatever the operation was.
                let (status, refused) =
                    present(&router, &held[deactivated], call_under_test, &user).await;
                assert_eq!(
                    status, 403,
                    "case {case:?}/{action}: expected a deactivated key to be refused 403, \
                     got {status}: {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    UNUSABLE_CODE,
                    "case {case:?}/{action}: expected {UNUSABLE_CODE}, got {:?}",
                    error_code(&refused)
                );
                assert_eq!(
                    element(&refused, "Message"),
                    UNUSABLE_MESSAGE,
                    "case {case:?}/{action}: expected AWS's fixed authentication wording, \
                     got {:?}",
                    element(&refused, "Message")
                );
                // The refusal landed ahead of the operation: a mutation signed with the
                // key minted nothing.
                assert_eq!(
                    keys_held(&router, &case, &user).await,
                    before,
                    "case {case:?}/{action}: a refused request must change nothing"
                );

                // The positive control: the SAME request presented with the `Active`
                // twin reaches its handler. Whatever that handler answers, it is not
                // the authentication refusal — a served call carries no error code at
                // all, which is why the body's codes are read as a set rather than as
                // the one code a refusal carries.
                let (status, served) = present(&router, usable, call_under_test, &user).await;
                assert!(
                    !elements(&served, "Code").iter().any(|code| code == UNUSABLE_CODE),
                    "case {case:?}/{action}: the Active twin must not be refused as an \
                     unusable credential, got {status}: {served}"
                );
            }

            // Returning the key to `Active` makes it usable again: deactivation is not a
            // one-way door, which is the documented "from Active to Inactive, or vice
            // versa" direction.
            set_status(&router, &case, &user, &held[deactivated], ACTIVE).await;
            let (status, body) = present(&router, &held[deactivated], &CALLS[0], &user).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a key returned to {ACTIVE} must be usable again, \
                 got {status}: {body}"
            );
            assert_eq!(
                element(&body, "UserName"),
                user,
                "case {case:?}: the reactivated key must still resolve to its own user, \
                 got {body}"
            );
        }
    }
}

/// RULE: the status check is AUTHENTICATION, so it is reached before the operation's
/// own refusals — a deactivated key presented with a request the handler would itself
/// refuse (a missing entity, an unauthorized action) is answered
/// `InvalidClientTokenId`, not the handler's answer.
#[tokio::test]
async fn an_unusable_key_is_refused_ahead_of_every_other_refusal() {
    let router = router();
    let user = "ahead";
    let held = user_with_two_keys(&router, "ahead of dispatch", user).await;
    set_status(&router, "ahead of dispatch", user, &held[1], "Inactive").await;
    // The key listing below is authorized against the caller's own policies, so the
    // user is given the grant AWS requires for it: what the first row is about is the
    // handler's MISSING-ENTITY refusal, which a caller without that grant would never
    // reach. Nothing grants the second row's action, which is the point of that row.
    let (status, put) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", user),
            ("PolicyName", "allow-listaccesskeys"),
            (
                "PolicyDocument",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListAccessKeys","Resource":"*"}]}"#,
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ahead of dispatch\": the fixture PutUserPolicy must succeed, got {status}: {put}"
    );

    // Rows: a request the handler would refuse as a missing entity (404
    // `NoSuchEntity`), and one the caller's own policies do not permit (403
    // `AccessDenied` — the user holds no identity-based policy at all). Each is first
    // shown to get that answer with the `Active` twin, so the row proves the
    // authentication refusal REPLACED a different one rather than coinciding with it.
    struct Row {
        /// What makes this row different from the other.
        case: &'static str,
        /// The `Action` the request names, with its parameters.
        action: &'static str,
        params: &'static [(&'static str, &'static str)],
        /// The status and error code the operation's OWN refusal carries.
        handler_status: u16,
        handler_code: &'static str,
    }

    let rows: &[Row] = &[
        Row {
            case: "a user the account does not hold",
            action: "ListAccessKeys",
            params: &[("UserName", "no-such-user")],
            handler_status: 404,
            handler_code: "NoSuchEntity",
        },
        Row {
            case: "an action no identity-based policy allows",
            action: "ListUsers",
            params: &[],
            handler_status: 403,
            handler_code: "AccessDenied",
        },
    ];

    for Row {
        case,
        action,
        params,
        handler_status,
        handler_code,
    } in rows
    {
        let (status, served) = call_signed_by(&router, Some(&held[0]), action, params).await;
        assert_eq!(
            status, *handler_status,
            "case {case:?}: expected the Active twin to reach the handler's own refusal, \
             got {status}: {served}"
        );
        assert_eq!(
            error_code(&served),
            *handler_code,
            "case {case:?}: expected the handler's own refusal code, got {:?}",
            error_code(&served)
        );

        let (status, refused) = call_signed_by(&router, Some(&held[1]), action, params).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the deactivated key to be refused 403, got {status}: \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            UNUSABLE_CODE,
            "case {case:?}: expected the authentication refusal to replace the handler's, \
             got {:?}",
            error_code(&refused)
        );
    }
}

/// RULE: the rule is about keys the ACCOUNT HOLDS, so a key id it holds no record of is
/// untouched by it — the credential is not an IAM user's, and nothing here can say
/// anything about its status. Such a request reaches its handler exactly as before.
#[tokio::test]
async fn a_key_the_account_does_not_hold_is_not_refused_by_this_rule() {
    let router = router();
    let user = "unheld";
    let held = user_with_two_keys(&router, "unheld credentials", user).await;
    set_status(&router, "unheld credentials", user, &held[0], "Inactive").await;

    // Rows: no signing key at all, the fallback id an unsigned request resolves to
    // (`emulator_server::extract_akid` answers "default"), ids at each bound
    // `accessKeyIdType` admits (length 16 and 128), the shape of a key this emulator
    // never issued, and the shape of the key pair the process publishes at start.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some(&"A".repeat(16)),
        Some(&"B".repeat(128)),
        Some("EMULATORAKAAAAAAAAAA"),
    ];
    for signer in signers {
        let (status, body) = call_signed_by(&router, *signer, "ListAccessKeys", &[("UserName", user)])
            .await;
        assert_eq!(
            status, 200,
            "case {signer:?}: a credential the account holds no record of must reach the \
             handler, got {status}: {body}"
        );
        assert!(
            elements(&body, "Code").is_empty(),
            "case {signer:?}: the status rule must say nothing about an unheld credential, \
             got {body}"
        );
        // And the deactivated key is still reported as the account holds it: the
        // refusal above is about presenting the key, not about listing it.
        assert!(
            elements(&body, "AccessKeyId").contains(&held[0]),
            "case {signer:?}: a deactivated key must stay listed, got {body}"
        );
    }
}
