// SPDX-License-Identifier: CC0-1.0

//! Reading whether a pull request's approval rule requirements have been set
//! aside, over the wire: `GetPullRequestOverrideState`, driven as awsJson1_1
//! requests through the router that serves the real emulator (the wire harness
//! comes from the sibling `repository_lifecycle` suite, and calling as a NAMED
//! principal from `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: the call REPORTS the one override fact the named pull
//! request holds — `overridden` is true exactly while an identity has set its
//! requirements aside, and `overrider` is that identity — for every well-formed
//! `revisionId`, because the override is held by the pull request rather than by
//! one of its revisions, so the model declares no not-current error for this
//! operation (`smithy_query(query="operation",
//! operation="GetPullRequestOverrideState")` lists `InvalidRevisionIdException`
//! and `RevisionIdRequiredException` but no `RevisionNotCurrentException`). Being
//! a report, it also leaves the pull request exactly as the next caller would have
//! found it.
//!
//! The tables below span what that rule generalises over: the form a well-formed
//! revision id takes (the pull request's current revision as it is handed back, the
//! same digest in upper and mixed case, another pull request's revision, and
//! digests no revision was ever minted as), the override the pull request holds
//! (none, set aside, taken back again, set aside by a second identity), the
//! identity making the read (the one that set the rules aside, another reviewer,
//! the author), and two pull requests of one repository each holding their own
//! override.
//!
//! The sibling `pull_request_approval_rule_override` suite owns the WRITE path —
//! which calls record an override, which are refused — and the refusals this
//! reader answers a malformed or unknown id with; this suite owns what the reader
//! reports for the ids it accepts.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{caller_arn, emulator_with_callers, fresh_client_request_token, ok_as};

/// The operation under test, the one that records what it reports, and the reader
/// the pull request itself is read back through.
const OPERATION: &str = "GetPullRequestOverrideState";
const OVERRIDE_OPERATION: &str = "OverridePullRequestApprovalRules";
const PULL_REQUEST_OPERATION: &str = "GetPullRequest";
const EVENTS_OPERATION: &str = "DescribePullRequestEvents";

/// Both values of `OverrideStatus`
/// (`smithy_query(query="shape",shape="OverrideStatus")`).
const OVERRIDE: &str = "OVERRIDE";
const REVOKE: &str = "REVOKE";

/// The principals the calls below are made by. A caller is identified by the access
/// key id its request is signed with, so an override a call records is attributed to
/// that caller's ARN — and a read by any of them reports that same ARN.
const AUTHOR: &str = "AKIDAUTHOR";
const OVERRIDER: &str = "AKIDOVERRIDER";
const OTHER_OVERRIDER: &str = "AKIDOTHEROVERRIDER";
const READER: &str = "AKIDREADER";

const REPOSITORY: &str = "override-state-read";
const DESTINATION: &str = "main";
/// The source branch of each pull request the fixture opens.
const SOURCES: [&str; 2] = ["feature", "second-feature"];

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// An approval rule document whose conditions the pull request's approvals do not
/// meet, so the pull request carries a requirement an override can set aside.
fn unmet_document() -> String {
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [{
            "Type": STATEMENT_TYPE,
            "NumberOfApprovalsNeeded": 1,
        }],
    }))
    .expect("a JSON document")
}

/// A hex digest of the length a `revisionId` holds, every character `filler` — a
/// well-formed revision id no revision of any pull request was minted as.
fn digest_of(filler: char) -> String {
    std::iter::repeat_n(filler, 64).collect()
}

/// The same digest with every other LETTER of it upper-cased: hex is case
/// insensitive, so this names the revision the lower-case form names. Only the
/// letters are touched, because upper-casing a digit changes nothing — which would
/// leave this form equal to the lower-case one instead of a case of its own.
fn mixed_case(digest: &str) -> String {
    let mut letters = 0;
    digest
        .chars()
        .map(|character| {
            if !character.is_ascii_alphabetic() {
                return character;
            }
            letters += 1;
            if letters % 2 == 1 {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect()
}

/// An emulator holding one repository with a destination branch and two open pull
/// requests from two source branches, each opened by [`AUTHOR`] and each carrying
/// one unmet approval rule.
struct Fixture {
    router: MultiServiceRouter,
    pull_request_ids: Vec<String>,
}

async fn fixture() -> Fixture {
    let (router, _engine) = emulator_with_callers(&[AUTHOR, OVERRIDER, OTHER_OVERRIDER, READER]);
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

    let mut pull_request_ids = Vec::new();
    for source in SOURCES {
        ok_as(
            &router,
            AUTHOR,
            "CreateBranch",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": source,
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
                "branchName": source,
                "filePath": format!("{source}.txt"),
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
                "title": format!("a change on {source}"),
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": source,
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
                "approvalRuleName": "needs-review",
                "approvalRuleContent": unmet_document(),
            }),
        )
        .await;
        pull_request_ids.push(pull_request_id);
    }
    Fixture {
        router,
        pull_request_ids,
    }
}

impl Fixture {
    /// The id of the pull request at `index` among the ones the fixture opened.
    fn pull_request_id(&self, index: usize) -> &str {
        &self.pull_request_ids[index]
    }

    /// The pull request `GetPullRequest` reports.
    async fn stored(&self, index: usize) -> Value {
        let read = ok(
            &self.router,
            PULL_REQUEST_OPERATION,
            &json!({ "pullRequestId": self.pull_request_id(index) }),
        )
        .await;
        read.get("pullRequest")
            .cloned()
            .unwrap_or_else(|| panic!("no pullRequest in {read}"))
    }

    /// The `revisionId` of that pull request's current revision.
    async fn revision_id(&self, index: usize) -> String {
        string_at(&self.stored(index).await, "/revisionId")
    }

    /// Every event the pull request's log holds, oldest first.
    async fn events(&self, index: usize) -> Vec<Value> {
        let read = ok(
            &self.router,
            EVENTS_OPERATION,
            &json!({ "pullRequestId": self.pull_request_id(index) }),
        )
        .await;
        read.get("pullRequestEvents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("no pullRequestEvents in {read}"))
    }

    /// The override state the operation under test reports for `revision_id`, read
    /// as `access_key_id`.
    async fn override_state_as(
        &self,
        access_key_id: &str,
        index: usize,
        revision_id: &str,
    ) -> Value {
        ok_as(
            &self.router,
            access_key_id,
            OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id(index),
                "revisionId": revision_id,
            }),
        )
        .await
    }

    /// Ask `access_key_id` to set the pull request's requirements aside, or to hold
    /// them again, on its current revision.
    async fn set_override(&self, access_key_id: &str, index: usize, status: &str) {
        let revision_id = self.revision_id(index).await;
        ok_as(
            &self.router,
            access_key_id,
            OVERRIDE_OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id(index),
                "revisionId": revision_id,
                "overrideStatus": status,
            }),
        )
        .await;
    }
}

/// What a reported override state says, in the two members the output carries.
fn reported(state: &Value) -> (Option<bool>, Option<String>) {
    (
        state.get("overridden").and_then(Value::as_bool),
        state
            .get("overrider")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// One row of the revision-id table: a well-formed `revisionId`, built from the
/// pull request's current revision and from another pull request's.
struct RevisionForm {
    case: &'static str,
    build: fn(current: &str, other: &str) -> String,
}

/// One row of the override table: the calls made against the pull request before it
/// is read, and the override those calls leave it holding.
struct OverrideHeld {
    case: &'static str,
    /// `(access key id, overrideStatus)` in the order the calls are made.
    calls: Vec<(&'static str, &'static str)>,
    overridden: bool,
    /// The access key id of the identity that set the rules aside, where they are.
    overrider: Option<&'static str>,
}

#[tokio::test]
async fn every_well_formed_revision_id_reads_the_override_the_pull_request_holds() {
    // Rows differ along the form of a well-formed revision id: the digest the pull
    // request's current revision is handed back as, that same digest written in
    // another case, the current revision of ANOTHER pull request, and digests no
    // revision of any pull request was ever minted as.
    let forms = [
        RevisionForm {
            case: "the current revision id as GetPullRequest hands it back",
            build: |current, _| current.to_string(),
        },
        RevisionForm {
            case: "the current revision id in upper-case hex",
            build: |current, _| current.to_ascii_uppercase(),
        },
        RevisionForm {
            case: "the current revision id in mixed-case hex",
            build: |current, _| mixed_case(current),
        },
        RevisionForm {
            case: "the current revision id of another pull request of the repository",
            build: |_, other| other.to_string(),
        },
        RevisionForm {
            case: "a digest of zeroes no revision was minted as",
            build: |_, _| digest_of('0'),
        },
        RevisionForm {
            case: "a digest of upper-case fs no revision was minted as",
            build: |_, _| digest_of('F'),
        },
    ];

    // Rows differ along the override the pull request holds when it is read, and
    // along which identity left it holding that.
    let held = [
        OverrideHeld {
            case: "no override ever asked for",
            calls: vec![],
            overridden: false,
            overrider: None,
        },
        OverrideHeld {
            case: "the requirements set aside",
            calls: vec![(OVERRIDER, OVERRIDE)],
            overridden: true,
            overrider: Some(OVERRIDER),
        },
        OverrideHeld {
            case: "the requirements set aside by the author of the pull request",
            calls: vec![(AUTHOR, OVERRIDE)],
            overridden: true,
            overrider: Some(AUTHOR),
        },
        OverrideHeld {
            case: "the override taken back again",
            calls: vec![(OVERRIDER, OVERRIDE), (OVERRIDER, REVOKE)],
            overridden: false,
            overrider: None,
        },
        OverrideHeld {
            case: "the requirements set aside again by a second identity",
            calls: vec![
                (OVERRIDER, OVERRIDE),
                (OVERRIDER, REVOKE),
                (OTHER_OVERRIDER, OVERRIDE),
            ],
            overridden: true,
            overrider: Some(OTHER_OVERRIDER),
        },
    ];

    for row in &held {
        let case = row.case;
        let fixture = fixture().await;
        for (caller, status) in &row.calls {
            fixture.set_override(caller, 0, status).await;
        }

        let current = fixture.revision_id(0).await;
        let other = fixture.revision_id(1).await;
        assert_ne!(
            current, other,
            "case {case:?}: the two pull requests must hold revisions of their own, got {current}"
        );
        // A revision id is a hex digest, so it carries letters, and the two forms
        // below are genuinely other spellings of it rather than the same string.
        assert!(
            mixed_case(&current) != current && current.to_ascii_uppercase() != current,
            "case {case:?}: the case-insensitivity rows need a digest holding letters, got \
             {current}"
        );
        let expected_overrider = row.overrider.map(caller_arn);

        for form in &forms {
            let revision_id = (form.build)(&current, &other);
            // The reader carries no identity of its own into the answer, so the
            // read is made by a principal that set nothing aside.
            let state = fixture.override_state_as(READER, 0, &revision_id).await;
            let (overridden, overrider) = reported(&state);
            assert_eq!(
                overridden,
                Some(row.overridden),
                "case {case:?}/{:?}: expected overridden {}, got {state}",
                form.case,
                row.overridden
            );
            assert_eq!(
                overrider, expected_overrider,
                "case {case:?}/{:?}: expected the overrider {expected_overrider:?}, got {state}",
                form.case
            );
        }
    }
}

#[tokio::test]
async fn the_identity_reading_the_override_state_does_not_change_what_it_reports() {
    let fixture = fixture().await;
    let revision_id = fixture.revision_id(0).await;
    fixture.set_override(OVERRIDER, 0, OVERRIDE).await;
    let expected = Some(caller_arn(OVERRIDER));

    // Rows differ along who makes the read: the identity that set the requirements
    // aside, one that set nothing aside, and the author of the pull request. Each
    // reports the identity that MADE the override, not the one asking.
    for caller in [OVERRIDER, OTHER_OVERRIDER, READER, AUTHOR] {
        let state = fixture.override_state_as(caller, 0, &revision_id).await;
        let (overridden, overrider) = reported(&state);
        assert_eq!(
            overridden,
            Some(true),
            "case {caller:?}: expected overridden true, got {state}"
        );
        assert_eq!(
            overrider, expected,
            "case {caller:?}: expected the overrider {expected:?}, got {state}"
        );
    }
}

#[tokio::test]
async fn reading_the_override_state_leaves_the_pull_request_as_it_was() {
    // Rows differ along the override the pull request holds while it is read:
    // reporting it is a read either way.
    for case in ["with no override in place", "with the requirements set aside"] {
        let fixture = fixture().await;
        if case == "with the requirements set aside" {
            fixture.set_override(OVERRIDER, 0, OVERRIDE).await;
        }

        let before = fixture.stored(0).await;
        let events_before = fixture.events(0).await;
        let revision_id = string_at(&before, "/revisionId");
        let other = fixture.revision_id(1).await;

        // Read it repeatedly, by different callers and through different forms of a
        // well-formed revision id: every answer is the same one.
        let first = fixture.override_state_as(READER, 0, &revision_id).await;
        for (caller, revision) in [
            (READER, revision_id.clone()),
            (OVERRIDER, revision_id.to_ascii_uppercase()),
            (AUTHOR, other),
            (OTHER_OVERRIDER, digest_of('0')),
        ] {
            let again = fixture.override_state_as(caller, 0, &revision).await;
            assert_eq!(
                again, first,
                "case {case:?}: a repeated read must report the same override, got {again} after \
                 {first}"
            );
        }

        let after = fixture.stored(0).await;
        assert_eq!(
            after, before,
            "case {case:?}: reading the override state must leave the pull request unchanged, got \
             {after} after {before}"
        );
        let events_after = fixture.events(0).await;
        assert_eq!(
            events_after, events_before,
            "case {case:?}: reading the override state must append no event, got {events_after:?} \
             after {events_before:?}"
        );
    }
}

/// One stage of the two-pull-request table: the calls made at this stage, as
/// `(pull request index, access key id, overrideStatus)`, and the identity each of
/// the two pull requests then holds an override from.
struct Stage {
    case: &'static str,
    calls: Vec<(usize, &'static str, &'static str)>,
    overriders: [Option<&'static str>; 2],
}

#[tokio::test]
async fn each_pull_request_reports_the_override_it_holds_itself() {
    let fixture = fixture().await;
    let revisions = [fixture.revision_id(0).await, fixture.revision_id(1).await];

    // Rows differ along which of the two pull requests holds an override: the
    // reader answers for the pull request its request NAMES.
    let stages = [
        Stage {
            case: "neither pull request overridden",
            calls: vec![],
            overriders: [None, None],
        },
        Stage {
            case: "only the first overridden",
            calls: vec![(0, OVERRIDER, OVERRIDE)],
            overriders: [Some(OVERRIDER), None],
        },
        Stage {
            case: "both overridden, by different identities",
            calls: vec![(1, OTHER_OVERRIDER, OVERRIDE)],
            overriders: [Some(OVERRIDER), Some(OTHER_OVERRIDER)],
        },
        Stage {
            case: "the first taken back again, the second still set aside",
            calls: vec![(0, OVERRIDER, REVOKE)],
            overriders: [None, Some(OTHER_OVERRIDER)],
        },
    ];

    for stage in &stages {
        let case = stage.case;
        for (index, caller, status) in &stage.calls {
            fixture.set_override(caller, *index, status).await;
        }
        for (index, expected_overrider) in stage.overriders.iter().enumerate() {
            let state = fixture
                .override_state_as(READER, index, &revisions[index])
                .await;
            let (overridden, overrider) = reported(&state);
            let expected_overrider = expected_overrider.map(caller_arn);
            assert_eq!(
                overridden,
                Some(expected_overrider.is_some()),
                "case {case:?}: pull request {index} expected overridden {}, got {state}",
                expected_overrider.is_some()
            );
            assert_eq!(
                overrider, expected_overrider,
                "case {case:?}: pull request {index} expected the overrider \
                 {expected_overrider:?}, got {state}"
            );
        }
    }
}
