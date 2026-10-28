Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.e2e-fine-tune-deploy-invoke-teardown
Issue-Key: bedrock.e2e-fine-tune-deploy-invoke-teardown|run#16|AssertionError|assert code == "AccessDeniedException", (code, message)
Issue-Commit: c0eb4295fc6918b9011f996ea325c28045b54b65

# CreateModelCustomizationJob is now accepted for every well-formed role ARN: the iam:PassRole gate admits where AWS answers 403 AccessDeniedException "Cross-account pass role is not allowed.", and a job + custom model are written (1 cause)

> _Note: this issue was identified against the code base at commit `c0eb4295fc6918b9011f996ea325c28045b54b65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelCustomizationJob

## Description
This issue carries 1 root cause, numbered below. It must be closed only when that cause is fixed and the check listed under Traces re-replays clean.

Context on prior work (not a REOPENS: the emulator does NOT refuse here, it ACCEPTS, so the "its fix left the call refused" case does not apply). Two resolved issues have already moved this gate in opposite directions: 20261025T105655Z-bedrock-createmodelcustomizationjob-never-applies-the-pass-role-accessdeniedexception-the.md reported the create being ACCEPTED with no pass-role decision, and 20261027T152401Z-bedrock-createmodelcustomizationjob-denies-every-rolearn-with-403-cross-account.md reported the resulting gate denying EVERY roleArn unconditionally. The gate is now the other extreme again: it denies only a string that does not parse as an IAM role ARN, so every real roleArn is admitted and the recorded refusal is gone. The fix must land a gate that is genuinely conditional rather than swinging between "always admit" and "always deny".

=== Cause 1 of 1: the pass-role gate for CreateModelCustomizationJob admits every parseable role ARN, so a create AWS refuses 403 AccessDeniedException is served a 201 and mints a job ===

AWS: Smithy (com.amazonaws.bedrock#AmazonBedrockControlPlaneService) models CreateModelCustomizationJob as POST /model-customization-jobs, success status 201, output member jobArn (ModelCustomizationJobArn, @required). roleArn is @required with RoleArn @length max 2048 and @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$, documented as "the ARN of an IAM service role that Amazon Bedrock can assume ... the caller of this API must have the iam:PassRole permission". Modeled errors include AccessDeniedException (@httpError 403, single `message` member, documented "The request is denied because of missing access permissions."), ValidationException (400), ResourceNotFoundException (404), ServiceQuotaExceededException (400), ConflictException (400), TooManyTagsException (400), ThrottlingException (429). The model declares the 403 but not the predicate behind it, so for the refusal itself: recorded behavior only. The recording this check replays submits a role of the CALLER's own account carrying the documented bedrock.amazonaws.com trust statement (with the aws:SourceAccount / aws:SourceArn confused-deputy conditions) plus an inline s3:GetObject/PutObject/ListBucket policy on the training bucket, and AWS answers HTTP 403 with error code AccessDeniedException and a message whose text contains "cross-account pass role" — the message this checkout already models as the fixed string "Cross-account pass role is not allowed." (entirely fixed text; it interpolates no role name, no account id and no other request or state value). The same recording then shows that the refusal leaves NO state behind: ListModelCustomizationJobs(nameContains=<jobName>) returns modelCustomizationJobSummaries == [] and ListCustomModels(nameContains=<customModelName>) returns modelSummaries == []. Hypothesis (neither the model nor the recording states it): the gate in that environment is an account-level restriction on model customization rather than a property of the particular role.

Emulator: the create is ADMITTED and returns 201 with a minted jobArn. services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_customization_job.rs runs `crate::state::customization_pass_role_denial(&input.role_arn)` as the first statement of `handle`, but that helper in src/state.rs is now `::service_common::iam_role::role_name(role_arn).is_none().then(|| CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string())` — it denies only a value that does not parse as an IAM role ARN at all. Because the member's own modeled @pattern already guarantees the value parses before the handler runs, the gate can never fire for any request that reaches it: every real roleArn is passable. The handler therefore falls through to the tag check, the customizationType resolution, the idempotency-token lookup and the mint, and writes both the job and the pending custom model. In replay the check's helper `_error_code_and_message` returned (None, None) — its sentinel for "the call did not raise" — against an expected ("AccessDeniedException", "...cross-account pass role..."), i.e. a 2xx where AWS answered 403.

Why: a client relying on AWS's behavior is told its fine-tuning job started when AWS would have refused it. It receives a 201 and a jobArn that, on AWS, never exists; it then polls GetModelCustomizationJob and lists custom models for a customization that real AWS never began. Any program whose correctness depends on the authorization outcome — a pre-flight capability probe, a CI gate that asserts customization is blocked for this account, an IaC apply that branches on AccessDeniedException, or a cost control that relies on customization being denied — takes the wrong branch, and a test suite validating the denial passes against the emulator and fails against AWS. It also leaks state AWS does not create: a job summary and a custom-model name become visible to ListModelCustomizationJobs / ListCustomModels where AWS shows an empty list after the refusal.

Fix: the iam:PassRole authorization gate for this operation — `customization_pass_role_denial` in services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs, as called from the top of `handle` in src/ops/create_model_customization_job.rs. It must be a real decision rather than a well-formedness test of the ARN, and in the environment the corpus records it must REFUSE: HTTP 403, code AccessDeniedException, message exactly "Cross-account pass role is not allowed." (fixed text, no interpolated values), taken before baseModelIdentifier is resolved so a denied pass is still a 403 and not a 404 about the base model, and writing nothing — no entry in `model_customization_jobs` / `model_customization_job_names`, no custom-model entry, and no `model_customization_job_create_tokens` entry — so the subsequent ListModelCustomizationJobs and ListCustomModels come back empty. Reconcile with 20261027T152401Z, which asked that the gate have an admitting path: the predicate belongs in state.rs and must be able to answer both ways, not be hardwired to one verdict in either direction.

Traces: bedrock.e2e-fine-tune-deploy-invoke-teardown.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, service bedrock: 2/9 passed, 7 failed. Six of the seven are ALREADY FILED (ApplyGuardrail scaffold stub) or OWNED by existing open issues; this cause is the one remaining failure.

Cause 1, check bedrock.e2e-fine-tune-deploy-invoke-teardown [mismatch]:
  expected: CreateModelCustomizationJob(jobName, customModelName, roleArn=<same-account role with bedrock.amazonaws.com trust>, baseModelIdentifier=<ACTIVE FINE_TUNING TEXT model from ListFoundationModels>, trainingDataConfig.s3Uri, outputDataConfig.s3Uri, jobTags, customModelTags) is refused HTTP 403 with Error.Code == "AccessDeniedException" and an Error.Message containing "cross-account pass role" (case-insensitive); afterwards ListModelCustomizationJobs(nameContains=jobName).modelCustomizationJobSummaries == [] and ListCustomModels(nameContains=customModelName).modelSummaries == [].
  actual:   the call SUCCEEDED (201 + jobArn). The check's `_error_code_and_message` helper returns (None, None) when `fn()` does not raise, and the replay reports:
    File "check_bedrock_e2e_fine_tune_deploy_invoke_teardown.py", line 53, in run
      assert code == "AccessDeniedException", (code, message)
    AssertionError: (None, None)
  The check's Converse control before this step passed (non-empty text), so the failure is isolated to the CreateModelCustomizationJob authorization outcome.

Source facts (this checkout):
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:2828-2832
    pub fn customization_pass_role_denial(role_arn: &str) -> Option<String> {
        ::service_common::iam_role::role_name(role_arn).is_none()
            .then(|| CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string())
    }
    -> fires only when the string is not an IAM role ARN; the member's modeled @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$ already excludes that, so the gate never fires in practice.
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_customization_job.rs:209-224
    the first statement of handle() consults that helper and returns AccessDeniedException { message } on Some(..); with None it falls through to the tag bound, customizationType resolution, the idempotency-token replay and the mint.
  CROSS_ACCOUNT_PASS_ROLE_DENIED is still declared in state.rs as the fixed string "Cross-account pass role is not allowed." — the wording is present, only the predicate no longer selects it.

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json):
  CreateModelCustomizationJob — POST /model-customization-jobs, success 201, @idempotent; output jobArn @required (ModelCustomizationJobArn, @length max 1011).
  roleArn @required, RoleArn @length max 2048, @pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$.
  errors: AccessDeniedException 403, ConflictException 400, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 400, ThrottlingException 429, TooManyTagsException 400, ValidationException 400.
  ListModelCustomizationJobs — GET /model-customization-jobs, 200, items modelCustomizationJobSummaries; ListCustomModels — GET /custom-models, 200, items modelSummaries.

Related issues (neither covers this cause): resolved 20261025T105655Z-bedrock-createmodelcustomizationjob-never-applies-the-pass-role-accessdeniedexception-the.md and resolved 20261027T152401Z-bedrock-createmodelcustomizationjob-denies-every-rolearn-with-403-cross-account.md.

## Steps to Reproduce
Boot the emulator from this checkout and replay bedrock.e2e-fine-tune-deploy-invoke-teardown (suite simparity-behavioral-ga-prod-v1), or by hand:

1. bedrock-runtime Converse against any ACTIVE on-demand TEXT model (from ListFoundationModels(byOutputModality=TEXT)) to confirm the control path answers; the check only requires non-empty text here.
2. s3 CreateBucket <bucket> (with CreateBucketConfiguration.LocationConstraint=<region> outside us-east-1), PutBucketTagging, then PutObject s3://<bucket>/train.jsonl with 64 JSONL records of {"prompt": ..., "completion": "positive"}.
3. sts GetCallerIdentity to take <account>. iam CreateRole <role> with a trust policy allowing sts:AssumeRole for Principal Service bedrock.amazonaws.com, conditioned on StringEquals aws:SourceAccount = <account> and ArnEquals aws:SourceArn = arn:aws:bedrock:<region>:<account>:model-customization-job/*; PutRolePolicy granting s3:GetObject/s3:PutObject/s3:ListBucket on arn:aws:s3:::<bucket> and arn:aws:s3:::<bucket>/*. Take the ARN from GetRole — it is a role of the CALLER's own account.
4. ListFoundationModels(byOutputModality=TEXT); pick the first ACTIVE model whose customizationsSupported contains FINE_TUNING as <base-model-id>.
5. bedrock CreateModelCustomizationJob(jobName=<job>, customModelName=<model>, roleArn=<role ARN from step 3>, baseModelIdentifier=<base-model-id>, trainingDataConfig={"s3Uri":"s3://<bucket>/train.jsonl"}, outputDataConfig={"s3Uri":"s3://<bucket>/output/"}, jobTags=[...], customModelTags=[...]).
   Expect (recorded AWS): HTTP 403, Error.Code "AccessDeniedException", Error.Message "Cross-account pass role is not allowed." — fixed text, no request- or state-derived values.
   Observe: HTTP 201 with a jobArn; no error is raised.
6. bedrock ListModelCustomizationJobs(nameContains=<job>) and ListCustomModels(nameContains=<model>).
   Expect (recorded AWS): modelCustomizationJobSummaries == [] and modelSummaries == [] — the refusal left nothing behind.
   Observe: the job minted in step 5 (and its pending custom model) are present, because the create was admitted.
7. Vary roleArn over any other well-formed value admitted by the modeled pattern (another same-account role, another account's role, arn:aws:iam:::role/x with an empty account field): every one is admitted, confirming the gate decides on ARN well-formedness only.
