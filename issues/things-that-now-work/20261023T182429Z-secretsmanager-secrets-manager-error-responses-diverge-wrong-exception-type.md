Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-rotation-configuration-and-cancel
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-rotation-configuration-and-cancel secretsmanager-put-secret-value-token-conflict secretsmanager-secret-version-staging-workflow secretsmanager-delete-secret-recovery-window-errors secretsmanager-duplicate-and-not-found-error-paths secretsmanager-list-pagination-invalid-next-token
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|GetSecretValue|4xx->4xx|$.Message
Issue-Key: secretsmanager|GetSecretValue|4xx->4xx|$.__type
Issue-Key: secretsmanager|PutSecretValue|4xx->4xx|$.Message
Issue-Key: secretsmanager|DeleteSecret|4xx->4xx|$.Message
Issue-Key: secretsmanager|DescribeSecret|4xx->4xx|$.Message
Issue-Key: secretsmanager|ListSecrets|4xx->4xx|$.Message
Issue-Key: secretsmanager|ListSecretVersionIds|4xx->4xx|$.Message
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# Secrets Manager error responses diverge: wrong exception type for a secret pending deletion, and emulator-authored message text

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
The error surface is modelled with the right HTTP status everywhere (all seven clusters are 400/400) but one path raises the wrong exception type, and the rest emit emulator-authored message strings in place of the messages AWS's documented rules produce. Seven causes.

Cause 1 of 7: GetSecretValue on a secret scheduled for deletion raises ResourceNotFoundException instead of InvalidRequestException.
AWS: Smithy GetSecretValue errors include both InvalidRequestException and ResourceNotFoundException. InvalidRequestException is documented as 'A parameter value is not valid for the current state of the resource. Possible causes: The secret is scheduled for deletion.' A secret inside its recovery window still exists, so AWS answers 400 InvalidRequestException with 'You can't perform this operation on the secret because it was marked for deletion.'; ResourceNotFoundException is reserved for a secret that does not exist.
Emulator: 400 with __type ResourceNotFoundException and a not-found message.
Why: the two errors mean opposite things. A client distinguishing 'deleted, restorable with RestoreSecret' from 'never existed, must CreateSecret' takes the wrong branch, and retry/recreate logic recreates a secret that is actually still present in its recovery window.
Fix: the DeleteSecret recovery-window state and the GetSecretValue handler's error selection - a secret with a DeletedDate inside the window must still be found and rejected as InvalidRequestException.

Cause 2 of 7: PutSecretValue ClientRequestToken conflict message is not built from the request.
AWS: Smithy PutSecretValue.ClientRequestToken is an idempotency token; reusing one for a different value yields 400 ResourceExistsException whose message is constructed from the request - it names the offending ClientRequestToken and the secret's ARN: "You can't use ClientRequestToken <token> because that value is already in use for a version of secret <arn>." That construction rule is the requirement, not the recorded token/ARN values.
Emulator: 400 with a generic sentence that names neither the token nor the secret ('The operation failed because a version with the id ... already exists and holds a different value.').
Why: callers reconciling a batch of idempotent writes rely on the token and ARN in the message to identify which write collided; a message that identifies neither makes the conflict unattributable.
Fix: the PutSecretValue handler's ResourceExistsException construction.

Cause 3 of 7: DeleteSecret RecoveryWindowInDays validation message.
AWS: Smithy DeleteSecretRequest.RecoveryWindowInDays documents 'The number of days from 7 to 30 that Secrets Manager waits before permanently deleting the secret.' The validation error for an out-of-range value states that constraint in AWS's fixed wording: 'The RecoveryWindowInDays value must be between 7 and 30 days (inclusive).'
Emulator: correct 400 and error code, but its own wording, which additionally echoes the submitted value.
Why: low impact - status and code match; only message-matching clients and log assertions diverge.
Fix: the DeleteSecret validation's message text.

Cause 4 of 7: ResourceNotFoundException message echoes the requested SecretId (DescribeSecret).
AWS: ResourceNotFoundException is documented as "Secrets Manager can't find the resource that you asked for" and the wire message is the constant sentence "Secrets Manager can't find the specified secret." with no request-derived suffix - the same constant appears in two independently recorded operations in this corpus, which pins the rule.
Emulator: appends ': <secret name>' to that sentence.
Why: low impact for control flow (status and code match), but the emulator reflects a caller-supplied identifier back into the error body, which AWS does not.
Fix: the shared ResourceNotFoundException construction for secretsmanager.

Cause 5 of 7: same ResourceNotFoundException message divergence on GetSecretValue.
AWS: as cause 4 - constant sentence, no identifier.
Emulator: same appended ': <secret name>'.
Why: as cause 4; listed separately because it is a distinct operation's error path and is replayed by its own step.
Fix: as cause 4, verified on the GetSecretValue path.

Cause 6 of 7: InvalidNextTokenException message on ListSecrets.
AWS: Smithy ListSecrets errors include InvalidNextTokenException ('The NextToken value is invalid'); the recorded 400 body carries the constant sentence 'The pagination token is no longer valid.' - identical in two separately recorded operations here.
Emulator: 400 with the correct code but its own sentence ('The NextToken you provided is not valid for this request.').
Why: low impact - pagination callers branch on the error code, which is right; message-matching clients diverge.
Fix: the pagination-token validation's message text.

Cause 7 of 7: same InvalidNextTokenException message divergence on ListSecretVersionIds.
AWS: as cause 6; ListSecretVersionIds is likewise paginated (NextToken in/out) and models InvalidNextTokenException.
Emulator: same emulator-authored sentence.
Why: as cause 6; separate operation and separate replay step.
Fix: as cause 6, verified on the ListSecretVersionIds path.

Note: no open issue covers any of these; the only open secretsmanager issue concerns CodeBuild builds faulting at PROVISIONING when resolving a SECRETS_MANAGER environment reference, which is unrelated to these error paths.

Traces: secretsmanager-rotation-configuration-and-cancel#teardown#7 (cause 1), secretsmanager-put-secret-value-token-conflict#task#3 (cause 2), secretsmanager-delete-secret-recovery-window-errors#task#2 (cause 3), secretsmanager-duplicate-and-not-found-error-paths#task#5 (cause 4), secretsmanager-duplicate-and-not-found-error-paths#task#6 (cause 5), secretsmanager-list-pagination-invalid-next-token#task#2 (cause 6), secretsmanager-list-pagination-invalid-next-token#task#3 (cause 7).

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, state_reset_failures=0). Every cluster below had expected_status 400 and actual_status 400 - the status is right in all seven; the divergence is the exception type (cause 1) or the message body.

Cause 1 - GetSecretValue, secretsmanager-rotation-configuration-and-cancel#teardown#7: $.__type: value_mismatch (expected 'InvalidRequestException', got 'ResourceNotFoundException'); $.Message: expected "You can't perform this operation on the secret because it was marked for deletion.", got "Secrets Manager can't find the specified secret: <secret arn>".

Cause 2 - PutSecretValue, secretsmanager-put-secret-value-token-conflict#task#3: $.Message: expected "You can't use ClientRequestToken <token> because that value is already in use for a version of secret <arn>.", got 'The operation failed because a version with the id <token> already exists and holds a different value.' affected_count=2 (also secretsmanager-secret-version-staging-workflow#task#8).

Cause 3 - DeleteSecret, secretsmanager-delete-secret-recovery-window-errors#task#2: $.Message: expected 'The RecoveryWindowInDays value must be between 7 and 30 days (inclusive).', got 'RecoveryWindowInDays must be between 7 and 30 days, inclusive; got 2.' affected_count=3 (also secretsmanager-delete-secret-recovery-window-errors#task#3 and secretsmanager-duplicate-and-not-found-error-paths#task#8).

Cause 4 - DescribeSecret, secretsmanager-duplicate-and-not-found-error-paths#task#5: $.Message: expected "Secrets Manager can't find the specified secret.", got the same sentence with ': <requested secret name>' appended.

Cause 5 - GetSecretValue, secretsmanager-duplicate-and-not-found-error-paths#task#6: identical divergence on the GetSecretValue path.

Cause 6 - ListSecrets, secretsmanager-list-pagination-invalid-next-token#task#2: $.Message: expected 'The pagination token is no longer valid.', got 'The NextToken you provided is not valid for this request.'

Cause 7 - ListSecretVersionIds, secretsmanager-list-pagination-invalid-next-token#task#3: same expected/actual pair as cause 6.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus: replay_wire_corpus().
2. Cause 1: fixture secretsmanager-rotation-configuration-and-cancel step teardown#7 - DeleteSecret with a recovery window, then GetSecretValue on the still-recoverable secret; inspect __type.
3. Cause 2: fixture secretsmanager-put-secret-value-token-conflict step task#3 - PutSecretValue reusing a ClientRequestToken with a different SecretString.
4. Cause 3: fixture secretsmanager-delete-secret-recovery-window-errors step task#2 - DeleteSecret with RecoveryWindowInDays outside 7..30.
5. Causes 4-5: fixture secretsmanager-duplicate-and-not-found-error-paths steps task#5 and task#6 - DescribeSecret and GetSecretValue on a secret that was never created.
6. Causes 6-7: fixture secretsmanager-list-pagination-invalid-next-token steps task#2 and task#3 - ListSecrets and ListSecretVersionIds with a malformed NextToken.
