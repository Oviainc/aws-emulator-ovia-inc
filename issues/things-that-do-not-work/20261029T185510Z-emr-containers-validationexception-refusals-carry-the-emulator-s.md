Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.job-template-client-token-reuse-conflict-rejected emr-containers.job-template-delete-twice-rejected emr-containers.security-configuration-create-describe-list-delete
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers.job-template-client-token-reuse-conflict-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: emr-containers.job-template-delete-twice-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: emr-containers.security-configuration-create-describe-list-delete|run#29|AssertionError|assert redelete_message == expected, (
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# emr-containers ValidationException refusals carry the emulator's own descriptive wording instead of AWS's fixed message text (job templates, security configurations, client-token conflicts)

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteJobTemplate

## Description
Three net-new root causes in the emr-containers emulator, all the same family of defect: the operation correctly refuses with HTTP 400 / `ValidationException`, but the `message` member is the emulator's own descriptive sentence (built by interpolating the requested id or clientToken) rather than the FIXED text AWS returns for that error class. A client that classifies or asserts on the refusal message therefore sees a message real AWS never emits. All three causes must be fixed and all three named checks must replay clean before this issue is closed.

Common structural note (Hypothesis, applies to Causes 2 and 3): both delete handlers reuse the service's single not-found wording helper `crate::state::ResourceKind::not_found_message(&id)`. That wording is CORRECT for the `ResourceNotFoundException` raised by the Describe* operations (the same checks assert `Job Template <id> doesn't exist.` and `Security Configuration <id> doesn't exist.` on describe-after-delete, and those assertions pass). It is the WRONG wording when the same miss is reported as `ValidationException` from a Delete*, which AWS words with a short fixed sentence that carries no id at all. The two error classes need separate wording.

Cause 1 of 3 — CreateJobTemplate: a clientToken replayed with DIFFERENT request parameters is refused with the wrong message.
AWS: Smithy: CreateJobTemplate (POST /jobtemplates) takes a required `clientToken` (ClientToken, @idempotencyToken, @length 1..64, @pattern \S) and models `ValidationException` at httpError 400. Recorded behavior: AWS refuses the conflicting replay with HTTP 400, error code `ValidationException`, and the message `At least one input parameter differs from a previous call using the same client token`. That message is entirely FIXED text — it has no variable parts, and in particular it does NOT name the token, the template name or the differing member.
Emulator: same status and same error code, but the message is `clientToken <token> was already used to create job template <templateId>, with different parameters` — i.e. a sentence of the emulator's own composition whose variable parts are the request's clientToken and the previously minted template id.
Why: a caller that distinguishes an idempotency-conflict from other `ValidationException`s does so on the message (the error code is shared by every validation refusal of this operation), so matching on AWS's fixed sentence never fires against the emulator. The emulator's message also leaks a resource id AWS does not disclose on this path, so a test or log scrubber written against AWS behaves differently here.
Fix: the clientToken replay/conflict path of CreateJobTemplate's handler/validation in the emr-containers service crate — emit the fixed AWS sentence, with no interpolated token or id, when a spent token is replayed with differing parameters. Note the open issue `20261025T143602Z-emr-containers-createvirtualcluster-wrong-behavior--r2.md` (a spent clientToken replayed without comparing the request) and `20261027T233546Z-emr-containers-createvirtualcluster-maintainability.md` (six copies of the replay/conflict rule): neither covers this defect — here the conflict IS detected and refused, only the message text diverges — but the shared helper those issues describe is where the wording should land once.
Traces: check emr-containers.job-template-client-token-reuse-conflict-rejected.

Cause 2 of 3 — DeleteJobTemplate on an already-deleted template is refused with the wrong message.
AWS: Smithy: DeleteJobTemplate (DELETE /jobtemplates/{id}) models exactly `InternalServerException` (500) and `ValidationException` (400) — no not-found error — so a second delete is correctly a `ValidationException`. Recorded behavior: HTTP 400, code `ValidationException`, message `Template does not exist`. The message is entirely FIXED text and carries no id.
Emulator: HTTP 400 and code `ValidationException` (correct), message `Job Template <id> doesn't exist.` — the describe-path not-found wording, with the requested id interpolated.
Why: the delete-twice refusal is indistinguishable from AWS's only by its message, so idempotent-teardown code that tolerates `Template does not exist` does not recognize the emulator's refusal and fails cleanup; conversely a caller that matches the describe wording would wrongly treat a validation refusal as a not-found.
Fix: the not-found branch of DeleteJobTemplate's handler in the emr-containers service crate — return the fixed AWS sentence for the ValidationException instead of calling the shared ResourceKind not-found wording.
Traces: check emr-containers.job-template-delete-twice-rejected; handler branch at services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/delete_job_template.rs:153-164.

Cause 3 of 3 — DeleteSecurityConfiguration on an already-deleted configuration is refused with the wrong message.
AWS: Smithy: DeleteSecurityConfiguration (DELETE /securityconfigurations/{id}) models `InternalServerException` (500) and `ValidationException` (400) only, while DescribeSecurityConfiguration additionally models `ResourceNotFoundException` (400). Recorded behavior: re-deleting a deleted configuration gives HTTP 400, code `ValidationException`, message `Security configuration not found` — entirely FIXED text, no id, and note the lower-case wording distinct from the describe path's `Security Configuration <id> doesn't exist.` (which the same check asserts and which the emulator already gets right).
Emulator: HTTP 400 and code `ValidationException` (correct), message `Security Configuration <id> doesn't exist.` — the describe wording with the requested id interpolated.
Why: same as Cause 2 — the only thing separating a dead-id mutation refusal from AWS's is the message, so clients keying on it (teardown that swallows `Security configuration not found`, dashboards classifying refusals) behave differently against the emulator.
Fix: the not-found branch of DeleteSecurityConfiguration's handler in the emr-containers service crate — return the fixed AWS sentence; leave the in-use refusal and the describe-path ResourceNotFoundException wording as they are.
Traces: check emr-containers.security-configuration-create-describe-list-delete; handler branch at services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/delete_security_configuration.rs:188-203.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (emr-containers 6/20 passed; 14 failed, 11 of them blocked by the already-filed unimplemented ec2 CreateInternetGateway stub and excluded here).

Cause 1 — [mismatch] emr-containers.job-template-client-token-reuse-conflict-rejected:
  code assertion PASSED (ValidationException, HTTP 400)
  expected message: 'At least one input parameter differs from a previous call using the same client token'
  actual   message: 'clientToken tok-86e5e650c7a9 was already used to create job template 000000000840vpgw08nbg4xt2, with different parameters'
  (the token and template id in the actual message are per-run values; the DEFECT is the sentence, not those values — AWS's text has no variable parts)

Cause 2 — [mismatch] emr-containers.job-template-delete-twice-rejected:
  code assertion PASSED (ValidationException, HTTP 400)
  expected message: 'Template does not exist'
  actual   message: "Job Template 000000000840vplu6l34d3w1k doesn't exist."
  same check's describe-after-delete assertion (ResourceNotFoundException, "Job Template <id> doesn't exist.") PASSED, confirming the two paths need different wording.

Cause 3 — [mismatch] emr-containers.security-configuration-create-describe-list-delete:
  create/describe/list/delete round-trip and the describe-after-delete refusal (ResourceNotFoundException, "Security Configuration <id> doesn't exist.") all PASSED;
  re-delete code assertion PASSED (ValidationException, HTTP 400)
  expected message: 'Security configuration not found'
  actual   message: "Security Configuration 000000000840vojhdqxo7t1rw doesn't exist."

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json):
  CreateJobTemplate errors = {InternalServerException 500, ResourceNotFoundException 400, ValidationException 400}; clientToken = ClientToken @idempotencyToken @length 1..64 @pattern \\S, required.
  DeleteJobTemplate errors = {InternalServerException 500, ValidationException 400} (no not-found error modeled).
  DeleteSecurityConfiguration errors = {InternalServerException 500, ValidationException 400} (no not-found error modeled).
  DescribeSecurityConfiguration errors = {InternalServerException 500, ResourceNotFoundException 400, ValidationException 400}.

Code evidence: both delete handlers call crate::state::ResourceKind::<Kind>.not_found_message(&input.id) to build the ValidationException message (delete_job_template.rs:159, delete_security_configuration.rs:196-199), which is the Describe-path ResourceNotFoundException wording.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay suite simparity-behavioral-aws-v1, checks emr-containers.job-template-client-token-reuse-conflict-rejected, emr-containers.job-template-delete-twice-rejected and emr-containers.security-configuration-create-describe-list-delete.
3. Cause 1: CreateJobTemplate(name=N, clientToken=T, jobTemplateData=D1); then CreateJobTemplate(name=N, clientToken=T, jobTemplateData=D2) with a different sparkSubmitJobDriver.entryPoint. Expect HTTP 400 ValidationException with message exactly 'At least one input parameter differs from a previous call using the same client token'; observed a message naming the clientToken and the first template's id.
4. Cause 2: CreateJobTemplate -> DeleteJobTemplate(id) -> DeleteJobTemplate(id) again. Expect HTTP 400 ValidationException with message exactly 'Template does not exist'; observed "Job Template <id> doesn't exist.".
5. Cause 3: CreateSecurityConfiguration -> DeleteSecurityConfiguration(id) -> DeleteSecurityConfiguration(id) again. Expect HTTP 400 ValidationException with message exactly 'Security configuration not found'; observed "Security Configuration <id> doesn't exist.". Confirm DescribeSecurityConfiguration(id) after the first delete still answers ResourceNotFoundException with "Security Configuration <id> doesn't exist." — that wording must NOT change.
6. Re-replay all three checks to confirm the fix; all three must pass.
