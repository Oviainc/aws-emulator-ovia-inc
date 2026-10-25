// SPDX-License-Identifier: CC0-1.0

//! A role's trust policy is re-read and re-evaluated on EVERY `AssumeRole`.
//!
//! RULE: each `AssumeRole` decides against the `AssumeRolePolicyDocument` the account
//! holds for the role AT THAT MOMENT — never a document captured when the role was
//! created or when an earlier call was served — so replacing the document through
//! `iam:UpdateAssumeRolePolicy` takes effect on the next call in BOTH directions: once
//! the stored document no longer authorizes the caller every later call is refused with
//! `AccessDenied` / HTTP 403 and stays refused, and once it authorizes the caller again
//! a session is minted again. A credential already vended is unaffected: it keeps
//! resolving until its own `Expiration`.
//!
//! The rows differ along what the rule generalises over — which ELEMENT of the stored
//! document changed (its `Condition`, its `Principal`, its `Action`, its `Effect`), which
//! condition key and operator the clause names, and whether the replacement withholds
//! the grant or restores it. The recorded case (a `Condition` on the calling principal's
//! ARN naming a principal the caller is not) is one row.
//!
//! Both services are mounted on ONE engine, as the router mounts them, so the document
//! the decision reads is the one `iam:UpdateAssumeRolePolicy` actually wrote and
//! `iam:GetRole` reports — not a value this suite poked into the store.

use smithy_server_runtime::MultiServiceRouter;

// The emulator constructor and the awsQuery wire helpers are this crate's own, written
// once in `sts_sessions`: `emulator_with_iam` mounts both services on one engine,
// `query` is the signed POST (its signing name is a parameter for exactly this suite),
// and `element` / `member` read one element out of the answer.
use super::sts_sessions::{
    element, emulator_account, emulator_with_iam, member, query, LONG_TERM_KEY, STS_API_VERSION,
    STS_SIGNING_NAME,
};

/// IAM's SigV4 signing name: the trust policy under test is written by IAM, so this
/// suite drives two signing names through the one shared wire helper.
const IAM_SIGNING_NAME: &str = "iam";

/// IAM's awsQuery API version.
const IAM_API_VERSION: &str = "2010-05-08";

/// The `Sid` of the statement a role is created with, so a read of the stored document
/// names which generation of it the control plane is reporting, whatever the escaping.
const BASELINE_SID: &str = "baseline-statement";

/// The `Sid` the replacement document of the table below carries. See [`BASELINE_SID`].
const REPLACEMENT_SID: &str = "replacement-statement";

/// The `Sid` of the second statement of the one replacement document that carries two:
/// a `Sid` "must be unique within the JSON policy document", so the `Deny` beside the
/// `Allow` cannot repeat [`REPLACEMENT_SID`].
const REPLACEMENT_DENY_SID: &str = "replacement-deny";

/// The session name every assume in this suite uses.
const SESSION_NAME: &str = "convergence";

/// One IAM action, from the caller every call of this suite presents.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        LONG_TERM_KEY,
        IAM_SIGNING_NAME,
        IAM_API_VERSION,
        action,
        params,
    )
    .await
}

/// One STS action, signed with `access_key_id`.
async fn sts_call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        access_key_id,
        STS_SIGNING_NAME,
        STS_API_VERSION,
        action,
        params,
    )
    .await
}

/// One statement of a trust policy, with the `Sid` that tells which generation of the
/// document it belongs to and an optional `Condition` block.
///
/// `sts_sessions::trust_policy` and `sts_authorization::trust_policy_for` build the
/// single-statement, `Allow`-only document their own suites need; the rows below also
/// vary the `Effect` and carry two statements in one document, and they identify a
/// generation of the document by its `Sid`, so the statement is built here and the
/// document assembled from a list of them.
fn statement(
    sid: &str,
    effect: &str,
    principal: &str,
    actions: &[&str],
    condition: Option<&str>,
) -> String {
    let actions = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<_>>()
        .join(",");
    let condition = condition
        .map(|clause| format!(",\"Condition\":{clause}"))
        .unwrap_or_default();
    format!(
        "{{\"Sid\":\"{sid}\",\"Effect\":\"{effect}\",\"Principal\":{{\"AWS\":\"{principal}\"}},\
         \"Action\":[{actions}]{condition}}}"
    )
}

/// A trust policy document carrying `statements`.
fn document(statements: &[String]) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        statements.join(",")
    )
}

/// The document a role of this suite is created with: the account's own principals may
/// assume it.
fn baseline_document(root: &str) -> String {
    document(&[statement(
        BASELINE_SID,
        "Allow",
        root,
        &["sts:AssumeRole"],
        None,
    )])
}

/// Create one role over IAM's own wire and answer its ARN.
async fn create_role(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    trust: &str,
) -> String {
    let (status, body) = iam_call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", "/".to_string()),
            ("AssumeRolePolicyDocument", trust.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateRole, got {status} {body}"
    );
    member(case, &body, "Arn")
}

/// Replace a role's trust policy over IAM's own wire, and assert the control plane
/// reports THAT replacement: `GetRole` carries the statement `now` identifies and no
/// longer the one `before` identifies, so the confirmation is a real one for every
/// generation of the document rather than only the first.
async fn replace_trust_policy(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    replacement: &str,
    now: &str,
    before: &str,
) {
    let (status, body) = iam_call(
        router,
        "UpdateAssumeRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyDocument", replacement.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateAssumeRolePolicy, got {status} {body}"
    );

    let (status, body) = iam_call(router, "GetRole", &[("RoleName", role_name.to_string())]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetRole, got {status} {body}"
    );
    assert!(
        body.contains(now) && !body.contains(before),
        "case {case:?}: expected GetRole to report the {now:?} trust document and not the \
         {before:?} one, got {body}"
    );
}

/// Replacing a role's trust policy decides every LATER `AssumeRole`, whichever element
/// of the document changed: a document that no longer authorizes the caller refuses with
/// `AccessDenied` / HTTP 403 and keeps refusing, one that still does keeps minting — and
/// a session vended before the replacement goes on resolving either way.
#[tokio::test]
async fn an_assume_role_is_decided_by_the_trust_document_stored_now() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let other_root = "arn:aws:iam::999999999999:root".to_string();

    // One emulator for the whole table: every row creates its OWN role, so the rows do
    // not reach each other, and the caller identity the clauses below are written
    // against is read from this same emulator rather than written down here — the rows
    // that test a clause ABOUT the caller are built from what the decision will see.
    let (router, _engine) = emulator_with_iam();
    let (status, body) = sts_call(&router, LONG_TERM_KEY, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from GetCallerIdentity, got {status} {body}"
    );
    let caller_arn = member("the calling identity", &body, "Arn");
    let absent = format!("arn:aws:iam::{account}:user/does-not-exist");

    // (case, the document that REPLACES the baseline, the status every assume after the
    // replacement must answer)
    let rows: Vec<(&str, String, u16)> = vec![
        (
            "a condition on the calling principal's ARN naming a principal the caller is not",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                &root,
                &["sts:AssumeRole"],
                Some(&format!(
                    "{{\"StringEquals\":{{\"aws:PrincipalArn\":\"{absent}\"}}}}"
                )),
            )]),
            403,
        ),
        (
            "a condition on the calling principal's ARN the caller satisfies",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                &root,
                &["sts:AssumeRole"],
                Some(&format!(
                    "{{\"StringEquals\":{{\"aws:PrincipalArn\":\"{caller_arn}\"}}}}"
                )),
            )]),
            200,
        ),
        (
            "an ARN-pattern condition that admits only role principals",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                &root,
                &["sts:AssumeRole"],
                Some("{\"ArnLike\":{\"aws:PrincipalArn\":\"arn:aws:iam::*:role/*\"}}"),
            )]),
            403,
        ),
        (
            "a condition on the calling account naming another account",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                &root,
                &["sts:AssumeRole"],
                Some("{\"StringEquals\":{\"aws:PrincipalAccount\":\"999999999999\"}}"),
            )]),
            403,
        ),
        (
            "a condition on the calling account the caller satisfies",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                &root,
                &["sts:AssumeRole"],
                Some(&format!(
                    "{{\"StringEquals\":{{\"aws:PrincipalAccount\":\"{account}\"}}}}"
                )),
            )]),
            200,
        ),
        (
            "an explicit Deny beside the Allow",
            document(&[
                statement(
                    REPLACEMENT_SID,
                    "Allow",
                    &root,
                    &["sts:AssumeRole"],
                    None,
                ),
                statement(
                    REPLACEMENT_DENY_SID,
                    "Deny",
                    &root,
                    &["sts:AssumeRole"],
                    None,
                ),
            ]),
            403,
        ),
        (
            "the principal replaced by another account's root",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                &other_root,
                &["sts:AssumeRole"],
                None,
            )]),
            403,
        ),
        (
            "the action narrowed away from the assume",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                &root,
                &["sts:TagSession"],
                None,
            )]),
            403,
        ),
        (
            "the principal widened to every principal",
            document(&[statement(
                REPLACEMENT_SID,
                "Allow",
                "*",
                &["sts:AssumeRole"],
                None,
            )]),
            200,
        ),
    ];

    for (index, (case, replacement, expected_status)) in rows.into_iter().enumerate() {
        let role_name = format!("convergence-{index}");
        let role_arn = create_role(&router, case, &role_name, &baseline_document(&root)).await;
        let assume = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", SESSION_NAME.to_string()),
        ];

        // The positive control: under the document the role was created with, the caller
        // may assume it.
        let (status, body) = sts_call(&router, LONG_TERM_KEY, "AssumeRole", &assume).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the baseline document must admit the caller, got {status} {body}"
        );
        let vended = member(case, &body, "AccessKeyId");

        replace_trust_policy(
            &router,
            case,
            &role_name,
            &replacement,
            REPLACEMENT_SID,
            BASELINE_SID,
        )
        .await;

        // Three consecutive calls: the answer is the one the document now stored states,
        // from the FIRST call after the replacement, and it does not drift afterwards.
        for attempt in 1..=3 {
            let (status, body) = sts_call(&router, LONG_TERM_KEY, "AssumeRole", &assume).await;
            assert_eq!(
                status, expected_status,
                "case {case:?} attempt {attempt}: expected HTTP {expected_status} from the \
                 document stored now, got {status} {body}"
            );
            if expected_status == 403 {
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some("AccessDenied"),
                    "case {case:?} attempt {attempt}: expected the AccessDenied code, got {body}"
                );
                assert_eq!(
                    member(case, &body, "Message"),
                    format!(
                        "User: {caller_arn} is not authorized to perform: sts:AssumeRole on \
                         resource: {role_arn}"
                    ),
                    "case {case:?} attempt {attempt}: expected the refusal to name the \
                     principal, the action and the role, got {body}"
                );
                assert!(
                    body.contains("<Type>Sender</Type>"),
                    "case {case:?} attempt {attempt}: expected an awsQuery ErrorResponse with a \
                     Sender fault, got {body}"
                );
            } else {
                assert!(
                    element(&body, "AccessKeyId").is_some(),
                    "case {case:?} attempt {attempt}: expected a minted session, got {body}"
                );
            }
        }

        // A credential already handed out is not what the trust policy decides: it keeps
        // resolving until its own Expiration, whatever the document now says.
        let (status, body) = sts_call(&router, &vended, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the session minted before the replacement must keep resolving, got \
             {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Arn"),
            format!("arn:aws:sts::{account}:assumed-role/{role_name}/{SESSION_NAME}"),
            "case {case:?}: the vended session must still speak for the role session it was \
             minted for, got {body}"
        );
    }
}

/// No decision is remembered in either direction: one role, assumed after each of four
/// successive documents, answers what the document in force at that moment states.
///
/// The sequence is what a break-glass revocation and its reversal look like — tighten,
/// confirm refused, loosen, confirm granted again — so it pins that the refusal is
/// neither cached away nor latched on.
#[tokio::test]
async fn each_trust_document_in_a_sequence_decides_the_assume_made_under_it() {
    let (router, _engine) = emulator_with_iam();
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let other_root = "arn:aws:iam::999999999999:root".to_string();
    let role_arn = create_role(
        &router,
        "the initial document",
        "revocable",
        &baseline_document(&root),
    )
    .await;
    let assume = vec![
        ("RoleArn", role_arn),
        ("RoleSessionName", SESSION_NAME.to_string()),
    ];

    // (case, the `Sid` identifying THIS generation of the document, the document to put
    // in force, the status the assume under it must answer). Each phase carries its own
    // `Sid`, so the `GetRole` confirmation that the write landed is a real one at every
    // phase and not only the first.
    let phases: Vec<(&str, &str, String, u16)> = vec![
        (
            "revoked by a condition the caller cannot satisfy",
            "revoked-once",
            document(&[statement(
                "revoked-once",
                "Allow",
                &root,
                &["sts:AssumeRole"],
                Some(
                    "{\"StringEquals\":\
                     {\"aws:PrincipalArn\":\"arn:aws:iam::000000000000:user/nobody\"}}",
                ),
            )]),
            403,
        ),
        (
            "restored to the account's own principals",
            "restored-once",
            document(&[statement(
                "restored-once",
                "Allow",
                &root,
                &["sts:AssumeRole"],
                None,
            )]),
            200,
        ),
        (
            "revoked again by narrowing the principal",
            "revoked-twice",
            document(&[statement(
                "revoked-twice",
                "Allow",
                &other_root,
                &["sts:AssumeRole"],
                None,
            )]),
            403,
        ),
        (
            "restored again",
            "restored-twice",
            document(&[statement(
                "restored-twice",
                "Allow",
                &root,
                &["sts:AssumeRole", "sts:TagSession"],
                None,
            )]),
            200,
        ),
    ];

    // The generation the stored document carried before the phase about to be put in
    // force, which starts out as the one the role was created with.
    let mut previous_sid = BASELINE_SID;
    for (case, sid, in_force, expected_status) in phases {
        replace_trust_policy(&router, case, "revocable", &in_force, sid, previous_sid).await;
        previous_sid = sid;
        let (status, body) = sts_call(&router, LONG_TERM_KEY, "AssumeRole", &assume).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} under the document in force, got \
             {status} {body}"
        );
        if expected_status == 403 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("AccessDenied"),
                "case {case:?}: expected the AccessDenied code, got {body}"
            );
        }
    }
}
