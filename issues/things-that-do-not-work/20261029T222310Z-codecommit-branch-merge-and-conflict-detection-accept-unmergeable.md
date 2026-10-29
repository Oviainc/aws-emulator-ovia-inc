Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-branch-merge-strategies
Issue-Step: task#18
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-branch-merge-strategies codecommit-merge-conflict-detection-and-resolution codecommit-create-pull-request-error-paths codecommit-pull-request-core-lifecycle
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|MergeBranchesByFastForward|4xx->2xx|$.__type
Issue-Key: codecommit|MergeBranchesByFastForward|4xx->2xx|$.message
Issue-Key: codecommit|MergeBranchesByFastForward|4xx->2xx|status
Issue-Key: codecommit|MergeBranchesByThreeWay|4xx->4xx|$.message
Issue-Key: codecommit|CreateUnreferencedMergeCommit|4xx->4xx|$.__type
Issue-Key: codecommit|CreateUnreferencedMergeCommit|4xx->4xx|$.message
Issue-Key: codecommit|CreatePullRequest|4xx->4xx|$.message
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit branch merge and conflict detection accept unmergeable input and misreport conflict errors (4 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
MergeBranchesByFastForward

## Description
FOUR net-new causes in the branch-merge/conflict surface: one merge that AWS refuses succeeds, and three refusals carry the wrong error class or the wrong message. A fixer must close ALL FOUR before submitting. Related but NOT covering: open issue '20261026T234121Z-codecommit-2-net-new-behavioral-root-causes-the' reports the branch-merge ManualMergeRequiredException wording from the behavioral suite; it carries no wire replay key, so Cause 2 below is unowned.

=== Cause 1 of 4: MergeBranchesByFastForward succeeds where the branches cannot be fast-forwarded ===
AWS: Smithy MergeBranchesByFastForward models ManualMergeRequiredException (client, httpError 400 [defaulted]). A fast-forward is only legal when the destination tip is an ancestor of the source tip; when the tips have diverged AWS answers 400 ManualMergeRequiredException with message 'Current branches cannot be merged, please try to merge using a Git client locally.' (entirely fixed text) and the destination branch is NOT moved.
Emulator: HTTP 200 with a commitId/treeId — it performs a merge AWS would never perform for this strategy.
Why: this is the worst failure mode in the bundle: a client using fast-forward precisely BECAUSE it must not create a merge commit silently gets history the real service refuses, and conflict-handling code paths are never exercised.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/merge_branches_by_fast_forward.rs — add the ancestry check and raise ManualMergeRequiredException when the destination tip is not an ancestor of the source tip.
Traces: codecommit-branch-merge-strategies#task#18.

=== Cause 2 of 4: MergeBranchesByThreeWay's ManualMergeRequiredException carries the pull-request wording ===
AWS: for a three-way BRANCH merge with unresolved conflicts, the modeled ManualMergeRequiredException message is 'Current branches cannot be merged, please try to merge using a Git client locally.' — the same fixed branch-scoped sentence as Cause 1.
Emulator: the PULL-REQUEST variant, 'The pull request cannot be merged automatically into the destination branch. You must manually merge the branches and resolve any conflicts.', on an operation whose request contains no pull request at all.
Why: the message names an object the caller never referenced, so message-based triage and assertions both break.
Fix: src/ops/merge_branches_by_three_way.rs — use the branch-scoped message constant (the pull-request one belongs only to MergePullRequestBy*).
Traces: codecommit-merge-conflict-detection-and-resolution#task#14.

=== Cause 3 of 4: CreateUnreferencedMergeCommit raises InvalidReplacementTypeException instead of InvalidConflictResolutionException ===
AWS: Smithy CreateUnreferencedMergeCommit models BOTH InvalidConflictResolutionException and InvalidReplacementTypeException (both client, 400). Recorded behavior: when a conflictResolution block is supplied together with conflictResolutionStrategy NONE, AWS rejects the COMBINATION with InvalidConflictResolutionException, message 'Conflict resolution parameter cannot be used together with NONE conflict resolution strategy' (fixed text).
Emulator: InvalidReplacementTypeException with 'Automerge was specified for resolving the conflict, but the specified replacement type is not valid.' — it validates the replacement entries instead of noticing that resolution entries are illegal under the NONE strategy, and reports a strategy (Automerge) the request did not ask for.
Why: AWS SDKs model these as distinct exception types, so a client catching InvalidConflictResolutionException does not enter its handler; the message also misdescribes the request.
Fix: src/ops/create_unreferenced_merge_commit.rs — check strategy/conflictResolution compatibility before validating individual replacement entries.
Traces: codecommit-merge-conflict-detection-and-resolution#task#15.

=== Cause 4 of 4: CreatePullRequest's same-source-and-destination message omits the identical-tips case ===
AWS: recorded behavior when source and destination resolve to the same branch or to identical tips is 400 with 'The source branch name and the destination branch name are either the same, or the tips of both branches are identical. Make sure that your source branch and destination branch have unique names and that the tips of the branches are different.' — entirely fixed text covering BOTH conditions.
Emulator: 400 with 'The source branch and destination branch for the pull request are the same. You must specify different branches for the source and destination.', which asserts the names are equal even when the real reason is identical tips.
Why: a client whose branches have different names but equal tips is told something untrue about its request and cannot act on it.
Fix: src/ops/create_pull_request.rs.
Traces: codecommit-create-pull-request-error-paths#task#3 (also #task#4, codecommit-pull-request-core-lifecycle#task#14).

## Evidence
Cause 1 — expected HTTP 400 with $.__type 'ManualMergeRequiredException' and $.message 'Current branches cannot be merged, please try to merge using a Git client locally.'; actual HTTP 200 with no error body at all (merge performed).
Cause 2 — status 400 as expected; expected $.message 'Current branches cannot be merged, please try to merge using a Git client locally.', actual 'The pull request cannot be merged automatically into the destination branch. You must manually merge the branches and resolve any conflicts.'
Cause 3 — status 400 as expected; expected $.__type 'InvalidConflictResolutionException' / $.message 'Conflict resolution parameter cannot be used together with NONE conflict resolution strategy'; actual $.__type 'InvalidReplacementTypeException' / $.message 'Automerge was specified for resolving the conflict, but the specified replacement type is not valid.'
Cause 4 — status 400 as expected; expected the two-condition message quoted above, actual the names-are-the-same-only message. 3 steps clustered.
All four expected messages are fixed text with no interpolated values.

## Steps to Reproduce
1. Replay codecommit-branch-merge-strategies#task#18 (MergeBranchesByFastForward over diverged tips) — must be 400 ManualMergeRequiredException, is 200.
2. Replay codecommit-merge-conflict-detection-and-resolution#task#14 (MergeBranchesByThreeWay with unresolved conflicts) and compare $.message.
3. Replay codecommit-merge-conflict-detection-and-resolution#task#15 (CreateUnreferencedMergeCommit with conflictResolution plus conflictResolutionStrategy NONE) and compare $.__type and $.message.
4. Replay codecommit-create-pull-request-error-paths#task#3 (CreatePullRequest with the same source and destination) and compare $.message.
