Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.retry-stage-not-latest-execution-rejected codepipeline.pipeline-execution-start-to-terminal-state codepipeline.approval-gate-withholds-the-artifact-until-it-is-approved codepipeline.approval-rejection-records-the-reviewer-and-withholds-the-downstream-stage codepipeline.artifact-store-cmk-encrypts-the-delivered-artifact-object codepipeline.deploy-with-extract-true-unzips-every-artifact-entry codepipeline.disable-stage-transition-holds-execution codepipeline.disabling-the-artifact-store-cmk-fails-the-execution-and-reenabling-restores-it codepipeline.missing-source-object-fails-the-execution-and-no-artifact-is-deployed codepipeline.override-stage-condition-conflict-rejected codepipeline.parallel-actions-in-one-stage-both-deliver-their-own-artifact codepipeline.put-approval-result-stale-token-rejected codepipeline.run-order-two-starts-only-after-run-order-one-finishes codepipeline.server-computes-pipeline-execution-status-rollup codepipeline.server-side-manual-approval-blocks-until-approved codepipeline.stopping-an-execution-leaves-no-deploy-artifact-and-a-restop-is-rejected codepipeline.superseded-execution-mode-drops-the-middle-queued-execution codepipeline.create-pipeline-dangling-input-artifact-rejected codepipeline.create-pipeline-first-stage-not-source-rejected codepipeline.create-pipeline-unassumable-role-rejected codepipeline.create-pipeline-v1-rejects-v2-only-fields codepipeline.create-pipeline-duplicate-name-conflict-rejected codepipeline.create-pipeline-duplicate-name-in-use-rejected codepipeline.get-pipeline-not-found codepipeline.deregister-webhook-malformed-name-rejected codepipeline.register-webhook-with-third-party-not-found codepipeline.pipeline-structure-unicode-names-round-trip codepipeline.update-pipeline-invalid-blocker-declaration-rejected codepipeline.action-configuration-empty-vs-absent-preserved codepipeline.start-pipeline-execution-concurrency-limit-rejected codepipeline.start-pipeline-execution-concurrent-limit-exceeded codepipeline.deploy-codepipeline-cfn-execution-behaves codepipeline.deploy-codepipeline-cloudformation-executes codepipeline.deploy-codepipeline-pipeline-executes-via-cloudformation codepipeline.deploy-pipeline-via-cloudformation-behaves codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object codepipeline.rollback-stage-restores-the-previous-artifact-bytes codepipeline.source-output-variable-resolves-into-the-downstream-object-key codepipeline.source-revision-override-delivers-the-pinned-object-version
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.retry-stage-not-latest-execution-rejected|_poll_until#13|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: codepipeline.pipeline-execution-start-to-terminal-state|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.approval-gate-withholds-the-artifact-until-it-is-approved|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.approval-rejection-records-the-reviewer-and-withholds-the-downstream-stage|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.artifact-store-cmk-encrypts-the-delivered-artifact-object|_poll#22|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.deploy-with-extract-true-unzips-every-artifact-entry|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.disable-stage-transition-holds-execution|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.disabling-the-artifact-store-cmk-fails-the-execution-and-reenabling-restores-it|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.missing-source-object-fails-the-execution-and-no-artifact-is-deployed|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.override-stage-condition-conflict-rejected|_wait_for_first_execution#2|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.parallel-actions-in-one-stage-both-deliver-their-own-artifact|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.put-approval-result-stale-token-rejected|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.run-order-two-starts-only-after-run-order-one-finishes|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.server-computes-pipeline-execution-status-rollup|_wait_for_sole_execution_id#8|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.server-side-manual-approval-blocks-until-approved|_drain_initial_approval#5|Timeout|time.sleep(3)
Issue-Key: codepipeline.stopping-an-execution-leaves-no-deploy-artifact-and-a-restop-is-rejected|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.superseded-execution-mode-drops-the-middle-queued-execution|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.create-pipeline-dangling-input-artifact-rejected|_error#10|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: codepipeline.create-pipeline-first-stage-not-source-rejected|run#12|AssertionError|assert code == "InvalidStructureException", (
Issue-Key: codepipeline.create-pipeline-unassumable-role-rejected|run#10|AssertionError|raise AssertionError(f"CreatePipeline accepted a roleArn that does not exist: {result!r}")
Issue-Key: codepipeline.create-pipeline-v1-rejects-v2-only-fields|_reject#14|AssertionError|raise AssertionError(f"expected CreatePipeline to be refused, but it succeeded and returned {result!r}")
Issue-Key: codepipeline.create-pipeline-duplicate-name-conflict-rejected|run#15|AssertionError|assert message == f"A pipeline with the name '{name}' already exists in account '{ctx.account}'", (
Issue-Key: codepipeline.create-pipeline-duplicate-name-in-use-rejected|run#12|AssertionError|assert message == f"A pipeline with the name '{name}' already exists in account '{ctx.account}'", (
Issue-Key: codepipeline.get-pipeline-not-found|run#12|AssertionError|assert refusal == (
Issue-Key: codepipeline.deregister-webhook-malformed-name-rejected|run#5|AssertionError|assert malformed_message == (
Issue-Key: codepipeline.register-webhook-with-third-party-not-found|run#5|AssertionError|assert malformed.get("Message") == expected_message, (
Issue-Key: codepipeline.pipeline-structure-unicode-names-round-trip|run#9|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: codepipeline.update-pipeline-invalid-blocker-declaration-rejected|run#12|AssertionError|assert message == (
Issue-Key: codepipeline.action-configuration-empty-vs-absent-preserved|run#17|AssertionError|assert message == (
Issue-Key: codepipeline.start-pipeline-execution-concurrency-limit-rejected|run#15|AssertionError|assert limit_errors > 0, (
Issue-Key: codepipeline.start-pipeline-execution-concurrent-limit-exceeded|_error_code#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: codepipeline|GetPipeline|PipelineNotFoundException
Issue-Key: codepipeline|PutObject|NoSuchBucket
Issue-Key: codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object|run#9|KeyError|version_id = s3.put_object(Bucket=bucket, Key="source.zip",
Issue-Key: codepipeline.rollback-stage-restores-the-previous-artifact-bytes|run#11|KeyError|old_version = s3.put_object(Bucket=bucket, Key="source.zip",
Issue-Key: codepipeline.source-output-variable-resolves-into-the-downstream-object-key|run#10|KeyError|version_id = put["VersionId"]
Issue-Key: codepipeline.source-revision-override-delivers-the-pinned-object-version|run#11|KeyError|old_version = s3.put_object(Bucket=bucket, Key="source.zip",
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodePipeline: executions never progress, structural validation absent, error/validation messages diverge, CFN provisioning is a no-op (9 root causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
Nine distinct root causes found in the codepipeline behavioral sweep. All nine must be closed before this issue is submitted.

=== Cause 1 of 9: pipeline executions never leave InProgress — no stage/action execution engine ===
AWS: StartPipelineExecution returns a pipelineExecutionId (Smithy: StartPipelineExecution.output.pipelineExecutionId, pattern ^[0-9a-f]{8}-...$) and the service then DRIVES the execution — each stage's actions run in runOrder, artifacts are produced into the artifactStore, and the execution reaches a terminal status (Succeeded/Failed/Stopped/Superseded) which GetPipelineExecution / ListPipelineExecutions report. Stage/action status is modelled on GetPipelineState and on PipelineExecutionStatus.
Emulator: the start is accepted and an executionId is returned, but the execution stays 'InProgress' forever. The retry-stage check polls the execution status and reports verbatim: "condition not met within 45.0s; last='InProgress'". Sixteen further checks that wait for any downstream consequence of execution (artifact delivered, approval reached, transition held, rollup computed, supersede applied, stop applied) all hit the harness ceiling with "Check timed out after 60.0s".
Why: a client that starts a pipeline and waits for it to finish hangs forever. Nothing downstream of StartPipelineExecution is observable — no artifacts in the artifact store, no approval tokens, no terminal status, no stage state — so every workflow built on CodePipeline is unusable against this emulator.
Fix: the pipeline execution engine / state machine behind StartPipelineExecution and the execution state it publishes to GetPipelineState, GetPipelineExecution and ListActionExecutions in the codepipeline service crate (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src).
Traces: codepipeline.retry-stage-not-latest-execution-rejected (explicit last='InProgress'), codepipeline.pipeline-execution-start-to-terminal-state, codepipeline.approval-gate-withholds-the-artifact-until-it-is-approved, codepipeline.approval-rejection-records-the-reviewer-and-withholds-the-downstream-stage, codepipeline.artifact-store-cmk-encrypts-the-delivered-artifact-object, codepipeline.deploy-with-extract-true-unzips-every-artifact-entry, codepipeline.disable-stage-transition-holds-execution, codepipeline.disabling-the-artifact-store-cmk-fails-the-execution-and-reenabling-restores-it, codepipeline.missing-source-object-fails-the-execution-and-no-artifact-is-deployed, codepipeline.override-stage-condition-conflict-rejected, codepipeline.parallel-actions-in-one-stage-both-deliver-their-own-artifact, codepipeline.put-approval-result-stale-token-rejected, codepipeline.run-order-two-starts-only-after-run-order-one-finishes, codepipeline.server-computes-pipeline-execution-status-rollup, codepipeline.server-side-manual-approval-blocks-until-approved, codepipeline.stopping-an-execution-leaves-no-deploy-artifact-and-a-restop-is-rejected, codepipeline.superseded-execution-mode-drops-the-middle-queued-execution.

=== Cause 2 of 9: CreatePipeline performs no structural validation of the pipeline declaration ===
AWS: CreatePipeline models InvalidStructureException, InvalidStageDeclarationException, InvalidActionDeclarationException and ValidationException, all HTTP 400 (Smithy: CreatePipeline.errors). A declaration is refused when it is structurally impossible: an inputArtifacts entry naming an artifact no earlier action outputs; a first stage that is not a Source stage (source action placed in stage 2); a roleArn that cannot be assumed / does not exist; and V2-only members (pipeline.variables) on a declaration whose pipelineType is V1.
Emulator: all four declarations are accepted with HTTP 200 and the pipeline is created. Recorded responses show HTTPStatusCode 200 with the bad declaration echoed back verbatim — e.g. inputArtifacts [{'name': 'NoSuchArtifact'}] accepted, roleArn 'arn:aws:iam::000000000000:role/sim2real-behav-cp-role-absent' accepted, pipelineType 'V1' with variables [{'name': 'Flavor', ...}] accepted, and the first-stage-not-source case reports "error code was None, expected InvalidStructureException".
Why: clients use CreatePipeline's refusal as their only structural lint. Here a pipeline that can never execute is created successfully, so the error surfaces (if at all) much later and in a different place than on AWS; negative tests that assert a 400 pass against AWS and fail here.
Fix: input/structural validation in the CreatePipeline handler (and the shared pipeline-declaration validator) of the codepipeline crate.
Traces: codepipeline.create-pipeline-dangling-input-artifact-rejected, codepipeline.create-pipeline-first-stage-not-source-rejected, codepipeline.create-pipeline-unassumable-role-rejected, codepipeline.create-pipeline-v1-rejects-v2-only-fields.

=== Cause 3 of 9: PipelineNameInUseException message is not the AWS-derived text ===
AWS: Smithy: CreatePipeline.errors includes PipelineNameInUseException (client, HTTP 400). The message is built from the request and the caller's account: "A pipeline with the name '<pipeline name>' already exists in account '<account id>'". Two independent checks assert exactly that derivation (name from the request, account from the caller), so the rule — not one recorded string — is the requirement.
Emulator: the code PipelineNameInUseException is correct, but the message is 'pipeline <name> already exists' — the account is missing and the wording does not follow the rule.
Why: callers that parse or surface the conflict message (and tests asserting the derived text) see the wrong text; the account the collision occurred in is not reported at all.
Fix: the error construction in the CreatePipeline handler's duplicate-name path.
Traces: codepipeline.create-pipeline-duplicate-name-conflict-rejected, codepipeline.create-pipeline-duplicate-name-in-use-rejected.

=== Cause 4 of 9: PipelineNotFoundException message is not the AWS-derived text ===
AWS: Smithy: GetPipeline.errors includes PipelineNotFoundException (client, HTTP 400). The message is derived from the request name and the caller's account: "Account '<account id>' does not have a pipeline with name '<name>'".
Emulator: code PipelineNotFoundException is correct; the message is 'pipeline <name> does not exist' — no account, different wording.
Why: same as cause 3 — the account scope of the lookup is not reported, and clients keyed on the derived text diverge.
Fix: the error construction in the GetPipeline handler (and wherever PipelineNotFoundException is raised, so the text stays uniform).
Traces: codepipeline.get-pipeline-not-found.

=== Cause 5 of 9: ValidationException messages include the offending value and render the anchored pattern ===
AWS: constraint-violation messages are built from the request by a known rule — "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" (and "... Member must satisfy enum value set: [...]" for enums). Recorded AWS behavior across three independent checks shows the offending VALUE is omitted, and for pattern constraints the regex is reported WITHOUT the anchors even though the model carries them (Smithy: PipelineName and WebhookName both pattern ^[A-Za-z0-9.@\-_]+$, length 1..100) — AWS reports [A-Za-z0-9.@\-_]+.
Emulator: emits the value and the anchored pattern, e.g. "Value 'bad name!' at 'webhookName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9.@\-_]+$" where AWS gives "Value at 'webhookName' ... pattern: [A-Za-z0-9.@\-_]+"; same divergence for pipeline.name with a non-ASCII value and for the blockers enum ("Value 'Unsupported' at 'pipeline.stages.2.member.blockers.1.member.type'" vs AWS's "Value at 'pipeline.stages.2.member.blockers.1.member.type'"). Code (ValidationException) and status are correct.
Why: the message is produced by a known rule, so it is a requirement. Clients and tests that assert the derived text diverge, and echoing the rejected value back verbatim leaks request content into error text AWS deliberately omits.
Fix: the shared constraint-violation message formatter used by the codepipeline request validator (value elision + anchor stripping for pattern constraints).
Traces: codepipeline.deregister-webhook-malformed-name-rejected, codepipeline.register-webhook-with-third-party-not-found, codepipeline.pipeline-structure-unicode-names-round-trip, codepipeline.update-pipeline-invalid-blocker-declaration-rejected.

=== Cause 6 of 9: map-value length violation is reported at the entry key path with only one bound ===
AWS: ActionConfigurationMap's value constraint is reported against the MAP member, listing both bounds: "1 validation error detected: Value at 'pipeline.stages.2.member.actions.1.member.configuration' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 50000, Member must have length greater than or equal to 1]" (recorded behavior; Smithy: ActionConfigurationMap value -> ActionConfigurationValue with a length constraint). Code is ValidationException, HTTP 400.
Emulator: reports the path of the individual entry and only the lower bound: "1 validation error detected: Value '' at 'pipeline.stages.2.member.actions.1.member.configuration.CustomData' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: the message is rule-derived (member path + the shape's constraints), so the path shape and the "Map value must satisfy constraint: [...]" framing are requirements; clients mapping the member path back to their input point at a key that AWS never names.
Fix: map-member handling in the shared constraint-violation formatter / validator for UpdatePipeline and CreatePipeline input.
Traces: codepipeline.action-configuration-empty-vs-absent-preserved.

=== Cause 7 of 9: the concurrent-pipeline-execution ceiling is never enforced ===
AWS: StartPipelineExecution models ConcurrentPipelineExecutionsLimitExceededException (client, HTTP 400) (Smithy: StartPipelineExecution.errors) and refuses a start once the pipeline's concurrent-execution ceiling is reached.
Emulator: every start succeeds. A burst reports "successes=80 throttled=0 limit_errors=0", and the one-too-many start returns a pipelineExecutionId instead of a 400.
Why: clients that handle the ceiling (back off, queue, or surface it) never exercise that path, and a load-shedding bug ships unnoticed. Note this cause is downstream-visible only because cause 1 leaves every execution InProgress forever — but the ceiling must still be enforced once executions do progress.
Fix: concurrency accounting in the StartPipelineExecution handler and the pipeline execution state.
Traces: codepipeline.start-pipeline-execution-concurrency-limit-rejected, codepipeline.start-pipeline-execution-concurrent-limit-exceeded.

=== Cause 8 of 9: CloudFormation stacks containing CodePipeline/S3/IAM resources report success but provision nothing ===
AWS: a CreateStack whose template declares AWS::CodePipeline::Pipeline, AWS::S3::Bucket (with VersioningConfiguration), AWS::IAM::Role etc. reaches CREATE_COMPLETE only after those resources exist and are addressable by their physical ids, and DescribeStacks Outputs carry an OutputValue per declared Output.
Emulator: the stack reports completion but the declared resources do not exist. Recorded follow-up calls fail: GetPipeline -> PipelineNotFoundException "pipeline <name> does not exist"; S3 PutObject into the template's bucket -> NoSuchBucket. Status/codes are those of a genuinely absent resource, i.e. nothing was created.
Why: every infrastructure-as-code path is broken — a client deploys a stack, sees success, then every reference to a stack resource 404s. Error codes are correct for 'absent', so the divergence is the provisioning, not the serializer.
Fix: the CloudFormation stack-resource provisioner — the resource-type handlers that must create the real underlying resources (and populate stack Outputs) for codepipeline/s3/iam types.
Traces: codepipeline.deploy-codepipeline-cfn-execution-behaves, codepipeline.deploy-codepipeline-cloudformation-executes, codepipeline.deploy-codepipeline-pipeline-executes-via-cloudformation, codepipeline.deploy-pipeline-via-cloudformation-behaves.

=== Cause 9 of 9: S3 PutObject into a versioning-enabled bucket returns no VersionId ===
AWS: once PutBucketVersioning sets Status=Enabled, every PutObject response carries the x-amz-version-id header, surfaced by the SDK as the VersionId member of PutObjectOutput. CodePipeline's S3 source action requires a versioned bucket and pins revisions by that version id.
Emulator: the bucket is created and PutBucketVersioning Status=Enabled succeeds, but PutObject returns a response with no VersionId member — four checks fail with KeyError: 'VersionId' on put_object(...)["VersionId"].
Why: a client cannot learn the version it just wrote, so it cannot pin a source revision, roll back to a previous object version, or address an object version at all — the entire CodePipeline source-revision/rollback surface is unreachable.
Fix: version-id assignment and response serialization in the S3 PutObject handler for versioning-enabled buckets (the bucket/object versioning state). Surfaced through codepipeline checks, but the defect is in the S3 PutObject path.
Traces: codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object, codepipeline.rollback-stage-restores-the-previous-artifact-bytes, codepipeline.source-output-variable-resolves-into-the-downstream-object-key, codepipeline.source-revision-override-delivers-the-pinned-object-version.

## Evidence
Cause 1 — expected: execution reaches a terminal PipelineExecutionStatus. actual: "AssertionError: condition not met within 45.0s; last='InProgress'"; 16 sibling checks "Error: Check timed out after 60.0s".
Cause 2 — expected: HTTP 400 InvalidStructureException. actual: HTTP 200, e.g. "expected a rejection, but the call succeeded and returned {'pipeline': {... 'inputArtifacts': [{'name': 'NoSuchArtifact'}] ...}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}"; "CreatePipeline accepted a roleArn that does not exist"; "error code was None, expected InvalidStructureException"; V1 pipeline with 'variables' accepted (HTTP 200).
Cause 3 — expected message: "A pipeline with the name '<name>' already exists in account '<account>'" (code PipelineNameInUseException, 400). actual: 'pipeline sim2real-cp-dupname-43eafe4df937 already exists'.
Cause 4 — expected: ('PipelineNotFoundException', "Account '<account>' does not have a pipeline with name '<name>'"). actual: ('PipelineNotFoundException', 'pipeline sim2real-cp-getnf-absent-cfa9bd1799d9 does not exist').
Cause 5 — expected: "1 validation error detected: Value at 'webhookName' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9.@\\-_]+". actual: "1 validation error detected: Value 'bad name!' at 'webhookName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9.@\\-_]+$". Same for pipeline.name and for the blockers enum ("Value 'Unsupported' at ..." vs AWS "Value at ...").
Cause 6 — expected: "...Value at 'pipeline.stages.2.member.actions.1.member.configuration' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 50000, Member must have length greater than or equal to 1]". actual: "...Value '' at 'pipeline.stages.2.member.actions.1.member.configuration.CustomData' failed to satisfy constraint: Member must have length greater than or equal to 1".
Cause 7 — expected: HTTP 400 ConcurrentPipelineExecutionsLimitExceededException. actual: "successes=80 throttled=0 limit_errors=0"; "expected the call to be rejected, but it succeeded with '9785a0ba-1169-40b6-9e98-d39d9861c308'".
Cause 8 — expected: stack resources exist after CREATE_COMPLETE. actual: "PipelineNotFoundException ... pipeline sim2real-cp-cfn-0b98157ee88c does not exist" and "NoSuchBucket ... when calling the PutObject operation".
Cause 9 — expected: PutObjectOutput.VersionId present on a Status=Enabled bucket. actual: KeyError: 'VersionId' at s3.put_object(Bucket=bucket, Key="source.zip", Body=...)["VersionId"].

## Steps to Reproduce
1. Boot the emulator and replay the codepipeline behavioral checks listed in trace_ids.
2. Cause 1: create an S3-source + S3-deploy pipeline, call StartPipelineExecution, then poll GetPipelineExecution — the status stays InProgress indefinitely.
3. Cause 2: call CreatePipeline with (a) an inputArtifacts name no earlier action outputs, (b) an Approval stage first and the Source action in stage 2, (c) a roleArn for a nonexistent role, (d) pipelineType V1 plus pipeline.variables — each returns HTTP 200 instead of a 400 InvalidStructureException.
4. Cause 3: CreatePipeline twice with the same name; inspect the PipelineNameInUseException message.
5. Cause 4: GetPipeline on an absent name; inspect the PipelineNotFoundException message.
6. Cause 5: RegisterWebhookWithThirdParty / DeregisterWebhookWithThirdParty with webhookName 'bad name!', and UpdatePipeline with a blocker type 'Unsupported'; inspect the ValidationException message.
7. Cause 6: UpdatePipeline (parameter_validation=False) with an action configuration value of ""; inspect the ValidationException message and path.
8. Cause 7: fire a burst of StartPipelineExecution calls at one pipeline; no call is ever refused with ConcurrentPipelineExecutionsLimitExceededException.
9. Cause 8: CreateStack with a template declaring AWS::CodePipeline::Pipeline and AWS::S3::Bucket, wait for completion, then GetPipeline / PutObject against the declared resources.
10. Cause 9: CreateBucket, PutBucketVersioning Status=Enabled, PutObject — the response has no VersionId.
