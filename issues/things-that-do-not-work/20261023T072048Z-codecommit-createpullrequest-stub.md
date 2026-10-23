Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.approval-rule-content-required-rejected codecommit.approval-rule-name-conflict-rejected codecommit.approval-rule-template-auto-applies-to-new-pull-requests codecommit.approval-rule-template-auto-attaches-to-new-pull-requests codecommit.author-cannot-approve-own-pull-request-rejected codecommit.cannot-delete-approval-rule-from-template-blocked codecommit.comment-lifecycle-delete-then-get-returns-deleted-state codecommit.comment-reaction-add-and-get-round-trip codecommit.comment-reaction-round-trip codecommit.concurrent-merge-after-branch-moved-rejected codecommit.concurrent-merge-to-same-branch-conflict-detected codecommit.delete-template-derived-approval-rule-rejected codecommit.delete-template-derived-pr-approval-rule-blocked codecommit.e2e-approval-rule-template-governs-new-repository codecommit.e2e-merge-pull-request-approval-rules-not-satisfied-rejected codecommit.e2e-pull-request-approval-and-merge-flow codecommit.e2e-pull-request-merge-flow codecommit.e2e-repository-branch-pr-merge-flow codecommit.merge-of-a-closed-pull-request-rejected codecommit.merge-pull-request-concurrent-reference-update-conflict codecommit.merge-pull-request-refused-when-approval-rules-not-satisfied codecommit.merge-pull-request-squash-with-unresolved-conflicts-fails codecommit.merge-pull-request-with-conflicts-rejected codecommit.merge-pull-request-with-unmet-approval-rules-blocked codecommit.merge-rejected-when-approval-rules-not-satisfied codecommit.pull-request-approval-rule-lifecycle codecommit.pull-request-approval-state-transitions codecommit.pull-request-approval-states-reflect-rule-evaluation codecommit.pull-request-comment-thread-anchors-to-the-diff-blob-ids codecommit.pull-request-empty-description-normalized-to-absent codecommit.pull-request-events-chronological-ordering codecommit.pull-request-lifecycle-create-merge-close codecommit.pull-request-override-state-reflects-override codecommit.pull-request-reopen-after-close-rejected codecommit.pull-request-reopen-rejected codecommit.pull-request-status-lifecycle-transitions codecommit.pull-request-status-reopen-rejected codecommit.put-comment-reaction-get-reactions-reflects codecommit.update-pull-request-title-after-close-rejected codecommit.update-title-on-closed-pull-request-rejected codecommit__events__sqs.pull-request-lifecycle-reaches-eventbridge-scoped-to-one-repository
Issue-Key: codecommit|CreatePullRequest|NotImplemented
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# Implement codecommit CreatePullRequest: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePullRequest

## Description
`CreatePullRequest` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_pull_request.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

41 recorded unit(s) fail on their `CreatePullRequest` call, so each one unblocks only when `CreatePullRequest` works: `codecommit.approval-rule-content-required-rejected`, `codecommit.approval-rule-name-conflict-rejected`, `codecommit.approval-rule-template-auto-applies-to-new-pull-requests`, `codecommit.approval-rule-template-auto-attaches-to-new-pull-requests`, `codecommit.author-cannot-approve-own-pull-request-rejected`, `codecommit.cannot-delete-approval-rule-from-template-blocked`, `codecommit.comment-lifecycle-delete-then-get-returns-deleted-state`, `codecommit.comment-reaction-add-and-get-round-trip`, `codecommit.comment-reaction-round-trip`, `codecommit.concurrent-merge-after-branch-moved-rejected`, `codecommit.concurrent-merge-to-same-branch-conflict-detected`, `codecommit.delete-template-derived-approval-rule-rejected`, `codecommit.delete-template-derived-pr-approval-rule-blocked`, `codecommit.e2e-approval-rule-template-governs-new-repository`, `codecommit.e2e-merge-pull-request-approval-rules-not-satisfied-rejected`, `codecommit.e2e-pull-request-approval-and-merge-flow`, `codecommit.e2e-pull-request-merge-flow`, `codecommit.e2e-repository-branch-pr-merge-flow`, `codecommit.merge-of-a-closed-pull-request-rejected`, `codecommit.merge-pull-request-concurrent-reference-update-conflict` (+21 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_pull_request.rs`. Replaying the corpus, the emulator refused the `CreatePullRequest` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePullRequest operation: operation CreatePullRequest is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codecommit emulator
Call CreatePullRequest and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
