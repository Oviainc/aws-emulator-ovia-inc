Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.create-custom-model-missing-permission-denied bedrock.custom-model-deployment-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.create-custom-model-missing-permission-denied|_error_code_and_message#17|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock.custom-model-deployment-lifecycle-transitions|_error#8|AssertionError|raise AssertionError("the call was ACCEPTED where a refusal was expected")
Issue-Commit: a577dca75911a15e8d14eba87791fcef54f4bfc0

# CreateCustomModel accepts requests AWS refuses: no IAM authorization gate (explicit Deny ignored) and no Amazon-managed-bucket check on modelSourceConfig

> _Note: this issue was identified against the code base at commit `a577dca75911a15e8d14eba87791fcef54f4bfc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomModel

## Description
Two distinct root causes on the Bedrock control plane's CreateCustomModel, both found by the behavioral suite, both in service `bedrock` and bundled here. A fixer must close BOTH causes before submitting.

Related open issue, which does NOT cover either cause as filed: 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md. Its Cause 2 recorded CreateCustomModel by an unprivileged principal as 'Check timed out after 60.0s' (no terminal response). On this replay the call now answers promptly — with HTTP 202 and a minted modelArn — so the observable divergence has changed shape (acceptance, not a hang) while the authorization gate is still absent, and the replay key that issue carries no longer matches this failure. Cause 2 below (the Amazon-managed-bucket pre-condition) is named in no open issue at all.

=== Cause 1 of 2: CreateCustomModel performs no authorization check — a principal with an explicit Deny on bedrock:CreateCustomModel gets 202 and a custom model is created ===
AWS: Smithy (com.amazonaws.bedrock#CreateCustomModel) models errors AccessDeniedException (httpError 403), ValidationException (400), ConflictException (400), ResourceNotFoundException (404), ServiceQuotaExceededException (400), ThrottlingException (429), TooManyTagsException (400); HTTP binding POST /custom-models/create-custom-model with success code 202. AccessDeniedException is documented as 'The request is denied because of missing access permissions.' IAM authorization is evaluated BEFORE Bedrock's own resource validation, so a caller whose identity carries an explicit Deny for the action is refused with HTTP 403 / AccessDeniedException and NO custom-model resource is created. The recorded behavior shows the denial message is built from the request and the evaluated policy (principal ARN, the action bedrock:CreateCustomModel, the custom-model resource ARN, and the fact that the decision came from an explicit deny in an identity-based policy), so that construction — not any one recorded string — is the requirement.
Emulator: the call from the assumed role carrying the explicit Deny SUCCEEDS: HTTP 202 with body {"modelArn": "arn:aws:bedrock:<region>:<account>:custom-model/imported/<12-char id>"}. No identity is consulted anywhere in the handler, so no 403 can ever be produced and the model is actually registered in state.
Why: every negative-authorization expectation inverts. A client, IAM policy test, or least-privilege CI gate that relies on Bedrock refusing an unauthorized mutation sees the mutation applied instead; code that handles AccessDeniedException is never exercised; and the emulator cannot be used to validate least-privilege policies for the control plane at all. It is also a resource-leak path: a principal AWS would have denied creates account state.
Fix: the request-authorization layer in front of the bedrock control-plane handlers (plus this operation's handler, which currently never consults the caller's identity), so a caller lacking — or explicitly denied — the action is answered with the modeled AccessDeniedException at HTTP 403 before any state is written.
Traces: bedrock.create-custom-model-missing-permission-denied; handler services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_custom_model.rs (the `handle` body at lines 191-358 validates members and mints the model with no authorization step).

=== Cause 2 of 2: modelSourceConfig naming a customer-owned S3 bucket is accepted, where AWS refuses it ===
AWS: recorded behavior only for the message; the model and its documentation state the pre-condition. Smithy: CreateCustomModel.modelSourceConfig targets the union ModelDataSource → S3DataSource.s3Uri (required, length 1..1024, pattern ^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/[-!_*'().a-z0-9A-Z]+(?:/[-!_*'().a-z0-9A-Z]+)*)?/?$); the member documentation is 'The Amazon S3 URI in the model source must be for the Amazon-managed Amazon S3 bucket containing your model artifacts', and the operation doc repeats it ('modelSourceConfig — Specify an Amazon S3 URI pointing to the Amazon-managed Amazon S3 bucket'). A syntactically valid s3:// URI that points at a caller-owned bucket therefore does not satisfy the pre-condition: the recording shows it refused with HTTP 400 / ValidationException whose message is 'S3 URI not supported', and the rejected modelName never appears in ListCustomModels.
Emulator: the same request — a freshly created caller-owned bucket holding model/config.json, with a service role granting s3:GetObject/PutObject/ListBucket on it — is ACCEPTED (HTTP 202, modelArn minted) and the model is registered, so a subsequent ListCustomModels(nameContains=<name>) reports it. The handler checks only presence/exclusivity of modelSourceConfig vs customModelDataSource, the conditional requirement of roleArn, the tag count, name uniqueness and the per-account quota; the s3Uri's bucket is stored verbatim and never evaluated.
Why: a client that handles AWS's refusal for an unsupported S3 source never sees it, and the emulator invents a success path AWS does not have — it mints a custom-model ARN and a Creating→Active progression for artifacts AWS would not have taken. Any flow written against the emulator (import a model from your own bucket, then deploy/invoke it) cannot work in production, and negative tests that assert the refusal pass only against real AWS.
Fix: CreateCustomModel's input validation in the operation's handler — evaluate the modelSourceConfig S3 data source against the Amazon-managed-bucket pre-condition and answer the modeled ValidationException (HTTP 400) instead of minting a model, leaving nothing in state for ListCustomModels/GetCustomModel to report.
Traces: bedrock.custom-model-deployment-lifecycle-transitions; handler services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_custom_model.rs (member checks at lines 224-320, state write at lines 322-357).

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock 20/76 passed.

Cause 1 — bedrock.create-custom-model-missing-permission-denied [mismatch]:
  expected: HTTP 403, error code AccessDeniedException, for CreateCustomModel issued by an assumed role whose inline policy carries {"Effect":"Deny","Action":"bedrock:CreateCustomModel","Resource":"*"}; no custom model created.
  actual:   AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '88', ...}, 'RetryAttempts': 0}, 'modelArn': 'arn:aws:bedrock:us-west-2:000000000000:custom-model/imported/00000185e7ji'}
  (the denial message AWS builds names the assumed-role principal, the action bedrock:CreateCustomModel, the custom-model resource ARN and 'with an explicit deny in an identity-based policy'; it is quoted here as an example of the construction, the requirement being the 403/AccessDeniedException and that construction)

Cause 2 — bedrock.custom-model-deployment-lifecycle-transitions [mismatch]:
  expected: ('ValidationException', 'S3 URI not supported') at HTTP 400 for CreateCustomModel(modelName=<new>, modelSourceConfig={'s3DataSource': {'s3Uri': 's3://<caller-owned bucket>/model/'}}, roleArn=<role with s3 access>), and ListCustomModels(nameContains=<name>) == []
  actual:   AssertionError: the call was ACCEPTED where a refusal was expected (HTTP 202 with a minted modelArn; the model is then visible to ListCustomModels)
  positive control in the same check passed: ListCustomModels(maxResults=1) answers normally, so the acceptance is not a broken-account artifact.

## Steps to Reproduce
Boot the emulator from this checkout and replay the two named checks (suite simparity-behavioral-ga-prod-v1).

Cause 1 (bedrock.create-custom-model-missing-permission-denied): iam.CreateRole with a trust policy for the account root, iam.PutRolePolicy attaching an explicit Deny for bedrock:CreateCustomModel on Resource "*", sts.AssumeRole into it, then call CreateCustomModel(modelName=<unique>, modelSourceConfig={'s3DataSource': {'s3Uri': 's3://<any bucket>/artifacts/'}}, roleArn=arn:aws:iam::<account>:role/<any role>) with those credentials. Expect HTTP 403 / AccessDeniedException and no model created; observe HTTP 202 with a minted modelArn.

Cause 2 (bedrock.custom-model-deployment-lifecycle-transitions): s3.CreateBucket a caller-owned bucket and PutObject 'model/config.json' into it; iam.CreateRole a bedrock.amazonaws.com service role with s3:GetObject/PutObject/ListBucket on that bucket; call CreateCustomModel(modelName=<unique>, modelSourceConfig={'s3DataSource': {'s3Uri': 's3://<that bucket>/model/'}}, roleArn=<that role ARN>). Expect HTTP 400 ValidationException 'S3 URI not supported' and ListCustomModels(nameContains=<name>) == []; observe HTTP 202 with a minted modelArn and the model listed.
