// SPDX-License-Identifier: CC0-1.0

//! WHO a request speaks for, checked against STS's own answer for the same credential.
//!
//! The rule under test: the identity this service attributes a request to is the principal
//! the credential it was signed with resolves to — exactly the `Arn`
//! `sts:GetCallerIdentity` answers for that same access key id — never an identity
//! invented from the key id's text. The two services resolve a caller from the one set of
//! records in the shared engine (the session registry `sts:AssumeRole` writes, and the
//! account's IAM access keys), so a client that reads its own identity once and then reads
//! back what it did addresses one principal.
//!
//! The rows span the kinds of credential that resolve to a principal: the long-term key
//! pair this emulator publishes at start (the one credential it issues to its clients, held
//! for the account's own user), an access key the account holds for an IAM user, the
//! temporary credentials of an assumed role, and an `sts:GetSessionToken` session speaking
//! for an IAM user.
//!
//! Each row drives the identity through three members of three operations — a pull
//! request's `authorArn`, the `overrider` `GetPullRequestOverrideState` reports, and the
//! `mergedBy` a merge records — so the rule is checked wherever this service names its
//! caller rather than for one of them.
//!
//! STS is MOUNTED beside this service on the one shared engine here, because the assertion
//! is an equality between what the two services answer for one credential; the sessions and
//! the IAM access keys are seeded into that engine the way the operations that mint one
//! record it, since what is under test is the resolution, not the minting.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts_service;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::repository_lifecycle::{base64, string_at};
use crate::wire::{fresh_client_request_token, hold_long_term_key_for, ok_as};

/// The operations the caller's identity is read back through.
const OVERRIDE_OPERATION: &str = "OverridePullRequestApprovalRules";
const OVERRIDE_STATE_OPERATION: &str = "GetPullRequestOverrideState";
const MERGE_OPERATION: &str = "MergePullRequestByFastForward";

/// `OverrideStatus`'s value asking for the rules to be set aside
/// (`smithy_query(query="shape",shape="OverrideStatus")`).
const OVERRIDE: &str = "OVERRIDE";

/// The schema version of an approval rule document and the only statement type its schema
/// defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

const DESTINATION: &str = "main";
const SOURCE: &str = "feature";
const RULE: &str = "needs-one-review";

/// How long a seeded live session lasts: longer than a row takes to run, so none expires
/// while the row is driven.
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

/// STS's awsQuery signing name and API version, the scope and `Version` its calls carry.
const STS_SIGNING_NAME: &str = "sts";
const STS_API_VERSION: &str = "2011-06-15";

/// The emulator these rows drive: one shared engine, this service and STS mounted on it,
/// request authentication off (the calls sign nothing — they carry the `Credential=` scope
/// each service reads its caller from).
fn emulator_with_sts() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(sts_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The `Arn` `sts:GetCallerIdentity` answers for `access_key_id`, read over the wire the
/// way a client asks for it: an `application/x-www-form-urlencoded` awsQuery POST with the
/// key id in the SigV4 `Credential=` scope.
async fn caller_identity_arn(
    router: &MultiServiceRouter,
    access_key_id: &str,
    case: &str,
) -> String {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/{STS_SIGNING_NAME}/\
         aws4_request, SignedHeaders=host;x-amz-date, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{STS_SIGNING_NAME}.amazonaws.com/"))
        .header("host", format!("{STS_SIGNING_NAME}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization)
        .body(bytes::Bytes::from(format!(
            "Action=GetCallerIdentity&Version={STS_API_VERSION}"
        )))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetCallerIdentity to answer 200, got {status} with {body}"
    );
    let open = "<Arn>";
    let close = "</Arn>";
    let start = body
        .find(open)
        .unwrap_or_else(|| panic!("case {case:?}: no <Arn> in {body}"))
        + open.len();
    let end = body[start..]
        .find(close)
        .unwrap_or_else(|| panic!("case {case:?}: no </Arn> in {body}"))
        + start;
    body[start..end].to_string()
}

/// Which credential a row's calls are signed with.
#[derive(Debug, Clone, Copy)]
enum Credential {
    /// The long-term key pair this emulator process publishes at start.
    Published,
    /// A long-term access key the account holds for an IAM user, as `iam:CreateAccessKey`
    /// records one.
    IamUserKey { user: &'static str },
    /// The temporary credentials of an assumed role, live in the session registry.
    AssumedRole {
        role: &'static str,
        session: &'static str,
    },
    /// The temporary credentials of an `sts:GetSessionToken` session, which speak for the
    /// IAM user that asked for them.
    SessionToken { user: &'static str },
}

fn account() -> String {
    engine::emulator_account_id().to_string()
}

fn user_arn(user: &str) -> String {
    format!("arn:aws:iam::{}:user/{user}", account())
}

fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", account())
}

fn assumed_role_arn(role: &str, session: &str) -> String {
    format!("arn:aws:sts::{}:assumed-role/{role}/{session}", account())
}

impl Credential {
    /// The access key id a request signed with this credential carries.
    ///
    /// The published pair's key id is asked of the authenticator that MINTED it, so it is
    /// the id requests are really verified against rather than a copy of its text.
    fn access_key_id(self) -> String {
        match self {
            Self::Published => {
                emulator_server::request_authentication::SigV4Authenticator::for_this_process()
                    .access_key_id
                    .clone()
            }
            Self::IamUserKey { user } => format!("AKID{}HELDKEY", user.to_uppercase()),
            Self::AssumedRole { session, .. } => format!("AKID{}", session.to_uppercase()),
            Self::SessionToken { user } => format!("AKID{}SESSION", user.to_uppercase()),
        }
    }

    /// Write the record this credential is resolved through into the shared engine, the way
    /// the operation that mints one records it. The published pair needs none: the account
    /// holds it from the moment the process publishes it.
    fn register(self, engine: &engine::InMemoryEngine, access_key_id: &str) {
        let now = SystemTime::now();
        let (identity, kind, role, expires_at) = match self {
            Self::Published => return,
            Self::IamUserKey { user } => {
                // A long-term key names no session: the account's record of WHO holds
                // the key is IAM's, written here through the harness's one writer of
                // those records.
                hold_long_term_key_for(engine, user, access_key_id);
                return;
            }
            Self::AssumedRole { role, session } => (
                sts::Identity {
                    user_id: format!("{}:{session}", sts::role_id(&role_arn(role))),
                    account: account(),
                    arn: assumed_role_arn(role, session),
                },
                sts::SessionKind::AssumedRole,
                Some(role_arn(role)),
                now + SESSION_LIFETIME,
            ),
            Self::SessionToken { user } => (
                sts::Identity {
                    user_id: sts::unique_id("AIDA", &user_arn(user)),
                    account: account(),
                    arn: user_arn(user),
                },
                sts::SessionKind::SessionToken,
                None,
                now + SESSION_LIFETIME,
            ),
        };
        let vend = sts::NewSession {
            role_arn: role,
            ..sts::NewSession::new(identity, kind, SESSION_LIFETIME.as_secs() as i32)
        };
        let registry =
            sts::service_state(engine).expect("the in-memory engine carries the STS store");
        registry
            .lock()
            .insert(sts::Session {
                // The vend's own clock, which each form varies: the credential was handed
                // out a lifetime ago and stops being valid at `expires_at`. Everything
                // else about the record is what the registry's constructor builds, so a
                // fact added to a session is recorded here as a mint records it.
                created_at: now - SESSION_LIFETIME,
                expires_at,
                ..vend.into_session(access_key_id, now)
            })
            .expect("the session registry has room for one session");
    }
}

/// An approval rule document asking for one approval from a pool nothing in it can give,
/// so the pull request's requirements are met only by an override.
fn unmeetable_document() -> String {
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [{
            "Type": STATEMENT_TYPE,
            "NumberOfApprovalsNeeded": 1,
            "ApprovalPoolMembers": [user_arn("no-such-approver")],
        }],
    }))
    .expect("a JSON document")
}

#[tokio::test]
async fn the_identity_this_service_records_is_the_one_sts_resolves_for_the_credential() {
    let rows = [
        (
            "the long-term key pair this emulator publishes",
            "published-key-caller",
            Credential::Published,
        ),
        (
            "an access key the account holds for an IAM user",
            "iam-user-key-caller",
            Credential::IamUserKey { user: "Mary_Major" },
        ),
        (
            "the temporary credentials of an assumed role",
            "assumed-role-caller",
            Credential::AssumedRole {
                role: "CodeCommitRelease",
                session: "release-session-1",
            },
        ),
        (
            "a session token speaking for an IAM user",
            "session-token-caller",
            Credential::SessionToken { user: "Li_Juan" },
        ),
    ];

    for (case, repository, credential) in rows {
        let (router, engine) = emulator_with_sts();
        let access_key_id = credential.access_key_id();
        credential.register(&engine, &access_key_id);

        // What STS answers for this very credential: the principal every later assertion
        // is made against, read from the service that owns the question rather than
        // written out here.
        let expected = caller_identity_arn(&router, &access_key_id, case).await;
        assert!(
            expected.starts_with("arn:aws:"),
            "case {case:?}: expected an ARN from GetCallerIdentity, got {expected:?}"
        );

        ok_as(
            &router,
            &access_key_id,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        let seeded = ok_as(
            &router,
            &access_key_id,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": DESTINATION,
                "filePath": "README.md",
                "fileContent": base64(b"the released version\n"),
                "commitMessage": "base",
            }),
        )
        .await;
        let base = string_at(&seeded, "/commitId");
        ok_as(
            &router,
            &access_key_id,
            "CreateBranch",
            &json!({
                "repositoryName": repository,
                "branchName": SOURCE,
                "commitId": base,
            }),
        )
        .await;
        ok_as(
            &router,
            &access_key_id,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": SOURCE,
                "filePath": "feature.txt",
                "fileContent": base64(b"the proposed change\n"),
                "parentCommitId": base,
                "commitMessage": "proposed change",
            }),
        )
        .await;
        let created = ok_as(
            &router,
            &access_key_id,
            "CreatePullRequest",
            &json!({
                "title": "a change needing review",
                "targets": [{
                    "repositoryName": repository,
                    "sourceReference": SOURCE,
                    "destinationReference": DESTINATION,
                }],
                "clientRequestToken": fresh_client_request_token(),
            }),
        )
        .await;
        let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");

        // The author of what the caller created is that caller.
        assert_eq!(
            string_at(&created, "/pullRequest/authorArn"),
            expected,
            "case {case:?}: expected the pull request authored by {expected}, got {created}"
        );

        ok_as(
            &router,
            &access_key_id,
            "CreatePullRequestApprovalRule",
            &json!({
                "pullRequestId": pull_request_id,
                "approvalRuleName": RULE,
                "approvalRuleContent": unmeetable_document(),
            }),
        )
        .await;
        let revision_id = string_at(&created, "/pullRequest/revisionId");

        // Before the override the pull request holds none, so it names no overrider at
        // all: the member reports an identity only once one set the rules aside.
        let before = ok_as(
            &router,
            &access_key_id,
            OVERRIDE_STATE_OPERATION,
            &json!({ "pullRequestId": pull_request_id, "revisionId": revision_id }),
        )
        .await;
        assert_eq!(
            before.get("overridden").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: an un-overridden pull request reports overridden false, got {before}"
        );
        assert_eq!(
            before.get("overrider"),
            None,
            "case {case:?}: an un-overridden pull request names no overrider, got {before}"
        );

        ok_as(
            &router,
            &access_key_id,
            OVERRIDE_OPERATION,
            &json!({
                "pullRequestId": pull_request_id,
                "revisionId": revision_id,
                "overrideStatus": OVERRIDE,
            }),
        )
        .await;

        // The audit field of the override is the principal that made the call — the same
        // one STS answers for the credential that signed it.
        let after = ok_as(
            &router,
            &access_key_id,
            OVERRIDE_STATE_OPERATION,
            &json!({ "pullRequestId": pull_request_id, "revisionId": revision_id }),
        )
        .await;
        assert_eq!(
            after.get("overridden").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the override is in place, got {after}"
        );
        assert_eq!(
            after.get("overrider").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the override attributed to {expected}, got {after}"
        );

        // And so is the merge the override lets through.
        let merged = ok_as(
            &router,
            &access_key_id,
            MERGE_OPERATION,
            &json!({ "repositoryName": repository, "pullRequestId": pull_request_id }),
        )
        .await;
        assert_eq!(
            string_at(
                &merged,
                "/pullRequest/pullRequestTargets/0/mergeMetadata/mergedBy"
            ),
            expected,
            "case {case:?}: expected the merge recorded for {expected}, got {merged}"
        );
    }
}
