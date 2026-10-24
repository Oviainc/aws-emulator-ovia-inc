Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.branch-protection-policy-denies-a-real-git-push-to-main codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch codecommit.comment-lifecycle-post-get-update-delete
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.branch-protection-policy-denies-a-real-git-push-to-main|_await#33|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch|_await#33|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.comment-lifecycle-post-get-update-delete|run#27|AssertionError|assert update_message == expected_message, (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CodeCommit: the announced git clone URL is still unusable by a standard git client, and UpdateComment reuses GetComment's CommentDeletedException message (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepository

## Description
Two net-new root causes in the codecommit crate, each grounded in a failing behavioral check the replay report left unmarked (not ALREADY FILED, not OWNED). A fixer must close BOTH before submitting.

Cause 1 of 2: the git clone URL the service announces still cannot be used by a standard git client, so no IAM-gated git-push behavior is reachable at all. Failing checks: codecommit.branch-protection-policy-denies-a-real-git-push-to-main, codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch.
AWS: recorded behavior only (the decision is made inside the git receive-pack negotiation, which no modeled operation expresses). Smithy (com.amazonaws.codecommit#CodeCommit_20150413) GetRepository/CreateRepository return RepositoryMetadata with cloneUrlHttp — the address the service answers git smart-HTTP on. Recorded against real AWS (the blessed check's own header: "Confirmed against real AWS"): with credentials for a principal that holds codecommit:GitPull, `git clone <cloneUrlHttp>` succeeds; with codecommit:GitPush allowed, `git push` to a branch lands and the control plane (GetBranch) then reports the pushed commit; and an IAM Deny on codecommit:GitPush conditioned on codecommit:References (plus the documented `Null: {"codecommit:References": "false"}` guard) makes the push to the named ref exit non-zero ("! [remote rejected] HEAD -> main (You don't have permission to push changes to this branch.)") while a push to another ref still lands.
Emulator: neither check ever completes a clone. Both bound their clone in a 120 s retry against the announced cloneUrlHttp and the harness cuts them off first — verdict 'mismatch', detail 'Check timed out after 60.0s', with no git protocol exchange ever reaching the push leg. The host/port vary per run and are not the requirement; the requirement is that the announced cloneUrlHttp be reachable by an unmodified git client. Hypothesis: the clone retries the same transport-level rejection already recorded for this family (git rc=128 'server verification failed: certificate signer not trusted') until its budget is gone.
Why: cloning cloneUrlHttp is the documented way to use a CodeCommit repository, so a client that reads repository metadata and clones it cannot reach the repository at all. Beyond that, branch protection via codecommit:References on GitPush is the ONLY branch-protection mechanism CodeCommit has; while the transport is unusable an emulator that authorizes every push (or that evaluates the condition once, with the ref in hand, so a Deny missing the Null guard does not widen to every branch) is indistinguishable from a correct one.
Note: open issue '20261023T185908Z-codecommit-8-net-new-behavioral-root-causes-the.md' Cause 1 describes the same transport family and open issue '20261023T072048Z-codecommit-10-behavioral-root-causes-three-operations-answer.md' Cause 5 the same area; NEITHER carries either of the two check ids above on its replay list, so they are recorded here. Fixing the transport may close this cause; both ids must be re-replayed to confirm, and the IAM GitPush authorization behavior above only becomes observable once they get past the clone.
Fix: the git smart-HTTP transport (its TLS termination / announced scheme) and the cloneUrlHttp construction in the codecommit crate's repository-metadata serializer, plus the GitPush authorization hook the transport consults when evaluating an IAM policy carrying codecommit:References.
Traces: codecommit.branch-protection-policy-denies-a-real-git-push-to-main, codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch.

Cause 2 of 2: UpdateComment on a content-deleted comment answers GetComment's CommentDeletedException message instead of this operation's own template. Failing check: codecommit.comment-lifecycle-post-get-update-delete.
AWS: Smithy models CommentDeletedException as a client error with httpError 400 on both GetComment and UpdateComment (UpdateComment input: commentId @required, content @required). Recorded behavior only: the two operations do NOT share one message. After DeleteCommentContent, GetComment answers 'The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.', while UpdateComment on the same id answers that same text prefixed by 'You cannot delete this comment.  ' — a distinct, fixed per-operation template.
Emulator: the status (400) and the error code (CommentDeletedException) are correct on both paths, and the GetComment assertion in the same check passes; UpdateComment returns the GetComment text verbatim ('The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.'), i.e. one shared message for both operations.
Why: the dead-comment refusal is the path a review client takes when it tries to edit a comment someone else has already removed; code or tooling that distinguishes the two refusals by their text sees one message where AWS sends two, so a differential client diverges on every such edit.
Note: open issue '20261022T213415Z-codecommit-putcommentreaction-is-a-501-stub-cmk-disablement.md' covers other codecommit message templates (GetCommit, RepositoryDoesNotExist) but not this one, and this check id is on no issue's replay list.
Fix: the CommentDeletedException construction in the UpdateComment handler of the codecommit crate (services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/update_comment.rs) — it must not reuse the shared get_comment/comment-lookup message for this operation.
Traces: codecommit.comment-lifecycle-post-get-update-delete.

## Evidence
All quoted strings are verbatim from the simparity-behavioral-ga-prod-v1 replay report (90 checks, 38 passed). Generated repository names, comment ids, hosts and ports vary per run and are examples; the rule in each cause is the requirement.

Cause 1 — codecommit.branch-protection-policy-denies-a-real-git-push-to-main [mismatch]: 'Check timed out after 60.0s'. The check calls require_tools('git','aws') (so git and the CLI were present), creates the repository, seeds main through PutFile, creates 'feature', creates a role whose inline policy allows codecommit:GitPull/GitPush on the repository ARN, assumes it, and then retries `git clone <cloneUrlHttp>` with the aws codecommit credential-helper inside a 120 s window whose stated expectation is "git clone of <cloneUrlHttp> as <roleArn> to succeed -- the Allow statement grants GitPull, so a persistent failure means the git transport is unreachable for this principal". The harness cap fires inside that window; the positive-control push to refs/heads/feature, the Deny leg and the removal leg are never reached. codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch [mismatch]: 'Check timed out after 60.0s' in the identical clone retry.

Cause 2 — codecommit.comment-lifecycle-post-get-update-delete [mismatch]: AssertionError at `assert update_message == expected_message` — "expected message 'You cannot delete this comment.  The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.', got 'The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.'". Everything before it passed: PostCommentForComparedCommit, GetComment (content and deleted=False), UpdateComment's echo, the independent GetComment read, DeleteCommentContent (deleted=True, content cleared), and the GetComment refusal including BOTH its code and its message — so only UpdateComment's message template diverges.

## Steps to Reproduce
Boot the emulator and point botocore at it; cc = client('codecommit'), iam = client('iam'), sts = client('sts').

Cause 1:
1. meta = cc.create_repository(repositoryName=R)['repositoryMetadata']; cc.put_file(repositoryName=R, branchName='main', filePath='README.md', fileContent=b'seed\n', commitMessage='seed'); cc.create_branch(repositoryName=R, branchName='feature', commitId=<that commit>).
2. Create a role trusted by the account, put an inline policy allowing codecommit:GitPull and codecommit:GitPush on meta['Arn'], assume it, and export the session credentials.
3. Run `git -c credential.helper='!aws codecommit credential-helper $@' -c credential.UseHttpPath=true clone <meta["cloneUrlHttp"]>` with an unmodified git client -> expect the clone to succeed; observe it never succeeding (transport refused) until the poll/harness budget is gone.
4. With the clone in hand, commit and `git push origin feature` -> expect exit 0 and cc.get_branch(repositoryName=R, branchName='feature') to report the pushed commit.
5. Add a Deny statement on codecommit:GitPush for the repository ARN with Condition {StringEqualsIfExists: {'codecommit:References': ['refs/heads/main']}, Null: {'codecommit:References': 'false'}} and push a FRESH commit to main -> expect a non-zero exit and an authorization refusal from receive-pack, with GetBranch('main') unchanged; a push to refs/heads/feature must still land. Remove the Deny -> the byte-identical push to main must land.
6. Re-replay codecommit.branch-protection-policy-denies-a-real-git-push-to-main and codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch.

Cause 2:
7. cc.create_repository(repositoryName=R2); c1 = cc.put_file(repositoryName=R2, branchName='main', filePath='a.txt', fileContent=b'v1\n')['commitId']; c2 = cc.put_file(repositoryName=R2, branchName='main', filePath='a.txt', fileContent=b'v2\n', parentCommitId=c1)['commitId'].
8. cid = cc.post_comment_for_compared_commit(repositoryName=R2, beforeCommitId=c1, afterCommitId=c2, content='initial comment')['comment']['commentId']; cc.delete_comment_content(commentId=cid).
9. cc.get_comment(commentId=cid) -> 400 CommentDeletedException, message 'The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.' (already correct).
10. cc.update_comment(commentId=cid, content='after delete') -> expect 400 CommentDeletedException whose message is that same text PREFIXED with 'You cannot delete this comment.  '; observe the unprefixed GetComment text.
11. Re-replay codecommit.comment-lifecycle-post-get-update-delete.
