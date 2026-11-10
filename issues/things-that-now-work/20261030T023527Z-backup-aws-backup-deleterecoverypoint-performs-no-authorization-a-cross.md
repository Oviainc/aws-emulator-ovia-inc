Issue-Source: trace-sweep-divergence
Issue-Trace: backup-recovery-point-error-paths-on-live-vault
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-recovery-point-error-paths-on-live-vault backup-recovery-point-mutation-not-found
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DeleteRecoveryPoint|4xx->4xx|$.Message
Issue-Key: backup|DeleteRecoveryPoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DeleteRecoveryPoint|4xx->4xx|status
Issue-Commit: b74f8bdf7dc8d00213b4902066e9bbcc7a280a48

# AWS Backup DeleteRecoveryPoint performs no authorization: a cross-account recovery point is refused 400 ResourceNotFoundException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `b74f8bdf7dc8d00213b4902066e9bbcc7a280a48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRecoveryPoint

## Description
Cause 1 of 1: DeleteRecoveryPoint on a recovery-point ARN owned by another account is misclassified as not-found instead of access-denied.
AWS: Smithy: DeleteRecoveryPoint DELETE /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}, both members required httpLabel; its modeled errors are InvalidParameterValueException / InvalidRequestException / InvalidResourceStateException / MissingParameterValueException / ResourceNotFoundException (all client/400) and ServiceUnavailableException (500) — AccessDeniedException is recorded behavior only, returned by the authorization layer ahead of resource lookup. The recording shows HTTP 403 with header x-amzn-errortype: AccessDeniedException and the standard AWS authorization message template: "User: <callerArn> is not authorized to perform: backup:DeleteRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DeleteRecoveryPoint action". Fixed text is "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the ", " action"; the variable parts are the caller's principal ARN (from the signing credentials), the action name (service prefix plus the operation, backup:DeleteRecoveryPoint) and the RecoveryPointArn taken from the request path. AWS authorizes before it resolves the resource, so a resource in an account the caller cannot reach never leaks its existence as a not-found.
Emulator: HTTP 400 with x-amzn-errortype: ResourceNotFoundException and $.Message 'Cannot find recovery point' — the request is routed straight to a state lookup, which misses, so every unreachable ARN (including one whose account differs from the caller's) is reported as absent. Both the live-vault error path and the mutation-not-found path collapse onto the same answer.
Why: the status class itself is wrong (4xx client-not-found vs 403 denied), so clients that branch on AccessDenied to re-assume a role, surface a permissions error, or stop retrying instead treat the recovery point as deleted/absent and continue — a delete that AWS refused reads as a no-op success in cleanup logic. Callers that assert on AWS's authorization sentence (action name, principal, resource) get none of it.
Fix: DeleteRecoveryPoint's handler in the backup crate — add the authorization/ownership check ahead of the recovery-point state lookup, and render AccessDeniedException with HTTP 403, the x-amzn-errortype header and the message template above from the caller principal, the operation's action name and the request's RecoveryPointArn. Open issue 20261029T175130Z-backup-aws-backup-recovery-point-restore-job-mpa-and.md covers recovery-point operations that answer 501; this operation does answer, and answers wrongly, so it is not covered there.
Traces: backup-recovery-point-error-paths-on-live-vault#task#9, backup-recovery-point-mutation-not-found#task#3 (same cluster).

## Evidence
Divergence cluster service=backup|DeleteRecoveryPoint|status=403->400: expected status 403, got 400; $headers.x-amzn-errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'; $.Message expected 'User: <callerArn> is not authorized to perform: backup:DeleteRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DeleteRecoveryPoint action', got 'Cannot find recovery point'. Affected steps: backup-recovery-point-error-paths-on-live-vault#task#9 and backup-recovery-point-mutation-not-found#task#3. (The principal ARN, the foreign account id and the recovery-point uuid in the recorded message are the template's variable parts, filled from the request and credentials; they are not requirements.)

## Steps to Reproduce
1. Replay fixture backup-recovery-point-error-paths-on-live-vault, step task#9: call DeleteRecoveryPoint against an existing vault with a RecoveryPointArn whose account is not the caller's.
2. Observe HTTP 400 with x-amzn-errortype ResourceNotFoundException and $.Message 'Cannot find recovery point'; the recording has HTTP 403 with x-amzn-errortype AccessDeniedException and the 'User: ... is not authorized to perform: backup:DeleteRecoveryPoint on resource: ... because no resource-based policy allows the backup:DeleteRecoveryPoint action' message.
3. Same divergence via backup-recovery-point-mutation-not-found#task#3.
