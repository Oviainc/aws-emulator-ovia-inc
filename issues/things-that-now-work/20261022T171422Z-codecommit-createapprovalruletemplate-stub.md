Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.approval-rule-template-association-lifecycle codecommit.approval-rule-template-auto-applies-to-new-pull-requests codecommit.approval-rule-template-content-preserved-verbatim codecommit.approval-rule-template-content-update-preserved codecommit.approval-rule-template-delete-in-use-rejected codecommit.approval-rule-template-empty-description-normalized-to-absent codecommit.approval-rule-template-in-use-delete-blocked-then-allowed codecommit.approval-rule-template-lifecycle codecommit.approval-rule-template-name-already-exists-rejected codecommit.approval-rule-template-name-conflict-rejected codecommit.approval-rule-template-repository-association-lifecycle codecommit.batch-associate-approval-rule-template-partial-failure codecommit.cannot-delete-approval-rule-from-template-blocked codecommit.create-approval-rule-template-duplicate-name-rejected codecommit.delete-approval-rule-template-in-use-blocked codecommit.delete-template-derived-approval-rule-rejected codecommit.delete-template-derived-pr-approval-rule-blocked codecommit.e2e-approval-rule-template-governs-new-repository codecommit.invalid-approval-rule-template-name-rejected codecommit.malformed-approval-rule-content-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreateApprovalRuleTemplate|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codecommit CreateApprovalRuleTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApprovalRuleTemplate

## Description
`CreateApprovalRuleTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_approval_rule_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `CreateApprovalRuleTemplate` call, so each one unblocks only when `CreateApprovalRuleTemplate` works: `codecommit.approval-rule-template-association-lifecycle`, `codecommit.approval-rule-template-auto-applies-to-new-pull-requests`, `codecommit.approval-rule-template-content-preserved-verbatim`, `codecommit.approval-rule-template-content-update-preserved`, `codecommit.approval-rule-template-delete-in-use-rejected`, `codecommit.approval-rule-template-empty-description-normalized-to-absent`, `codecommit.approval-rule-template-in-use-delete-blocked-then-allowed`, `codecommit.approval-rule-template-lifecycle`, `codecommit.approval-rule-template-name-already-exists-rejected`, `codecommit.approval-rule-template-name-conflict-rejected`, `codecommit.approval-rule-template-repository-association-lifecycle`, `codecommit.batch-associate-approval-rule-template-partial-failure`, `codecommit.cannot-delete-approval-rule-from-template-blocked`, `codecommit.create-approval-rule-template-duplicate-name-rejected`, `codecommit.delete-approval-rule-template-in-use-blocked`, `codecommit.delete-template-derived-approval-rule-rejected`, `codecommit.delete-template-derived-pr-approval-rule-blocked`, `codecommit.e2e-approval-rule-template-governs-new-repository`, `codecommit.invalid-approval-rule-template-name-rejected`, `codecommit.malformed-approval-rule-content-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_approval_rule_template.rs`. Replaying the corpus, the emulator refused the `CreateApprovalRuleTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateApprovalRuleTemplate operation: operation CreateApprovalRuleTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codecommit emulator
Call CreateApprovalRuleTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
