Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__stepfunctions.create-activity-disabled-cmk-invalid-encryption-config-rejected kms__stepfunctions.cmk-trust-removed-then-restored-execution-redrive-succeeds
Issue-Key: kms__stepfunctions.create-activity-disabled-cmk-invalid-encryption-config-rejected|run#12|Timeout|time.sleep(3.0)
Issue-Key: kms__stepfunctions.cmk-trust-removed-then-restored-execution-redrive-succeeds|run#26|Timeout|time.sleep(3)
Issue-Commit: 5598dd881b8e225c34bc5a9814ddc1ff981b448f

# kms__stepfunctions: 2 net-new causes — CreateActivity with a disabled CMK is never refused with InvalidEncryptionConfiguration, and a CMK-encrypted execution blocked by a removed role trust is never redrivable after the trust is restored

> _Note: this issue was identified against the code base at commit `5598dd881b8e225c34bc5a9814ddc1ff981b448f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateActivity

## Description
Two net-new root causes, grounded in two failing behavioral checks the replay report left unmarked (no ALREADY-FILED marker and no OWNED marker). Both are killed by the harness with "Error: Check timed out after 60.0s" because the emulator never settles on the outcome the check polls for. Smithy model used: services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json.

Cause 1 of 2 — CreateActivity with encryptionConfiguration naming a DISABLED customer managed KMS key is never refused with InvalidEncryptionConfiguration (and so never settles), and the re-enabled-key control never confirms that the same call then succeeds and creates the activity.
Failing check id: kms__stepfunctions.create-activity-disabled-cmk-invalid-encryption-config-rejected.
AWS: Smithy models CreateActivity with input {name (required, @length 1..80), tags, encryptionConfiguration} and output {activityArn (required), creationDate (required)}; its modeled errors are ActivityAlreadyExists, ActivityLimitExceeded, InvalidEncryptionConfiguration, InvalidName, KmsAccessDeniedException, KmsThrottlingException (all HTTP 400) and TooManyTags. For a CUSTOMER_MANAGED_KMS_KEY whose key state is Disabled, the refusal is HTTP 400 with error code InvalidEncryptionConfiguration — NOT a KMS-prefixed code such as KmsInvalidStateException — and the message's fixed text is the "kmsKeyId is not enabled" wording for that class, with the key identifier taken from the request; nothing is created, so the activity name does not appear in ListActivities. With the key re-enabled the identical call returns 200 with activityArn and creationDate.
Emulator: the check polls for that refusal (allowing for KMS key-state propagation) and is killed at the 60 s harness cap without ever seeing it, so it is not established that the call is refused, that the refusal carries InvalidEncryptionConfiguration rather than a KMS-specific code, or that no activity was created.
Why: a client that encrypts activities with its own CMK is given no reliable signal when that key is unusable: it either gets an activity it cannot use, or an error code it does not expect, and its error handling (which branches on InvalidEncryptionConfiguration, not on a Kms* code) does not fire.
Fix: the CreateActivity handler's encryption-configuration validation — resolve the kmsKeyId, check the key state before creating anything, and map a non-enabled key to HTTP 400 InvalidEncryptionConfiguration with the "kmsKeyId is not enabled" wording, leaving the activity store untouched.
Traces: kms__stepfunctions.create-activity-disabled-cmk-invalid-encryption-config-rejected.

Cause 2 of 2 — a CMK-encrypted STANDARD execution started while its execution role's trust policy has been removed never reaches the States.Runtime failure, so restoring the trust and calling RedriveExecution never resumes it to SUCCEEDED.
Failing check id: kms__stepfunctions.cmk-trust-removed-then-restored-execution-redrive-succeeds.
AWS: Smithy models RedriveExecution with input {executionArn (required), clientToken (@pattern ^[!-~]+$, @length 1..64)} and output {redriveDate (required)}; modeled errors ExecutionDoesNotExist, ExecutionLimitExceeded, ExecutionNotRedrivable, InvalidArn, ValidationException, all HTTP 400. Recorded behavior for the scenario: with the role's sts:AssumeRole trust for states.amazonaws.com removed, an execution of a CMK-encrypted state machine fails with error States.Runtime and a cause carrying the fixed "not authorized to assume the provided role" wording plus the role ARN from the state machine's configuration; after the trust is restored, RedriveExecution on that SAME execution re-runs it to SUCCEEDED (the same execution ARN, with its redrive count incremented) rather than requiring a new StartExecution.
Emulator: the check is killed at the 60 s harness cap while polling for that transition (its own poll budget is 90 s), so neither the States.Runtime failure under the removed trust nor the post-restore redrive to SUCCEEDED is ever observed.
Why: a client cannot recover from the most common Step Functions misconfiguration. The trust failure is not surfaced as States.Runtime with a cause naming the un-assumable role, so an operator cannot diagnose it; and redrive — the documented way to resume after fixing the cause without losing the execution's identity or input — does not work, forcing a fresh execution with a new ARN.
Fix: the execution start path's role-assumption check (it must fail the execution with States.Runtime and the "not authorized to assume the provided role" cause when the trust policy does not allow states.amazonaws.com) and the RedriveExecution handler, which must re-run the failed execution in place under the CMK with the original input.
Traces: kms__stepfunctions.cmk-trust-removed-then-restored-execution-redrive-succeeds.

## Evidence
kms__stepfunctions.create-activity-disabled-cmk-invalid-encryption-config-rejected — verdict mismatch.
Expected: kms:CreateKey then kms:DisableKey (DescribeKey KeyState = Disabled); sfn:CreateActivity(name=<n>, encryptionConfiguration={type: CUSTOMER_MANAGED_KMS_KEY, kmsKeyId: <key>}) is refused HTTP 400 with error code InvalidEncryptionConfiguration and a message whose fixed text is the "kmsKeyId is not enabled" wording (key identifier from the request), and ListActivities does not contain <n>; after kms:EnableKey the identical call returns activityArn + creationDate.
Actual: "Error: Check timed out after 60.0s" — the refusal never arrived while the check polled for it, so neither the code, the message nor the no-side-effect assertion was reached.

kms__stepfunctions.cmk-trust-removed-then-restored-execution-redrive-succeeds — verdict mismatch.
Expected: with the execution role's trust policy replaced so states.amazonaws.com can no longer assume it, StartExecution on a CMK-encrypted STANDARD state machine ends FAILED with error States.Runtime and a cause carrying the "not authorized to assume the provided role" wording and the role ARN; after the good trust is restored, RedriveExecution(executionArn=<same ex>) returns a redriveDate and that same execution ends SUCCEEDED.
Actual: "Error: Check timed out after 60.0s" — the check never observed the States.Runtime failure (own poll budget 90 s), so the restore-and-redrive half was never exercised.

## Steps to Reproduce
Cause 1:
1. kms:CreateKey (SYMMETRIC_DEFAULT, ENCRYPT_DECRYPT). 2. kms:DisableKey; kms:DescribeKey must report KeyState "Disabled".
3. sfn:CreateActivity(name=<unique>, encryptionConfiguration={"type":"CUSTOMER_MANAGED_KMS_KEY","kmsKeyId":"<key-id>"}) — expected: HTTP 400, error code InvalidEncryptionConfiguration, message with the "kmsKeyId is not enabled" fixed text. Observed: no such refusal within the harness window.
4. sfn:ListActivities — expected: <unique> absent.
5. kms:EnableKey, repeat step 3 with a fresh name — expected: 200 with activityArn and creationDate.

Cause 2:
1. kms:CreateKey (SYMMETRIC_DEFAULT). 2. iam:CreateRole trusting states.amazonaws.com. 3. sfn:CreateStateMachine (STANDARD) with {"StartAt":"Pass","States":{"Pass":{"Type":"Pass","End":true}}}, that role and encryptionConfiguration naming the CMK.
4. iam:UpdateAssumeRolePolicy to a trust that does NOT allow states.amazonaws.com.
5. sfn:StartExecution and poll sfn:DescribeExecution — expected: FAILED with error States.Runtime and a cause carrying the "not authorized to assume the provided role" wording and the role ARN. Observed: never reached; the check is killed at the 60 s harness cap.
6. (unreachable today) iam:UpdateAssumeRolePolicy back to the states.amazonaws.com trust, then sfn:RedriveExecution(executionArn=<same>) -> redriveDate, and DescribeExecution on that same execution ARN -> SUCCEEDED.
