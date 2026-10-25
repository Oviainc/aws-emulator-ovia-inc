Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.delete-secret-again-while-scheduled-reschedules-and-force-delete-overrides-the-window secretsmanager.recovery-window-condition-keys-forbid-a-force-delete-and-a-short-window
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.delete-secret-again-while-scheduled-reschedules-and-force-delete-overrides-the-window|run#10|AssertionError|assert _whole_days(first["DeletionDate"], first_marked) == 7, (
Issue-Key: secretsmanager.recovery-window-condition-keys-forbid-a-force-delete-and-a-short-window|run#29|AssertionError|assert force_refused[0] == "AccessDeniedException", (
Issue-Commit: a348f7a60ca5edb956d11f71c3f040d86e771b20

# Secrets Manager DeleteSecret: DescribeSecret.DeletedDate is stamped with the future deletion instant (window offset collapses to zero), and identity-policy Denies over the DeleteSecret request-parameter condition keys are never evaluated so an irreversible force-delete is permitted

> _Note: this issue was identified against the code base at commit `a348f7a60ca5edb956d11f71c3f040d86e771b20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSecret

## Description
Two root causes, both reached through DeleteSecret. Cause count: 2.

=== Cause 1 of 2: DescribeSecret.DeletedDate carries the scheduled deletion instant instead of the deletion-request time, so the recovery window is unobservable (DeletionDate - DeletedDate == 0) ===

AWS: DeleteSecret with RecoveryWindowInDays=N schedules the secret and returns DeletionDate = (now + N days) (Smithy: DeleteSecret input RecoveryWindowInDays -> RecoveryWindowInDaysType long, ForceDeleteWithoutRecovery -> BooleanType; output ARN, Name, DeletionDate -> DeletionDateType timestamp). DescribeSecret separately reports DeletedDate (Smithy: DescribeSecretResponse.DeletedDate -> DeletedDateType timestamp), which is the instant the delete was REQUESTED. The two together are what makes the recovery window measurable: DeletionDate - DeletedDate must equal the requested window (7 days for RecoveryWindowInDays=7), and the DeleteSecret API reference states Secrets Manager "attaches a DeletionDate stamp to the secret that specifies the end of the recovery window" while the minimum window is 7 days and the default is 30.

Emulator: DeletedDate is stamped with the same future instant as DeletionDate, so the measured offset is zero. The check computes DescribeSecret.DeletedDate immediately after DeleteSecret(RecoveryWindowInDays=7) and gets DeletionDate - DeletedDate = 0:00:00 instead of 7 days ("the first schedule must be 7 days out, got 0:00:00"). Note that DeletionDate itself does look correct in absolute terms - in the second check below, a 30-day window produced a DeletionDate 29-31 days after the live clock reading - so the defect is isolated to the DeletedDate stamp, not to the window arithmetic.

Why: a client that computes "how long do I have left to restore this secret" or audits "when was this secret scheduled for deletion" reads DeletedDate. With DeletedDate pinned to the future, every such computation yields zero remaining window and the audit trail reports the secret as having been deleted days before the request actually happened. The check also cannot proceed to the second transition it exists to grade (a second DeleteSecret re-scheduling with a NEW window, and a force delete overriding a pending window), so that part of the deletion state machine is untested behind this one stamp.

Fix: the DeleteSecret handler's recovery-window state transition and the DescribeSecret serializer - stamp DeletedDate with the request time and keep the scheduled instant only in DeletionDate.

Related open issue, which does NOT carry this check's replay key: 20261024T190718Z-secretsmanager-secrets-manager-replication-delete-lifecycle-access-metadata-and.md (its Cause 4 describes the same DeletedDate stamp seen through two other checks). This check additionally shows the consequence as a zero-length window measured across the DeleteSecret response and DescribeSecret, and it is the only check that then exercises re-scheduling and force-delete-over-a-pending-window.

Traces: secretsmanager.delete-secret-again-while-scheduled-reschedules-and-force-delete-overrides-the-window.

=== Cause 2 of 2: the DeleteSecret request-parameter condition keys (secretsmanager:ForceDeleteWithoutRecovery, secretsmanager:RecoveryWindowInDays) are not populated or evaluated, so an identity-policy Deny guardrail does not refuse an irreversible force-delete ===

AWS: Secrets Manager populates the request-level condition keys secretsmanager:ForceDeleteWithoutRecovery (Bool) and secretsmanager:RecoveryWindowInDays (Numeric) from the DeleteSecret request and IAM evaluates them, so an identity-based policy that Allows secretsmanager:DeleteSecret on * but Denies it under Bool secretsmanager:ForceDeleteWithoutRecovery = true, and under NumericLessThan secretsmanager:RecoveryWindowInDays = 30, refuses exactly those two request shapes and permits the compliant one. The three probed calls differ in nothing but a request parameter, so no (principal, action, resource) matcher can separate them. Because Secrets Manager speaks awsJson1_1 the explicit-deny refusal is the code/status pair AccessDeniedException / HTTP 400 (not an operation-modelled error - Smithy DeleteSecret models only InternalServiceError 500, InvalidParameterException 400, InvalidRequestException 400, ResourceNotFoundException 400). The message is request-derived, of the form "User: <assumed-role session arn> is not authorized to perform: secretsmanager:DeleteSecret on resource: <secret arn> with an explicit deny in an identity-based policy"; that construction rule is the requirement, the exact sentence is an example. The check establishes the positive control first with unrestricted credentials - RecoveryWindowInDays=7 is a VALID request that returns a DeletionDate - so the expected refusal is authorization, not validation.

Emulator: the Deny does not apply. Signed by the guardrailed assumed-role session, DeleteSecret(SecretId=<secret>, ForceDeleteWithoutRecovery=True) SUCCEEDS - the probe reports 'OK' instead of 'AccessDeniedException' - and the secret is irreversibly destroyed. The compliant 30-day delete by the same role also succeeds (correctly), so the emulator's answer is identical for the permitted and the explicitly denied request shape.

Why: "nobody may destroy a secret without a 30-day recovery window" is a real, common Secrets Manager guardrail and it is expressible ONLY through these request-parameter condition keys. A client relying on AWS's behavior believes that policy protects it; against the emulator the guardrail is a no-op, the most over-permissive outcome available on this API (unrecoverable data loss) is admitted, and negative tests that assert the force-delete is denied pass while the secret is actually gone. Code branching on AccessDeniedException/400 is never exercised.

Fix: the Secrets Manager request-authorization layer - populate the per-operation request condition keys (at minimum secretsmanager:ForceDeleteWithoutRecovery and secretsmanager:RecoveryWindowInDays from the DeleteSecret input) into the IAM evaluation context, evaluate identity-based Deny statements with Bool/Numeric condition operators before the DeleteSecret handler mutates state, and serialize an awsJson1_1 AccessDeniedException at HTTP 400 with the principal/action/resource-derived explicit-deny message.

Related open issues, neither of which carries this check's replay key: 20261025T093540Z-iam-secretsmanager-secrets-manager-performs-no-identity-based.md (no identity policy is consulted at all, observed on ListSecrets) and 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md (Cause 2, the aws:RequestTag / aws:ResourceTag / secretsmanager:VersionStage condition keys). This cause is the DeleteSecret-specific request-parameter half: even once an identity policy is consulted, these two keys must be derived from the DeleteSecret input or the guardrail still cannot separate the three otherwise-identical calls.

Traces: secretsmanager.recovery-window-condition-keys-forbid-a-force-delete-and-a-short-window.

## Evidence
Cause 1 - check secretsmanager.delete-secret-again-while-scheduled-reschedules-and-force-delete-overrides-the-window [mismatch]:
  Expected (AWS): after DeleteSecret(SecretId=<name>, RecoveryWindowInDays=7), DescribeSecret.DeletedDate is the request time and the DeleteSecret response's DeletionDate is ~7 days later, i.e. round((DeletionDate - DeletedDate)/1 day) == 7.
  Actual (emulator): AssertionError: "the first schedule must be 7 days out, got 0:00:00" at check line 106 -> assert _whole_days(first["DeletionDate"], first_marked) == 7, where first_marked = sm.describe_secret(SecretId=name)["DeletedDate"]. DeletedDate equals DeletionDate, so the recovery window measures as zero. The earlier beats of the same check passed: RestoreSecret on an active secret is accepted as a no-op, DescribeSecret reports no DeletedDate, and the secret stays readable with the created VersionId.

Cause 2 - check secretsmanager.recovery-window-condition-keys-forbid-a-force-delete-and-a-short-window [mismatch]:
  Positive controls that PASSED: DeleteSecret(RecoveryWindowInDays=7) with unrestricted credentials returns a DeletionDate (so 7 is a valid request, not a validation failure); AssumeRole of the guardrailed role works and GetCallerIdentity returns arn:aws:sts::<account>:assumed-role/<role>/...; the role's compliant DeleteSecret(RecoveryWindowInDays=30) is permitted and sets a DeletionDate 29-31 days out, visible as DeletedDate on an independent DescribeSecret.
  Expected (AWS): with an inline role policy that Allows secretsmanager:DeleteSecret on * plus Deny under Bool {secretsmanager:ForceDeleteWithoutRecovery: "true"} and Deny under NumericLessThan {secretsmanager:RecoveryWindowInDays: "30"}, the same role's DeleteSecret(ForceDeleteWithoutRecovery=True) is refused with Error.Code 'AccessDeniedException' at HTTP 400, message 'User: <session arn> is not authorized to perform: secretsmanager:DeleteSecret on resource: <secret arn> with an explicit deny in an identity-based policy'; the secret keeps no DeletedDate and stays readable.
  Actual (emulator): AssertionError: "the guardrail denies DeleteSecret when ForceDeleteWithoutRecovery is true, so the SAME principal, action and resource-pattern must be refused with AccessDeniedException; got 'OK'" at check line 190 -> _delete_probe(as_role, forced["arn"], ForceDeleteWithoutRecovery=True) returned ('OK', ...). The force delete was carried out.

## Steps to Reproduce
Cause 1:
1. CreateSecret(Name=<fresh>, SecretString=<any>).
2. RestoreSecret(SecretId=<name>) - accepted no-op; DescribeSecret shows no DeletedDate and GetSecretValue still serves the created version.
3. DeleteSecret(SecretId=<name>, RecoveryWindowInDays=7); record the response's DeletionDate.
4. DescribeSecret(SecretId=<name>); record DeletedDate.
   Expected: round((DeletionDate - DeletedDate) / 1 day) == 7 (DeletedDate ~= now).
   Observed: DeletionDate - DeletedDate == 0:00:00 (DeletedDate is the future scheduled instant).
5. Then (currently unreachable) DeleteSecret again with RecoveryWindowInDays=12 and confirm DeletionDate moves to the new window, and DeleteSecret(ForceDeleteWithoutRecovery=True) overrides the pending window.

Cause 2:
1. CreateSecret three subject secrets (force / short / compliant), plus a fourth to prove validity.
2. With unrestricted credentials, DeleteSecret(SecretId=<fourth>, RecoveryWindowInDays=7) -> must return a DeletionDate; RestoreSecret it.
3. iam CreateRole trusting arn:aws:iam::<account>:root, then iam PutRolePolicy with: Allow secretsmanager:DeleteSecret+DescribeSecret on *; Deny secretsmanager:DeleteSecret on * when Bool secretsmanager:ForceDeleteWithoutRecovery = "true"; Deny secretsmanager:DeleteSecret on * when NumericLessThan secretsmanager:RecoveryWindowInDays = "30".
4. sts AssumeRole and build a secretsmanager client signing as that session (confirm with GetCallerIdentity).
5. As the role: DeleteSecret(SecretId=<compliant>, RecoveryWindowInDays=30) -> expected and observed success.
6. As the role: DeleteSecret(SecretId=<force>, ForceDeleteWithoutRecovery=True).
   Expected: ClientError Error.Code 'AccessDeniedException', HTTP 400, explicit-deny message naming the session arn, the action and the secret arn; DescribeSecret(<force>) still has no DeletedDate and GetSecretValue still serves its value.
   Observed: HTTP 200 - the delete succeeded.
7. As the role: DeleteSecret(SecretId=<short>, RecoveryWindowInDays=7) -> expected the same AccessDeniedException (step 2 proves 7 is a valid window), then changing only RecoveryWindowInDays to 30 must succeed.
