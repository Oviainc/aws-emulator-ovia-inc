Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.approval-rule-name-conflict-rejected codecommit.approval-rule-template-auto-attaches-to-new-pull-requests codecommit.author-cannot-approve-own-pull-request-rejected codecommit.branch-create-get-list codecommit.branch-delete-then-get-not-found codecommit.branch-protection-policy-denies-a-real-git-push-to-main codecommit.comment-lifecycle-delete-then-get-returns-deleted-state codecommit.comment-reaction-add-and-get-round-trip codecommit.comment-reaction-round-trip codecommit.concurrent-merge-after-branch-moved-rejected codecommit.concurrent-merge-to-same-branch-conflict-detected codecommit.create-pull-request-missing-token-rejected codecommit.delete-default-branch-rejected codecommit.describe-merge-conflicts-detail-matches-summary codecommit.describe-merge-conflicts-detects-real-conflict codecommit.e2e-pull-request-merge-flow codecommit.e2e-repository-branch-pr-merge-flow codecommit.fast-forward-merge-refused-on-diverged-branches codecommit.get-merge-conflicts-detects-conflicting-edits codecommit.get-merge-conflicts-reports-conflicting-paths codecommit.get-merge-options-and-conflicts-consistent codecommit.get-merge-options-reflects-mergeability codecommit.get-merge-options-returns-available-strategies codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch codecommit.merge-branch-tag-name-rejected codecommit.merge-of-a-closed-pull-request-rejected codecommit.merge-pull-request-concurrent-reference-update-conflict codecommit.merge-pull-request-refused-when-approval-rules-not-satisfied codecommit.merge-pull-request-squash-with-unresolved-conflicts-fails codecommit.merge-pull-request-with-conflicts-rejected codecommit.merge-pull-request-with-unmet-approval-rules-blocked codecommit.merge-rejected-when-approval-rules-not-satisfied codecommit.pull-request-approval-state-transitions codecommit.pull-request-approval-states-reflect-rule-evaluation codecommit.pull-request-comment-thread-anchors-to-the-diff-blob-ids codecommit.pull-request-empty-description-normalized-to-absent codecommit.pull-request-events-chronological-ordering codecommit.pull-request-override-state-reflects-override codecommit.pull-request-reopen-rejected codecommit.pull-request-status-lifecycle-transitions codecommit.pull-request-status-reopen-rejected codecommit.put-comment-reaction-get-reactions-reflects codecommit.reference-condition-key-denies-api-writes-to-a-protected-branch codecommit.squash-merge-collapses-history-but-keeps-the-three-way-tree codecommit.three-way-merge-then-conflict-resolved-succeeds codecommit.unreferenced-merge-commit-accept-source-tree codecommit.update-pull-request-title-after-close-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreateBranch|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codecommit CreateBranch: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBranch

## Description
`CreateBranch` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_branch.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

47 recorded unit(s) fail on their `CreateBranch` call, so each one unblocks only when `CreateBranch` works: `codecommit.approval-rule-name-conflict-rejected`, `codecommit.approval-rule-template-auto-attaches-to-new-pull-requests`, `codecommit.author-cannot-approve-own-pull-request-rejected`, `codecommit.branch-create-get-list`, `codecommit.branch-delete-then-get-not-found`, `codecommit.branch-protection-policy-denies-a-real-git-push-to-main`, `codecommit.comment-lifecycle-delete-then-get-returns-deleted-state`, `codecommit.comment-reaction-add-and-get-round-trip`, `codecommit.comment-reaction-round-trip`, `codecommit.concurrent-merge-after-branch-moved-rejected`, `codecommit.concurrent-merge-to-same-branch-conflict-detected`, `codecommit.create-pull-request-missing-token-rejected`, `codecommit.delete-default-branch-rejected`, `codecommit.describe-merge-conflicts-detail-matches-summary`, `codecommit.describe-merge-conflicts-detects-real-conflict`, `codecommit.e2e-pull-request-merge-flow`, `codecommit.e2e-repository-branch-pr-merge-flow`, `codecommit.fast-forward-merge-refused-on-diverged-branches`, `codecommit.get-merge-conflicts-detects-conflicting-edits`, `codecommit.get-merge-conflicts-reports-conflicting-paths` (+27 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_branch.rs`. Replaying the corpus, the emulator refused the `CreateBranch` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateBranch operation: operation CreateBranch is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codecommit emulator
Call CreateBranch and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
