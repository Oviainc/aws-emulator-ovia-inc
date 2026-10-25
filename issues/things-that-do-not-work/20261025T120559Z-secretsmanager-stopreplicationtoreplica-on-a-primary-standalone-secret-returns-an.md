Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.replication-lifecycle
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.replication-lifecycle|run#8|AssertionError|assert stop_message == (
Issue-Commit: a97363f812621dd47aa0891b39cb5eeebafd4acb

# StopReplicationToReplica on a primary/standalone secret returns an emulator-authored InvalidParameterException message

> _Note: this issue was identified against the code base at commit `a97363f812621dd47aa0891b39cb5eeebafd4acb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopReplicationToReplica

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the InvalidParameterException message for StopReplicationToReplica on a non-replica secret is not AWS's wording ===

AWS: StopReplicationToReplica is the replica-side promotion call; it is only meaningful when the SecretId names a REPLICA and the call is made in that replica's region (Smithy: StopReplicationToReplica, input SecretId (SecretIdType, required, @length 1..2048), output ARN; modeled errors InvalidParameterException (400), InvalidRequestException (400), ResourceNotFoundException (400), InternalServiceError (500)). Called against a secret that is a primary or a standalone (unreplicated) secret, AWS refuses with HTTP 400, error code InvalidParameterException and the message "Operation not permitted on a primary/standalone secret." That sentence is entirely FIXED text — it has no variable part, it interpolates no secret name, ARN or region from the request or from state. The model carries no prose for this case, so the sentence itself is recorded behavior only; the status (400) and the code (InvalidParameterException) are modeled.

Emulator: the status and the error code are correct (400 / InvalidParameterException), but the message body is the emulator's own sentence: "Operation not permitted on a primary secret. Call must be made in replica secret's region." It differs both in the resource class it names (AWS's single "primary/standalone secret" vs the emulator's "primary secret") and in appending a second sentence about the caller's region that AWS does not emit for this refusal.

Why: an error message is part of the contract. A client, test or runbook that branches on, logs or asserts the refusal text for "this secret is not a replica" sees text AWS never produces; in particular the emulator's extra "Call must be made in replica secret's region" sentence mis-diagnoses a standalone secret (which has no replica region at all) as a wrong-region call, sending a caller to retry in another region instead of concluding the secret is not a replica. This is the first assertion of the replication-lifecycle check, so the divergence also halts the rest of that lifecycle (re-replication idempotence, replica-blocked delete, RemoveRegionsFromReplication, force delete) before it is exercised.

Fix: the StopReplicationToReplica handler's refusal path for a primary/standalone secret in the secretsmanager service crate (services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/stop_replication_to_replica.rs) — keep the 400/InvalidParameterException classification and emit AWS's fixed sentence verbatim, with no request- or state-derived interpolation.

Note on neighbouring filed work: open issue 20261024T190718Z-secretsmanager-secrets-manager-replication-delete-lifecycle-access-metadata-and.md lists this same check among its trace ids, but its replication causes are (a) StopReplicationToReplica answering 400 NotImplemented and (b) ReplicationStatus always InSync with no StatusMessage — a different replay key further down the same check. The operation now answers a real modeled refusal, so that cause is no longer what this assertion fails on; the message text of that refusal is a separate, net-new cause and is not carried by any open issue. Fixed issue 20261023T182429Z-secretsmanager-secrets-manager-error-responses-diverge-wrong-exception-type.md covers emulator-authored message text for GetSecretValue, PutSecretValue, DeleteSecret, DescribeSecret, ListSecrets and ListSecretVersionIds only; it does not name StopReplicationToReplica, and this call is refused exactly where AWS refuses it, so this is not a regression of that fix.

Traces: secretsmanager.replication-lifecycle (assertion at check line 81, run#…, on the StopReplicationToReplica message).

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, check [mismatch] secretsmanager.replication-lifecycle:

  File "check_secretsmanager_replication_lifecycle.py", line 81, in run
    assert stop_message == (
        "Operation not permitted on a primary/standalone secret."
    ), f"StopReplicationToReplica on a non-replica secret returned message {stop_message!r}"
  AssertionError: StopReplicationToReplica on a non-replica secret returned message "Operation not permitted on a primary secret. Call must be made in replica secret's region."

Expected (AWS): HTTP 400, Error.Code = InvalidParameterException, Error.Message = "Operation not permitted on a primary/standalone secret." (fixed text, no variable parts).
Actual (emulator): HTTP 400, Error.Code = InvalidParameterException (matches — the preceding assertion on stop_code passed), Error.Message = "Operation not permitted on a primary secret. Call must be made in replica secret's region."

The divergence is confined to the message body: the fixed sentence is reworded ("primary secret" for AWS's "primary/standalone secret") and an additional region sentence is appended that AWS does not emit. No per-run value (secret name, ARN, region, request id, timestamp) is involved on either side.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (the single check is secretsmanager.replication-lifecycle).
2. With a secretsmanager client in the emulator's region, CreateSecret(Name=<fresh name>, SecretString="value") — the secret is standalone, with no replica regions.
3. Call StopReplicationToReplica(SecretId=<that name>) and catch the ClientError.
4. Read response["Error"]["Code"] and response["Error"]["Message"].
   Observed: Code = InvalidParameterException (correct), Message = "Operation not permitted on a primary secret. Call must be made in replica secret's region."
   Expected: Code = InvalidParameterException, Message = "Operation not permitted on a primary/standalone secret."
5. The same refusal must hold for a PRIMARY secret that does have live replicas: ReplicateSecretToRegions(SecretId=<name>, AddReplicaRegions=[{"Region": <a second region>}]), then StopReplicationToReplica(SecretId=<name>) against the primary — AWS answers the identical fixed sentence.
