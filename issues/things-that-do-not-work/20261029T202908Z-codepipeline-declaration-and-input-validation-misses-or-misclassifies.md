Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-create-pipeline-structural-validation-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-create-pipeline-structural-validation-errors codepipeline-update-pipeline-invalid-action-declaration-error codepipeline-update-pipeline-invalid-stage-declaration-error codepipeline-stage-transition-reason-boundary-values codepipeline-webhook-and-tagging-error-surface
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|CreatePipeline|4xx->2xx|$.__type
Issue-Key: codepipeline|CreatePipeline|4xx->2xx|$.message
Issue-Key: codepipeline|CreatePipeline|4xx->2xx|status
Issue-Key: codepipeline|CreatePipeline|4xx->4xx|$.__type
Issue-Key: codepipeline|CreatePipeline|4xx->4xx|$.message
Issue-Key: codepipeline|UpdatePipeline|4xx->2xx|$.__type
Issue-Key: codepipeline|UpdatePipeline|4xx->2xx|$.message
Issue-Key: codepipeline|UpdatePipeline|4xx->2xx|status
Issue-Key: codepipeline|UpdatePipeline|4xx->4xx|$.__type
Issue-Key: codepipeline|UpdatePipeline|4xx->4xx|$.message
Issue-Key: codepipeline|DisableStageTransition|4xx->4xx|$.message
Issue-Key: codepipeline|ListTagsForResource|4xx->4xx|$.message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodePipeline declaration and input validation misses or misclassifies structural rules

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipeline

## Description
CreatePipeline/UpdatePipeline accept declarations AWS refuses, classify two structural refusals under the wrong modeled error, and the generic constraint validator reports a different set of violations than AWS does.

Cause 1 of 6 — a pipeline with a single stage is accepted. AWS: CreatePipeline models InvalidStructureException (Smithy: CreatePipeline errors include InvalidStructureException, client, httpError 400) and refuses a declaration with fewer than two stages with 400 InvalidStructureException and message "Pipeline has only <n> stage(s). There should be a minimum of 2 stages in a pipeline" — fixed text, with the stage count taken from the request. Emulator: 200, pipeline created, no __type and no message. Why: a client's negative test passes and an unusable one-stage pipeline exists in state. Fix: the CreatePipeline structural validator, before state insert. Traces: codepipeline-create-pipeline-structural-validation-errors#task#4.

Cause 2 of 6 — a pipeline whose only non-Source stage has no non-Source action is refused under the wrong error. AWS: 400 InvalidStructureException with message "Pipeline should contain at least 1 action whose category is not Source". Emulator: 400 but __type='InvalidStageDeclarationException' and message 'stage Approval must include at least one action in actions'. Why: a client branching on __type (InvalidStructureException vs InvalidStageDeclarationException) takes the wrong path; both are modeled on CreatePipeline, so the distinction is observable. Fix: ordering/classification in the CreatePipeline structural validator — the "at least one non-Source action" rule is a structure rule, not a per-stage declaration rule. Traces: codepipeline-create-pipeline-structural-validation-errors#task#5 (2 affected steps).

Cause 3 of 6 — UpdatePipeline accepts a stage containing two actions with the same name. AWS: 400 InvalidActionDeclarationException (Smithy: UpdatePipeline errors include InvalidActionDeclarationException) with message "Action name '<actionName>' is used more than once in pipeline stage '<stageName>'" — fixed text with the action and stage names taken from the request. Emulator: 200, and the duplicate action is persisted and the pipeline version bumped. Why: a duplicate-named action is silently stored, so later action addressing (approvals, job dispatch, action executions) is ambiguous. Fix: the UpdatePipeline action-declaration validator, before state mutation. Traces: codepipeline-update-pipeline-invalid-action-declaration-error#task#4.

Cause 4 of 6 — same misclassification as Cause 2 on the update path. AWS: 400 InvalidStructureException with "Pipeline should contain at least 1 action whose category is not Source". Emulator: 400 InvalidStageDeclarationException with 'stage Approval must include at least one action in actions'. Fix: UpdatePipeline's structural validator must share the CreatePipeline classification. Traces: codepipeline-update-pipeline-invalid-stage-declaration-error#task#6.

Cause 5 of 6 — DisableStageTransition reports an extra constraint violation for an empty reason. AWS: reason targets DisabledReason with @length min 1 max 300 and pattern ^[a-zA-Z0-9!@ \(\)\.\*\?\-]+$ (Smithy: DisableStageTransition.reason, required); for an empty string AWS reports exactly ONE violation — "1 validation error detected: Value at 'reason' failed to satisfy constraint: Member must have length greater than or equal to 1". Emulator: 400 with "2 validation errors detected: ...length greater than or equal to 1; ...Member must satisfy regular expression pattern: [a-zA-Z0-9!@ \(\)\.\*\?\-]+". Why: AWS does not evaluate the pattern against a value that already failed the length floor, so a client asserting on the violation count/text mismatches. The counted prefix and the per-member clauses are fixed text; the member name and the constraint values come from the model. Fix: the shared constraint validator's rule — suppress the pattern check for an empty value that already violated the length minimum. Traces: codepipeline-stage-transition-reason-boundary-values#task#1.

Cause 6 of 6 — ListTagsForResource reports only one of two violations for a short malformed ARN. AWS: resourceArn targets ResourceArn with pattern ^arn:aws(-[\w]+)*:codepipeline:.+:[0-9]{12}:.+$ AND a minimum length of 37, and reports BOTH: "2 validation errors detected: Value at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[\w]+)*:codepipeline:.+:[0-9]{12}:.+; Value at 'resourceArn' failed to satisfy constraint: Member must have length greater than or equal to 37". Emulator: 400 with only the pattern clause and the count '1 validation error detected'. Why: the length floor on the ARN is not enforced at all, so values that AWS rejects for length alone are accepted. Fix: add the length constraint to ResourceArn validation and emit the violations in model order. Traces: codepipeline-webhook-and-tagging-error-surface#task#5.

Related but not covering: open issues 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md and 20261029T183239Z-codepipeline-declaration-validation-and-response-shapes-10-net.md; the replay reports owned_by=[] for all six clusters.

## Evidence
codepipeline-create-pipeline-structural-validation-errors#task#4 (CreatePipeline): 'status: expected 400, got 200', '$.__type: missing (expected InvalidStructureException)', "$.message: missing (expected 'Pipeline has only 1 stage(s). There should be a minimum of 2 stages in a pipeline')".
codepipeline-create-pipeline-structural-validation-errors#task#5 (CreatePipeline, 400->400): '$.__type: expected InvalidStructureException, got InvalidStageDeclarationException', "$.message: expected 'Pipeline should contain at least 1 action whose category is not Source', got 'stage Approval must include at least one action in actions'".
codepipeline-update-pipeline-invalid-action-declaration-error#task#4 (UpdatePipeline): 'status: expected 400, got 200', '$.__type: missing (expected InvalidActionDeclarationException)', "$.message: missing (expected \"Action name 'SourceAction' is used more than once in pipeline stage 'Source'\")".
codepipeline-update-pipeline-invalid-stage-declaration-error#task#6 (UpdatePipeline, 400->400): same misclassification as above.
codepipeline-stage-transition-reason-boundary-values#task#1 (DisableStageTransition, 400->400): expected '1 validation error detected: ... length greater than or equal to 1' / got '2 validation errors detected: ... ; ... regular expression pattern: [a-zA-Z0-9!@ \(\)\.\*\?\-]+'.
codepipeline-webhook-and-tagging-error-surface#task#5 (ListTagsForResource, 400->400): expected '2 validation errors detected: ... pattern ...; ... length greater than or equal to 37' / got '1 validation error detected: ... pattern ...'.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus.
2. CreatePipeline with exactly one stage -> expect 400 InvalidStructureException, observe 200.
3. CreatePipeline (and UpdatePipeline) where no stage holds a non-Source action -> expect InvalidStructureException, observe InvalidStageDeclarationException.
4. UpdatePipeline with two identically named actions in one stage -> expect 400 InvalidActionDeclarationException, observe 200.
5. DisableStageTransition with reason="" and ListTagsForResource with a short malformed ARN -> compare the validation-error counts and clauses.
6. Re-replay codepipeline-create-pipeline-structural-validation-errors#task#4/#task#5, codepipeline-update-pipeline-invalid-action-declaration-error#task#4, codepipeline-update-pipeline-invalid-stage-declaration-error#task#6, codepipeline-stage-transition-reason-boundary-values#task#1 and codepipeline-webhook-and-tagging-error-surface#task#5.
