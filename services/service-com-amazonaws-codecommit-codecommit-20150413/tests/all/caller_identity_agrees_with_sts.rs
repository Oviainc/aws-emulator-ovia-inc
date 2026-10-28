// SPDX-License-Identifier: CC0-1.0

//! WHO a recorded identity names, checked against the service that answers that
//! question: `sts:GetCallerIdentity`.
//!
//! The rule under test: the principal this service records for a request is the one the
//! access key id the request presents RESOLVES to — the session that key names, the IAM
//! user the account holds the key for, or the account's own principal for a key no store
//! holds — which is exactly the identity `sts:GetCallerIdentity` answers for the same
//! credential. So every ARN this service attributes to a caller (`Comment.authorArn`,
//! `ReactionForComment.reactionUsers`) equals that answer, and
//! `GetCommentReactions.reactionUserArn` matches a reaction against that same value.
//!
//! Nothing here writes an expected ARN by hand: each row ASKS STS, mounted beside this
//! service on the one shared engine, and compares. The rows span what the rule
//! generalises over — the kind of credential a call is signed with (a key no store holds,
//! a long-term key the account holds for a user, a second such key held for a DIFFERENT
//! user, a session minted by `sts:GetSessionToken`, a session of an assumed role) — and
//! each IAM user's name is deliberately unrelated to the text of the access key id it
//! holds, so an identity synthesised from that text cannot pass any row.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use crate::repository_lifecycle::{base64, string_at};
use crate::wire::{call_as, hold_long_term_key_for as hold_key_for, ok_as};

/// STS's awsQuery API version and SigV4 signing name: the scope its calls are made
/// under.
const STS_API_VERSION: &str = "2011-06-15";
const STS_SIGNING_NAME: &str = "sts";

/// The reaction each row's caller leaves, and the one a second principal leaves beside
/// it, so a narrowed read has something to leave out
/// (`smithy_query(query="doc",shape="PutCommentReactionInput",member="reactionValue")`).
const OWN_REACTION: &str = ":thumbsup:";
const WITNESS_REACTION: &str = ":eyes:";

/// The principal that sets each repository up, and a second principal that reacts to
/// every comment beside the row's caller.
const SETUP_ACCESS_KEY_ID: &str = "AKIDSETUPCALLER";
const WITNESS_USER: &str = "Carlos_Salazar";
const WITNESS_ACCESS_KEY_ID: &str = "AKIDWITNESSKEY";

/// The IAM users the long-term rows' keys are held for, and the role the assumed-role
/// row is a session of. None of these names appears in the access key id that reaches
/// them.
const KEY_HOLDING_USER: &str = "Mary_Major";
const OTHER_KEY_HOLDING_USER: &str = "Li_Juan";
const SESSION_USER: &str = "Saanvi_Sarkar";
const REVIEW_ROLE: &str = "CodeCommitReview";

/// The emulator for one row: this service and STS mounted on the SAME shared engine, so
/// the identity STS answers for a credential is resolved from the very records this
/// service resolves it from, and request authentication off (the tests sign nothing —
/// they carry only the `Credential=` scope each service reads the caller from).
fn emulator_with_sts() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One STS awsQuery call, signed (in the routing sense) with `access_key_id`.
async fn sts_call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={STS_API_VERSION}");
    for (name, value) in params {
        // Only the delimiters of an urlencoded body need escaping here; an ARN's `:`
        // and `/` are carried as a client's own encoder carries them.
        let encoded = value
            .replace('%', "%25")
            .replace('&', "%26")
            .replace('=', "%3D")
            .replace('+', "%2B");
        body.push_str(&format!("&{name}={encoded}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{STS_SIGNING_NAME}.amazonaws.com/"))
        .header("host", format!("{STS_SIGNING_NAME}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/\
                 {STS_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the one `<name>…</name>` element an awsQuery response carries.
fn element(case: &str, body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("case {case:?}: no </{name}> in {body}"))
        + start;
    body[start..end].to_string()
}

/// The ARN `sts:GetCallerIdentity` answers for a request signed with `access_key_id`:
/// the identity that credential resolves to, asked of the service that owns the
/// question.
async fn identity_arn_from_sts(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
) -> String {
    let (status, body) = sts_call(router, access_key_id, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: sts:GetCallerIdentity must answer the signing credential, got \
         {status} with {body}"
    );
    element(case, &body, "Arn")
}

/// Record a role of the account assumable by any principal of it, as `iam:CreateRole`
/// records one, and answer its ARN.
fn hold_role(engine: &engine::InMemoryEngine, role_name: &str) -> String {
    let account = engine::emulator_account_id();
    let arn = format!("arn:aws:iam::{account}:role/{role_name}");
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
         \"Action\":[\"sts:AssumeRole\"]}}]}}"
    );
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    state.lock().roles.insert(
        iam::name_key(role_name),
        Role {
            path: "/".to_string(),
            role_name: role_name.to_string(),
            role_id: sts::state::role_id(&arn),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: Some(trust),
            description: None,
            max_session_duration: Some(3_600),
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// Which credential a row's calls are signed with.
#[derive(Debug, Clone, Copy)]
enum Credential {
    /// A long-term key no store holds a record for: the position the credential this
    /// emulator publishes at start is in.
    UnheldKey,
    /// A long-term key the account holds for one of its IAM users.
    IamUserKey { user: &'static str },
    /// The temporary credentials of an `sts:GetSessionToken` session, minted here for
    /// an IAM user's own long-term key.
    SessionToken { user: &'static str },
    /// The temporary credentials of a role session, minted here by `sts:AssumeRole`.
    AssumedRole {
        role: &'static str,
        session: &'static str,
    },
}

impl Credential {
    /// Put this credential in place and answer the access key id a call signed with it
    /// carries. A session is MINTED through STS over the wire, so the registry the
    /// identity is later resolved from is the one that operation wrote.
    async fn access_key_id(
        self,
        router: &MultiServiceRouter,
        engine: &engine::InMemoryEngine,
        case: &str,
    ) -> String {
        match self {
            Self::UnheldKey => "AKIDNOSTOREHOLDSTHIS".to_string(),
            Self::IamUserKey { user } => {
                let access_key_id = format!("AKIDHELDKEY{}", user.len());
                hold_key_for(engine, user, &access_key_id);
                access_key_id
            }
            Self::SessionToken { user } => {
                let long_term = format!("AKIDTOKENHOLDER{}", user.len());
                hold_key_for(engine, user, &long_term);
                let (status, body) =
                    sts_call(router, &long_term, "GetSessionToken", &[]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: sts:GetSessionToken must mint a session, got {status} \
                     with {body}"
                );
                element(case, &body, "AccessKeyId")
            }
            Self::AssumedRole { role, session } => {
                let role_arn = hold_role(engine, role);
                let (status, body) = sts_call(
                    router,
                    SETUP_ACCESS_KEY_ID,
                    "AssumeRole",
                    &[
                        ("RoleArn", role_arn),
                        ("RoleSessionName", session.to_string()),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: sts:AssumeRole must mint a session, got {status} with \
                     {body}"
                );
                element(case, &body, "AccessKeyId")
            }
        }
    }
}

/// One row: a credential kind, and the label its failures are reported under.
struct Row {
    case: &'static str,
    credential: Credential,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a long-term key no store holds",
            credential: Credential::UnheldKey,
        },
        Row {
            case: "a long-term key the account holds for one of its users",
            credential: Credential::IamUserKey {
                user: KEY_HOLDING_USER,
            },
        },
        Row {
            case: "a long-term key held for a different user",
            credential: Credential::IamUserKey {
                user: OTHER_KEY_HOLDING_USER,
            },
        },
        Row {
            case: "a session token speaking for the user that asked for it",
            credential: Credential::SessionToken { user: SESSION_USER },
        },
        Row {
            case: "a session of an assumed role",
            credential: Credential::AssumedRole {
                role: REVIEW_ROLE,
                session: "review-session-1",
            },
        },
    ]
}

/// A repository with one commit, and a comment on it posted by `access_key_id`: the
/// comment the row's reactions are left on, and the first ARN this service attributes
/// to that caller.
async fn comment_by(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    access_key_id: &str,
) -> (String, String) {
    ok_as(
        router,
        SETUP_ACCESS_KEY_ID,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let committed = ok_as(
        router,
        SETUP_ACCESS_KEY_ID,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\n"),
            "commitMessage": "the version commented on",
        }),
    )
    .await;
    let commit_id = string_at(&committed, "/commitId");
    let posted = ok_as(
        router,
        access_key_id,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": repository,
            "afterCommitId": commit_id,
            "content": format!("a comment by the caller of {case}"),
        }),
    )
    .await;
    (
        string_at(&posted, "/comment/commentId"),
        string_at(&posted, "/comment/authorArn"),
    )
}

/// The reactions `GetCommentReactions` reports, optionally narrowed to one principal.
async fn reactions(
    router: &MultiServiceRouter,
    case: &str,
    comment_id: &str,
    reaction_user_arn: Option<&str>,
) -> Vec<Value> {
    let mut body = json!({ "commentId": comment_id });
    if let Some(arn) = reaction_user_arn {
        body["reactionUserArn"] = json!(arn);
    }
    let read = ok_as(router, SETUP_ACCESS_KEY_ID, "GetCommentReactions", &body).await;
    read.get("reactionsForComment")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no reactionsForComment in {read}"))
        .clone()
}

/// The `reactionUsers` of the entry carrying `short_code`, or an empty list when the
/// read reports no such entry.
fn users_of(entries: &[Value], short_code: &str) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| {
            entry.pointer("/reaction/shortCode").and_then(Value::as_str) == Some(short_code)
        })
        .filter_map(|entry| entry.get("reactionUsers").and_then(Value::as_array))
        .flatten()
        .filter_map(|user| user.as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn a_reaction_is_attributed_to_the_principal_sts_reports_for_the_same_credential() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let (router, engine) = emulator_with_sts();
        let witness = hold_key_for(&engine, WITNESS_USER, WITNESS_ACCESS_KEY_ID);
        let access_key_id = row.credential.access_key_id(&router, &engine, case).await;

        // The identity the credential resolves to, asked of STS rather than written
        // out here.
        let expected = identity_arn_from_sts(&router, case, &access_key_id).await;

        let repository = format!("caller-identity-{index}");
        let (comment_id, author_arn) =
            comment_by(&router, case, &repository, &access_key_id).await;
        assert_eq!(
            author_arn, expected,
            "case {case:?}: expected the comment attributed to {expected:?}, got \
             {author_arn:?}"
        );

        // One reaction by the row's caller, and one by a second principal, so the read
        // has to tell them apart.
        for (signer, reaction) in [
            (access_key_id.as_str(), OWN_REACTION),
            (WITNESS_ACCESS_KEY_ID, WITNESS_REACTION),
        ] {
            ok_as(
                &router,
                signer,
                "PutCommentReaction",
                &json!({ "commentId": comment_id, "reactionValue": reaction }),
            )
            .await;
        }

        let whole = reactions(&router, case, &comment_id, None).await;
        assert_eq!(
            users_of(&whole, OWN_REACTION),
            vec![expected.clone()],
            "case {case:?}: expected the reaction attributed to {expected:?}, got {whole:?}"
        );
        assert_eq!(
            users_of(&whole, WITNESS_REACTION),
            vec![witness.clone()],
            "case {case:?}: expected the other principal's reaction left as it was, got \
             {whole:?}"
        );

        // The same value is the key `reactionUserArn` is matched against: narrowing to
        // it reports that caller's own reaction and nothing else.
        let narrowed = reactions(&router, case, &comment_id, Some(&expected)).await;
        assert_eq!(
            users_of(&narrowed, OWN_REACTION),
            vec![expected.clone()],
            "case {case:?}: expected the caller's own reaction when narrowed to \
             {expected:?}, got {narrowed:?}"
        );
        assert!(
            users_of(&narrowed, WITNESS_REACTION).is_empty(),
            "case {case:?}: a read narrowed to {expected:?} must report no other \
             principal's reaction, got {narrowed:?}"
        );
        let narrowed_to_witness = reactions(&router, case, &comment_id, Some(&witness)).await;
        assert_eq!(
            users_of(&narrowed_to_witness, WITNESS_REACTION),
            vec![witness.clone()],
            "case {case:?}: expected the other principal's own reaction when narrowed to \
             it, got {narrowed_to_witness:?}"
        );

        // A reaction is withdrawn by the principal that left it, and the other
        // principal's stands: the stored key is the resolved identity throughout.
        ok_as(
            &router,
            &access_key_id,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": "none" }),
        )
        .await;
        let after = reactions(&router, case, &comment_id, None).await;
        assert!(
            users_of(&after, OWN_REACTION).is_empty(),
            "case {case:?}: expected the withdrawn reaction gone, got {after:?}"
        );
        assert_eq!(
            users_of(&after, WITNESS_REACTION),
            vec![witness.clone()],
            "case {case:?}: expected the other principal's reaction to stand, got {after:?}"
        );

        // Nothing about the row's identity depends on the access key id's TEXT: an ARN
        // built from that text is not what the service recorded, unless the key is the
        // one the account holds for a user of exactly that name.
        let derived_from_key_text = format!(
            "arn:aws:iam::{}:user/{access_key_id}",
            engine::emulator_account_id()
        );
        assert_ne!(
            expected, derived_from_key_text,
            "case {case:?}: the identity must come from the credential's record, not from \
             the key id's text"
        );

        // A call signed with the row's credential is still answered after all of this,
        // and STS still resolves it to the same principal: the resolution is a read.
        let again = identity_arn_from_sts(&router, case, &access_key_id).await;
        assert_eq!(
            again, expected,
            "case {case:?}: expected a stable identity across reads, got {again:?}"
        );

        // The witness keeps an identity of its own, so no row collapses two callers
        // into one principal.
        assert_ne!(
            expected, witness,
            "case {case:?}: two credentials of two principals must resolve apart"
        );
        let (status, _) = call_as(
            &router,
            &access_key_id,
            "GetCommentReactions",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the row's caller can read the reactions it asked about"
        );
    }
}
