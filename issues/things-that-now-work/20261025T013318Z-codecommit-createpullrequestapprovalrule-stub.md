Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.author-cannot-approve-own-pull-request-rejected codecommit.e2e-pull-request-approval-and-merge-flow codecommit.pull-request-approval-state-transitions codecommit.pull-request-approval-states-reflect-rule-evaluation codecommit.pull-request-override-state-reflects-override
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreatePullRequestApprovalRule|NotImplemented
Issue-Commit: d6e9bd85505b2fcf81ff15dd47c3206271b87270

# Implement codecommit CreatePullRequestApprovalRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d6e9bd85505b2fcf81ff15dd47c3206271b87270`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePullRequestApprovalRule

## Description
`CreatePullRequestApprovalRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_pull_request_approval_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreatePullRequestApprovalRule` call, so each one unblocks only when `CreatePullRequestApprovalRule` works: `codecommit.author-cannot-approve-own-pull-request-rejected`, `codecommit.e2e-pull-request-approval-and-merge-flow`, `codecommit.pull-request-approval-state-transitions`, `codecommit.pull-request-approval-states-reflect-rule-evaluation`, `codecommit.pull-request-override-state-reflects-override`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_pull_request_approval_rule.rs`. Replaying the corpus, the emulator refused the `CreatePullRequestApprovalRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePullRequestApprovalRule operation: operation CreatePullRequestApprovalRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codecommit emulator
Call CreatePullRequestApprovalRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
