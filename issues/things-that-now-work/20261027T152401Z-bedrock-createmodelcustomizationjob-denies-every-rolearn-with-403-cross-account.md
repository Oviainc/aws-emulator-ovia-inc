Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.model-customization-job-stop-transitions bedrock.model-customization-job-stop-transitions-to-stopped bedrock.stop-terminated-model-customization-job-is-an-idempotent-no-op bedrock.timestamp-precision-preserved-model-customization-job
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.model-customization-job-stop-transitions|_call_with_role_propagation_retry#11|Timeout|time.sleep(interval_s)
Issue-Key: bedrock.model-customization-job-stop-transitions-to-stopped|_retry_transient#10|Timeout|time.sleep(interval_s)
Issue-Key: bedrock.stop-terminated-model-customization-job-is-an-idempotent-no-op|_retry_transient#7|Timeout|time.sleep(interval_s)
Issue-Key: bedrock.timestamp-precision-preserved-model-customization-job|_retry_until_ready#13|Timeout|time.sleep(interval_s)
Issue-Commit: aa18f88fae373078a1f99b72f5a777b4869171d6

# CreateModelCustomizationJob denies every roleArn with 403 "Cross-account pass role is not allowed.", so no customization job can ever be created and the whole job lifecycle (Get/Stop/status/timestamps) is unreachable

> _Note: this issue was identified against the code base at commit `aa18f88fae373078a1f99b72f5a777b4869171d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelCustomizationJob

## Description
REOPENS 20261025T105655Z-bedrock-createmodelcustomizationjob-never-applies-the-pass-role-accessdeniedexception-the.md: its fix left the call refused.

That issue asked for the missing iam:PassRole decision in front of CreateModelCustomizationJob, evaluated before baseModelIdentifier resolution. The fix installed the denial as an UNCONDITIONAL one: the gate now refuses every value roleArn can carry, so the operation has no admitting path at all and nothing downstream of it can be exercised. Four behavioral checks that create a job and then drive its state machine now never get past the create and are killed by the harness deadline.

=== Cause 1 of 1: the pass-role gate is unconditional — a same-account role is refused 403 AccessDeniedException, so CreateModelCustomizationJob can never succeed ===

AWS: Smithy (com.amazonaws.bedrock) models CreateModelCustomizationJob as POST /model-customization-jobs with success status 201 and output member jobArn (ModelCustomizationJobArn, @required). roleArn is @required, RoleArn @length max 2048, @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$, documented "The Amazon Resource Name (ARN) of an IAM service role that Amazon Bedrock can assume ... To pass this role to Amazon Bedrock, the caller of this API must have the iam:PassRole permission." Modeled errors include AccessDeniedException (@httpError 403, single `message` member, "The request is denied because of missing access permissions.") and ValidationException (400). GetModelCustomizationJob reports status from ModelCustomizationJobStatus (InProgress | Completed | Failed | Stopping | Stopped), and StopModelCustomizationJob is what drives a running job to Stopped. The model therefore describes an operation with BOTH outcomes: a create that is admitted mints a job (201 + jobArn) that later reads see as InProgress and that Stop winds down to Stopped, and a create whose role may not be passed is the 403. The denial is a decision about iam:PassRole on a specific role — a role of the caller's own account carrying the documented bedrock.amazonaws.com trust statement is passable — so it cannot be the answer for every roleArn. For the exact denial text, recorded behavior only: the corpus recording cited by the reopened issue shows HTTP 403, code AccessDeniedException, message "Cross-account pass role is not allowed." — fixed text with no variable part — for a role that could not be passed in that recording's account/IAM environment.

Emulator: the gate is a comparison against a constant that is `None`, so it can never admit. services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs declares `const CUSTOMIZATION_ASSUMING_ACCOUNT: Option<&str> = None;` and `customization_pass_role_denial` is `matches!((role_arn_account(role_arn), CUSTOMIZATION_ASSUMING_ACCOUNT), (Some(a), Some(b)) if a == b)` negated — the second element is always None, so `passable` is always false and the function always returns Some(CROSS_ACCOUNT_PASS_ROLE_DENIED). create_model_customization_job.rs runs that check as the first statement of `handle` and returns 403 AccessDeniedException with message "Cross-account pass role is not allowed." before it reaches the store, the catalog, the quota or the mint. Consequence in replay: all four checks create an IAM role in the caller's own account with the bedrock.amazonaws.com trust statement, then call CreateModelCustomizationJob inside a retry wrapper that treats AccessDeniedException/ValidationException as IAM propagation lag and retries it for 90s. The emulator returns the same 403 on every attempt, the wrapper never exits, and the harness reports "Check timed out after 60.0s" for each of the four. No jobArn is ever minted, so GetModelCustomizationJob's InProgress status, its creationTime, StopModelCustomizationJob's transition to Stopped and the idempotent second Stop are all unreachable through the public API — the state machine that create_model_customization_job.rs, get_model_customization_job.rs and stop_model_customization_job.rs implement has no entry point.

Why: a client relying on AWS's behavior cannot start a model customization at all. Every CreateModelCustomizationJob is a 403 AccessDeniedException, which clients classify as non-retryable authorization failure, so an SDK call, a Terraform/CloudFormation apply or a test fixture that passes a perfectly passable same-account service role gets a hard permissions error instead of a 201 with a jobArn. Worse for automation, a caller that (correctly) treats a freshly created role's denial as IAM eventual consistency retries forever and hangs rather than failing, as these four checks demonstrate. And because no job can exist, nothing can exercise the documented status progression or Stop semantics: every downstream behavior of custom models built by customization (GetModelCustomizationJob status/creationTime, Stop -> Stopped, the idempotent no-op Stop on a terminal job, the custom model a completed job brings into being) is dead code behind the gate.

Fix: the pass-role authorization gate for this operation — `customization_pass_role_denial` / `CUSTOMIZATION_ASSUMING_ACCOUNT` in services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs, as called from the top of `handle` in ops/create_model_customization_job.rs. It must be a decision that can ADMIT: a roleArn whose account is the caller's own account (and which the account may pass to the Bedrock service principal) passes the gate and the handler proceeds to mint the job 201/InProgress; only a role the caller may not pass is the 403 AccessDeniedException with the fixed message "Cross-account pass role is not allowed." (no variable part), writing no job and no custom model. The reopened issue's ordering requirement stays: the decision is still taken before baseModelIdentifier resolution, so a role that may not be passed is still a 403 and not a 404 about the base model.

Traces: bedrock.model-customization-job-stop-transitions, bedrock.model-customization-job-stop-transitions-to-stopped, bedrock.stop-terminated-model-customization-job-is-an-idempotent-no-op, bedrock.timestamp-precision-preserved-model-customization-job — all four report "Check timed out after 60.0s" because the create is refused on every retry. Gate: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs lines 2805-2860 (CUSTOMIZATION_ASSUMING_ACCOUNT, customization_pass_role_denial); handler: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_customization_job.rs lines 209-224.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): bedrock 50/75 passed, 25 failed. Four of those failures are this cause (the rest are ALREADY FILED / OWNED by other issues).

Cause 1, check bedrock.timestamp-precision-preserved-model-customization-job [mismatch]:
  expected: CreateModelCustomizationJob with a same-account role returns 201 + jobArn; GetModelCustomizationJob returns a stable creationTime inside the wall-clock window that bracketed the create
  actual:   "Error: Check timed out after 60.0s" — the create is answered 403 AccessDeniedException "Cross-account pass role is not allowed." on every attempt, so the check's _retry_until_ready(timeout_s=90) wrapper (which retries AccessDeniedException as IAM propagation lag) never returns and no jobArn is ever obtained.

Cause 1, checks bedrock.model-customization-job-stop-transitions and bedrock.model-customization-job-stop-transitions-to-stopped [mismatch]:
  expected: create -> status InProgress; StopModelCustomizationJob -> status Stopped with jobArn unchanged
  actual:   "Error: Check timed out after 60.0s" in the same create-retry loop (_call_with_role_propagation_retry / _retry_transient, both of which retry AccessDeniedException and ValidationException for 90s). Stop is never reached.

Cause 1, check bedrock.stop-terminated-model-customization-job-is-an-idempotent-no-op [mismatch]:
  expected: Stop on a running job -> Stopped; a second Stop on the now-terminal job succeeds as a no-op and does not change the terminal state
  actual:   "Error: Check timed out after 60.0s" in the create-retry loop (_retry_transient, 90s, retries AccessDeniedException).

Source facts (this checkout):
  services/.../src/state.rs:2812  const CUSTOMIZATION_ASSUMING_ACCOUNT: Option<&str> = None;
  services/.../src/state.rs:2854  pub fn customization_pass_role_denial(role_arn: &str) -> Option<String> { let passable = matches!((role_arn_account(role_arn), CUSTOMIZATION_ASSUMING_ACCOUNT), (Some(role_account), Some(assuming_account)) if role_account == assuming_account); (!passable).then(|| CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string()) }  // second element is always None => always Some(..)
  services/.../src/state.rs:2775  pub const CROSS_ACCOUNT_PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";
  services/.../src/ops/create_model_customization_job.rs:215  the first statement of handle() returns AccessDeniedException from that helper, before any store/catalog/quota work.
  The handler's own doc comment states the condition explicitly: "the far side names no account, so the comparison fails for every value the member can carry: a role of the very account making the call included".

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): CreateModelCustomizationJob POST /model-customization-jobs, success 201, output jobArn required; roleArn required, RoleArn len ..2048, pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$, documented as requiring iam:PassRole; AccessDeniedException httpError 403 with a single `message` member; ModelCustomizationJobStatus = InProgress | Completed | Failed | Stopping | Stopped.

## Steps to Reproduce
Boot the emulator from this checkout and replay the four named checks (suite simparity-behavioral-ga-prod-v1), or by hand:

1. S3 CreateBucket <bucket> and PutObject s3://<bucket>/train.jsonl with ~32 JSONL {"prompt","completion"} records.
2. IAM CreateRole <role> in the SAME account as the caller, trust policy allowing sts:AssumeRole for Principal Service bedrock.amazonaws.com; PutRolePolicy granting s3:GetObject/s3:PutObject/s3:ListBucket on that bucket. Take the ARN from GetRole (arn:aws:iam::<caller-account>:role/<role>).
3. ListFoundationModels(byCustomizationType=FINE_TUNING) and pick an ACTIVE model whose outputModalities contains TEXT; take its modelArn.
4. CreateModelCustomizationJob(jobName=<name>, customModelName=<model-name>, roleArn=<that same-account role ARN>, baseModelIdentifier=<that modelArn>, trainingDataConfig.s3Uri=s3://<bucket>/train.jsonl, outputDataConfig.s3Uri=s3://<bucket>/output/, hyperParameters={epochCount:1,batchSize:1,learningRate:0.00001}).
   Expect: HTTP 201 with a jobArn.
   Observe: HTTP 403, code AccessDeniedException, message "Cross-account pass role is not allowed." — and the same answer for every retry and for every other roleArn value, including any other well-formed same-account role ARN.
5. Because step 4 never succeeds, confirm the unreachable remainder: there is no jobArn to pass to GetModelCustomizationJob (expected status InProgress with a stable creationTime) or to StopModelCustomizationJob (expected transition to Stopped, and a second Stop on the terminal job expected to succeed as a no-op).
6. A client that retries AccessDeniedException as IAM eventual consistency (as all four checks do, for 90s) hangs instead of ever creating a job; the harness kills each check at 60s.
