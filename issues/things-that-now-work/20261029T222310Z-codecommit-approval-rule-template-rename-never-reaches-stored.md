Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-approval-rule-template-lifecycle-and-batch-association
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-approval-rule-template-lifecycle-and-batch-association
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetApprovalRuleTemplate|2xx->4xx|$.approvalRuleTemplate
Issue-Key: codecommit|GetApprovalRuleTemplate|2xx->4xx|status
Issue-Key: codecommit|ListApprovalRuleTemplates|2xx->2xx|$.approvalRuleTemplateNames[*]
Issue-Key: codecommit|AssociateApprovalRuleTemplateWithRepository|2xx->4xx|$
Issue-Key: codecommit|AssociateApprovalRuleTemplateWithRepository|2xx->4xx|status
Issue-Key: codecommit|ListAssociatedApprovalRuleTemplatesForRepository|2xx->2xx|$.approvalRuleTemplateNames[*]
Issue-Key: codecommit|ListRepositoriesForApprovalRuleTemplate|2xx->4xx|$.repositoryNames
Issue-Key: codecommit|ListRepositoriesForApprovalRuleTemplate|2xx->4xx|status
Issue-Key: codecommit|BatchAssociateApprovalRuleTemplateWithRepositories|2xx->4xx|$.associatedRepositoryNames
Issue-Key: codecommit|BatchAssociateApprovalRuleTemplateWithRepositories|2xx->4xx|$.errors
Issue-Key: codecommit|BatchAssociateApprovalRuleTemplateWithRepositories|2xx->4xx|status
Issue-Key: codecommit|DisassociateApprovalRuleTemplateFromRepository|2xx->4xx|$
Issue-Key: codecommit|DisassociateApprovalRuleTemplateFromRepository|2xx->4xx|status
Issue-Key: codecommit|CreateApprovalRuleTemplate|4xx->2xx|$.__type
Issue-Key: codecommit|CreateApprovalRuleTemplate|4xx->2xx|$.message
Issue-Key: codecommit|CreateApprovalRuleTemplate|4xx->2xx|status
Issue-Key: codecommit|DeleteApprovalRuleTemplate|2xx->4xx|$.approvalRuleTemplateId
Issue-Key: codecommit|DeleteApprovalRuleTemplate|2xx->4xx|status
Issue-Key: codecommit|GetApprovalRuleTemplate|4xx->2xx|$.__type
Issue-Key: codecommit|GetApprovalRuleTemplate|4xx->2xx|$.message
Issue-Key: codecommit|GetApprovalRuleTemplate|4xx->2xx|status
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit approval-rule-template rename never reaches stored state, breaking the whole template lifecycle (10 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApprovalRuleTemplate

## Description
TEN net-new causes in the approval-rule-template surface. They share one fault: renaming a template does not mutate the stored template, so after a rename the emulator's registry still holds the OLD name and every subsequent operation addressed by the NEW name fails, while operations addressed by the old name still succeed when they should not. A fixer must close ALL TEN before submitting. Related but NOT covering: the open behavioral issue '20261029T180833Z-codecommit-9-net-new-behavioral-root-causes-updateapprovalruletemplatename' reports UpdateApprovalRuleTemplateName answering 501; that issue carries no wire replay key, so these ten wire steps are unowned and the rename must additionally be made to PERSIST, not merely to answer 200.

Shared AWS grounding: CodeCommit approval rule templates are addressed BY NAME. UpdateApprovalRuleTemplateName replaces the stored approvalRuleTemplateName in place, preserving approvalRuleTemplateId, content and ruleContentSha256; after it, GetApprovalRuleTemplate/ListApprovalRuleTemplates/Associate*/Disassociate*/Delete* all resolve the new name and the old name resolves to ApprovalRuleTemplateDoesNotExistException. Smithy models ApprovalRuleTemplateDoesNotExistException and ApprovalRuleTemplateNameAlreadyExistsException as client errors, httpError 400 [defaulted].
Emulator: the rename leaves state untouched, so the new name is unknown and the old name is still live.
Why: a client that renames a template and then associates it with repositories (the documented workflow) gets ApprovalRuleTemplateDoesNotExistException for every follow-up call, cannot delete the template, and can silently create a SECOND template with the old name — the template registry diverges from what the client believes it created.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/update_approval_rule_template_name.rs plus the template registry in the crate's state module — the rename must re-key the stored template so name lookups in get/list/associate/disassociate/delete follow it.

=== Cause 1 of 10: GetApprovalRuleTemplate by the new name -> 400 instead of 200 ===
AWS returns 200 with the full approvalRuleTemplate (id, name = new name, content, description, creationDate, lastModifiedDate, lastModifiedUser, ruleContentSha256). Emulator returns 400.
Traces: codecommit-approval-rule-template-lifecycle-and-batch-association#task#10.
=== Cause 2 of 10: ListApprovalRuleTemplates still lists the OLD name ===
AWS lists the renamed template only. Emulator lists the pre-rename name.
Traces: ...#task#11.
=== Cause 3 of 10: AssociateApprovalRuleTemplateWithRepository by the new name -> 400 ===
AWS returns 200 with an empty body. Emulator returns 400 ApprovalRuleTemplateDoesNotExistException.
Traces: ...#task#12.
=== Cause 4 of 10: ListAssociatedApprovalRuleTemplatesForRepository returns no template ===
AWS lists the renamed template for the repository. Emulator returns an empty list (consequence of Cause 3).
Traces: ...#task#13.
=== Cause 5 of 10: ListRepositoriesForApprovalRuleTemplate -> 400 instead of the repository list ===
Traces: ...#task#14 (also #task#16).
=== Cause 6 of 10: BatchAssociateApprovalRuleTemplateWithRepositories -> 400 instead of 200 with associatedRepositoryNames and an empty errors list ===
Traces: ...#task#15.
=== Cause 7 of 10: DisassociateApprovalRuleTemplateFromRepository -> 400 instead of 200 empty body ===
Traces: ...#task#18.
=== Cause 8 of 10: CreateApprovalRuleTemplate with the pre-rename name SUCCEEDS (200) where AWS rejects it ===
AWS: 400 ApprovalRuleTemplateNameAlreadyExistsException, message 'The approval rule template cannot be created because a template with that name already exists in your AWS account.' (fixed text). Emulator: 200, creating a duplicate — because the stored template still occupies that name, the conflict check and the rename disagree about which name is taken.
Traces: ...#task#19.
=== Cause 9 of 10: DeleteApprovalRuleTemplate by the new name -> 400 instead of 200 with approvalRuleTemplateId ===
Traces: ...#task#26.
=== Cause 10 of 10: GetApprovalRuleTemplate after the delete returns 200 instead of 400 ===
AWS: 400 ApprovalRuleTemplateDoesNotExistException, message 'No approval rule template exists in this AWS Region with the following name: <approvalRuleTemplateName>. Make sure that you are signed in to the AWS Region where you created the rule, and then try again.' — fixed text with the requested template name as its one variable part. Emulator: 200 (the template was never deleted, per Cause 9).
Traces: ...#task#27.

## Evidence
Cause 1 — expected 200 with $.approvalRuleTemplate populated (approvalRuleTemplateName = the renamed name, plus id/content/description/dates/lastModifiedUser/ruleContentSha256); actual 400, $.approvalRuleTemplate absent.
Cause 2 — expected $.approvalRuleTemplateNames[0] = the renamed name; actual the pre-rename name.
Causes 3, 5, 6, 7, 9 — expected 200 (empty body / repositoryNames / associatedRepositoryNames+errors:[] / empty body / approvalRuleTemplateId); actual 400 with {'__type': 'ApprovalRuleTemplateDoesNotExistException', 'message': 'No approval rule template exists in this AWS Region with the following name: <renamed name>. ...'}.
Cause 4 — expected $.approvalRuleTemplateNames[0] = the renamed name; actual None.
Cause 8 — expected 400 __type ApprovalRuleTemplateNameAlreadyExistsException with message 'The approval rule template cannot be created because a template with that name already exists in your AWS account.'; actual 200 with no error at all.
Cause 10 — expected 400 __type ApprovalRuleTemplateDoesNotExistException; actual 200.
In every message above the template NAME is the request-supplied variable part; the rest of each message is fixed text. Template ids, SHA-256 digests and timestamps in the recording are per-run values and are not requirements.

## Steps to Reproduce
1. Replay fixture codecommit-approval-rule-template-lifecycle-and-batch-association end to end: it creates a template, renames it (UpdateApprovalRuleTemplateName), then exercises get/list/associate/batch-associate/list-repositories/disassociate/create-duplicate/delete/get-after-delete by the NEW name.
2. Observe steps task#10, task#11, task#12, task#13, task#14, task#15, task#18, task#19, task#26, task#27.
3. Each either answers 400 ApprovalRuleTemplateDoesNotExistException for the new name, or succeeds for the old name, proving the rename never reached stored state.
