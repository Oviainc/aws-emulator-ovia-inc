Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-model-invocation-logging-config-without-permission-denied bedrock.create-custom-model-missing-permission-denied bedrock.guardrail-kms-key-id-and-alias-resolve-to-key-arn bedrock.create-model-invocation-job-malformed-s3-uri-rejected bedrock.model-invocation-job-timeout-transitions-to-failed bedrock.model-import-job-progression
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.delete-model-invocation-logging-config-without-permission-denied|_error#33|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock.create-custom-model-missing-permission-denied|run#18|Timeout|time.sleep(3.0)
Issue-Key: bedrock.guardrail-kms-key-id-and-alias-resolve-to-key-arn|run#22|AssertionError|assert after_update.get("kmsKeyArn") == key_arn, (
Issue-Key: bedrock.create-model-invocation-job-malformed-s3-uri-rejected|run#15|AssertionError|assert bad_message == expected_message, f"expected message {expected_message!r}, got {bad_message!r}"
Issue-Key: bedrock|CreateModelInvocationJob|ValidationException
Issue-Key: bedrock|PutObject|ValidationException
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# bedrock control plane: no IAM authorization gate, guardrail kmsKeyArn dropped, validation messages echo the rejected value, and the emulator's 16 MiB per-bucket S3 cap blocks model import

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteModelInvocationLoggingConfiguration, CreateCustomModel, UpdateGuardrail/GetGuardrail, CreateModelInvocationJob, PutObject

## Description
Five distinct root causes found on the Bedrock control plane by the behavioral suite. All five are in the same service (bedrock) and are bundled here; a fixer must close ALL FIVE before submitting.

=== Cause 1 of 5: DeleteModelInvocationLoggingConfiguration performs no authorization check and succeeds for a principal with no permission ===
AWS: Smithy (com.amazonaws.bedrock) models DeleteModelInvocationLoggingConfiguration with errors AccessDeniedException (httpError 403), InternalServerException (500), ThrottlingException (429); HTTP binding DELETE /logging/modelinvocations, success code 200. A caller whose identity has no IAM permission for bedrock:DeleteModelInvocationLoggingConfiguration is rejected with HTTP 403 / error code AccessDeniedException and the account's logging configuration is left untouched. Recorded behavior of the check confirms the denial is expected before any resource work happens.
Emulator: the call made with the low-privilege principal returns HTTP 200 with an empty JSON body ({}), i.e. the delete is actually performed. The replay shows: AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...'content-length': '2'...}}.
Why: a client (or an IAM policy test, or a least-privilege CI gate) that relies on Bedrock refusing unauthorized mutations sees them silently applied. An account-singleton configuration can be destroyed by a principal that AWS would have denied, so negative-authorization tests pass against the emulator and fail in production — and the emulator cannot be used to validate least-privilege policies at all.
Fix: the identity/authorization gate in front of the bedrock control-plane handlers (the operation's handler plus whatever shared request-authorization layer the crate has), so modelled AccessDeniedException is returned with status 403 when the caller's identity lacks the action.
Traces: bedrock.delete-model-invocation-logging-config-without-permission-denied; handler services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/delete_model_invocation_logging_configuration.rs.

=== Cause 2 of 5: CreateCustomModel by an unprivileged principal never answers — the call does not complete and no AccessDeniedException is ever observed ===
AWS: Smithy models CreateCustomModel with errors AccessDeniedException (403), ValidationException (400), ConflictException (400), ResourceNotFoundException (404), ServiceQuotaExceededException (400), ThrottlingException (429), TooManyTagsException (400); HTTP POST /custom-models/create-custom-model, success code 202. IAM authorization is evaluated before Bedrock's own resource validation, so a principal lacking bedrock:CreateCustomModel gets HTTP 403 / AccessDeniedException promptly, and no custom-model resource is created; the same request from a fully-permitted principal is NOT denied for that reason (it proceeds to request validation).
Emulator: the check never reaches its assertions — it is reported as 'Check timed out after 60.0s'. Within the 60s budget the sequence (create role, assume it, call CreateCustomModel) produced neither the expected denial nor any other terminal response, so the authorization boundary is unobservable. Hypothesis: the same missing authorization layer as Cause 1 leaves the unprivileged CreateCustomModel call waiting/retrying rather than being refused quickly.
Why: a client cannot distinguish 'denied' from 'hung'; a negative-authorization test and any least-privilege validation hang instead of getting the modelled 403, and a request AWS answers in one round trip costs the caller its whole timeout budget.
Fix: the same authorization gate plus CreateCustomModel's handler, so the operation answers a terminal response (modelled AccessDeniedException 403 for an unpermitted identity) instead of not completing.
Traces: bedrock.create-custom-model-missing-permission-denied; handler services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_custom_model.rs.

=== Cause 3 of 5: UpdateGuardrail's kmsKeyId is not persisted/resolved — GetGuardrail returns no kmsKeyArn ===
AWS: Smithy UpdateGuardrail input member kmsKeyId targets KmsKeyId, length 1..2048, pattern permitting a KMS key ARN, an alias ARN (alias/<name>), a bare 36-char key id or a bare alias/<name>. GetGuardrail output member kmsKeyArn targets KmsKeyArn with pattern ^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$ — a KEY ARN only, with no representation for an alias ARN. So a guardrail configured with a customer-managed key reports that key's ARN on read, and an alias passed on update must be resolved to the underlying key ARN it targets (the field cannot hold the alias form).
Emulator: after CreateGuardrail with a full key ARN and UpdateGuardrail with that key's alias ARN, GetGuardrail returns kmsKeyArn absent (None) — neither the resolved key ARN nor the alias. The replay shows: AssertionError: GetGuardrail after UpdateGuardrail(kmsKeyId='arn:aws:kms:...:alias/...') must resolve to the SAME underlying key ARN '...:key/<uuid>' the alias targets, not the alias's own ARN; got None.
Why: a client reading back a guardrail cannot tell whether it is encrypted with its own CMK or with the service key, cannot audit which key protects the guardrail, and any code that compares the configured key identity across create/update (or resolves an alias once and pins the key ARN) breaks. Dropping the member also makes UpdateGuardrail's kmsKeyId look accepted-but-ignored.
Fix: UpdateGuardrail's handler/state (persist kmsKeyId on the guardrail, resolving an alias id or alias ARN to the target key's ARN via KMS) and GetGuardrail's serializer (emit kmsKeyArn whenever the guardrail has one).
Traces: bedrock.guardrail-kms-key-id-and-alias-resolve-to-key-arn; handlers services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/update_guardrail.rs and .../get_guardrail.rs.

=== Cause 4 of 5: constraint-violation messages embed the rejected value, diverging from Bedrock's validation-message rule ===
AWS: recorded behavior only (the model states the constraints, not the message text). Smithy: CreateModelInvocationJob.timeoutDurationInHours targets ModelInvocationJobTimeoutDurationInHours with range min 24 / max 168; inputDataConfig.s3InputDataConfig.s3Uri targets S3Uri with pattern ^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/[-!_*'().a-z0-9A-Z]+(?:/[-!_*'().a-z0-9A-Z]+)*)?/?$; both violations are ValidationException (httpError 400, member `message`). The rule the two independent recordings agree on is the message TEMPLATE Bedrock renders: "<N> validation error detected: Value at '<member path>' failed to satisfy constraint: <constraint phrase>" — the member path and the constraint phrase are built from the request, and the offending VALUE is omitted. Examples recorded: "1 validation error detected: Value at 'inputDataConfig.s3InputDataConfig.s3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://..." and "1 validation error detected: Value at 'timeoutDurationInHours' failed to satisfy constraint: Member must have value greater than or equal to 24".
Emulator: returns the right status and error code but renders the other template, interpolating the rejected value: "1 validation error detected: Value 'ftp://not-a-valid-s3-uri/records.jsonl' at 'inputDataConfig.s3InputDataConfig.s3Uri' failed to satisfy constraint: ..." and "1 validation error detected: Value '1' at 'timeoutDurationInHours' failed to satisfy constraint: Member must have value greater than or equal to 24".
Why: clients and test suites that match Bedrock's validation text (and log scrapers built on it) do not match, and echoing the submitted value back into an error string reflects request content — including an S3 location — into logs that AWS deliberately keeps value-free for this API.
Fix: the shared constraint-validation message renderer used by the bedrock control-plane handlers (the component that formats pattern/range violations), so it emits "Value at '<path>'" without interpolating the value.
Traces: bedrock.create-model-invocation-job-malformed-s3-uri-rejected, bedrock.model-invocation-job-timeout-transitions-to-failed; validation for services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_invocation_job.rs.

=== Cause 5 of 5: the emulator's S3 refuses PutObject past a per-bucket byte cap, so a model-import flow cannot stage its artifacts ===
AWS: recorded behavior only. S3 PutObject has no per-bucket aggregate size limit (buckets are unlimited in total size; the per-object limit is 5 TiB), so staging a multi-megabyte model checkpoint plus tokenizer under one bucket succeeds with HTTP 200, and CreateModelImportJob can then be driven to a terminal state. The check writes an in-process from-scratch Llama checkpoint (hidden_size 64, 2 attention heads, vocab_size 32000) and a fast-tokenizer tokenizer.json, together ~17 MB.
Emulator: PutObject of "model/tokenizer.json" is rejected with HTTP 400 ValidationException: 'storing "model/tokenizer.json" would leave the bucket "sim2real-bedrock-import-..." holding 17119868 bytes of objects, past the 16777216 bytes this emulator keeps per bucket'. The import-job progression is never exercised: the check errors out during setup.
Why: an emulator-invented quota (16 MiB per bucket) rejects a request AWS accepts, with an error code AWS never returns for it. Any workflow that stages realistically-sized artifacts in S3 — model import, batch-inference inputs, build/deploy bundles — fails against the emulator for a reason no client can anticipate or handle, and it makes whole Bedrock state machines (CreateModelImportJob → InProgress → Completed) untestable.
Why/attribution note: the rejecting operation is S3's PutObject; this cause is filed under the bedrock service because it is grounded in the bedrock check id bedrock.model-import-job-progression, per the check-id service rule.
Fix: the S3 object-storage state/quota layer of the emulator (the per-bucket accounting that raises this ValidationException) — raise or remove the aggregate per-bucket cap so PutObject accepts multi-megabyte artifacts, and never report an invented quota as a modelled AWS error.
Traces: bedrock.model-import-job-progression.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), bedrock 11/75 passed.

Cause 1 — bedrock.delete-model-invocation-logging-config-without-permission-denied [mismatch]:
  expected: HTTP 403, error code AccessDeniedException (modelled on DeleteModelInvocationLoggingConfiguration), config unchanged
  actual:   AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Cause 2 — bedrock.create-custom-model-missing-permission-denied [mismatch]:
  expected: prompt HTTP 403 / AccessDeniedException for the principal lacking bedrock:CreateCustomModel, then the same request from a bedrock:*-permitted principal not denied for that reason
  actual:   Error: Check timed out after 60.0s — no terminal response observed for the unprivileged CreateCustomModel call

Cause 3 — bedrock.guardrail-kms-key-id-and-alias-resolve-to-key-arn [mismatch]:
  expected: GetGuardrail.kmsKeyArn == 'arn:aws:kms:<region>:<account>:key/<36-char key id>' (the key the supplied alias targets); model pattern for KmsKeyArn admits only key/ ARNs
  actual:   AssertionError: ... got None (kmsKeyArn absent from GetGuardrail after UpdateGuardrail(kmsKeyId=<alias ARN>))

Cause 4 — bedrock.create-model-invocation-job-malformed-s3-uri-rejected and bedrock.model-invocation-job-timeout-transitions-to-failed [mismatch, both]:
  expected message (recorded AWS, value omitted): "1 validation error detected: Value at 'inputDataConfig.s3InputDataConfig.s3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/[-!_*'().a-z0-9A-Z]+(?:/[-!_*'().a-z0-9A-Z]+)*)?/?"
  actual:   "1 validation error detected: Value 'ftp://not-a-valid-s3-uri/records.jsonl' at 'inputDataConfig.s3InputDataConfig.s3Uri' failed to satisfy constraint: ..."
  expected message (recorded AWS): "1 validation error detected: Value at 'timeoutDurationInHours' failed to satisfy constraint: Member must have value greater than or equal to 24"
  actual:   "1 validation error detected: Value '1' at 'timeoutDurationInHours' failed to satisfy constraint: Member must have value greater than or equal to 24"
  (status 400 / ValidationException agree in both cases; only the rendered template differs)

Cause 5 — bedrock.model-import-job-progression [error]:
  expected: PutObject of the ~17 MB checkpoint + tokenizer set succeeds (HTTP 200), CreateModelImportJob then progresses to a terminal state
  actual:   botocore ClientError (ValidationException) on PutObject: 'storing "model/tokenizer.json" would leave the bucket "sim2real-bedrock-import-16bea4bb46b6" holding 17119868 bytes of objects, past the 16777216 bytes this emulator keeps per bucket'

Related open issues (do NOT cover these causes): 20261023T232022Z-bedrock-catalogued-foundation-model-refused-as-invalid-responses.md (foundation-model catalog / request-id / CFN guardrail issues) and 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md (runtime data plane). Neither names the control-plane authorization gate, the guardrail kmsKeyArn read-back, the validation-message template, or the S3 per-bucket cap.

## Steps to Reproduce
Boot the emulator from this checkout and replay the named checks (suite simparity-behavioral-ga-prod-v1):

Cause 1: create an IAM role/principal with no bedrock permissions, assume it, and call DeleteModelInvocationLoggingConfiguration (DELETE /logging/modelinvocations) after a baseline PutModelInvocationLoggingConfiguration. Expect 403 AccessDeniedException; observe 200 {} and the configuration removed. Replay bedrock.delete-model-invocation-logging-config-without-permission-denied.

Cause 2: create a role whose policy omits bedrock:CreateCustomModel, assume it, and call CreateCustomModel with modelName + modelSourceConfig.s3DataSource.s3Uri + roleArn. Expect a prompt 403 AccessDeniedException; observe the call not completing (check times out at 60s). Replay bedrock.create-custom-model-missing-permission-denied.

Cause 3: kms:CreateKey, kms:CreateAlias; CreateGuardrail with kmsKeyId = full key ARN, wait for status READY, then UpdateGuardrail with kmsKeyId = the alias ARN and call GetGuardrail. Expect kmsKeyArn == the key ARN the alias targets; observe kmsKeyArn missing. Replay bedrock.guardrail-kms-key-id-and-alias-resolve-to-key-arn.

Cause 4: with client-side param validation disabled, call CreateModelInvocationJob twice — once with inputDataConfig.s3InputDataConfig.s3Uri = 'ftp://not-a-valid-s3-uri/records.jsonl', once with timeoutDurationInHours = 1 — and read the ValidationException message. Expect "Value at '<member>' failed to satisfy constraint: ..."; observe "Value '<rejected value>' at '<member>' ...". Replay bedrock.create-model-invocation-job-malformed-s3-uri-rejected and bedrock.model-invocation-job-timeout-transitions-to-failed.

Cause 5: CreateBucket, then PutObject a set of objects totalling more than 16777216 bytes into that one bucket (e.g. a ~17 MB model checkpoint plus tokenizer.json). Expect 200 for each PutObject; observe 400 ValidationException naming the emulator's per-bucket byte budget. Replay bedrock.model-import-job-progression.
