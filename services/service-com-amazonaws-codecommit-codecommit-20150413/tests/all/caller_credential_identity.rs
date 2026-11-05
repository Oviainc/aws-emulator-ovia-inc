// SPDX-License-Identifier: CC0-1.0

//! WHICH IDENTITY a request is attributed to, over the wire: an approval recorded by
//! `UpdatePullRequestApprovalState`, read back through `DescribePullRequestEvents`, and
//! the approval rule `MergePullRequestBySquash` counts it against.
//!
//! The rule under test: a request speaks for the principal the CREDENTIAL it was signed
//! with was minted for, so the identity this service records for it is the identity of
//! the session that access key id names — for the temporary credentials of an assumed
//! role, that session's `assumed-role` ARN, which is the spelling an approval rule's
//! `ApprovalPoolMembers` entry names. An access key id no live session is held for is a
//! long-term key, and such a request speaks for the IAM user the account holds that key
//! for; a key no store holds at all speaks for the account's own principal, the identity
//! this emulator's published credential resolves to — never an identity derived from the
//! key id's text, which no caller could correlate with the one it reads for itself.
//!
//! The consequence the table drives end to end: an approval given by a session inside a
//! rule's pool MEETS that rule, so the identical merge that was refused before the
//! approval goes through — HTTP 200, the pull request CLOSED and the destination branch
//! advanced — while an approval by a principal the pool does not name leaves the rule
//! unmet and the merge refused with HTTP 400
//! `PullRequestApprovalRulesNotSatisfiedException`, nothing written.
//!
//! The rows span what that rule generalises over: the kind of credential the approver
//! signs with (an assumed-role session, an `sts:GetSessionToken` session speaking for an
//! IAM user, a session whose expiry has passed, a long-term key), the form of the pool
//! entry matched against it (every session of a role, one exact session, a wildcarded
//! account, another role's sessions, an IAM user ARN, no pool at all), how many approvals
//! a statement needs and how many approvers give one, the order two approvers call in,
//! and an approval taken back again.
//!
//! The sessions are written into the STS session registry in the shared engine the way
//! `sts:AssumeRole` and `sts:GetSessionToken` record one, because what is under test is
//! what THIS service does with the principal a credential names, not how the credential
//! was minted.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{
    emulator_with_callers, fresh_client_request_token, hold_long_term_key_for, ok_as, refusal_as,
};

/// The operations driven here: the approval, the merge the approval gates, and the
/// readers the recorded identity and the merge's effect are read back through.
const APPROVAL_OPERATION: &str = "UpdatePullRequestApprovalState";
const SQUASH_OPERATION: &str = "MergePullRequestBySquash";

/// The status every error asserted here models (`errorKind: client`, `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// `ApprovalState`'s two values (`smithy_query(query="shape",shape="ApprovalState")`).
const APPROVE: &str = "APPROVE";
const REVOKE: &str = "REVOKE";

/// The `PullRequestEventType` an approval state change is logged under, and the
/// `PullRequestStatusEnum` values a pull request holds
/// (`smithy_query(query="shape",shape="PullRequestEventType")`,
/// `smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const APPROVAL_STATE_CHANGED: &str = "PULL_REQUEST_APPROVAL_STATE_CHANGED";
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// The one sentence pair `PullRequestApprovalRulesNotSatisfiedException` carries,
/// whichever rule went unmet.
const RULES_NOT_SATISFIED: &str = "You cannot merge the pull request because the requirements for \
                                   one or more approval rules have not been met. For details \
                                   about the approval rules that still have requirements, use \
                                   EvaluatePullRequestApprovalRules.";

const REPOSITORY: &str = "credential-identity";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";
const RULE: &str = "needs-one-review";

/// The access key id the pull request's author signs with. The author may not approve
/// its own pull request, so no row below has the author approve.
const AUTHOR: &str = "AKIDAUTHOR";

/// The roles whose sessions approve below, and the IAM users a session token and an
/// account-held long-term access key speak for.
const REVIEW_ROLE: &str = "CodeCommitReview";
const RELEASE_ROLE: &str = "CodeCommitRelease";
const REVIEW_USER: &str = "Mary_Major";
const KEY_HOLDING_USER: &str = "Li_Juan";

/// How long a seeded live session lasts: longer than any row takes to run, so none
/// expires while the merge it gates is driven.
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

/// The emulator's account: every identity these rows name belongs to it.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// One session of an assumed role, as STS reports it:
/// `arn:aws:sts::<account>:assumed-role/<role>/<session>`.
fn assumed_role_arn(role: &str, session: &str) -> String {
    format!("arn:aws:sts::{}:assumed-role/{role}/{session}", account())
}

/// The ARN of the IAM role a session was minted for.
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", account())
}

/// An IAM user of this account.
fn user_arn(user: &str) -> String {
    format!("arn:aws:iam::{}:user/{user}", account())
}

/// Which credential an approver's request is signed with — and so which principal this
/// service must record its approval for.
#[derive(Debug, Clone, Copy)]
enum Credential {
    /// The temporary credentials of an assumed role, live in the session registry.
    AssumedRole {
        role: &'static str,
        session: &'static str,
    },
    /// The temporary credentials of an assumed role whose expiry the clock has passed.
    ExpiredAssumedRole {
        role: &'static str,
        session: &'static str,
    },
    /// The temporary credentials of an `sts:GetSessionToken` session, which speak for
    /// the IAM user that asked for them.
    SessionToken { user: &'static str },
    /// A long-term access key the account holds for an IAM user, as
    /// `iam:CreateAccessKey` records one.
    IamUserKey { user: &'static str },
    /// A long-term key no store holds anything for.
    LongTerm,
}

impl Credential {
    /// The identity this service must attribute a request signed with this credential
    /// to: the session's identity where a live session names one, the IAM user the
    /// account holds the key for, and otherwise the account's own principal.
    fn identity_arn(self) -> String {
        match self {
            Self::AssumedRole { role, session } => assumed_role_arn(role, session),
            Self::SessionToken { user } | Self::IamUserKey { user } => user_arn(user),
            Self::ExpiredAssumedRole { .. } | Self::LongTerm => sts::ambient_identity().arn,
        }
    }

    /// Write the session this credential names into the STS session registry in the
    /// shared engine, the way the operation that mints one records it. A long-term key
    /// names no session, so nothing is written for it.
    fn register(self, engine: &engine::InMemoryEngine, access_key_id: &str) {
        let now = SystemTime::now();
        let (identity, kind, role, expires_at) = match self {
            Self::LongTerm => return,
            // A long-term key names no session: the account's record of WHO holds the
            // key is IAM's, so that is the store this row seeds — through the harness's
            // one writer of those records.
            Self::IamUserKey { user } => {
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
            Self::ExpiredAssumedRole { role, session } => (
                sts::Identity {
                    user_id: format!("{}:{session}", sts::role_id(&role_arn(role))),
                    account: account(),
                    arn: assumed_role_arn(role, session),
                },
                sts::SessionKind::AssumedRole,
                Some(role_arn(role)),
                // One second BEHIND the clock: an expiry the clock has already passed.
                now - Duration::from_secs(1),
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
        let registry =
            sts::service_state(engine).expect("the in-memory engine carries the STS store");
        registry
            .lock()
            .insert(sts::Session {
                access_key_id: access_key_id.to_string(),
                created_at: now - SESSION_LIFETIME,
                expires_at,
                duration_seconds: SESSION_LIFETIME.as_secs() as i32,
                identity,
                kind,
                source_identity: None,
                tags: Vec::new(),
                transitive_tag_keys: Vec::new(),
                packed_policy_size: None,
                policies: sts::SessionPolicies::default(),
                external_id: None,
                role_arn: role,
                revoked: false,
            })
            .expect("the session registry has room for one session");
    }
}

/// One approver of a row: the access key id its calls are signed with, the credential
/// that key names, and the approval states it asks for in order.
struct Approver {
    access_key_id: &'static str,
    credential: Credential,
    states: &'static [&'static str],
}

/// One row: the approval rule the pull request carries, who approves it and with which
/// credential, and whether the rule is then met — which is whether the merge goes
/// through.
struct Row {
    case: &'static str,
    /// How many approvals from the pool the rule's one statement needs.
    needed: u32,
    /// The statement's `ApprovalPoolMembers`, or empty for a statement naming no pool.
    pool: Vec<String>,
    approvers: Vec<Approver>,
    merged: bool,
}

/// An approval rule document asking for `needed` approvals, from `pool` where it names
/// one, as the compact JSON text the service stores.
fn document(needed: u32, pool: &[String]) -> String {
    let mut statement = json!({
        "Type": STATEMENT_TYPE,
        "NumberOfApprovalsNeeded": needed,
    });
    if !pool.is_empty() {
        statement["ApprovalPoolMembers"] = json!(pool);
    }
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [statement],
    }))
    .expect("a JSON document")
}

/// An emulator holding one repository with two diverged branches, one open pull request
/// between them opened by [`AUTHOR`], and the row's approval rule applied to it.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    pull_request_id: String,
    /// The tip the destination branch holds before the merge.
    destination_tip: String,
}

async fn fixture(rule: &str) -> Fixture {
    let (router, engine) = emulator_with_callers(&[AUTHOR]);
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let destination = ok_as(
        &router,
        AUTHOR,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": DESTINATION,
            "filePath": "README.md",
            "fileContent": base64(b"alpha\n"),
            "commitMessage": "the destination version",
        }),
    )
    .await;
    let base = string_at(&destination, "/commitId");
    ok_as(
        &router,
        AUTHOR,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    ok_as(
        &router,
        AUTHOR,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": SOURCE,
            "filePath": "feature.txt",
            "fileContent": base64(b"beta\n"),
            "parentCommitId": base,
            "commitMessage": "the source version",
        }),
    )
    .await;
    let created = ok_as(
        &router,
        AUTHOR,
        "CreatePullRequest",
        &json!({
            "title": "a change needing review",
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
            "clientRequestToken": fresh_client_request_token(),
        }),
    )
    .await;
    let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
    ok_as(
        &router,
        AUTHOR,
        "CreatePullRequestApprovalRule",
        &json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": RULE,
            "approvalRuleContent": rule,
        }),
    )
    .await;
    Fixture {
        router,
        engine,
        pull_request_id,
        destination_tip: base,
    }
}

impl Fixture {
    /// The pull request `GetPullRequest` reports.
    async fn stored(&self) -> Value {
        let read = ok(
            &self.router,
            "GetPullRequest",
            &json!({ "pullRequestId": self.pull_request_id }),
        )
        .await;
        read.get("pullRequest")
            .cloned()
            .unwrap_or_else(|| panic!("no pullRequest in {read}"))
    }

    /// The `revisionId` of the pull request's current revision — the revision an
    /// approval is given on.
    async fn revision_id(&self) -> String {
        string_at(&self.stored().await, "/revisionId")
    }

    async fn status(&self) -> String {
        string_at(&self.stored().await, "/pullRequestStatus")
    }

    /// The tip the destination branch holds now.
    async fn destination_head(&self) -> String {
        let read = ok(
            &self.router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": DESTINATION }),
        )
        .await;
        string_at(&read, "/branch/commitId")
    }

    /// The approval state changes the pull request's event log holds, oldest first.
    async fn approval_events(&self) -> Vec<Value> {
        let read = ok(
            &self.router,
            "DescribePullRequestEvents",
            &json!({
                "pullRequestId": self.pull_request_id,
                "pullRequestEventType": APPROVAL_STATE_CHANGED,
            }),
        )
        .await;
        read.get("pullRequestEvents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("no pullRequestEvents in {read}"))
    }
}

#[tokio::test]
async fn an_approval_is_recorded_for_the_principal_its_credential_was_minted_for() {
    // The approver of most rows: one session of the reviewing role.
    const REVIEW_SESSION: Credential = Credential::AssumedRole {
        role: REVIEW_ROLE,
        session: "review-session-1",
    };
    const SECOND_REVIEW_SESSION: Credential = Credential::AssumedRole {
        role: REVIEW_ROLE,
        session: "review-session-2",
    };
    const RELEASE_SESSION: Credential = Credential::AssumedRole {
        role: RELEASE_ROLE,
        session: "release-session-1",
    };

    let every_review_session = vec![format!(
        "arn:aws:sts::{}:assumed-role/{REVIEW_ROLE}/*",
        account()
    )];
    let one_review_session = vec![assumed_role_arn(REVIEW_ROLE, "review-session-1")];
    let any_account_review_session = vec![format!("arn:aws:sts::*:assumed-role/{REVIEW_ROLE}/*")];
    let every_release_session = vec![format!(
        "arn:aws:sts::{}:assumed-role/{RELEASE_ROLE}/*",
        account()
    )];
    let one_user = vec![user_arn(REVIEW_USER)];
    let key_holding_user = vec![user_arn(KEY_HOLDING_USER)];
    let one_long_term_caller = vec![sts::ambient_identity().arn];
    let no_pool: Vec<String> = Vec::new();

    let rows = vec![
        Row {
            case: "a session of the role the pool names every session of",
            needed: 1,
            pool: every_review_session.clone(),
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE],
            }],
            merged: true,
        },
        Row {
            case: "a session the pool names exactly",
            needed: 1,
            pool: one_review_session,
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE],
            }],
            merged: true,
        },
        Row {
            case: "a session matched by a pool entry wildcarding the account",
            needed: 1,
            pool: any_account_review_session,
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE],
            }],
            merged: true,
        },
        Row {
            case: "a session of another role than the pool names",
            needed: 1,
            pool: every_release_session.clone(),
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE],
            }],
            merged: false,
        },
        Row {
            case: "a session approving a pool that names an IAM user",
            needed: 1,
            pool: one_user.clone(),
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE],
            }],
            merged: false,
        },
        Row {
            case: "a session approving a statement that names no pool",
            needed: 1,
            pool: no_pool,
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE],
            }],
            merged: true,
        },
        Row {
            case: "two sessions of the pool on a statement needing two",
            needed: 2,
            pool: every_review_session.clone(),
            approvers: vec![
                Approver {
                    access_key_id: "AKIDREVIEWSESSION",
                    credential: REVIEW_SESSION,
                    states: &[APPROVE],
                },
                Approver {
                    access_key_id: "AKIDSECONDREVIEWSESSION",
                    credential: SECOND_REVIEW_SESSION,
                    states: &[APPROVE],
                },
            ],
            merged: true,
        },
        Row {
            case: "one session of the pool on a statement needing two",
            needed: 2,
            pool: every_review_session.clone(),
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE],
            }],
            merged: false,
        },
        Row {
            case: "a session outside the pool calling before one inside it",
            needed: 1,
            pool: every_review_session.clone(),
            approvers: vec![
                Approver {
                    access_key_id: "AKIDRELEASESESSION",
                    credential: RELEASE_SESSION,
                    states: &[APPROVE],
                },
                Approver {
                    access_key_id: "AKIDREVIEWSESSION",
                    credential: REVIEW_SESSION,
                    states: &[APPROVE],
                },
            ],
            merged: true,
        },
        Row {
            case: "a session of the pool that took its approval back again",
            needed: 1,
            pool: every_review_session.clone(),
            approvers: vec![Approver {
                access_key_id: "AKIDREVIEWSESSION",
                credential: REVIEW_SESSION,
                states: &[APPROVE, REVOKE],
            }],
            merged: false,
        },
        Row {
            case: "a session token speaking for the IAM user the pool names",
            needed: 1,
            pool: one_user,
            approvers: vec![Approver {
                access_key_id: "AKIDSESSIONTOKEN",
                credential: Credential::SessionToken { user: REVIEW_USER },
                states: &[APPROVE],
            }],
            merged: true,
        },
        Row {
            case: "a session whose expiry the clock has passed",
            needed: 1,
            pool: every_release_session,
            approvers: vec![Approver {
                access_key_id: "AKIDEXPIREDSESSION",
                credential: Credential::ExpiredAssumedRole {
                    role: RELEASE_ROLE,
                    session: "release-session-9",
                },
                states: &[APPROVE],
            }],
            merged: false,
        },
        Row {
            case: "an IAM user's own long-term key, pool naming that user",
            needed: 1,
            pool: key_holding_user.clone(),
            approvers: vec![Approver {
                access_key_id: "AKIDUSERHELDKEY",
                credential: Credential::IamUserKey {
                    user: KEY_HOLDING_USER,
                },
                states: &[APPROVE],
            }],
            merged: true,
        },
        Row {
            case: "a key no store holds, pool naming the user another key is held for",
            needed: 1,
            pool: key_holding_user,
            approvers: vec![Approver {
                access_key_id: "AKIDLONGTERMREVIEWER",
                credential: Credential::LongTerm,
                states: &[APPROVE],
            }],
            merged: false,
        },
        Row {
            case: "a long-term key against a pool of the role's sessions",
            needed: 1,
            pool: every_review_session,
            approvers: vec![Approver {
                access_key_id: "AKIDLONGTERMREVIEWER",
                credential: Credential::LongTerm,
                states: &[APPROVE],
            }],
            merged: false,
        },
        Row {
            case: "a key no store holds, the pool naming the account's own principal",
            needed: 1,
            pool: one_long_term_caller,
            approvers: vec![Approver {
                access_key_id: "AKIDLONGTERMREVIEWER",
                credential: Credential::LongTerm,
                states: &[APPROVE],
            }],
            merged: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let fixture = fixture(&document(row.needed, &row.pool)).await;
        for approver in &row.approvers {
            approver
                .credential
                .register(&fixture.engine, approver.access_key_id);
        }
        let revision = fixture.revision_id().await;

        // Before any approval the rule states conditions nothing meets, so the merge is
        // refused: every row's positive leg is the SAME call going through afterwards.
        let merge = json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": fixture.pull_request_id,
        });
        let (code, message) = refusal_as(
            &fixture.router,
            AUTHOR,
            SQUASH_OPERATION,
            &merge,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "PullRequestApprovalRulesNotSatisfiedException",
            "case {case:?}: an unapproved pull request is refused with \
             PullRequestApprovalRulesNotSatisfiedException, got {code:?}"
        );
        assert_eq!(
            message, RULES_NOT_SATISFIED,
            "case {case:?}: expected {RULES_NOT_SATISFIED:?}, got {message:?}"
        );

        // The approvals, each signed with its approver's own credential.
        let mut logged: Vec<(String, &str)> = Vec::new();
        for approver in &row.approvers {
            let recorded = approver.credential.identity_arn();
            for state in approver.states {
                ok_as(
                    &fixture.router,
                    approver.access_key_id,
                    APPROVAL_OPERATION,
                    &json!({
                        "pullRequestId": fixture.pull_request_id,
                        "revisionId": revision,
                        "approvalState": state,
                    }),
                )
                .await;
                logged.push((recorded.clone(), state));
            }
        }

        // Each change is attributed to the principal its credential was minted for, so
        // the identity the rule is counted against is readable through the event log.
        let events = fixture.approval_events().await;
        assert_eq!(
            events.len(),
            logged.len(),
            "case {case:?}: expected {} logged change(s), got {events:?}",
            logged.len()
        );
        for (event, (actor, state)) in events.iter().zip(&logged) {
            assert_eq!(
                event.get("actorArn").and_then(Value::as_str),
                Some(actor.as_str()),
                "case {case:?}: expected the change attributed to {actor:?}, got {event}"
            );
            assert_eq!(
                event
                    .pointer("/approvalStateChangedEventMetadata/approvalStatus")
                    .and_then(Value::as_str),
                Some(*state),
                "case {case:?}: expected the state {state:?} logged, got {event}"
            );
        }

        // The approvals the pull request now holds are what its rule is counted
        // against: the identical merge goes through exactly when they meet it.
        if row.merged {
            let merged = ok_as(&fixture.router, AUTHOR, SQUASH_OPERATION, &merge).await;
            assert_eq!(
                string_at(&merged, "/pullRequest/pullRequestStatus"),
                STATUS_CLOSED,
                "case {case:?}: a merged pull request is closed, got {merged}"
            );
            assert_eq!(
                fixture.status().await,
                STATUS_CLOSED,
                "case {case:?}: the stored pull request is closed too"
            );
            let head = fixture.destination_head().await;
            assert_ne!(
                head, fixture.destination_tip,
                "case {case:?}: the merge advances the destination branch past \
                 {:?}, got {head:?}",
                fixture.destination_tip
            );
        } else {
            let (code, message) = refusal_as(
                &fixture.router,
                AUTHOR,
                SQUASH_OPERATION,
                &merge,
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "PullRequestApprovalRulesNotSatisfiedException",
                "case {case:?}: expected PullRequestApprovalRulesNotSatisfiedException, \
                 got {code:?}"
            );
            assert_eq!(
                message, RULES_NOT_SATISFIED,
                "case {case:?}: expected {RULES_NOT_SATISFIED:?}, got {message:?}"
            );
            assert_eq!(
                fixture.status().await,
                STATUS_OPEN,
                "case {case:?}: a refused merge leaves the pull request open"
            );
            let head = fixture.destination_head().await;
            assert_eq!(
                head, fixture.destination_tip,
                "case {case:?}: a refused merge leaves the destination branch at {:?}, \
                 got {head:?}",
                fixture.destination_tip
            );
        }
    }
}
