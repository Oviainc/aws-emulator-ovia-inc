// SPDX-License-Identifier: CC0-1.0

//! Reading the comments of a pull request over the wire: `GetCommentsForPullRequest`
//! against the comments `PostCommentForPullRequest` and `PostCommentReply` stored,
//! driven as awsJson1_1 requests through the router that serves the real emulator
//! (the wire harness, the pull request fixture and the refusal helper come from the
//! sibling comment suites rather than a second copy).
//!
//! The rule under test: a read of a pull request's comments reports, from the
//! service's stored state, exactly the comment threads posted on that pull request
//! — each carrying the pull request, the repository that holds it, the comparison
//! the thread was filed under with the blobs of the commented file in each of its
//! two commits, the place in that comparison, and the thread's comments oldest
//! first with each reply inside the thread it was posted into — narrowed to one
//! comparison when the request names one, paged by the maximum the operation
//! publishes, and changing nothing a later caller sees.
//!
//! The tables below span what that rule generalises over: whether a comment names a
//! location at all, which version of the file it sits on (`RelativeFileVersionEnum`
//! = BEFORE | AFTER), how deep its thread runs, the content's kind, which identity
//! signs the read, page sizes at both published bounds, and each request the
//! service cannot serve.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    COMMENTS_PAGE_DEFAULT, COMMENTS_PAGE_MAX,
};

use crate::comment_lifecycle::OTHER_ACCESS_KEY_ID;
use crate::compared_commit_comments::{refused, reported_location};
use crate::pull_request_comments::{pull_request_on, PullRequest, AFTER_LINES, PATH};
use crate::repository_lifecycle::{base64, emulator_with_engine, ok, string_at};
use crate::wire::{ok_as, DEFAULT_ACCESS_KEY_ID};

/// One row of the thread table: what is posted on the pull request, and what the
/// read must then report for it.
struct ThreadCase {
    label: &'static str,
    content: &'static str,
    /// The `location` as sent; `None` comments on the pull request as a whole.
    sent_location: Option<Value>,
    /// The `location` the service must report (a file path is held in the form a
    /// git tree holds it, with leading separators removed).
    expected_location: Option<Value>,
    /// Does the comment sit on a file? Only then is the thread anchored to blobs.
    on_file: bool,
    /// The replies posted into the thread, each replying to the one before it, so a
    /// row can run the thread deeper than one level.
    replies: &'static [&'static str],
}

/// The table: rows differ along whether a location is named at all, which version
/// of the file the comment sits on, how the path is spelled, whether a position is
/// named, how deep the thread runs, and the content's kind.
fn thread_cases() -> Vec<ThreadCase> {
    vec![
        ThreadCase {
            label: "whole pull request, no reply",
            content: "Looks good to me.",
            sent_location: None,
            expected_location: None,
            on_file: false,
            replies: &[],
        },
        ThreadCase {
            label: "after version last line, one reply",
            content: "this line is new",
            sent_location: Some(json!({
                "filePath": PATH,
                "filePosition": AFTER_LINES,
                "relativeFileVersion": "AFTER",
            })),
            expected_location: Some(json!({
                "filePath": PATH,
                "filePosition": AFTER_LINES,
                "relativeFileVersion": "AFTER",
            })),
            on_file: true,
            replies: &["agreed"],
        },
        ThreadCase {
            label: "before version first line, thread three deep",
            content: "this line was here before",
            // A leading separator addresses the same file as none at all.
            sent_location: Some(json!({
                "filePath": format!("/{PATH}"),
                "filePosition": 1,
                "relativeFileVersion": "BEFORE",
            })),
            expected_location: Some(json!({
                "filePath": PATH,
                "filePosition": 1,
                "relativeFileVersion": "BEFORE",
            })),
            on_file: true,
            replies: &["a reply", "a reply to the reply"],
        },
        ThreadCase {
            label: "whole file, unicode content, one reply",
            content: "⚙ reseña — ¿sí?\nlínea 2\n",
            sent_location: Some(json!({ "filePath": PATH })),
            expected_location: Some(json!({ "filePath": PATH })),
            on_file: true,
            replies: &["⚙ respuesta — ¡sí!"],
        },
    ]
}

/// The `commentsForPullRequestData` of a read, named for the case under test.
fn threads(read: &Value, case: &str) -> Vec<Value> {
    read.get("commentsForPullRequestData")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no commentsForPullRequestData in {read}"))
        .clone()
}

/// The comment ids of one reported thread, in the order the thread reports them.
fn thread_comment_ids(entry: &Value, case: &str) -> Vec<String> {
    entry
        .get("comments")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no comments in {entry}"))
        .iter()
        .map(|comment| string_at(comment, "/commentId"))
        .collect()
}

/// A `GetCommentsForPullRequest` body naming only the pull request.
fn read_body(pull_request_id: &str) -> Value {
    json!({ "pullRequestId": pull_request_id })
}

/// Post one row's comment and its chain of replies, and report the comment ids the
/// service minted, root first.
async fn post_thread(router: &MultiServiceRouter, pr: &PullRequest, case: &ThreadCase) -> Vec<String> {
    let mut body = json!({
        "pullRequestId": pr.pull_request_id,
        "repositoryName": pr.repository,
        "beforeCommitId": pr.before_commit,
        "afterCommitId": pr.after_commit,
        "content": case.content,
    });
    if let Some(location) = &case.sent_location {
        body["location"] = location.clone();
    }
    let posted = ok(router, "PostCommentForPullRequest", &body).await;
    let mut ids = vec![string_at(&posted, "/comment/commentId")];
    for reply in case.replies {
        let parent = ids.last().expect("the root was pushed above").clone();
        let answered = ok(
            router,
            "PostCommentReply",
            &json!({ "inReplyTo": parent, "content": reply }),
        )
        .await;
        ids.push(string_at(&answered, "/comment/commentId"));
    }
    ids
}

#[tokio::test]
async fn the_comment_threads_of_a_pull_request_are_read_back_from_stored_state() {
    let (router, _engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "pull-request-comment-reads").await;
    let cases = thread_cases();

    // Every row's thread is posted first, so each one is read back out of a pull
    // request that holds the others too.
    let mut posted_ids: Vec<Vec<String>> = Vec::new();
    for case in &cases {
        posted_ids.push(post_thread(&router, &pr, case).await);
    }

    let read = ok(
        &router,
        "GetCommentsForPullRequest",
        &read_body(&pr.pull_request_id),
    )
    .await;
    assert_eq!(
        read.get("nextToken"),
        None,
        "case \"one page\": every thread fits one page, so no continuation token is issued: {read}"
    );
    let reported = threads(&read, "one page");
    assert_eq!(
        reported.len(),
        cases.len(),
        "case \"one page\": one entry per thread posted, in {read}"
    );

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let entry = &reported[index];
        for (member, expected) in [
            ("pullRequestId", Some(pr.pull_request_id.as_str())),
            ("repositoryName", Some(pr.repository.as_str())),
            ("beforeCommitId", Some(pr.before_commit.as_str())),
            ("afterCommitId", Some(pr.after_commit.as_str())),
            (
                "beforeBlobId",
                case.on_file.then_some(pr.before_blob.as_str()),
            ),
            ("afterBlobId", case.on_file.then_some(pr.after_blob.as_str())),
        ] {
            assert_eq!(
                entry.get(member).and_then(Value::as_str),
                expected,
                "case {label:?}: {member} must be the stored value, in {entry}"
            );
        }
        assert_eq!(
            reported_location(entry),
            case.expected_location,
            "case {label:?}: location in {entry}"
        );

        // The thread is the root comment and then its replies, oldest first, and
        // exactly the comments this row posted.
        let ids = thread_comment_ids(entry, label);
        assert_eq!(
            ids, posted_ids[index],
            "case {label:?}: the thread must report the comments it was posted with, in {entry}"
        );
        for id in &ids {
            assert!(
                seen.insert(id.clone()),
                "case {label:?}: comment {id} is reported in more than one thread"
            );
        }

        // Each comment reported is the stored record, read back through the
        // implemented single-comment reader.
        let comments = entry
            .get("comments")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no comments in {entry}"));
        let contents: Vec<&str> = std::iter::once(case.content)
            .chain(case.replies.iter().copied())
            .collect();
        for (position, comment) in comments.iter().enumerate() {
            let one = ok(
                &router,
                "GetComment",
                &json!({ "commentId": ids[position] }),
            )
            .await;
            assert_eq!(
                one.get("comment"),
                Some(comment),
                "case {label:?}: comment {position} must be the comment GetComment reports"
            );
            assert_eq!(
                comment.get("content").and_then(Value::as_str),
                Some(contents[position]),
                "case {label:?}: comment {position} must carry what was posted, in {comment}"
            );
            let in_reply_to = comment.get("inReplyTo").and_then(Value::as_str);
            assert_eq!(
                in_reply_to,
                (position > 0).then(|| ids[position - 1].as_str()),
                "case {label:?}: comment {position} must name the comment it replies to, in \
                 {comment}"
            );
        }
    }

    // A read changes nothing a later caller sees.
    let again = ok(
        &router,
        "GetCommentsForPullRequest",
        &read_body(&pr.pull_request_id),
    )
    .await;
    assert_eq!(
        again, read,
        "case \"read twice\": a second read must report the same threads"
    );

    // Naming the repository and the two commits of the pull request narrows the
    // read to that comparison, which is the one every thread above was filed on.
    let narrowed = ok(
        &router,
        "GetCommentsForPullRequest",
        &json!({
            "pullRequestId": pr.pull_request_id,
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
        }),
    )
    .await;
    assert_eq!(
        threads(&narrowed, "narrowed to the pull request's own comparison"),
        reported,
        "case \"narrowed to the pull request's own comparison\": the same threads must be \
         reported, in {narrowed}"
    );
}

#[tokio::test]
async fn a_read_reports_only_the_comments_of_the_pull_request_and_comparison_it_names() {
    let (router, _engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "pull-request-comment-reads-scope").await;
    // A second pull request, in its own repository on the same emulator.
    let elsewhere = pull_request_on(&router, "pull-request-comment-reads-elsewhere").await;

    let on_pull_request = ok(
        &router,
        "PostCommentForPullRequest",
        &json!({
            "pullRequestId": pr.pull_request_id,
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
            "content": "on the pull request",
        }),
    )
    .await;
    // A comment on the same two commits as a BARE comparison, which is not a
    // comment on the pull request.
    ok(
        &router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
            "content": "on the comparison alone",
        }),
    )
    .await;
    // And a comment on the other pull request entirely.
    let on_elsewhere = ok(
        &router,
        "PostCommentForPullRequest",
        &json!({
            "pullRequestId": elsewhere.pull_request_id,
            "repositoryName": elsewhere.repository,
            "beforeCommitId": elsewhere.before_commit,
            "afterCommitId": elsewhere.after_commit,
            "content": "on the other pull request",
        }),
    )
    .await;

    for (case, pull_request_id, expected) in [
        (
            "this pull request",
            &pr.pull_request_id,
            string_at(&on_pull_request, "/comment/commentId"),
        ),
        (
            "the other pull request",
            &elsewhere.pull_request_id,
            string_at(&on_elsewhere, "/comment/commentId"),
        ),
    ] {
        let read = ok(
            &router,
            "GetCommentsForPullRequest",
            &read_body(pull_request_id),
        )
        .await;
        let reported = threads(&read, case);
        let ids: Vec<String> = reported
            .iter()
            .flat_map(|entry| thread_comment_ids(entry, case))
            .collect();
        assert_eq!(
            ids,
            vec![expected],
            "case {case:?}: only the comments of that pull request are reported, in {read}"
        );
    }

    // A third version of the file, so the repository holds a commit the pull
    // request's comment was NOT filed against.
    let third = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": pr.repository,
            "branchName": "feature",
            "filePath": PATH,
            "fileContent": base64(b"alpha\nbeta\ngamma\ndelta\n"),
            "parentCommitId": pr.after_commit,
            "commitMessage": "a third version",
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetCommentsForPullRequest",
        &json!({
            "pullRequestId": pr.pull_request_id,
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": string_at(&third, "/commitId"),
        }),
    )
    .await;
    assert_eq!(
        threads(&read, "another comparison of the same repository").len(),
        0,
        "case \"another comparison of the same repository\": no thread was filed on it, in {read}"
    );
    assert_eq!(
        read.get("nextToken"),
        None,
        "case \"another comparison of the same repository\": an empty answer issues no \
         continuation token, in {read}"
    );
}

#[tokio::test]
async fn paging_follows_max_results_and_its_published_bounds() {
    let (router, _engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "pull-request-comment-reads-paged").await;

    // Seven threads, so every page size below is exercised with a partial last
    // page.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        let posted = ok(
            &router,
            "PostCommentForPullRequest",
            &json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": pr.repository,
                "beforeCommitId": pr.before_commit,
                "afterCommitId": pr.after_commit,
                "content": format!("comment {index}"),
            }),
        )
        .await;
        expected.push(string_at(&posted, "/comment/commentId"));
    }

    // Every page size — including both published bounds, 1 and 500 — walks the same
    // threads, in the same order, exactly once.
    for page_size in [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(7),
        Some(COMMENTS_PAGE_DEFAULT),
        Some(COMMENTS_PAGE_MAX),
    ] {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = read_body(&pr.pull_request_id);
            if let Some(page_size) = page_size {
                input["maxResults"] = json!(page_size);
            }
            if let Some(token) = &next_token {
                input["nextToken"] = json!(token);
            }
            let read = ok(&router, "GetCommentsForPullRequest", &input).await;
            let reported = threads(&read, &case);
            assert!(
                reported.len() <= page_size.unwrap_or(COMMENTS_PAGE_DEFAULT) as usize,
                "case {case:?}: a page must not exceed maxResults, got {} entries",
                reported.len()
            );
            for entry in &reported {
                walked.extend(thread_comment_ids(entry, &case));
            }
            pages += 1;
            assert!(
                pages <= 16,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            next_token = read
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every thread once, in post order"
        );
    }

    // One past each published bound is not a valid number of results.
    for max_results in [0, -1, COMMENTS_PAGE_MAX + 1] {
        let case = format!("maxResults={max_results}");
        let mut input = read_body(&pr.pull_request_id);
        input["maxResults"] = json!(max_results);
        let code = refused(&router, &case, "GetCommentsForPullRequest", &input).await;
        assert_eq!(
            code, "InvalidMaxResultsException",
            "case {case:?}: a maxResults outside 1..={COMMENTS_PAGE_MAX} must be refused"
        );
    }

    // A continuation token belongs to the listing it was issued for: one this
    // service never issued, and one issued for the unnarrowed listing sent to the
    // listing narrowed to a comparison, both name no position to resume from.
    let mut invented = read_body(&pr.pull_request_id);
    invented["nextToken"] = json!("not-a-token");
    let code = refused(
        &router,
        "nextToken=not-a-token",
        "GetCommentsForPullRequest",
        &invented,
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "a continuation token this service never issued must be refused"
    );

    let mut first_page = read_body(&pr.pull_request_id);
    first_page["maxResults"] = json!(1);
    let page = ok(&router, "GetCommentsForPullRequest", &first_page).await;
    let token = page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("seven threads paged one at a time issue a token: {page}"))
        .to_string();
    let code = refused(
        &router,
        "a token of another listing",
        "GetCommentsForPullRequest",
        &json!({
            "pullRequestId": pr.pull_request_id,
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
            "maxResults": 1,
            "nextToken": token,
        }),
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "a token issued for another listing must be refused"
    );
}

#[tokio::test]
async fn a_read_reports_each_callers_own_reactions_on_the_thread() {
    let (router, _engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "pull-request-comment-reads-reactions").await;
    let posted = ok(
        &router,
        "PostCommentForPullRequest",
        &json!({
            "pullRequestId": pr.pull_request_id,
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
            "content": "a comment two callers react to",
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");

    // Each caller sends the reaction in a different form, so what is reported is
    // the stored reaction rather than the spelling of the request.
    for (access_key_id, reaction) in [
        (DEFAULT_ACCESS_KEY_ID, ":rocket:"),
        (OTHER_ACCESS_KEY_ID, "HOORAY"),
    ] {
        ok_as(
            &router,
            access_key_id,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": reaction }),
        )
        .await;
    }

    // Every principal holding a reaction is counted, while `callerReactions` is the
    // asking principal's own.
    for (access_key_id, own) in [
        (DEFAULT_ACCESS_KEY_ID, "ROCKET"),
        (OTHER_ACCESS_KEY_ID, "HOORAY"),
    ] {
        let read = ok_as(
            &router,
            access_key_id,
            "GetCommentsForPullRequest",
            &read_body(&pr.pull_request_id),
        )
        .await;
        let reported = threads(&read, access_key_id);
        let comment = reported
            .first()
            .and_then(|entry| entry.pointer("/comments/0"))
            .unwrap_or_else(|| panic!("case {access_key_id:?}: no thread in {read}"))
            .clone();
        assert_eq!(
            comment.get("callerReactions"),
            Some(&json!([own])),
            "case {access_key_id:?}: expected the caller's own reaction, got {comment}"
        );
        assert_eq!(
            comment.get("reactionCounts"),
            Some(&json!({ "ROCKET": 1, "HOORAY": 1 })),
            "case {access_key_id:?}: expected one count per reaction held, got {comment}"
        );
    }
}

#[tokio::test]
async fn a_read_the_service_cannot_serve_is_refused_with_the_modeled_error() {
    let (router, _engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "pull-request-comment-reads-refusals").await;
    let elsewhere = pull_request_on(&router, "pull-request-comment-reads-other").await;
    // A well-formed object name no commit of this repository has, and a decimal id
    // no pull request was minted as.
    let unknown_commit = "0".repeat(40);
    let unknown_pull_request = "999999";

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "unknown pull request",
            read_body(unknown_pull_request),
            "PullRequestDoesNotExistException",
        ),
        (
            "empty pull request id",
            read_body(""),
            "InvalidPullRequestIdException",
        ),
        (
            "malformed pull request id",
            read_body("not-a-pull-request"),
            "InvalidPullRequestIdException",
        ),
        (
            "unknown repository",
            json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": "no-such-repository",
                "beforeCommitId": pr.before_commit,
                "afterCommitId": pr.after_commit,
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "repository of another pull request",
            json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": elsewhere.repository,
                "beforeCommitId": elsewhere.before_commit,
                "afterCommitId": elsewhere.after_commit,
            }),
            "RepositoryNotAssociatedWithPullRequestException",
        ),
        (
            "a repository with neither commit",
            json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": pr.repository,
            }),
            "CommitIdRequiredException",
        ),
        (
            "a repository with only the after commit",
            json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": pr.repository,
                "afterCommitId": pr.after_commit,
            }),
            "CommitIdRequiredException",
        ),
        (
            "a repository with a blank before commit",
            json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": pr.repository,
                "beforeCommitId": "",
                "afterCommitId": pr.after_commit,
            }),
            "CommitIdRequiredException",
        ),
        (
            "both commits and no repository",
            json!({
                "pullRequestId": pr.pull_request_id,
                "beforeCommitId": pr.before_commit,
                "afterCommitId": pr.after_commit,
            }),
            "RepositoryNameRequiredException",
        ),
        (
            "malformed after commit id",
            json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": pr.repository,
                "beforeCommitId": pr.before_commit,
                "afterCommitId": "not-a-commit-id",
            }),
            "InvalidCommitIdException",
        ),
        (
            "a commit the repository does not hold",
            json!({
                "pullRequestId": pr.pull_request_id,
                "repositoryName": pr.repository,
                "beforeCommitId": pr.before_commit,
                "afterCommitId": unknown_commit.clone(),
            }),
            "CommitDoesNotExistException",
        ),
    ];
    for (case, body, expected) in rows {
        let code = refused(&router, case, "GetCommentsForPullRequest", &body).await;
        assert_eq!(code, expected, "case {case:?}: GetCommentsForPullRequest");
    }
}
