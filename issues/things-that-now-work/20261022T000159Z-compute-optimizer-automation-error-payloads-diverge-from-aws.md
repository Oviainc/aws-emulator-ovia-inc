Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer-automation.update-automation-rule-stale-revision-rejected compute-optimizer-automation.delete-automation-rule-stale-revision-rejected compute-optimizer-automation.concurrent-updates-optimistic-lock-race compute-optimizer-automation.get-automation-rule-nonexistent-not-found compute-optimizer-automation.create-get-delete-automation-rule-lifecycle compute-optimizer-automation.update-automation-rule-increments-revision compute-optimizer-automation.create-automation-rule-invalid-name-rejected compute-optimizer-automation.create-automation-rule-idempotent-parameter-mismatch-rejected compute-optimizer-automation.update-enrollment-configuration-invalid-status-rejected
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation.update-automation-rule-stale-revision-rejected|run#13|AssertionError|assert message == (
Issue-Key: compute-optimizer-automation.delete-automation-rule-stale-revision-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: compute-optimizer-automation.concurrent-updates-optimistic-lock-race|run#18|AssertionError|assert losers[0][3] == expected_message, f"unexpected loser message: {losers[0][3]!r}"
Issue-Key: compute-optimizer-automation.get-automation-rule-nonexistent-not-found|run#14|AssertionError|assert message == "The specified resource could not be found", (
Issue-Key: compute-optimizer-automation.create-get-delete-automation-rule-lifecycle|run#19|AssertionError|assert message == "The specified resource could not be found", (
Issue-Key: compute-optimizer-automation.update-automation-rule-increments-revision|run#17|AssertionError|assert message == "The specified resource could not be found", (
Issue-Key: compute-optimizer-automation.create-automation-rule-invalid-name-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: compute-optimizer-automation.create-automation-rule-idempotent-parameter-mismatch-rejected|run#11|AssertionError|assert message == "The request tied to this token is different!", f"unexpected mismatch message: {message!r}"
Issue-Key: compute-optimizer-automation.update-enrollment-configuration-invalid-status-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Compute Optimizer Automation: error payloads diverge from AWS — stale-revision rejection hides the current revision, not-found leaks the ARN, and constraint/idempotency/enrollment rejections use non-AWS wording

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutomationRule

## Description
Five root causes in how the compute-optimizer-automation emulator builds the body of its (correctly classified) client errors. In every cause below the error CODE and HTTP status already match AWS; what diverges is the message content, and in causes 1, 2 and 3 the divergence removes or adds request/state-derived information that clients and operators read, so the generating rule — not just the recorded text — differs. Recorded texts are quoted as examples.

Cause 1 of 5: the stale-ruleRevision rejection does not report the rule's current revision number.
AWS: UpdateAutomationRule/DeleteAutomationRule take ruleRevision (Smithy: UpdateAutomationRuleRequest.ruleRevision Long @required, "The revision number of the rule to update"; same on DeleteAutomationRuleRequest) and enforce it as an optimistic lock. Recorded behavior: the losing call is rejected with error code InvalidParameterValueException (Smithy: @httpError 400) and a message whose RULE is "state the current revision number of the rule" — recorded example "Rule revision does not match the current revision number 2". Both the serial stale-revision case and the concurrent race land on that same rule.
Emulator: code/status are right, but the message is a generic constraint-violation sentence that omits the current revision and instead echoes the stale input and the rule ARN: "Value '1' at 'ruleRevision' failed to satisfy constraint: Member must match the current revision of rule 'arn:aws:compute-optimizer::000000000000:automation-rule/<id>'".
Why: the current revision is the one piece of information a client needs to recover from a lost optimistic-lock race; AWS hands it back in the rejection so a caller can re-read-modify-write without an extra GetAutomationRule. Clients that parse it see nothing to recover with, and the emulator additionally discloses the resource ARN in an error where AWS does not.
Fix: the UpdateAutomationRule and DeleteAutomationRule handlers' revision-conflict branch (and the same branch used by the concurrent path) must format the message from the rule's CURRENT revision, not from the rejected input.
Traces: compute-optimizer-automation.update-automation-rule-stale-revision-rejected, compute-optimizer-automation.delete-automation-rule-stale-revision-rejected, compute-optimizer-automation.concurrent-updates-optimistic-lock-race (this last one also asserts exactly one winner — the winner/loser split itself was correct, only the loser's message diverged).

Cause 2 of 5: ResourceNotFoundException for a missing/deleted automation rule carries a per-ARN message instead of AWS's fixed one.
AWS: Smithy ResourceNotFoundException @httpError 404, doc "The specified resource was not found." Recorded behavior: GetAutomationRule on a deleted rule, and on a well-formed ARN in the caller's account that was never created, both return code ResourceNotFoundException with the constant, resource-independent message "The specified resource could not be found" — the rule is that the message is a constant and does not echo the resource identifier.
Emulator: returns ResourceNotFoundException (correct code/status) with a generated per-resource message, e.g. "No automation rule found with ARN 'arn:aws:compute-optimizer::000000000000:automation-rule/<id>'."
Why: callers and log scrapers keyed on AWS's constant not-found text do not match, and the emulator echoes a resource identifier back in an error where AWS deliberately does not.
Fix: the shared not-found branch of the automation-rule handlers (e.g. services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/get_automation_rule.rs, Operation::handle lines 195-205, and the equivalent branch reached after DeleteAutomationRule) should emit the fixed AWS not-found message.
Traces: compute-optimizer-automation.get-automation-rule-nonexistent-not-found, compute-optimizer-automation.create-get-delete-automation-rule-lifecycle, compute-optimizer-automation.update-automation-rule-increments-revision (all three failed only on this message; the lifecycle, revision increment and 404 classification around it passed).

Cause 3 of 5: constraint-violation messages do not use AWS's validation-report format.
AWS: CreateAutomationRule.name is RuleName @required, @length 0..128, @pattern ^[a-zA-Z0-9_-]{0,}$ (model value: ^[a-zA-Z0-9_-]*$). A violating name is rejected with code InvalidParameterValueException (400) and AWS's standard validation-report message, whose rule is known and is built from the request: "<N> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — recorded example "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]*". Note AWS does NOT echo the rejected value and reports the pattern unanchored.
Emulator: code/status are right, but the message drops the "<N> validation error detected: " preamble, echoes the offending value, and prints the anchored model pattern: "Value 'sim2real-rule-<id> x' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]*$".
Why: AWS's validation-report format is the cross-service contract clients parse to count and attribute constraint failures; echoing the submitted value can also replay user data (and secrets, for other members) into logs.
Fix: the framework-level modeled-constraint rejection path that formats this message for the service (count preamble, no value echo, unanchored pattern).
Traces: compute-optimizer-automation.create-automation-rule-invalid-name-rejected (the InvalidParameterValueException code, the no-rule-created effect and the valid-name positive control all passed).

Cause 4 of 5: IdempotentParameterMismatchException message differs from AWS's.
AWS: Smithy IdempotentParameterMismatchException @httpError 409 is a modeled error of CreateAutomationRule; recorded behavior for a clientToken replayed with a different name is code IdempotentParameterMismatchException with message "The request tied to this token is different!" (recorded behavior only — this is a fixed AWS string, not derived from the request).
Emulator: right code and status, message "The same client token was used with different parameters." (which reads as the model's doc sentence rather than AWS's wire message).
Why: lowest-impact of the five — clients matching on AWS's idempotency-mismatch text (common in retry wrappers) do not match. The classification itself is correct.
Fix: the CreateAutomationRule handler's idempotency-token comparison branch (message text only).
Traces: compute-optimizer-automation.create-automation-rule-idempotent-parameter-mismatch-rejected (code, the "no second rule created" effect and the untouched original rule all passed).

Cause 5 of 5: UpdateEnrollmentConfiguration's rejection of a server-only status does not name the allowed values.
AWS: UpdateEnrollmentConfigurationRequest.status is EnrollmentStatus @required, and the model states "The Pending and Failed options cannot be used to update the enrollment status of an account. They are returned in the response of a request to update the enrollment status of an account." Recorded behavior: status=Pending is rejected with code InvalidParameterValueException (400) and a message built from the request that names the rejected value AND enumerates the settable set — recorded example "Invalid status: Pending. Only ACTIVE and INACTIVE are allowed."
Emulator: right code and status, message "The status 'Pending' cannot be used to update the enrollment status of an account." — it names the rejected value but never the allowed set.
Why: a caller is told what is wrong but not what to send instead, where AWS's message states the two settable values.
Fix: the UpdateEnrollmentConfiguration handler's status-validation branch (message must enumerate the settable statuses).
Traces: compute-optimizer-automation.update-enrollment-configuration-invalid-status-rejected (code, the unchanged-status effect and the re-affirm positive control all passed).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), service compute-optimizer-automation 4/20 passed. In each failure the asserted error code matched; the assertion that failed is the message assertion.

Cause 1:
  update-automation-rule-stale-revision-rejected [mismatch]: expected 'Rule revision does not match the current revision number 2', got "Value '1' at 'ruleRevision' failed to satisfy constraint: Member must match the current revision of rule 'arn:aws:compute-optimizer::000000000000:automation-rule/bf8f1bb76f882caf'"
  delete-automation-rule-stale-revision-rejected [mismatch]: expected 'Rule revision does not match the current revision number 2', got "Value '1' at 'ruleRevision' failed to satisfy constraint: Member must match the current revision of rule 'arn:aws:compute-optimizer::000000000000:automation-rule/082234c4cd39194c'"
  concurrent-updates-optimistic-lock-race [mismatch]: exactly one winner/one loser with code InvalidParameterValueException as expected; AssertionError: unexpected loser message: "Value '1' at 'ruleRevision' failed to satisfy constraint: Member must match the current revision of rule 'arn:aws:compute-optimizer::000000000000:automation-rule/34617c7ce868ab5e'"

Cause 2:
  get-automation-rule-nonexistent-not-found [mismatch]: code ResourceNotFoundException OK; AssertionError: unexpected not-found message: "No automation rule found with ARN 'arn:aws:compute-optimizer::000000000000:automation-rule/sim2real-missing-98deaf42bd97'." (expected "The specified resource could not be found")
  create-get-delete-automation-rule-lifecycle [mismatch]: same, ARN .../6d140cf9ca521820
  update-automation-rule-increments-revision [mismatch]: same, ARN .../aac3e0b1d1667634

Cause 3:
  create-automation-rule-invalid-name-rejected [mismatch]: expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]*", got "Value 'sim2real-rule-6486aa333d30 x' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]*$"

Cause 4:
  create-automation-rule-idempotent-parameter-mismatch-rejected [mismatch]: code IdempotentParameterMismatchException OK; expected 'The request tied to this token is different!', got 'The same client token was used with different parameters.'

Cause 5:
  update-enrollment-configuration-invalid-status-rejected [mismatch]: code InvalidParameterValueException OK; expected 'Invalid status: Pending. Only ACTIVE and INACTIVE are allowed.', got "The status 'Pending' cannot be used to update the enrollment status of an account."

## Steps to Reproduce
Setup (all causes): boot the emulator, create a botocore client for 'compute-optimizer-automation', enroll via compute-optimizer UpdateEnrollmentStatus(status=Active) then UpdateEnrollmentConfiguration(status=Active) until GetEnrollmentConfiguration returns Active.

Cause 1: create_automation_rule(status=Inactive) -> note ruleRevision R0; update_automation_rule(ruleArn, ruleRevision=R0, description='bumped') -> revision R1; then update_automation_rule(ruleArn, ruleRevision=R0, ...) and delete_automation_rule(ruleArn, ruleRevision=R0). Each is rejected InvalidParameterValueException but the message omits R1. Same with two concurrent update_automation_rule calls sharing one stale revision.
Cause 2: create a rule, delete it with its current revision, then get_automation_rule(ruleArn=<deleted arn>); also get_automation_rule on a never-created but well-formed arn:aws:compute-optimizer::<account>:automation-rule/<name>.
Cause 3: create_automation_rule(name='has a space x', ...) with an otherwise valid request.
Cause 4: create_automation_rule(name=A, clientToken=T), then create_automation_rule(name=B, clientToken=T).
Cause 5: update_enrollment_configuration(status='Pending').

Replay: run_behavioral_suite and re-check the nine ids in trace_ids.
