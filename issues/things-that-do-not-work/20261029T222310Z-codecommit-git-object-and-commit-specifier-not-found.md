Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-blob-differences-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-blob-differences-error-paths codecommit-get-blob-differences-error-paths codecommit-get-blob-differences-errors codecommit-branch-and-commit-conflict-errors codecommit-branch-and-commit-workflow codecommit-branch-and-file-conflict-error-paths codecommit-branch-merge-strategies codecommit-put-file-parent-commit-errors
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetBlobDifferences|4xx->4xx|$.message
Issue-Key: codecommit|CreateBranch|4xx->4xx|$.message
Issue-Key: codecommit|GetMergeOptions|4xx->4xx|$.message
Issue-Key: codecommit|PutFile|4xx->4xx|$.message
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit git-object and commit-specifier not-found errors use internal prose instead of AWS messages (4 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlobDifferences

## Description
FOUR net-new causes sharing one fault: when a git object or commit specifier cannot be resolved, the emulator returns the correct status and error code but its own internal sentence, dropping or reformatting the identifier AWS puts in the message. A fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: GetBlobDifferences InvalidBlobIdException message drops the offending blob id ===
AWS: Smithy GetBlobDifferences models InvalidBlobIdException (client, httpError 400 [defaulted]) with a single `message`. Recorded behavior: for a blobId that is not a git object id, AWS answers 400 with '<blobId> is not a valid git object ID' — the one variable part is the rejected id, copied verbatim from the request; the rest is fixed text.
Emulator: 400 with 'The specified blob is not valid.', naming nothing.
Why: a request can carry both beforeBlobId and afterBlobId, so AWS's message is the only way a client learns WHICH id was rejected.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_blob_differences.rs — build the message from the rejected id.
Traces: codecommit-blob-differences-error-paths#task#4 (also #task#5, codecommit-get-blob-differences-error-paths#task#3, codecommit-get-blob-differences-errors#task#3 and #task#4).

=== Cause 2 of 4: CreateBranch CommitDoesNotExistException message is not AWS's 'Object missing' form ===
AWS: recorded behavior for a commitId that is absent from the repository is 400 with 'Object missing: <commitId>' — fixed prefix plus the requested commit id.
Emulator: 400 with 'Could not find commit <commitId>'.
Why: operators and tests match the documented wording; the emulator's variant is a different sentence for the same error class.
Fix: src/ops/create_branch.rs.
Traces: codecommit-branch-and-commit-conflict-errors#task#3 (also codecommit-branch-and-commit-workflow#task#12, codecommit-branch-and-file-conflict-error-paths#task#4).

=== Cause 3 of 4: GetMergeOptions CommitDoesNotExistException message omits AWS's explanation ===
AWS: recorded behavior for an unresolvable commit specifier is 400 with 'AWS CodeCommit could not find the commit specifier <commitSpecifier>. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.' — fixed text with the request's specifier interpolated.
Emulator: 400 with 'Could not find commit <commitSpecifier>', losing the second sentence that tells the client the branch may simply be unpushed.
Why: the dropped sentence is the actionable half of the message for this error class.
Fix: src/ops/get_merge_options.rs (and any shared commit-specifier resolver it calls, which must distinguish the specifier wording from the raw-commit wording in Cause 2).
Traces: codecommit-branch-merge-strategies#task#19.

=== Cause 4 of 4: PutFile ParentCommitIdDoesNotExistException message is lowercase internal prose ===
AWS: recorded behavior for a parentCommitId that does not exist is 400 with 'The parent commit ID is not valid. The specified parent commit ID does not exist.' — entirely FIXED text; AWS deliberately echoes neither the id nor the repository name.
Emulator: 400 with 'the parent commit id <parentCommitId> does not exist in the repository <repositoryName>' — lowercase, and it leaks two request values AWS keeps out of the message.
Why: clients asserting on AWS's sentence fail, and the emulator's form is not a message any AWS client can be written against.
Fix: src/ops/put_file.rs.
Traces: codecommit-put-file-parent-commit-errors#task#4.

## Evidence
Cause 1 — status 400 and code as expected; expected $.message '<blobId> is not a valid git object ID' (recorded: 'not-a-valid-blob-id is not a valid git object ID'), actual 'The specified blob is not valid.' 5 steps clustered.
Cause 2 — expected $.message 'Object missing: <commitId>', actual 'Could not find commit <commitId>'. 3 steps clustered.
Cause 3 — expected $.message 'AWS CodeCommit could not find the commit specifier <specifier>. Either the specifier does not exist, or the commit has not yet been pushed to the remote repository.', actual 'Could not find commit <specifier>'.
Cause 4 — expected $.message 'The parent commit ID is not valid. The specified parent commit ID does not exist.' (fixed, value-free), actual 'the parent commit id <id> does not exist in the repository <repo>'.
In Causes 1-3 the identifier is a request value filling the template; in Cause 4 the template has no variable parts at all.

## Steps to Reproduce
1. Replay codecommit-blob-differences-error-paths#task#4 (GetBlobDifferences with a non-git-object blob id) and compare $.message.
2. Replay codecommit-branch-and-commit-conflict-errors#task#3 (CreateBranch pointing at an absent commitId).
3. Replay codecommit-branch-merge-strategies#task#19 (GetMergeOptions with an unresolvable commit specifier).
4. Replay codecommit-put-file-parent-commit-errors#task#4 (PutFile with a nonexistent parentCommitId).
