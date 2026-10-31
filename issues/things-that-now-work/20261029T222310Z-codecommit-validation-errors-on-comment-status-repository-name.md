Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-post-comment-for-pull-request-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-post-comment-for-pull-request-error-paths codecommit-pull-request-core-lifecycle codecommit-repository-creation-error-paths codecommit-repository-triggers-and-tagging
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|PostCommentForPullRequest|4xx->4xx|$.__type
Issue-Key: codecommit|UpdatePullRequestStatus|4xx->4xx|$.message
Issue-Key: codecommit|CreateRepository|4xx->4xx|$.message
Issue-Key: codecommit|TagResource|4xx->4xx|$.message
Issue-Related: 20261029T222310Z-codecommit-reports-invalid-idexception-cases-as-requiredexception-and.md
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit validation errors on comment, status, repository-name and tag paths return the wrong class or wording (4 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostCommentForPullRequest

## Description
FOUR net-new causes, each a validation/error-reporting defect on a different operation; grouped because all four are the same kind of fault — the emulator's own error prose (or its own precedence between error classes) replaces AWS's. A fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: PostCommentForPullRequest reports RepositoryDoesNotExistException where AWS reports PullRequestDoesNotExistException ===
AWS: Smithy PostCommentForPullRequest models both RepositoryDoesNotExistException and PullRequestDoesNotExistException (client, httpError 400 [defaulted]). Recorded behavior: when the repositoryName does not exist, AWS resolves the PULL REQUEST first and answers 400 PullRequestDoesNotExistException with 'The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests.' (fixed text).
Emulator: 400 RepositoryDoesNotExistException with message '<repositoryName> for <accountId>' — both the wrong error class AND an internal identifier-pair string that is not an AWS message at all.
Why: a client catching PullRequestDoesNotExistException does not enter its handler, and whatever does catch the error is handed a message no SDK or human can interpret. Validation PRECEDENCE is the requirement here: the pull-request lookup comes before the repository lookup.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/post_comment_for_pull_request.rs — order the lookups pull-request-first and format RepositoryDoesNotExistException (wherever it is still raised) with AWS's sentence rather than '<name> for <account>'.
Traces: codecommit-post-comment-for-pull-request-error-paths#task#4.

=== Cause 2 of 4: UpdatePullRequestStatus's InvalidPullRequestStatusUpdateException message uses upper-case status tokens ===
AWS: recorded behavior for an illegal status transition is 400 with 'The pull request status update is not valid. The only valid value change is from Open to Closed.' — fixed text, with the statuses written as 'Open' and 'Closed'.
Emulator: 'The pull request status update is not valid. The only valid update is from OPEN to CLOSED.' — a reworded second sentence using the enum tokens.
Why: assertions and operator runbooks keyed on AWS's sentence fail on a message that differs in both wording and casing.
Fix: src/ops/update_pull_request_status.rs.
Traces: codecommit-pull-request-core-lifecycle#task#16.

=== Cause 3 of 4: CreateRepository's InvalidRepositoryNameException message drops the entire naming rule ===
AWS: Smithy RepositoryName is @length min 1 max 100 with pattern ^[\w\.-]+$, and InvalidRepositoryNameException is a client 400. Recorded behavior: the message states the rule in full — 'The repository name is not valid. Repository names can be any valid combination of letters, numbers, periods, underscores, and dashes between 1 and 100 characters in length. Names are case sensitive. For more information, see Limits in the AWS CodeCommit User Guide.' — entirely fixed text matching the modeled constraints.
Emulator: 'A specified repository name is not valid.'
Why: the dropped text is the only statement of the allowed character set and length bounds, so a client cannot correct its input from the response.
Fix: src/ops/create_repository.rs (and any shared repository-name validator).
Traces: codecommit-repository-creation-error-paths#task#3.

=== Cause 4 of 4: TagResource reports an aws:-prefixed tag key as a key-format error instead of a system-tag authorization refusal ===
AWS: recorded behavior only — tag keys beginning with 'aws:' are RESERVED system tags, and an end-user caller attempting to set one is refused with HTTP 400 and the message 'Caller is an end user and not allowed to mutate system tags' (fixed text; it names neither the key nor the resource).
Emulator: 400 with 'the tag key aws:reserved is not valid: key names cannot be prefixed with aws:' — a lowercase key-format complaint that echoes the request's tag key.
Why: the two messages describe different failures (an authorization refusal vs a malformed key), so a client cannot tell that the key is legal but reserved to the service, and the emulator's text is not an AWS message for this class.
Fix: src/ops/tag_resource.rs — refuse aws:-prefixed keys as a system-tag mutation by an end-user caller, with AWS's fixed message.
Traces: codecommit-repository-triggers-and-tagging#task#11.

## Evidence
Cause 1 — status 400 as expected; expected $.__type 'PullRequestDoesNotExistException' and $.message 'The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests.'; actual $.__type 'RepositoryDoesNotExistException' and $.message '<repositoryName> for <accountId>'.
Cause 2 — status 400 as expected; expected $.message 'The pull request status update is not valid. The only valid value change is from Open to Closed.', actual '... The only valid update is from OPEN to CLOSED.'
Cause 3 — status 400 as expected; expected the full naming-rule message quoted above, actual 'A specified repository name is not valid.'
Cause 4 — status 400 as expected; expected $.message 'Caller is an end user and not allowed to mutate system tags', actual 'the tag key aws:reserved is not valid: key names cannot be prefixed with aws:'. The tag key is a request value the AWS message does not interpolate.
All four expected messages are fixed text with no variable parts.

## Steps to Reproduce
1. Replay codecommit-post-comment-for-pull-request-error-paths#task#4 (PostCommentForPullRequest naming a repository that does not exist) and compare $.__type and $.message.
2. Replay codecommit-pull-request-core-lifecycle#task#16 (UpdatePullRequestStatus with an illegal transition) and compare $.message.
3. Replay codecommit-repository-creation-error-paths#task#3 (CreateRepository with a name violating ^[\w\.-]+$) and compare $.message.
4. Replay codecommit-repository-triggers-and-tagging#task#11 (TagResource with an 'aws:'-prefixed tag key) and compare $.message.
