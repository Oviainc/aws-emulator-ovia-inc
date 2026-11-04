// SPDX-License-Identifier: CC0-1.0

//! Who a request that names no `UserName` speaks for, end to end over the awsQuery wire.
//!
//! RULE: an operation of this service whose `UserName` is optional resolves an absent one
//! "implicitly based on the Amazon Web Services access key ID used to sign the request"
//! (`GetUser`), and a long-term access key belongs to an IAM USER of the account — so a
//! nameless call is served for the user the signing credential belongs to: the user that
//! minted the key, or, for the long-term credential this emulator publishes at start, the
//! account's own user, which is the principal `sts:GetCallerIdentity` reports for that same
//! credential. Only a credential that is no IAM user's at all resolves to no user, and
//! that is the one case the family refuses (HTTP 400 `ValidationError` with AWS's fixed
//! sentence). Because the two halves resolve to ONE record, the identity a caller reads of
//! itself is the identity the account inventory reports for it, and the key it is
//! authenticating with is among the keys its own user holds.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation of the family is called (reads, mutations, and operations addressing a
//! credential, a certificate or a password), which credential the request presents (the
//! published one, a key an `iam:CreateAccessKey` minted, one that belongs to no IAM user),
//! how many keys the user holds, and the page size the listing is asked for.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares; this suite adds
//! only the router that PUBLISHES a long-term credential, because that credential is what
//! the rule's second half is about.

use smithy_server_runtime::MultiServiceRouter;

use crate::signing_certificates::certificate_pem;
use crate::user_lifecycle::{account_id, call_signed_by, element, elements, error_code};

/// AWS's fixed sentence for a call whose implicit `UserName` resolved to no IAM user,
/// written out here rather than taken from the implementation's constant: what this suite
/// pins is the SENTENCE IAM answers.
const MUST_SPECIFY_USER_NAME: &str =
    "Must specify userName when calling with non-User credentials";

/// The error code IAM answers a request whose values it cannot accept with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The status a credential in service carries — `statusType`'s `Active`, the value
/// `ListAccessKeys` reports for a key that can sign (`AccessKeyMetadata$Status`).
const ACTIVE: &str = "Active";

/// The other `statusType` value `UpdateAccessKey` moves a key to.
const INACTIVE: &str = "Inactive";

/// The friendly name of the user this emulator's own long-term credential belongs to, and
/// the path it sits under: read from the one module that states them, because what this
/// suite is about is that IAM and STS answer for ONE record rather than what that record
/// is called.
const ACCOUNT_CALLER: &str = ::service_common::caller_principal::AMBIENT_USER_NAME;

/// See [`ACCOUNT_CALLER`].
const ACCOUNT_CALLER_PATH: &str = ::service_common::caller_principal::AMBIENT_USER_PATH;

/// The type prefix AWS gives an IAM user's unique id, from the IAM "unique identifier
/// prefixes" table (`AIDA` for a user).
const USER_ID_PREFIX: &str = "AIDA";

/// A key id of the published AWS long-term form (`accessKeyIdType`, len 16..128, pattern
/// `^[\w]+$`) that nothing in this emulator issued, so it belongs to no IAM user.
const FOREIGN_ACCESS_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// An access key id of the modelled shape that no create in this suite minted.
const UNMINTED_ACCESS_KEY_ID: &str = "EMULATORAKUNMINTED02";

/// A certificate id of the modelled shape (`certificateIdType`, len 24..128) that no
/// upload in this suite minted.
const UNMINTED_CERTIFICATE_ID: &str = "EMULATORCERTUNMINTED00002";

/// A credential id of the modelled shape (`serviceSpecificCredentialId`, len 20..128) that
/// no create in this suite minted.
const UNMINTED_CREDENTIAL_ID: &str = "EMULATORNEVERMINTEDTHIS02";

/// A router serving only this service, plus the access key id of the long-term credential
/// this emulator process publishes.
///
/// The router with authentication ON — the only configuration in which the process
/// publishes that pair, and `vended_credentials` the only place its id can be read — is the
/// one `tests/all/authentication_refusal_wording.rs` owns; the authenticator is then taken
/// off again, because what this suite drives is how the HANDLERS resolve that id and not
/// whether a signature verifies (the shared harness signs nothing, and the signature check
/// has its own suite there).
fn published_credential_router() -> (MultiServiceRouter, String) {
    let (mut router, access_key_id) = crate::authentication_refusal_wording::authenticating_router();
    router.set_request_authentication(false);
    (router, access_key_id)
}

/// Whether `(status, body)` is the family's refusal of an implicit `UserName` that resolved
/// to no IAM user: its status, its code and its sentence together, checked in one place so
/// a row can say either that it was answered or that it was not.
fn refused_as_unresolvable(status: u16, body: &str) -> bool {
    status == 400
        && elements(body, "Code").iter().any(|code| code == VALIDATION_ERROR)
        && elements(body, "Message")
            .iter()
            .any(|message| message == MUST_SPECIFY_USER_NAME)
}

/// The `<AccessKeyId>`/`<Status>` pairs a `ListAccessKeys` body reports, in document order.
fn listed_keys(body: &str) -> Vec<(String, String)> {
    let ids = elements(body, "AccessKeyId");
    let statuses = elements(body, "Status");
    assert_eq!(
        ids.len(),
        statuses.len(),
        "every reported key must carry a status, got {ids:?} and {statuses:?}"
    );
    ids.into_iter().zip(statuses).collect()
}

/// RULE, first half: the credential this emulator publishes belongs to the account's own
/// IAM user, so a nameless `GetUser` reports that user — and reports it as ONE record: the
/// same name, path, ARN and id the account inventory (`ListUsers`) and a `GetUser` naming
/// it report, which is what lets a client join its own identity to the inventory.
#[tokio::test]
async fn a_nameless_user_read_reports_the_user_the_signing_credential_belongs_to() {
    let (router, published) = published_credential_router();

    let (status, body) = call_signed_by(&router, Some(&published), "GetUser", &[]).await;
    assert_eq!(
        status, 200,
        "case 'the published credential': expected the caller's own user, got {status}: {body}"
    );
    let reported_name = element(&body, "UserName");
    let reported_arn = element(&body, "Arn");
    let reported_id = element(&body, "UserId");
    assert_eq!(
        (
            reported_name.as_str(),
            element(&body, "Path").as_str(),
            reported_arn.as_str()
        ),
        (
            ACCOUNT_CALLER,
            ACCOUNT_CALLER_PATH,
            format!("arn:aws:iam::{}:user/{ACCOUNT_CALLER}", account_id()).as_str()
        ),
        "case 'the published credential': expected the account's own user, got {body}"
    );
    assert!(
        reported_id.starts_with(USER_ID_PREFIX) && reported_id.len() > USER_ID_PREFIX.len(),
        "case 'the published credential': expected a {USER_ID_PREFIX} unique id, \
         got {reported_id:?}"
    );
    assert_eq!(
        elements(&body, "CreateDate").len(),
        1,
        "case 'the published credential': expected one CreateDate, got {body}"
    );

    // The join the operator identity exists for: the account's user list carries the very
    // user the nameless read reported, under the same name and ARN.
    let (status, listed) = call_signed_by(&router, Some(&published), "ListUsers", &[]).await;
    assert_eq!(
        status, 200,
        "case 'the inventory': expected the user list, got {status}: {listed}"
    );
    assert!(
        elements(&listed, "UserName").contains(&reported_name)
            && elements(&listed, "Arn").contains(&reported_arn),
        "case 'the inventory': expected it to carry {reported_name:?} / {reported_arn:?}, \
         got {listed}"
    );

    // Naming that same user explicitly reaches the same record, in either case spelling:
    // IAM names "are not distinguished by case".
    for spelled in [reported_name.clone(), reported_name.to_uppercase()] {
        let (status, named) = call_signed_by(
            &router,
            Some(&published),
            "GetUser",
            &[("UserName", spelled.as_str())],
        )
        .await;
        assert_eq!(
            (status, element(&named, "Arn"), element(&named, "UserId")),
            (200, reported_arn.clone(), reported_id.clone()),
            "case {spelled:?}: expected the same record the nameless read reported, \
             got {status}: {named}"
        );
    }
}

/// RULE, applied to the keys of that user: the long-term credential a caller is
/// authenticating with is one of its own user's access keys, so a nameless
/// `ListAccessKeys` reports it beside every key `iam:CreateAccessKey` minted — in access
/// key id order, one page at a time — and the ids are only ever reported to the caller that
/// presented that credential.
#[tokio::test]
async fn a_nameless_key_listing_reports_the_credential_the_caller_authenticates_with() {
    let (router, published) = published_credential_router();

    // The caller holds exactly the credential it is signing with.
    let (status, body) = call_signed_by(&router, Some(&published), "ListAccessKeys", &[]).await;
    assert_eq!(
        (status, listed_keys(&body)),
        (200, vec![(published.clone(), ACTIVE.to_string())]),
        "case 'only the published credential': expected the caller's own key, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "UserName"),
        ACCOUNT_CALLER,
        "case 'only the published credential': expected the key to be reported under its \
         holder, got {body}"
    );

    // A key minted for the same caller joins the listing, and the two are reported in
    // ascending id order whichever of them sorts first.
    let (status, minted) =
        call_signed_by(&router, Some(&published), "CreateAccessKey", &[]).await;
    assert_eq!(
        (status, element(&minted, "UserName")),
        (200, ACCOUNT_CALLER.to_string()),
        "case 'a minted key': expected it to be minted for the caller, got {status}: {minted}"
    );
    let minted_id = element(&minted, "AccessKeyId");
    let mut expected = vec![
        (published.clone(), ACTIVE.to_string()),
        (minted_id.clone(), ACTIVE.to_string()),
    ];
    expected.sort();
    let (status, body) = call_signed_by(&router, Some(&published), "ListAccessKeys", &[]).await;
    assert_eq!(
        (status, listed_keys(&body)),
        (200, expected.clone()),
        "case 'both keys': expected both keys in id order, got {status}: {body}"
    );

    // One page at a time: `MaxItems` 1 truncates after the first of them, and the marker
    // resumes strictly after it.
    let (status, first) = call_signed_by(
        &router,
        Some(&published),
        "ListAccessKeys",
        &[("MaxItems", "1")],
    )
    .await;
    assert_eq!(
        (status, listed_keys(&first), element(&first, "IsTruncated")),
        (200, vec![expected[0].clone()], "true".to_string()),
        "case 'the first page': expected one key and a truncated page, got {status}: {first}"
    );
    let marker = element(&first, "Marker");
    let (status, second) = call_signed_by(
        &router,
        Some(&published),
        "ListAccessKeys",
        &[("Marker", marker.as_str())],
    )
    .await;
    assert_eq!(
        (status, listed_keys(&second), element(&second, "IsTruncated")),
        (200, vec![expected[1].clone()], "false".to_string()),
        "case 'the second page': expected the remaining key, got {status}: {second}"
    );

    // What a nameless write does to the caller's own keys is read back through the same
    // listing: the minted key's status moves, and the published credential is untouched.
    let (status, updated) = call_signed_by(
        &router,
        Some(&published),
        "UpdateAccessKey",
        &[("AccessKeyId", minted_id.as_str()), ("Status", INACTIVE)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'the status move': expected the update to be served, got {status}: {updated}"
    );
    let (status, body) = call_signed_by(&router, Some(&published), "ListAccessKeys", &[]).await;
    let after_update: Vec<(String, String)> = expected
        .iter()
        .map(|(id, status)| {
            if *id == minted_id {
                (id.clone(), INACTIVE.to_string())
            } else {
                (id.clone(), status.clone())
            }
        })
        .collect();
    assert_eq!(
        (status, listed_keys(&body)),
        (200, after_update),
        "case 'after the status move': expected only the minted key to have moved, \
         got {status}: {body}"
    );

    // Deleting the minted key leaves the caller holding the credential it signs with.
    let (status, deleted) = call_signed_by(
        &router,
        Some(&published),
        "DeleteAccessKey",
        &[("AccessKeyId", minted_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'the delete': expected it to be served, got {status}: {deleted}"
    );
    let (status, body) = call_signed_by(&router, Some(&published), "ListAccessKeys", &[]).await;
    assert_eq!(
        (status, listed_keys(&body)),
        (200, vec![(published.clone(), ACTIVE.to_string())]),
        "case 'after the delete': expected the published credential to remain, \
         got {status}: {body}"
    );

    // The id of that credential is told only to the caller that PRESENTED it: another
    // caller asking for the same user's keys is answered the keys the account holds for
    // it, which is none.
    let (status, foreign) = call_signed_by(
        &router,
        Some(FOREIGN_ACCESS_KEY_ID),
        "ListAccessKeys",
        &[("UserName", ACCOUNT_CALLER)],
    )
    .await;
    assert_eq!(
        (status, listed_keys(&foreign)),
        (200, Vec::new()),
        "case 'another caller': expected no key id to be disclosed, got {status}: {foreign}"
    );
}

/// What a row expects of the answer.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// The call is served in full: HTTP 200.
    Served,
    /// The caller RESOLVED, and what the call then met is about the resource it named (a
    /// credential, certificate or password that is not there) rather than about the
    /// identity — so whatever the answer is, it is not the family's refusal.
    Resolved,
}

/// One row of the family table: the operation, the members its model requires beside the
/// absent `UserName`, and what the answer must be.
#[derive(Debug)]
struct Row<'a> {
    action: &'a str,
    params: Vec<(&'a str, &'a str)>,
    expect: Expect,
}

/// One row, so the table below reads as a list of calls.
fn row<'a>(action: &'a str, params: Vec<(&'a str, &'a str)>, expect: Expect) -> Row<'a> {
    Row {
        action,
        params,
        expect,
    }
}

/// RULE over the whole FAMILY: every operation of this service that resolves an absent
/// `UserName` from the signing credential serves the nameless call for the user the
/// published credential belongs to, and refuses the very same call — with HTTP 400,
/// `ValidationError` and AWS's fixed sentence — when the credential presented belongs to no
/// IAM user of the account.
///
/// The rows span the family: the user read, the three access-key calls, the three
/// login-profile calls, the four signing-certificate calls and the four
/// service-specific-credential calls; each id a row names is of the modelled shape and
/// names nothing, because the caller is resolved before any resource is looked up.
#[tokio::test]
async fn every_nameless_operation_of_the_family_resolves_the_published_credential() {
    let (router, published) = published_credential_router();
    // The upload row needs a well-formed `CertificateBody`: the certificate is read before
    // the caller's identity is resolved, so a malformed one would reach another answer than
    // the resolution under test. The fixture is the one `tests/all/signing_certificates.rs`
    // owns.
    let certificate = certificate_pem(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2024, 2034),
        "caller-identity.example.com",
    );
    let password = "S3cret-caller-identity-42";

    let family = vec![
        row("GetUser", vec![], Expect::Served),
        row("ListAccessKeys", vec![], Expect::Served),
        row("ListSigningCertificates", vec![], Expect::Served),
        row("ListServiceSpecificCredentials", vec![], Expect::Served),
        row("CreateAccessKey", vec![], Expect::Served),
        row(
            "UploadSigningCertificate",
            vec![("CertificateBody", certificate.as_str())],
            Expect::Served,
        ),
        row(
            "CreateLoginProfile",
            vec![("Password", password)],
            Expect::Served,
        ),
        row("GetLoginProfile", vec![], Expect::Resolved),
        row("DeleteLoginProfile", vec![], Expect::Resolved),
        row(
            "UpdateAccessKey",
            vec![
                ("AccessKeyId", UNMINTED_ACCESS_KEY_ID),
                ("Status", INACTIVE),
            ],
            Expect::Resolved,
        ),
        row(
            "DeleteAccessKey",
            vec![("AccessKeyId", UNMINTED_ACCESS_KEY_ID)],
            Expect::Resolved,
        ),
        row(
            "UpdateSigningCertificate",
            vec![
                ("CertificateId", UNMINTED_CERTIFICATE_ID),
                ("Status", INACTIVE),
            ],
            Expect::Resolved,
        ),
        row(
            "DeleteSigningCertificate",
            vec![("CertificateId", UNMINTED_CERTIFICATE_ID)],
            Expect::Resolved,
        ),
        row(
            "UpdateServiceSpecificCredential",
            vec![
                ("ServiceSpecificCredentialId", UNMINTED_CREDENTIAL_ID),
                ("Status", INACTIVE),
            ],
            Expect::Resolved,
        ),
        row(
            "ResetServiceSpecificCredential",
            vec![("ServiceSpecificCredentialId", UNMINTED_CREDENTIAL_ID)],
            Expect::Resolved,
        ),
        row(
            "DeleteServiceSpecificCredential",
            vec![("ServiceSpecificCredentialId", UNMINTED_CREDENTIAL_ID)],
            Expect::Resolved,
        ),
    ];

    for Row {
        action,
        params,
        expect,
    } in &family
    {
        let case = format!("{action} signed by the published credential");
        let (status, body) = call_signed_by(&router, Some(&published), action, params).await;
        assert!(
            !refused_as_unresolvable(status, &body),
            "case {case:?}: expected the caller to resolve, got {status}: {body}"
        );
        if matches!(expect, Expect::Served) {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be served, got {status}: {body}"
            );
        }

        // The other half of the rule, on the SAME call: a credential that belongs to no
        // IAM user resolves to no user, and the family refuses it in one wording.
        let case = format!("{action} signed by a credential of no IAM user");
        let (status, body) =
            call_signed_by(&router, Some(FOREIGN_ACCESS_KEY_ID), action, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert!(
            refused_as_unresolvable(status, &body),
            "case {case:?}: expected {VALIDATION_ERROR} with AWS's fixed sentence, \
             got {status}: {body}"
        );
        assert!(
            elements(&body, "UserName").is_empty(),
            "case {case:?}: expected no user to be invented, got {body}"
        );
    }
}

/// THE SAME RULE on the one member of the family whose request carries no `UserName` member
/// at all: `ChangePassword` "changes the password of the IAM user who is calling this
/// operation", so the caller is resolved the same way — and only then does the operation's
/// own state decide. A caller the credential resolves to an IAM user for is told about its
/// PASSWORD (there is none yet, then there is one and the change is served); a caller whose
/// credential belongs to no IAM user is the wrong TYPE of user for the transaction, the
/// modelled refusal, which is unchanged.
///
/// The rows differ along the password class `passwordType` admits (len 1..128, pattern
/// `^[\u{0009}\u{000A}\u{000D}\u{0020}-\u{00FF}]+$`): the published minimum length, the
/// characters the alphabet admits at its top, and an ordinary secret.
#[tokio::test]
async fn a_password_change_resolves_the_caller_the_same_way() {
    let (router, published) = published_credential_router();

    // Resolved, and the user it resolved to holds no console password: the missing entity,
    // not the wrong user type, and the message names the user the account holds.
    let (status, body) = call_signed_by(
        &router,
        Some(&published),
        "ChangePassword",
        &[("OldPassword", "S3cret-nobody"), ("NewPassword", "N3w-one")],
    )
    .await;
    assert_eq!(
        (status, error_code(&body), element(&body, "Message")),
        (
            404,
            "NoSuchEntity".to_string(),
            format!("Login Profile for User {ACCOUNT_CALLER} cannot be found.")
        ),
        "case 'no password yet': expected the missing login profile, got {status}: {body}"
    );

    // Given a password, the same nameless call is served for that user, and each change
    // leaves the one it set in force — read back by presenting it to the next change.
    let (status, created) = call_signed_by(
        &router,
        Some(&published),
        "CreateLoginProfile",
        &[("Password", "S3cret-first-42")],
    )
    .await;
    assert_eq!(
        (status, element(&created, "UserName")),
        (200, ACCOUNT_CALLER.to_string()),
        "case 'the first password': expected it to be created for the caller, \
         got {status}: {created}"
    );
    let mut in_force = "S3cret-first-42".to_string();
    for next in ["~", "N3w-\u{00FF}-caller", "S3cret-last-42"] {
        let (status, changed) = call_signed_by(
            &router,
            Some(&published),
            "ChangePassword",
            &[("OldPassword", in_force.as_str()), ("NewPassword", next)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {next:?}: expected the change to be served for the caller, \
             got {status}: {changed}"
        );
        // The password just set is the one in force: presenting the PREVIOUS one is now
        // refused, which is what makes the change observable.
        let (status, refused) = call_signed_by(
            &router,
            Some(&published),
            "ChangePassword",
            &[
                ("OldPassword", in_force.as_str()),
                ("NewPassword", "N3w-again"),
            ],
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (403, "AccessDenied".to_string()),
            "case {next:?}: expected the superseded password to be refused, \
             got {status}: {refused}"
        );
        in_force = next.to_string();
    }

    // A credential that belongs to no IAM user is the wrong type of user for this
    // transaction — the modelled refusal, and nothing of the caller's password changed.
    let (status, refused) = call_signed_by(
        &router,
        Some(FOREIGN_ACCESS_KEY_ID),
        "ChangePassword",
        &[
            ("OldPassword", in_force.as_str()),
            ("NewPassword", "N3w-foreign"),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "InvalidUserType".to_string()),
        "case 'a credential of no IAM user': expected the modelled wrong-user-type refusal, \
         got {status}: {refused}"
    );
    let (status, served) = call_signed_by(
        &router,
        Some(&published),
        "ChangePassword",
        &[
            ("OldPassword", in_force.as_str()),
            ("NewPassword", "S3cret-after-the-refusal"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'after the refusal': expected the held password to still be in force, \
         got {status}: {served}"
    );
}
