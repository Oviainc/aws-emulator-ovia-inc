Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.comment-reaction-add-and-get-round-trip codecommit.put-comment-reaction-get-reactions-reflects
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.comment-reaction-add-and-get-round-trip|run#23|AssertionError|assert caller_arn in entry["reactionUsers"], entry["reactionUsers"]
Issue-Key: codecommit.put-comment-reaction-get-reactions-reflects|run#21|AssertionError|assert entry["reactionUsers"] == [caller_arn], (
Issue-Commit: 51acfe3f2ac52bf94dbabe00a764942f56e02eae

# CodeCommit: comment-reaction attribution uses an access-key-derived stand-in ARN, so reactionUsers never matches the calling identity STS reports

> _Note: this issue was identified against the code base at commit `51acfe3f2ac52bf94dbabe00a764942f56e02eae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCommentReactions

## Description
One net-new root cause in the codecommit crate, grounded in two failing behavioral checks the replay report left unmarked (not ALREADY FILED, not OWNED by any open issue).

Cause 1 of 1: a reaction written by PutCommentReaction is attributed to an identity the emulator synthesizes from the request's access key id, not to the principal the request authenticates as, so GetCommentReactions.reactionUsers reports an ARN that no caller can correlate with its own identity.

AWS: Smithy PutCommentReaction (commentId @required CommentId, reactionValue @required ReactionValue) documents 'Adds or updates a reaction to a specified comment for the user whose identity is used to make the request. You can only add or update a reaction for yourself.' GetCommentReactions returns reactionsForComment (@required ReactionsForCommentList) whose ReactionForComment.reactionUsers member is documented as 'The Amazon Resource Names (ARNs) of users who have provided reactions to the comment.' The requirement is therefore an identity equality, not a literal: the single entry in reactionUsers after one PutCommentReaction by one caller is EXACTLY the principal ARN that the same credential resolves to — i.e. the Arn that sts:GetCallerIdentity returns for that same request signer. GetCommentReactions also exposes reactionUserArn (Arn) as a server-side filter over that same value, and models InvalidReactionUserArnException (client, HTTP 400) for a malformed one, so the stored ARN is the key that filter is matched against.

Emulator: PutCommentReaction succeeds (HTTP 200) and GetCommentReactions returns exactly one ReactionForComment with the right reaction formats — the shortCode ':thumbsup:' and the emoji glyph assertions pass — but reactionUsers carries an ARN of the shape arn:aws:iam::<accountId>:user/<ACCESS-KEY-ID>, built from the access key id the request was signed with, while sts:GetCallerIdentity for that very same credential answers arn:aws:iam::<accountId>:user/emulator. The two replayed checks observe exactly this: reactionUsers == ['arn:aws:iam::000000000000:user/EMULATORBB1C2F03EBCA'] and ['arn:aws:iam::000000000000:user/EMULATOR7268F7C33CAB'] where the caller identity is arn:aws:iam::000000000000:user/emulator. (Account id and the key-id text are per-run values; the DIVERGENCE is that the user-path segment is derived from the key id instead of from the resolved principal, so the two answers disagree for one credential.)

Why: a reaction surface is only usable if a client can tell whether IT reacted. A review tool reads its own identity once (sts:GetCallerIdentity) and then decides 'did I already react to this comment?' by testing membership of that ARN in reactionUsers, or by passing it as GetCommentReactions.reactionUserArn; against this emulator both answers are always 'no', so toggling a reaction, rendering 'your reaction' state, and any per-user reaction accounting are silently wrong. Because the same caller-identity resolution feeds every authorship/reviewer ARN this crate records, any field that must agree with GetCallerIdentity diverges the same way.

Fix: the shared caller-identity resolution in the codecommit crate's state module — it must report the principal the credential resolves to (the identity STS answers for the same access key id) rather than an identity synthesized from the key id's text, so the stand-in path cannot disagree with sts:GetCallerIdentity. The reaction read path must then project that stored principal into ReactionForComment.reactionUsers and match GetCommentReactions.reactionUserArn against it.

Related but NOT covering: open issue 20261022T213415Z-codecommit-putcommentreaction-is-a-501-stub-cmk-disablement.md (its Cause 1 is PutCommentReaction answering 501; that call now succeeds, and neither of these two check ids is on its replay list), and the resolved issue 20261024T114145Z-codecommit-getcommentreactions-stub.md (the operation is implemented now; this is a fidelity defect in what it returns, not a refusal).

Traces: codecommit.comment-reaction-add-and-get-round-trip, codecommit.put-comment-reaction-get-reactions-reflects. Component citations: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/put_comment_reaction.rs:173 (caller_identity_arn) and :275 (reactions keyed by that ARN); services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs:4028 (caller_identity_arn), :4057 (long_term_caller_identity) and :4079 (stand_in_identity, which builds user/<access-key-id>).

## Evidence
codecommit.comment-reaction-add-and-get-round-trip [mismatch]: AssertionError at `assert caller_arn in entry["reactionUsers"]` — reactionUsers was ['arn:aws:iam::000000000000:user/EMULATORBB1C2F03EBCA'] while sts.get_caller_identity()['Arn'] for the same client/credential was arn:aws:iam::000000000000:user/emulator. The preceding assertions passed, so exactly one ReactionForComment is returned and reaction.shortCode == ':thumbsup:' and reaction.emoji == U+1F44D are correct; only the attributed principal diverges.
codecommit.put-comment-reaction-get-reactions-reflects [mismatch]: AssertionError 'expected reactionUsers == [arn:aws:iam::000000000000:user/emulator], got [\'arn:aws:iam::000000000000:user/EMULATOR7268F7C33CAB\']' at `assert entry["reactionUsers"] == [caller_arn]`. The shortCode assertion immediately before it passed.
Both calls returned HTTP 200 with no error code — this is a response-content divergence, not a refusal. Expected: reactionUsers == [<the Arn sts:GetCallerIdentity returns for the signing credential>]. Actual: reactionUsers == [arn:aws:iam::<accountId>:user/<ACCESS-KEY-ID>], i.e. the fixed text 'arn:aws:iam::', the emulator account id, ':user/' and then the request's access key id in place of the resolved principal's name. Account id, repository names, pull-request ids and the key-id text vary per run; the invariant that must hold is the equality between reactionUsers and the caller identity, which never holds here.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (region/credentials as the harness uses; note the harness signs with an access key id like 'EMULATOR<hex>').
2. caller = sts.get_caller_identity()['Arn']  # observe arn:aws:iam::<accountId>:user/emulator
3. cc.create_repository(repositoryName=R)
4. base = cc.put_file(repositoryName=R, branchName='main', filePath='a.txt', fileContent=b'v1\n')['commitId']
5. cc.create_branch(repositoryName=R, branchName='feature', commitId=base); feat = cc.put_file(repositoryName=R, branchName='feature', filePath='a.txt', fileContent=b'v2\n', parentCommitId=base)['commitId']
6. pr = cc.create_pull_request(title=T, targets=[{'repositoryName': R, 'sourceReference': 'feature', 'destinationReference': 'main'}])['pullRequest']
7. cid = cc.post_comment_for_pull_request(pullRequestId=pr['pullRequestId'], repositoryName=R, beforeCommitId=base, afterCommitId=feat, content='LGTM')['comment']['commentId']
8. cc.put_comment_reaction(commentId=cid, reactionValue=':thumbsup:')  # 200
9. entry = cc.get_comment_reactions(commentId=cid)['reactionsForComment'][0] -> expect entry['reactionUsers'] == [caller]; observe ['arn:aws:iam::<accountId>:user/<ACCESS-KEY-ID>'].
10. cc.get_comment_reactions(commentId=cid, reactionUserArn=caller) -> expect the same single ReactionForComment (the caller's own reaction); observe no match for the caller's ARN.
11. Re-replay both check ids in trace_ids.
