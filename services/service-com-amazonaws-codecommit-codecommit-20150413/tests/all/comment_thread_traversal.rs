// SPDX-License-Identifier: CC0-1.0

//! How `GetCommentsForComparedCommit` walks a comment thread.
//!
//! The rule under test: a thread reports each of its comments EXACTLY ONCE, in post
//! order, however the stored reply edges are shaped — including a shape no write
//! path can produce today (a comment replying to itself, or two comments replying to
//! each other). The walk follows stored `in_reply_to` edges, so without a record of
//! what it has already expanded a cycle would make it re-expand the same comments
//! for ever, growing its answer without end while the store lock is held: that is a
//! hang of the whole service, not of one request, so the walk's termination cannot
//! depend on the edges being acyclic.
//!
//! The rows span the shapes the rule generalises over: no replies, a chain, a fan,
//! a self-reply, a two-comment cycle and a cycle beside a healthy chain. The cyclic
//! rows are planted directly in state, because the ids a reply is keyed by are
//! minted by the service and no request can name one that makes a cycle. A comment
//! a cycle leaves unreachable from every root belongs to no thread and so is
//! reported nowhere; what the cyclic rows pin is that the read ANSWERS, and reports
//! no comment twice.

use serde_json::json;

use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State as Store;

use crate::repository_lifecycle::{base64, emulator_with_engine, ok, string_at};

/// The file the comparison every row is built around holds, and its two versions.
const PATH: &str = "README.md";
const BEFORE_CONTENT: &[u8] = b"alpha\nbeta\n";
const AFTER_CONTENT: &[u8] = b"alpha\nbeta\ngamma\n";

/// How the planted `in_reply_to` edges are shaped over the comments posted for a
/// row, each named by its index in post order.
struct Case {
    label: &'static str,
    /// How many comments to post on the comparison.
    comments: usize,
    /// `(child, parent)` edges to plant, by index: the child's `in_reply_to` is set
    /// to the parent's id. `child == parent` is a self-reply. A later edge for the
    /// same child replaces its earlier one, as a stored record holds one parent.
    edges: &'static [(usize, usize)],
    /// Whether every posted comment is still reachable from a root. False for a row
    /// whose cycle leaves a comment with no path from any root: such a comment is
    /// part of no thread, so a read reports it nowhere — which is the answer, not a
    /// hang.
    all_reachable: bool,
}

/// The shapes: the acyclic ones a write path can produce, then the cyclic ones it
/// cannot, which are what the visited set exists for.
const CASES: &[Case] = &[
    Case {
        label: "one comment, no replies",
        comments: 1,
        edges: &[],
        all_reachable: true,
    },
    Case {
        label: "a chain of three",
        comments: 3,
        edges: &[(1, 0), (2, 1)],
        all_reachable: true,
    },
    Case {
        label: "two replies to one root",
        comments: 3,
        edges: &[(1, 0), (2, 0)],
        all_reachable: true,
    },
    Case {
        label: "a comment replying to itself",
        comments: 2,
        edges: &[(1, 1)],
        all_reachable: false,
    },
    Case {
        label: "two comments replying to each other",
        comments: 3,
        edges: &[(2, 1), (1, 2)],
        all_reachable: false,
    },
    Case {
        label: "a cycle hanging off a healthy chain",
        comments: 4,
        edges: &[(1, 0), (2, 3), (3, 2)],
        all_reachable: false,
    },
];

/// Every comment of the thread is reported exactly once, for every edge shape —
/// and the read terminates, which a cyclic shape would not if the walk kept no
/// record of what it had expanded.
#[tokio::test]
async fn a_thread_reports_each_comment_once_whatever_the_reply_edges() {
    for case in CASES {
        let label = case.label;
        // One emulator per row, and the engine handle beside it: the cyclic rows are
        // planted in state, which is reached the way the sibling suites reach it.
        let (router, engine) = emulator_with_engine();
        let repository = format!("repo-threads-{}", case.comments);
        ok(
            &router,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        let first = ok(
            &router,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": "main",
                "filePath": PATH,
                "fileContent": base64(BEFORE_CONTENT),
            }),
        )
        .await;
        let before_commit = string_at(&first, "/commitId");
        let second = ok(
            &router,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": "main",
                "filePath": PATH,
                "fileContent": base64(AFTER_CONTENT),
                "parentCommitId": before_commit,
            }),
        )
        .await;
        let after_commit = string_at(&second, "/commitId");

        let mut posted: Vec<String> = Vec::new();
        for index in 0..case.comments {
            let response = ok(
                &router,
                "PostCommentForComparedCommit",
                &json!({
                    "repositoryName": repository,
                    "beforeCommitId": before_commit,
                    "afterCommitId": after_commit,
                    "content": format!("comment {index}"),
                }),
            )
            .await;
            posted.push(string_at(&response, "/comment/commentId"));
        }

        // Plant the row's edges straight in state: a reply names a service-minted
        // comment id, so a cyclic shape is unreachable over the wire.
        {
            let bound = engine.state::<Store>();
            let mut store = bound.lock();
            let stored = store
                .repositories
                .get_mut(&repository)
                .unwrap_or_else(|| panic!("case {label}: the repository must exist"));
            for (child, parent) in case.edges {
                let parent_id = posted[*parent].clone();
                stored
                    .comments
                    .get_mut(&posted[*child])
                    .unwrap_or_else(|| panic!("case {label}: comment {child} must be stored"))
                    .in_reply_to = Some(parent_id);
            }
        }

        let read = ok(
            &router,
            "GetCommentsForComparedCommit",
            &json!({
                "repositoryName": repository,
                "beforeCommitId": before_commit,
                "afterCommitId": after_commit,
            }),
        )
        .await;

        // Every comment id the read reports, across every thread of the answer.
        let mut reported: Vec<String> = Vec::new();
        for entry in read["commentsForComparedCommitData"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no data array in {read}"))
        {
            for comment in entry["comments"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label}: no comments array in {entry}"))
            {
                reported.push(
                    comment["commentId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {label}: no commentId in {comment}"))
                        .to_string(),
                );
            }
        }

        let mut unique = reported.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            reported.len(),
            "case {label}: every comment must be reported once, got {reported:?}"
        );
        let mut expected = posted.clone();
        expected.sort();
        if case.all_reachable {
            assert_eq!(
                unique, expected,
                "case {label}: the read must report exactly the posted comments, \
                 got {reported:?}"
            );
        } else {
            // A comment a cycle leaves unreachable from every root belongs to no
            // thread, so it is reported nowhere; what must hold is that the read
            // answers at all, reports only comments that were posted, and still
            // reports the thread that IS rooted.
            assert!(
                unique.iter().all(|id| expected.contains(id)),
                "case {label}: the read must report only posted comments, got {reported:?}"
            );
            assert!(
                reported.contains(&posted[0]),
                "case {label}: the rooted thread must still be reported, got {reported:?}"
            );
        }
    }
}
