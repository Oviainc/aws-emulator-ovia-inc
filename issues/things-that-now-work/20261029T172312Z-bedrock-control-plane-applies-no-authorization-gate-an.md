Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-resource-policy-access-denied bedrock.create-model-invocation-job-without-permission-denied bedrock.stop-evaluation-job-terminal-state-rejected
Issue-Key: bedrock.delete-resource-policy-access-denied|run#24|Timeout|time.sleep(3.0)
Issue-Key: bedrock.create-model-invocation-job-without-permission-denied|_error_code_and_message#17|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock.stop-evaluation-job-terminal-state-rejected|_error_code_and_message#8|AssertionError|raise AssertionError(
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock control plane applies no authorization gate: an explicit IAM Deny is ignored, and CreateModelInvocationJob / CreateEvaluationJob admit a passed role AWS refuses with AccessDeniedException (3 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelInvocationJob

## Description
Three authorization decisions the bedrock control plane never makes. The open issue 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md names a missing IAM gate for other replay keys, and 20261027T212745Z-bedrock-createmodelcustomizationjob-the-iam-passrole-gate-admits-a.md names the pass-role gate for CreateModelCustomizationJob; neither carries the three keys below, and causes 2 and 3 are different operations with their own handlers.

=== Cause 1 of 3: an explicit IAM Deny on an assumed role is ignored, so a denied principal is served control-plane writes ===
AWS: Smithy (com.amazonaws.bedrock) models AccessDeniedException (httpError 403) on CreateGuardrail, DeleteResourcePolicy and the rest of the control plane. Authorization outcome: recorded behavior only — a role whose inline policy is {Effect: Deny, Action: "*", Resource: "*"} is refused 403 AccessDeniedException on every bedrock call it makes (an explicit Deny beats any Allow), while the SAME call made by the owning caller with full permissions succeeds. That contrast is the whole point of the check: the only difference producing the refusal is the identity.
Emulator: the restricted principal's CreateGuardrail SUCCEEDS. The check polls that sanity control for a bounded 60s (treating an early success as IAM propagation lag) and the Deny never takes effect, so the check consumes its whole budget and is graded a timeout at 60.0s. No sigv4 identity is turned into an authorization decision at all.
Why: every negative-authorization path of the service is unreachable — a caller cannot test that a scoped-down role is actually scoped down, and an emulator-backed test suite will certify a policy as effective when it is not. The AccessDeniedException half of DeleteResourcePolicy (and its positive control, the identical delete by the owning caller returning 200 idempotently) is never exercised.
Fix: the request-authorization layer the control-plane handlers run under must evaluate the caller's identity-based policy (explicit Deny first) and answer the modelled 403 AccessDeniedException.
Traces: bedrock.delete-resource-policy-access-denied.

=== Cause 2 of 3: CreateModelInvocationJob accepts a call AWS refuses 403 AccessDeniedException ===
AWS: Smithy CreateModelInvocationJob = POST /model-invocation-job, success 200, required members jobName, roleArn (@pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$), modelId, inputDataConfig, outputDataConfig; errors AccessDeniedException (403), ValidationException (400), ResourceNotFoundException (404), ConflictException (400). Outcome: recorded behavior only — the call is refused 403 AccessDeniedException when the calling principal is explicitly denied bedrock:CreateModelInvocationJob (the same principal's un-denied ListFoundationModels still succeeds, proving the denial is scoped), and in the recorded environment passing a roleArn is likewise refused AccessDeniedException with a message containing "cross-account pass role". Either way the answer is 403 AccessDeniedException and NO job is created (ListModelInvocationJobs(nameContains=<jobName>) stays empty).
Emulator: HTTP 200 with jobArn=<minted model-invocation-job ARN> — the job is created for a principal that is explicitly denied the action, and no pass-role decision is made.
Why: batch-inference authorization is unverifiable: a denied principal can start jobs that bill and write to S3, and a caller's AccessDeniedException handling is dead code. The effect-absent assertion (no job created) is also violated.
Fix: CreateModelInvocationJob's handler — evaluate the caller's permission for the action and the iam:PassRole decision over roleArn, and answer the modelled AccessDeniedException without creating the job.
Traces: bedrock.create-model-invocation-job-without-permission-denied.

=== Cause 3 of 3: CreateEvaluationJob accepts a passed role AWS refuses 403 AccessDeniedException ===
AWS: Smithy CreateEvaluationJob = POST /evaluation-jobs, success 202, required members jobName, roleArn, evaluationConfig, inferenceConfig, outputDataConfig; errors AccessDeniedException (403), ValidationException (400), ConflictException (400). Outcome: recorded behavior only — in the recorded environment every CreateEvaluationJob that passes a role (including a same-account role correctly trusted for bedrock.amazonaws.com with S3 and bedrock:InvokeModel grants, and a real ACTIVE on-demand model) is refused 403 AccessDeniedException whose message contains "cross-account pass role", with a Converse positive control on the same credentials proving the account is otherwise healthy. This is the same gate already filed for CreateModelCustomizationJob in 20261027T212745Z.
Emulator: HTTP 202 with jobArn=<minted evaluation-job ARN> — the role is passed with no decision.
Why: the pass-role boundary is the only thing standing between a caller and handing Bedrock a role it may not assume; with it absent, an emulator-backed test certifies an evaluation-job wiring that production refuses. The check's subject (StopEvaluationJob's terminal-state refusal) sits behind this call and is never reached.
Fix: CreateEvaluationJob's handler — make the same pass-role decision over roleArn as CreateModelCustomizationJob must, and answer the modelled AccessDeniedException instead of accepting.
Traces: bedrock.stop-evaluation-job-terminal-state-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service bedrock 91/199 passed.

[mismatch] bedrock.delete-resource-policy-access-denied — "Check timed out after 60.0s"
  setup (from the check body): CreateGuardrail; iam:CreateRole trusted by <account>:root; iam:PutRolePolicy with {Effect: Deny, Action: '*', Resource: '*'}; iam:GetRolePolicy confirms the policy landed; sts:AssumeRole to get a restricted bedrock client.
  sanity control: `sanity_deadline = time.monotonic() + 60.0` polling restricted_bedrock.create_guardrail(...) until it is DENIED.
  expected: 403 AccessDeniedException for the restricted principal (and a 200 for the owning caller on the identical DeleteResourcePolicy).
  actual:   the denied principal's CreateGuardrail keeps SUCCEEDING for the full 60s window, so the check never leaves the poll and is graded a timeout. The emulator consumed the whole budget because the Deny is never applied.

[mismatch] bedrock.create-model-invocation-job-without-permission-denied
  setup: role trusted by <account>:root AND by Service bedrock.amazonaws.com; inline policy {Allow bedrock:ListFoundationModels; Deny bedrock:CreateModelInvocationJob}; positive control: the restricted principal's ListFoundationModels succeeds.
  request: CreateModelInvocationJob(jobName=<unique>, roleArn=<that role>, modelId='meta.llama3-1-8b-instruct-v1:0', inputDataConfig/outputDataConfig in s3://<bucket>/...)
  expected: AccessDeniedException, no job created
  actual:   "expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobArn': 'arn:aws:bedrock:us-west-2:<account>:model-invocation-job/<id>'}"

[mismatch] bedrock.stop-evaluation-job-terminal-state-rejected
  setup: scratch S3 bucket + same-account role trusted for bedrock.amazonaws.com with s3 and bedrock:InvokeModel; positive control: Converse on a real on-demand text model returns 200.
  request: CreateEvaluationJob(jobName=<unique>, roleArn=<that role>, applicationType='ModelEvaluation', automated Builtin.Bold/Builtin.Accuracy, inferenceConfig naming the conversable model, outputDataConfig s3://<bucket>/eval-output/)
  expected: AccessDeniedException whose message contains 'cross-account pass role'
  actual:   "expected CreateEvaluationJob with a same-account role to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'jobArn': 'arn:aws:bedrock:us-west-2:<account>:evaluation-job/<id>'}"

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): CreateModelInvocationJob POST /model-invocation-job code 200, roleArn required @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$, errors AccessDeniedException 403 / ValidationException 400 / ResourceNotFoundException 404 / ConflictException 400; CreateEvaluationJob POST /evaluation-jobs code 202, roleArn required, errors AccessDeniedException 403 / ValidationException 400; DeleteResourcePolicy and CreateGuardrail both model AccessDeniedException 403.

## Steps to Reproduce
Boot the emulator from this checkout and replay the three checks in trace_ids (suite simparity-behavioral-aws-v1).
1. iam:CreateRole (trusted by <account>:root) + iam:PutRolePolicy {Deny *:*}; sts:AssumeRole; with those credentials call bedrock:CreateGuardrail. Expect 403 AccessDeniedException; observe 202/success (repeatedly, for at least 60s). Then DeleteResourcePolicy on a guardrail as the restricted principal (expect 403) and as the owning caller (expect 200).
2. Create a role trusted by <account>:root and by bedrock.amazonaws.com, inline {Allow bedrock:ListFoundationModels, Deny bedrock:CreateModelInvocationJob}; assume it; confirm ListFoundationModels works; then CreateModelInvocationJob with modelId='meta.llama3-1-8b-instruct-v1:0'. Expect AccessDeniedException and ListModelInvocationJobs(nameContains=<jobName>) empty; observe 200 with a jobArn.
3. Create a scratch bucket and a same-account role trusted for bedrock.amazonaws.com; CreateEvaluationJob as above. Expect 403 AccessDeniedException containing 'cross-account pass role'; observe 202 with a jobArn.

## Resolution

Causes 1 and 2 are fixed, and their checks replay clean: the identity-based decision is taken for
every operation of this service in the layer each one is served behind
(`src/service/authorization.rs`, reached from `src/service.rs`), and
`CreateModelInvocationJob` / `CreateEvaluationJob` take the `iam:PassRole` decision over `roleArn`
through the one helper `state::job_pass_role_denial` that `CreateModelCustomizationJob` uses.

Cause 3's gate is implemented — `CreateEvaluationJob` makes exactly the decision this report asks
for — but its check, `bedrock.stop-evaluation-job-terminal-state-rejected`, still diverges and is
DECLINED as recorded: the role the recording passes is a role of the caller's own account that IAM
holds and whose trust policy grants `bedrock.amazonaws.com` the assume this pass makes, so the
decision grants it. Refusing it anyway would need a verdict hardwired to deny every pass of this
operation, which the resolved issues
`20261027T152401Z-bedrock-createmodelcustomizationjob-denies-every-rolearn-with-403-cross-account.md`
and `20261027T212357Z-bedrock-createmodelcustomizationjob-is-now-accepted-for-every-well-formed.md`
forbid, and no recording or document states a rule that separates that role from one AWS accepts.
A later recording that does state such a rule is what would reopen it.
