// SPDX-License-Identifier: CC0-1.0

//! One role ARN, two services: STS never says whether the role EXISTS, IAM always does.
//!
//! RULE: a request that asks STS to vend credentials for a role ARN the account's IAM
//! records do not resolve is refused as an AUTHORIZATION failure — error code
//! `AccessDenied`, HTTP 403, worded as the principal, the action and the resource — the
//! same refusal a role the account DOES hold gets when its trust policy withholds the
//! grant, so the answer discloses nothing about the role's existence and nothing is
//! minted. IAM, the service that owns the role, answers for the same name with its own
//! modelled `NoSuchEntity` (HTTP 404). The refusal ends exactly when IAM holds a role
//! whose ARN the request names and whose trust policy admits the caller, and resumes when
//! that record goes away again — which is what makes "retry the `AccessDenied` until the
//! grant is in place" a decidable contract for a client rather than a sleep: the client
//! can tell "not yet" from "never" only by asking the two services separately.
//!
//! Only a cross-service suite can pin this. The two halves are owned by different
//! services — IAM holds the role record and words the not-found answer, STS reads that
//! record and words the refusal — so a crate-local suite can assert at most one of them
//! and never that the pair disagrees on purpose.
//!
//! The rows differ along what the rule generalises over: WHY the ARN resolves to nothing
//! (never created, created and then deleted, held under another path than the ARN names),
//! the role name (its length, from `roleNameType`'s 1-character floor to its 64-character
//! ceiling, and the punctuation its pattern `^[\w+=,.@-]+$` admits) and the path the ARN
//! embeds.
//!
//! The awsQuery encoder and the XML element readers are this test crate's own
//! (`query_wire`); the IAM+STS router and its two call helpers are the ones
//! `iam_role_id_agrees_with_sts` already builds for exactly this pair of services.

use crate::iam_role_id_agrees_with_sts::{build_router, iam, member, sts, LONG_TERM_KEY};
use crate::query_wire::{element, opt_element};

/// Why the role ARN under test resolves to no role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Absence {
    /// The account never held a role of that name.
    NeverCreated,
    /// The account held it and `iam:DeleteRole` removed it.
    Deleted,
    /// The account holds a role of that name, but under a different path — so the ARN the
    /// request names is no role's ARN.
    HeldUnderAnotherPath,
}

/// The wording of the refusal AWS gives an authorization failure: the principal, the
/// action and the resource.
fn not_authorized(caller_arn: &str, action: &str, resource: &str) -> String {
    format!("User: {caller_arn} is not authorized to perform: {action} on resource: {resource}")
}

/// Phrases that would DISCLOSE the role's non-existence. None of them may appear in an
/// STS answer, whichever of the three kinds of absence produced it.
const DISCLOSURES: &[&str] = &[
    "NoSuchEntity",
    "cannot be found",
    "does not exist",
    "NotFound",
    "ResourceNotFound",
];

/// A trust policy granting `principal` permission to assume the role.
fn trust_policy_for(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// Create one role through IAM's wire and answer the `Arn` and `RoleId` it reported.
async fn create_role(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    role_name: &str,
    path: &str,
    trust_policy: &str,
) -> (String, String) {
    let (status, body) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", path.to_string()),
            ("AssumeRolePolicyDocument", trust_policy.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateRole, got {status} with {body}"
    );
    (
        member(case, &body, "Arn"),
        member(case, &body, "RoleId"),
    )
}

/// Delete one role through IAM's wire.
async fn delete_role(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    role_name: &str,
) {
    let (status, body) = iam(router, "DeleteRole", &[("RoleName", role_name.to_string())]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteRole, got {status} with {body}"
    );
}

/// `sts:AssumeRole` on `role_arn`, as the long-term caller.
async fn assume_role(
    router: &smithy_server_runtime::MultiServiceRouter,
    role_arn: &str,
    session_name: &str,
) -> (u16, String) {
    sts(
        router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", role_arn.to_string()),
            ("RoleSessionName", session_name.to_string()),
        ],
    )
    .await
}

/// STS refuses an ARN that resolves to no role with the SAME non-disclosing
/// `AccessDenied`/403 a withheld grant gets, and mints nothing; IAM answers about the
/// same NAME with its own modelled not-found error. The refusal ends when IAM holds the
/// role the ARN names, and resumes when the record is removed again.
#[tokio::test]
async fn an_absent_role_is_refused_by_sts_as_access_denied_and_reported_absent_by_iam() {
    // `roleNameType` is length 1..64 with the pattern `^[\w+=,.@-]+$`
    // (smithy_query(query="shape",shape="roleNameType") on the IAM model), so the names
    // below walk both bounds of that length and the punctuation the pattern admits.
    let ceiling_name = format!("sim2real-absent-{}", "n".repeat(64 - "sim2real-absent-".len()));
    let rows: Vec<(&str, String, &str, Absence)> = vec![
        (
            "a role that was never created",
            "never-created".to_string(),
            "/",
            Absence::NeverCreated,
        ),
        (
            "a never-created role under a path",
            "never-created-pathed".to_string(),
            "/engineering/",
            Absence::NeverCreated,
        ),
        (
            "a never-created role at roleNameType's 1-character floor",
            "r".to_string(),
            "/",
            Absence::NeverCreated,
        ),
        (
            "a never-created role at roleNameType's 64-character ceiling",
            ceiling_name,
            "/",
            Absence::NeverCreated,
        ),
        (
            "a never-created role spelled with the punctuation the pattern admits",
            "never.created-role_name@example=1+2,3".to_string(),
            "/deeply/nested/",
            Absence::NeverCreated,
        ),
        (
            "a role the account created and then deleted",
            "created-then-deleted".to_string(),
            "/",
            Absence::Deleted,
        ),
        (
            "a deleted role under a path",
            "created-then-deleted-pathed".to_string(),
            "/engineering/",
            Absence::Deleted,
        ),
        (
            "a role of that name held under another path than the ARN names",
            "held-elsewhere".to_string(),
            "/",
            Absence::HeldUnderAnotherPath,
        ),
    ];

    for (case, role_name, path, absence) in rows {
        // A fresh emulator per row, so no row's records decide another's answer.
        let router = build_router();

        // Who the caller IS, and which account it speaks for, read from the emulator
        // rather than assumed: both are variable parts of the refusal below.
        let (status, identity) = sts(&router, LONG_TERM_KEY, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetCallerIdentity, got {status} {identity}"
        );
        let caller_arn = member(case, &identity, "Arn");
        let account = member(case, &identity, "Account");
        let trust_policy = trust_policy_for(&format!("arn:aws:iam::{account}:root"));
        let requested = format!("arn:aws:iam::{account}:role{path}{role_name}");

        // Put the account's IAM records into the state this row is about.
        match absence {
            Absence::NeverCreated => {}
            Absence::Deleted => {
                create_role(&router, case, &role_name, path, &trust_policy).await;
                delete_role(&router, case, &role_name).await;
            }
            Absence::HeldUnderAnotherPath => {
                create_role(&router, case, &role_name, "/service-role/", &trust_policy).await;
            }
        }

        // IAM's answer about the NAME: the owning service discloses exactly what STS
        // withholds.
        let (status, body) = iam(&router, "GetRole", &[("RoleName", role_name.clone())]).await;
        if absence == Absence::HeldUnderAnotherPath {
            assert_eq!(
                status, 200,
                "case {case:?}: IAM holds a role of that name, so GetRole must report it, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from GetRole for an absent role, \
                 got {status} {body}"
            );
            assert_eq!(
                opt_element(&body, "Code").as_deref(),
                Some("NoSuchEntity"),
                "case {case:?}: expected NoSuchEntity from the service that owns the role, \
                 got {body}"
            );
            assert_eq!(
                opt_element(&body, "Message").as_deref(),
                Some(format!("The role with name {role_name} cannot be found.").as_str()),
                "case {case:?}: the not-found message must name the role asked for, got {body}"
            );
        }

        // STS's answer about the ARN: an authorization refusal that says nothing about
        // existence, and no credential.
        let (status, body) = assume_role(&router, &requested, "sim2real-behav-absent").await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 from AssumeRole on an ARN that resolves to no \
             role, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code"),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            not_authorized(&caller_arn, "sts:AssumeRole", &requested),
            "case {case:?}: the refusal must name the principal, the action and the role, \
             got {body}"
        );
        for disclosure in DISCLOSURES {
            assert!(
                !body.contains(disclosure),
                "case {case:?}: STS must not disclose the role's non-existence, but its \
                 answer carries {disclosure:?}: {body}"
            );
        }
        for minted in ["AccessKeyId", "SessionToken", "AssumedRoleId"] {
            assert!(
                opt_element(&body, minted).is_none(),
                "case {case:?}: a refused assume must mint nothing, but the answer carries \
                 <{minted}>: {body}"
            );
        }

        // The retry contract terminates: once IAM holds the role the ARN names, with a
        // trust policy admitting the caller, the very same call succeeds.
        if absence == Absence::HeldUnderAnotherPath {
            delete_role(&router, case, &role_name).await;
        }
        let (arn, role_id) = create_role(&router, case, &role_name, path, &trust_policy).await;
        assert_eq!(
            arn, requested,
            "case {case:?}: expected CreateRole to report the ARN the refused request named, \
             got {arn:?}"
        );
        let (status, body) = assume_role(&router, &requested, "sim2real-behav-absent").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from AssumeRole once IAM holds the role, \
             got {status} {body}"
        );
        assert_eq!(
            element(&body, "AssumedRoleId"),
            format!("{role_id}:sim2real-behav-absent"),
            "case {case:?}: the minted session must carry the role id IAM holds, got {body}"
        );

        // And it resumes: the grant is gone with the record, so the client is back to the
        // refusal it was retrying — the two are the same answer, never a not-found.
        delete_role(&router, case, &role_name).await;
        let (status, body) = assume_role(&router, &requested, "sim2real-behav-absent").await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 from AssumeRole once the role is gone again, \
             got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code"),
            "AccessDenied",
            "case {case:?}: expected AccessDenied after the role was deleted, got {body}"
        );
    }
}

/// The two refusals are INDISTINGUISHABLE: a role the account does not hold and a role it
/// holds whose trust policy withholds the grant answer with the same status, the same code
/// and the same message, so nothing in the answer tells a client which of the two it hit.
///
/// The rows differ in HOW the held role withholds the grant — no trust policy at all,
/// one naming another account's root, one allowing only another action, and an explicit
/// deny — so the comparison spans the class of withheld grants rather than one policy.
#[tokio::test]
async fn a_withheld_grant_and_an_absent_role_are_refused_in_the_same_words() {
    let router = build_router();
    let (status, identity) = sts(&router, LONG_TERM_KEY, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from GetCallerIdentity, got {status} {identity}"
    );
    let caller_arn = member("caller", &identity, "Arn");
    let account = member("caller", &identity, "Account");

    // The reference answer: an ARN no role of the account resolves to.
    let absent = format!("arn:aws:iam::{account}:role/never-created-reference");
    let (absent_status, absent_body) = assume_role(&router, &absent, "probe").await;
    assert_eq!(
        absent_status, 403,
        "expected HTTP 403 for an absent role, got {absent_status} {absent_body}"
    );
    let absent_code = element(&absent_body, "Code");
    let absent_message = element(&absent_body, "Message");
    assert_eq!(
        absent_message,
        not_authorized(&caller_arn, "sts:AssumeRole", &absent),
        "the refusal for an absent role must name the principal, the action and the role, \
         got {absent_body}"
    );

    let rows: Vec<(&str, &str, Option<String>)> = vec![
        ("a held role carrying no trust policy", "no-trust", None),
        (
            "a held role trusting only another account's root",
            "other-account",
            Some(trust_policy_for("arn:aws:iam::111122223333:root")),
        ),
        (
            "a held role allowing only another action",
            "other-action",
            Some(
                trust_policy_for(&format!("arn:aws:iam::{account}:root"))
                    .replace("sts:AssumeRole", "sts:TagSession"),
            ),
        ),
        (
            "a held role explicitly denying the assume",
            "explicit-deny",
            Some(
                trust_policy_for(&format!("arn:aws:iam::{account}:root"))
                    .replace("\"Allow\"", "\"Deny\""),
            ),
        ),
    ];

    for (case, role_name, trust_policy) in rows {
        // A role with no trust document at all is not creatable through the wire
        // (`AssumeRolePolicyDocument` is required), so that row is seeded as a document
        // that grants nothing instead: the withheld grant is what is under test.
        let document = trust_policy.unwrap_or_else(|| {
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string()
        });
        let (arn, _role_id) = create_role(&router, case, role_name, "/", &document).await;

        // The role IS there: IAM reports it, which is the half STS withholds.
        let (status, held) = iam(&router, "GetRole", &[("RoleName", role_name.to_string())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRole for a held role, got {status} {held}"
        );

        let (status, body) = assume_role(&router, &arn, "probe").await;
        assert_eq!(
            status, absent_status,
            "case {case:?}: a withheld grant must be refused with the same status as an \
             absent role ({absent_status}), got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code"),
            absent_code,
            "case {case:?}: a withheld grant must be refused with the same code as an absent \
             role ({absent_code}), got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            not_authorized(&caller_arn, "sts:AssumeRole", &arn),
            "case {case:?}: the refusal must be worded exactly as the one for an absent \
             role, with this role's own ARN, got {body}"
        );
        assert_eq!(
            element(&body, "Message").replace(&arn, "<role>"),
            absent_message.replace(&absent, "<role>"),
            "case {case:?}: apart from the ARN it names, the refusal must be the same text as \
             the one an absent role gets, got {body}"
        );
    }
}
