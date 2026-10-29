Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-approval-rule-content-validation-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-approval-rule-content-validation-errors codecommit-approval-rule-crud-against-nonexistent-pull-request codecommit-approval-rule-crud-malformed-pull-request-id codecommit-approval-rule-pull-request-id-errors codecommit-approval-rule-template-lifecycle-and-batch-association
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreatePullRequestApprovalRule|4xx->4xx|$.message
Issue-Key: codecommit|UpdateApprovalRuleTemplateContent|4xx->4xx|$.message
Issue-Key: codecommit|BatchAssociateApprovalRuleTemplateWithRepositories|2xx->2xx|$.errors[*].errorMessage
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit approval-rule and template validation messages use the wrong noun, hash wording and batch error text (3 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePullRequestApprovalRule

## Description
THREE net-new causes, all message-template defects in the approval-rule/template surface: the correct status and error code are returned but the message text is not AWS's. A fixer must close ALL THREE before submitting.

=== Cause 1 of 3: CreatePullRequestApprovalRule's invalid-JSON message says 'approval rule template' where AWS says 'approval rule' ===
AWS: Smithy CreatePullRequestApprovalRule models InvalidApprovalRuleContentException (client, httpError 400 [defaulted]). Recorded behavior: for approvalRuleContent that is not valid JSON, AWS answers 400 with message 'The JSON in the approval rule is not valid. Review the JSON and make sure it is valid, and then try again.' — entirely fixed text; the subject is the approval RULE, because this operation attaches a rule to a pull request, not a template.
Emulator: 400 with 'The JSON in the approval rule template is not valid. Review the JSON and make sure it is valid, and then try again.' — the TEMPLATE variant of the message is being reused for the rule operation. Hypothesis: one shared message constant serves both the template and the rule code paths.
Why: the two messages distinguish which object a client sent bad JSON for; clients and operators triaging on the message are pointed at a template that was never part of the request.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_pull_request_approval_rule.rs — use the rule-specific message constant.
Traces: codecommit-approval-rule-content-validation-errors#task#1 (also #task#2 and four more steps across codecommit-approval-rule-crud-against-nonexistent-pull-request / -malformed-pull-request-id / codecommit-approval-rule-pull-request-id-errors).

=== Cause 2 of 3: UpdateApprovalRuleTemplateContent's SHA-256 mismatch message is the emulator's own prose ===
AWS: models InvalidRuleContentSha256Exception (client, 400). Recorded behavior: when existingRuleContentSha256 does not match, AWS answers 400 with 'The SHA-256 hash for the current rule content does not match the hash you provided: <existingRuleContentSha256>. Use GetApprovalRuleTemplate to get the SHA-256 of the current approval rule.' — fixed text whose one variable part is the hash the CLIENT supplied; AWS does NOT disclose the stored hash.
Emulator: 400 with a lowercase, differently-structured message that names the template and discloses the STORED digest: 'the SHA-256 hash signature for the rule content is not valid: the content stored for <templateName> has signature <storedSha256>'.
Why: the message loses the documented remediation (call GetApprovalRuleTemplate) and inverts which hash is echoed, so a client that parses the echoed hash to retry reads the server's value as if it were its own.
Fix: src/ops/update_approval_rule_template_content.rs — echo the request's existingRuleContentSha256 in AWS's template.
Traces: codecommit-approval-rule-template-lifecycle-and-batch-association#task#20.

=== Cause 3 of 3: BatchAssociateApprovalRuleTemplateWithRepositories per-item errorMessage is a bare identifier pair ===
AWS: the operation returns 200 with an `errors` list of BatchAssociateApprovalRuleTemplateWithRepositoriesError entries. Recorded behavior: for a repository name that does not exist the entry's errorMessage is 'No repository with the name <repositoryName> was found. Please specify a valid repository name' — fixed text with the requested repository name as its only variable part.
Emulator: errorMessage '<repositoryName> for <accountId>', the internal not-found formatting with no sentence at all.
Why: batch APIs surface per-item failures only through errorMessage; a client logging or displaying it gets an unintelligible token pair instead of the documented reason.
Fix: src/ops/batch_associate_approval_rule_template_with_repositories.rs — format per-item repository-not-found errors with AWS's sentence.
Traces: codecommit-approval-rule-template-lifecycle-and-batch-association#task#21.

## Evidence
Cause 1 — status 400 as expected; expected $.message 'The JSON in the approval rule is not valid. Review the JSON and make sure it is valid, and then try again.', actual 'The JSON in the approval rule template is not valid. Review the JSON and make sure it is valid, and then try again.' (one word inserted). 6 fixture steps clustered.
Cause 2 — status 400 as expected; expected $.message 'The SHA-256 hash for the current rule content does not match the hash you provided: <client hash>. Use GetApprovalRuleTemplate to get the SHA-256 of the current approval rule.', actual 'the SHA-256 hash signature for the rule content is not valid: the content stored for <template name> has signature <server hash>'. The hash digits themselves are per-run; the template and which side's hash is echoed are the requirement.
Cause 3 — status 200 as expected; expected $.errors[0].errorMessage 'No repository with the name <repositoryName> was found. Please specify a valid repository name', actual '<repositoryName> for <accountId>'.

## Steps to Reproduce
1. Replay codecommit-approval-rule-content-validation-errors#task#1 (CreatePullRequestApprovalRule with malformed approvalRuleContent JSON) and compare $.message.
2. Replay codecommit-approval-rule-template-lifecycle-and-batch-association#task#20 (UpdateApprovalRuleTemplateContent with a wrong existingRuleContentSha256) and compare $.message.
3. Replay codecommit-approval-rule-template-lifecycle-and-batch-association#task#21 (BatchAssociateApprovalRuleTemplateWithRepositories including a nonexistent repository name) and compare $.errors[0].errorMessage.
