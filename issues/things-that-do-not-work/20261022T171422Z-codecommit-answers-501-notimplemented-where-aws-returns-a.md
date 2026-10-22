Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.list-approval-rule-templates-invalid-continuation-token-rejected codecommit.list-approval-rule-templates-invalid-max-results-rejected codecommit.associate-nonexistent-approval-rule-template-blocked codecommit.approval-rule-template-invalid-content-rejected codecommit.create-commit-branch-does-not-exist-rejected codecommit.get-commit-unknown-id-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.list-approval-rule-templates-invalid-continuation-token-rejected|run#4|AssertionError|assert code == "InvalidContinuationTokenException", f"unexpected code: {code}"
Issue-Key: codecommit.list-approval-rule-templates-invalid-max-results-rejected|run#3|AssertionError|assert code == "InvalidMaxResultsException", f"unexpected code: {code}"
Issue-Key: codecommit.associate-nonexistent-approval-rule-template-blocked|run#7|AssertionError|assert code == "ApprovalRuleTemplateDoesNotExistException", (
Issue-Key: codecommit.approval-rule-template-invalid-content-rejected|run#7|AssertionError|assert code == "InvalidApprovalRuleTemplateContentException", (
Issue-Key: codecommit.create-commit-branch-does-not-exist-rejected|run#9|AssertionError|assert code == "BranchDoesNotExistException", code
Issue-Key: codecommit.get-commit-unknown-id-rejected|run#13|AssertionError|assert code == "CommitIdDoesNotExistException", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeCommit answers 501 NotImplemented where AWS returns a modeled 400 rejection (5 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApprovalRuleTemplates

## Description
Five causes, all the same shape: a request real AWS refuses with a specific modeled client error instead gets the scaffold's 'NotImplemented' answer, so the caller cannot distinguish bad input from an absent feature. Triage note: the per-operation stub issues filed from the checkout name CreateApprovalRuleTemplate, CreateCommit and GetCommit as unimplemented stubs; causes 3-5 are recorded here because these checks are not on those issues' replay lists and because the requirement they encode is the specific error code, not merely that the handler exists.

Cause 1 of 5: ListApprovalRuleTemplates performs no pagination-parameter validation.
AWS: Smithy ListApprovalRuleTemplates models exactly two errors, InvalidContinuationTokenException (client, httpError 400) and InvalidMaxResultsException (client, 400); the op is @paginated (inputToken nextToken, pageSize maxResults). A garbage nextToken is refused with InvalidContinuationTokenException (recorded message example 'Error parsing next token: not-a-real-continuation-token') and maxResults=0 or -1 with InvalidMaxResultsException (recorded message example 'A valid limit is between 1 and 1000'); omitting both succeeds with 200 and an approvalRuleTemplateNames list.
Emulator: both calls come back with code 'NotImplemented'.
Why: a paginating client cannot tell a corrupt/expired token from an unsupported call, and retries forever instead of failing fast; the positive control (no parameters) is unreachable too.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/list_approval_rule_templates.rs plus its input validation.
Traces: codecommit.list-approval-rule-templates-invalid-continuation-token-rejected, codecommit.list-approval-rule-templates-invalid-max-results-rejected.

Cause 2 of 5: AssociateApprovalRuleTemplateWithRepository does not reject an unknown template.
AWS: Smithy models ApprovalRuleTemplateDoesNotExistException (client, 400) on AssociateApprovalRuleTemplateWithRepository; associating a template name that was never created is refused with it, and the repository gains no association.
Emulator: code 'NotImplemented'.
Why: a caller wiring approval rules onto a repository cannot tell a typo'd template name from an unavailable API, and the association state is unverifiable.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/associate_approval_rule_template_with_repository.rs.
Traces: codecommit.associate-nonexistent-approval-rule-template-blocked.

Cause 3 of 5: CreateApprovalRuleTemplate does not reject malformed template content.
AWS: Smithy models InvalidApprovalRuleTemplateContentException (client, 400) on CreateApprovalRuleTemplate, whose approvalRuleTemplateContent is @required with @length min 1 max 3000. Content that is not valid approval-rule JSON (e.g. '{ this is not valid approval rule template JSON ') is refused with that code and no template is created.
Emulator: code 'NotImplemented'.
Why: malformed policy content is silently indistinguishable from an unimplemented API; a deployment pipeline cannot validate its templates.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_approval_rule_template.rs and its content validation.
Traces: codecommit.approval-rule-template-invalid-content-rejected.

Cause 4 of 5: CreateCommit does not reject a branch that does not exist.
AWS: Smithy models BranchDoesNotExistException (client, 400) on CreateCommit. A CreateCommit naming a branch that was never created is refused with it and no commit is made.
Emulator: code 'NotImplemented'.
Why: the whole commit path is unusable and its principal negative class is unobservable.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_commit.rs.
Traces: codecommit.create-commit-branch-does-not-exist-rejected.

Cause 5 of 5: GetCommit does not reject an unknown commit id.
AWS: Smithy models CommitIdDoesNotExistException (client, 400) on GetCommit (commitId is @required, target ObjectId). A well-formed 40-hex id that exists in no repository is refused with that code (recorded message example 'Could not retrieve <commitId>').
Emulator: code 'NotImplemented'.
Why: a client that walks history or verifies a commit id it was handed cannot distinguish "no such commit" from "API missing".
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/get_commit.rs.
Traces: codecommit.get-commit-unknown-id-rejected.

## Evidence
Cause 1 — expected 400 InvalidContinuationTokenException; actual AssertionError 'unexpected code: NotImplemented'. Expected 400 InvalidMaxResultsException for maxResults=0 and -1; actual 'unexpected code: NotImplemented'.
Cause 2 — expected ApprovalRuleTemplateDoesNotExistException; actual AssertionError "expected ApprovalRuleTemplateDoesNotExistException, got 'NotImplemented'".
Cause 3 — expected InvalidApprovalRuleTemplateContentException; actual AssertionError "expected InvalidApprovalRuleTemplateContentException for malformed content, got 'NotImplemented'".
Cause 4 — expected BranchDoesNotExistException; actual AssertionError 'NotImplemented'.
Cause 5 — expected CommitIdDoesNotExistException; actual AssertionError "expected CommitIdDoesNotExistException for unknown id '0d0c1f049b4e446ac20e9b10824eb5151f3ab290', got 'NotImplemented'".
The stub responses carry HTTP 501 with message 'operation <Name> is scaffolded but not implemented in this emulator'; AWS answers HTTP 400 with the codes above.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc.list_approval_rule_templates(nextToken='not-a-real-continuation-token') -> expect 400 InvalidContinuationTokenException; observe NotImplemented. Same with maxResults=0 / -1 -> expect InvalidMaxResultsException.
3. cc.create_repository(repositoryName=R); cc.associate_approval_rule_template_with_repository(approvalRuleTemplateName='ghost', repositoryName=R) -> expect 400 ApprovalRuleTemplateDoesNotExistException; observe NotImplemented.
4. cc.create_approval_rule_template(approvalRuleTemplateName='bad', approvalRuleTemplateContent='{ this is not valid approval rule template JSON ') -> expect 400 InvalidApprovalRuleTemplateContentException; observe NotImplemented.
5. cc.create_commit(repositoryName=R, branchName='no-such-branch', putFiles=[...]) -> expect 400 BranchDoesNotExistException; observe NotImplemented.
6. cc.get_commit(repositoryName=R, commitId='0d0c1f049b4e446ac20e9b10824eb5151f3ab290') -> expect 400 CommitIdDoesNotExistException; observe NotImplemented.
7. Re-replay the check ids in trace_ids.
