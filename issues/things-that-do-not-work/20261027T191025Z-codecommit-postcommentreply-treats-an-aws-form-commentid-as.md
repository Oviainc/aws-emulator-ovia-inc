Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.pull-request-comment-thread-anchors-to-the-diff-blob-ids
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.pull-request-comment-thread-anchors-to-the-diff-blob-ids|run#51|AssertionError|assert code == "CommentDoesNotExistException", (
Issue-Commit: 19fca1009697525153c46de34cf9f84b5fd316c3

# CodeCommit PostCommentReply treats an AWS-form commentId as malformed: InvalidCommentIdException instead of CommentDoesNotExistException, and the not-found message template diverges

> _Note: this issue was identified against the code base at commit `19fca1009697525153c46de34cf9f84b5fd316c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostCommentReply

## Description
One net-new root cause in the codecommit crate, grounded in the only failing behavioral check the replay report left unmarked (not ALREADY FILED, not OWNED).

Cause 1 of 1: PostCommentReply classifies a well-formed-but-absent CodeCommit commentId as malformed, so it answers InvalidCommentIdException where AWS answers CommentDoesNotExistException; and the CommentDoesNotExistException message it does build on its own not-found path is not AWS's template.

AWS: Smithy PostCommentReply takes inReplyTo (@required, target CommentId) and content (@required, Content), and models BOTH CommentDoesNotExistException (client, httpError 400 — "No comment exists with the provided ID. Verify that you have used the correct ID, and then try again.") and InvalidCommentIdException (client, httpError 400 — "The comment ID is not in a valid format. Make sure that you have provided the full comment ID."). The two are distinguished by the FORM of the id, not by whether it resolves: a commentId that has CodeCommit's shape but names no comment is a not-found, i.e. HTTP 400 / CommentDoesNotExistException. Recorded behavior: a CodeCommit commentId is a pair of UUIDs joined by a colon (`<uuid>:<uuid>`), so an all-zero id of that form (`00000000-0000-0000-0000-000000000000:00000000-0000-0000-0000-000000000000`) is well formed and draws CommentDoesNotExistException; a single-UUID id is the malformed case that draws InvalidCommentIdException. The recorded not-found message is the fixed text "The following comment ID does not exist: " followed by the commentId taken verbatim from the request (the id is the only variable part; the leading sentence is fixed).

Emulator: the reply is refused with error code 'InvalidCommentIdException' (replay detail: "PostCommentReply(inReplyTo='00000000-0000-0000-0000-000000000000:00000000-0000-0000-0000-000000000000') returned 'InvalidCommentIdException', expected 'CommentDoesNotExistException'"). The crate's shared id classifier accepts ONLY a single canonical 8-4-4-4-12 UUID — `crate::state::is_comment_id` splits on '-' and requires exactly five hex groups of widths 8/4/4/4/12 — so any colon-joined pair fails it and `normalize_comment_id` returns `CommentIdProblem::Invalid`, which the handler maps to InvalidCommentIdException before any lookup happens. Consistently with that classifier, the emulator MINTS comment ids as a single version-8 UUID (`crate::state::comment_id`), which is also not AWS's `<uuid>:<uuid>` form. Separately, the handler's own not-found refusal (reached only for a single-UUID id that resolves to nothing) carries the message "no comment exists with the ID <id>" instead of AWS's "The following comment ID does not exist: <id>"; the status (400) and the shape are otherwise right.

Why: a client that retries or branches on CommentDoesNotExistException (the normal "the comment was deleted / the thread moved" path) never sees it — it gets the malformed-input error instead, which a well-behaved client treats as a bug in its own request and does not retry. Worse, because the classifier rejects AWS's id form outright, any client that carries a real CodeCommit commentId (recorded ids, ids pasted from the console, ids persisted by a previous run against AWS) cannot address a comment through this emulator at all: every PostCommentReply, and every other operation sharing `normalize_comment_id`, refuses it as malformed. Tools matching the not-found message text also diverge.

Fix: the comment-id form rule in services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs (`is_comment_id` / `normalize_comment_id`, with `comment_id` minting ids in the same form so the validator and the minter agree), and the CommentDoesNotExistException message construction on the not-found path of services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/post_comment_reply.rs. The classification must be form-vs-existence: an id of CodeCommit's form that resolves to no stored comment is the not-found error, not the malformed-id error.

Traces: codecommit.pull-request-comment-thread-anchors-to-the-diff-blob-ids.

Related-but-not-covering: the open issue 20261022T213415Z-codecommit-putcommentreaction-is-a-501-stub-cmk-disablement.md bundles other codecommit error-message template divergences (GetCommit, RepositoryDoesNotExist) and the PutCommentReaction stub, but neither lists this check nor PostCommentReply's comment-id classification.

## Evidence
Check codecommit.pull-request-comment-thread-anchors-to-the-diff-blob-ids [mismatch].

Everything earlier in the check PASSES: the repository, the base/feature commits, GetDifferences' before/after blob ids, CreatePullRequest, PostCommentForPullRequest, two PostCommentReply calls, and the GetCommentsForPullRequest thread (one group for the path, correct pullRequestId/repositoryName/beforeCommitId/afterCommitId, beforeBlobId/afterBlobId equal to GetDifferences' blob ids, location echoed verbatim, the three comment ids with inReplyTo pointing at the root, non-decreasing creationDates). The failure is the final negative leg.

Assertion that fired (check line 201):
  assert code == "CommentDoesNotExistException"
  AssertionError: PostCommentReply(inReplyTo='00000000-0000-0000-0000-000000000000:00000000-0000-0000-0000-000000000000') returned 'InvalidCommentIdException', expected 'CommentDoesNotExistException'

Expected (AWS): HTTP 400, Error.Code 'CommentDoesNotExistException', Error.Message = "The following comment ID does not exist: " + the inReplyTo value from the request (fixed sentence + request-supplied id). No comment is created and the thread still holds exactly the three real comments.
Actual (emulator): HTTP 400, Error.Code 'InvalidCommentIdException', Error.Message "the comment ID <inReplyTo> is not in a valid format" (built at src/ops/post_comment_reply.rs:199-209 from the `CommentIdProblem::Invalid` branch). The assertion on the message template behind it (expected_message at check line 204) is therefore never reached and remains unverified.

Source of the misclassification: services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs:3685 `is_comment_id` accepts only five '-'-separated hex groups of widths 8/4/4/4/12, and :3709 `normalize_comment_id` returns `CommentIdProblem::Invalid` for anything else — a colon-joined UUID pair can never reach the comment lookup. src/state.rs:3781 `comment_id` mints ids as a single version-8 UUID, the same single-UUID form.
The handler's own not-found message is at src/ops/post_comment_reply.rs:262-271: "no comment exists with the ID {in_reply_to}", vs AWS's "The following comment ID does not exist: {id}".
The all-zero placeholder id is a constant of the check, not a per-run value; the generated repository name, commit ids and real comment ids vary per run and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (region/credentials as the harness uses).
2. R = a fresh repository name; cc.create_repository(repositoryName=R).
3. base = cc.put_file(repositoryName=R, branchName='main', filePath='reviewed.txt', fileContent=b'line one\nline two\nline three\n', commitMessage='base version')['commitId'].
4. cc.create_branch(repositoryName=R, branchName='feature', commitId=base); tip = cc.put_file(repositoryName=R, branchName='feature', filePath='reviewed.txt', fileContent=b'line one\nline two CHANGED ON FEATURE\nline three\n', parentCommitId=base, commitMessage='change the reviewed line')['commitId'].
5. pr = cc.create_pull_request(title='thread anchoring', targets=[{'repositoryName': R, 'sourceReference': 'feature', 'destinationReference': 'main'}])['pullRequest']['pullRequestId'].
6. root = cc.post_comment_for_pull_request(pullRequestId=pr, repositoryName=R, beforeCommitId=base, afterCommitId=tip, location={'filePath': 'reviewed.txt', 'filePosition': 2, 'relativeFileVersion': 'AFTER'}, content='root: is this change intentional?')['comment']['commentId']; cc.post_comment_reply(inReplyTo=root, content='reply one: yes, deliberate.').
7. absent = '00000000-0000-0000-0000-000000000000:00000000-0000-0000-0000-000000000000'; cc.post_comment_reply(inReplyTo=absent, content='reply to nothing').
   Expect: HTTP 400, Code 'CommentDoesNotExistException', Message 'The following comment ID does not exist: ' + absent.
   Observe: HTTP 400, Code 'InvalidCommentIdException', Message 'the comment ID <absent> is not in a valid format'.
8. Confirm the refusal had no effect: cc.get_comments_for_pull_request(pullRequestId=pr, repositoryName=R, beforeCommitId=base, afterCommitId=tip) still reports exactly the real comments.
9. Cross-check the form rule: a commentId of AWS's '<uuid>:<uuid>' form must be accepted as well formed by every codecommit operation taking a commentId, and a single-UUID id must be the one that draws InvalidCommentIdException.
10. Re-replay codecommit.pull-request-comment-thread-anchors-to-the-diff-blob-ids.
