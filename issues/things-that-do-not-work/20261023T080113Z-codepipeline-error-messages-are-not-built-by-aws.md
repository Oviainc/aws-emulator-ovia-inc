Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.pipeline-create-active-update-delete codepipeline.pipeline-lifecycle-create-active-delete codepipeline.pipeline-lifecycle-delete-then-404 codepipeline.pipeline-name-unicode-round-trip codepipeline.update-pipeline-version-history-stale-read codepipeline.get-pipeline-execution-unknown-execution-id-rejected codepipeline.disable-stage-transition-invalid-reason-rejected codepipeline.disable-stage-transition-unknown-stage-not-permissive codepipeline.list-pipeline-executions-invalid-next-token-rejected codepipeline.list-rule-executions-invalid-next-token-rejected codepipeline.create-custom-action-type-too-many-tags-fails codepipeline.deregister-webhook-validation-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.pipeline-create-active-update-delete|run#33|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal}"
Issue-Key: codepipeline.pipeline-lifecycle-create-active-delete|run#25|AssertionError|assert get_refusal == expected, (
Issue-Key: codepipeline.pipeline-lifecycle-delete-then-404|run#13|AssertionError|assert get_refusal == read_expected, f"expected GetPipeline on deleted pipeline to raise {read_expected!r}, got {get_refusal!r}"
Issue-Key: codepipeline.pipeline-name-unicode-round-trip|run#20|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: codepipeline.update-pipeline-version-history-stale-read|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codepipeline.get-pipeline-execution-unknown-execution-id-rejected|run#13|AssertionError|assert refusal == (
Issue-Key: codepipeline.disable-stage-transition-invalid-reason-rejected|run#11|AssertionError|assert message == f"Pipeline '{name}' does not have a stage with name 'NoSuchStage'", f"unexpected message: {message!r}"
Issue-Key: codepipeline.disable-stage-transition-unknown-stage-not-permissive|run#12|AssertionError|assert message == f"Pipeline '{name}' does not have a stage with name 'NoSuchStage-does-not-exist'", f"unexpected message: {message!r}"
Issue-Key: codepipeline.list-pipeline-executions-invalid-next-token-rejected|run#12|AssertionError|assert refusal == (
Issue-Key: codepipeline.list-rule-executions-invalid-next-token-rejected|run#11|AssertionError|assert refusal == (
Issue-Key: codepipeline|CreateCustomActionType|TooManyTagsException
Issue-Key: codepipeline.deregister-webhook-validation-rejected|run#9|AssertionError|assert msg_validation == (
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# CodePipeline error messages are not built by AWS's request-derived rules: PipelineNotFound, pipeline-version, PipelineExecutionNotFound, StageNotFound, InvalidNextToken, TooManyTags and webhookName validation (7 root causes)

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipeline

## Description
Seven distinct root causes, all in error-message construction. All seven must be closed before this issue is submitted. In every one the HTTP status (400) and the error CODE are already correct — the divergence is the message, and in each case the message is produced by a rule that is known (AWS builds it from the request: the account id, the pipeline/stage/execution/version identifier, or the member path and the shape's constraint), so the rule is the requirement. The recorded strings below are examples of that rule, not literals to hard-code.

Related open issue: 20261022T173520Z-codepipeline-executions-never-progress-structural-validation-absent-error.md. Its Cause 4 names the same PipelineNotFoundException formatter (trace: get-pipeline-not-found) and its Cause 5 the same ValidationException formatter (traces: deregister-webhook-malformed-name-rejected, register-webhook-with-third-party-not-found, pipeline-structure-unicode-names-round-trip). Causes 1 and 7 here reach those formatters from check ids that issue does NOT carry (post-DeletePipeline lookups, a unicode pipeline name, a second webhook operation), and causes 2-6 are message rules it does not mention at all.

=== Cause 1 of 7: PipelineNotFoundException message omits the account and does not follow the AWS rule ===
AWS: Smithy: GetPipeline.errors / GetPipelineExecution.errors include PipelineNotFoundException (errorKind client, httpError 400). The message is derived from the caller's account and the requested name: "Account '<account id>' does not have a pipeline with name '<name>'" (and, from GetPipelineExecution, the variant "The account with id '<account id>' does not include a pipeline with the name '<name>'"). Four checks assert that derivation independently, after DeletePipeline and for a unicode-ish name.
Emulator: code PipelineNotFoundException and 400 are right; the message is 'pipeline <name> does not exist' — the account is absent and the wording does not follow the rule.
Why: the account scope of the lookup is never reported, so a caller cannot tell "wrong account/region" from "deleted pipeline"; clients and tests keyed on the derived text diverge.
Fix: the PipelineNotFoundException construction shared by the GetPipeline / GetPipelineExecution handlers (one formatter, so the text stays uniform wherever the error is raised).
Traces: codepipeline.pipeline-create-active-update-delete, codepipeline.pipeline-lifecycle-create-active-delete, codepipeline.pipeline-lifecycle-delete-then-404, codepipeline.pipeline-name-unicode-round-trip.

=== Cause 2 of 7: the pipeline-VERSION not-found message is not the AWS-derived text ===
AWS: GetPipeline(name, version) for a version that was never created is refused with PipelineNotFoundException (client, 400) whose message adds the version to the same rule: "Account '<account id>' does not have a pipeline with name '<name>' and version '<version>'."
Emulator: message 'pipeline <name> has no version 99' — no account, different wording, no trailing period.
Why: a client reading version history cannot distinguish an absent pipeline from an absent version by message, and the account is again unreported.
Fix: the version-lookup branch of the GetPipeline handler's PipelineNotFoundException construction.
Traces: codepipeline.update-pipeline-version-history-stale-read.

=== Cause 3 of 7: PipelineExecutionNotFoundException message is not the AWS-derived text ===
AWS: Smithy: GetPipelineExecution.errors includes PipelineExecutionNotFoundException (client, 400). The message is built from both request members: "Pipeline Execution with id '<pipelineExecutionId>' does not exist in pipeline with name '<pipelineName>'".
Emulator: message 'pipeline <name> has no execution <id>' — the same two values in a different, non-AWS shape.
Why: the check also proves the code is correctly distinct from PipelineNotFoundException, so only the derived text diverges; clients that surface or match the message diverge.
Fix: the PipelineExecutionNotFoundException construction in the GetPipelineExecution handler.
Traces: codepipeline.get-pipeline-execution-unknown-execution-id-rejected.

=== Cause 4 of 7: StageNotFoundException message is not the AWS-derived text ===
AWS: Smithy: DisableStageTransition.errors includes StageNotFoundException (client, 400). The message is derived from the pipeline name and the requested stage name: "Pipeline '<pipelineName>' does not have a stage with name '<stageName>'". Two independent checks assert that rule.
Emulator: code StageNotFoundException is correct; the message is 'pipeline <name> has no stage named <stage>'.
Why: same class as causes 1-3 — a rule-derived message that clients and tests match on is wrong while the code is right.
Fix: the StageNotFoundException construction in the DisableStageTransition handler (and anywhere else the stage lookup raises it, for uniformity).
Traces: codepipeline.disable-stage-transition-invalid-reason-rejected, codepipeline.disable-stage-transition-unknown-stage-not-permissive.

=== Cause 5 of 7: InvalidNextTokenException message does not quote the rejected token, and differs between two paginated operations ===
AWS: Smithy: ListPipelineExecutions / ListRuleExecutions model InvalidNextTokenException (client, 400). The message is derived from the request: "The nextToken value '<token>' is not valid. Specify a valid nextToken value, and then try again."
Emulator: two different non-conforming messages for the same rule — ListPipelineExecutions answers 'nextToken was not issued by ListPipelineExecutions for this pipeline', ListRuleExecutions answers 'The next token was specified in an invalid format. Make sure that the next token you provide is the token returned by a previous call.'. Neither quotes the rejected token.
Why: the rejected token is not echoed, so a paginating client cannot log which cursor it lost, and the two list operations disagree with each other on an error that should come from one formatter.
Fix: a single InvalidNextTokenException formatter used by the pagination/cursor validation of all paginated codepipeline handlers.
Traces: codepipeline.list-pipeline-executions-invalid-next-token-rejected, codepipeline.list-rule-executions-invalid-next-token-rejected.

=== Cause 6 of 7: TooManyTagsException message does not name the resource the tag limit was exceeded for ===
AWS: Smithy: CreateCustomActionType.errors includes TooManyTagsException (client, 400). The message is derived from the request's resource identity: "Tag limit exceeded for custom action type with category=[<category>], provider=[<provider>] and version=[<version>]." The check also verifies the 50-tag request is accepted and the 51-tag request creates nothing.
Emulator: code TooManyTagsException is correct; the message is 'tags must hold at most 50 entries, but this request carries 51' — it reports the count instead of identifying the action type.
Why: a batch caller tagging many resources cannot tell WHICH resource was refused from the message; the limit is the one thing AWS does not put there.
Fix: the TooManyTagsException construction in the CreateCustomActionType handler's tag validation.
Traces: codepipeline.create-custom-action-type-too-many-tags-fails.

=== Cause 7 of 7: ValidationException for webhookName echoes the rejected value and renders the anchored pattern ===
AWS: constraint-violation messages follow a known rule — "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — with the offending VALUE omitted and the pattern reported WITHOUT anchors even though the model carries them (Smithy: WebhookName, type string, length 1..100, pattern ^[A-Za-z0-9.@\-_]+$; AWS reports [A-Za-z0-9.@\-_]+).
Emulator: emits both the value and the anchors: "1 validation error detected: Value 'bad name with spaces!' at 'webhookName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9.@\-_]+$". Code (ValidationException) and status (400) are correct.
Why: the message is rule-derived, so it is a requirement; and echoing the rejected value back verbatim leaks request content into error text AWS deliberately omits.
Fix: the shared constraint-violation message formatter used by the codepipeline request validator — value elision plus anchor stripping for pattern constraints. Same formatter as Cause 5 of the related open issue; this check id is not in that issue's trace list, so re-replay it too.
Traces: codepipeline.deregister-webhook-validation-rejected.

## Evidence
Cause 1 — expected ('PipelineNotFoundException', "Account '000000000000' does not have a pipeline with name '<name>'"). actual: ('PipelineNotFoundException', 'pipeline sim2real-cp-crud-1f130bc8f3b8 does not exist') / '... sim2real-cp-lifecycle-930bc1be484a does not exist' / '... sim2real-cp-pl-e539b852543e does not exist' / 'pipeline sim2real.cp@unicode-name_cb0cb3a2e8ae does not exist'.
Cause 2 — expected message "Account '000000000000' does not have a pipeline with name 'sim2real-behav-cpver-5a0fdada41ff' and version '99'.". actual: 'pipeline sim2real-behav-cpver-5a0fdada41ff has no version 99'.
Cause 3 — expected ('PipelineExecutionNotFoundException', "Pipeline Execution with id '<uuid>' does not exist in pipeline with name '<name>'"). actual: ('PipelineExecutionNotFoundException', 'pipeline sim2real-cp-pipe-1f4d96060679 has no execution d8e17ed2-0971-4d07-acf5-0ba41a52682a').
Cause 4 — expected message "Pipeline '<name>' does not have a stage with name 'NoSuchStage'". actual: 'pipeline sim2real-cp-64e2a7b23253 has no stage named NoSuchStage' and 'pipeline sim2real-cp-unkstage-4ff657205d8d has no stage named NoSuchStage-does-not-exist'.
Cause 5 — expected ('InvalidNextTokenException', "The nextToken value 'not-a-real-token==' is not valid. Specify a valid nextToken value, and then try again."). actual: ('InvalidNextTokenException', 'nextToken was not issued by ListPipelineExecutions for this pipeline') from ListPipelineExecutions and ('InvalidNextTokenException', 'The next token was specified in an invalid format. Make sure that the next token you provide is the token returned by a previous call.') from ListRuleExecutions.
Cause 6 — expected ('TooManyTagsException', 'Tag limit exceeded for custom action type with category=[Build], provider=[<provider>] and version=[1].'). actual: {'Code': 'TooManyTagsException', 'Message': 'tags must hold at most 50 entries, but this request carries 51'}.
Cause 7 — expected "1 validation error detected: Value at 'webhookName' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9.@\\-_]+". actual: "1 validation error detected: Value 'bad name with spaces!' at 'webhookName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9.@\\-_]+$".

## Steps to Reproduce
1. Boot the emulator and replay the codepipeline behavioral checks listed in trace_ids.
2. Cause 1: CreatePipeline, DeletePipeline, then GetPipeline on the deleted name — inspect the PipelineNotFoundException message (repeat with a name containing '.', '@' and '_').
3. Cause 2: CreatePipeline, UpdatePipeline, then GetPipeline(name=..., version=99) — inspect the message.
4. Cause 3: StartPipelineExecution, then GetPipelineExecution with a fresh random uuid as pipelineExecutionId — inspect the PipelineExecutionNotFoundException message.
5. Cause 4: DisableStageTransition with stageName='NoSuchStage' on an existing pipeline — inspect the StageNotFoundException message.
6. Cause 5: ListPipelineExecutions(pipelineName=..., nextToken='not-a-real-token==') and ListRuleExecutions with the same bogus token — inspect both messages.
7. Cause 6: CreateCustomActionType with 51 tags — inspect the TooManyTagsException message.
8. Cause 7: DeregisterWebhookWithThirdParty with webhookName='bad name with spaces!' — inspect the ValidationException message.
