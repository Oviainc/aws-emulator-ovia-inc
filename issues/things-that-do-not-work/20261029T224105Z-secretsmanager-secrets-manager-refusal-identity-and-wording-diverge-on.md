Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-rotation-configuration-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-rotation-configuration-errors secretsmanager-cross-region-replication-lifecycle secretsmanager-version-stage-move-conflicts
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|RotateSecret|4xx->4xx|$.Message
Issue-Key: secretsmanager|RotateSecret|4xx->2xx|$.Message
Issue-Key: secretsmanager|RotateSecret|4xx->2xx|$.__type
Issue-Key: secretsmanager|RotateSecret|4xx->2xx|status
Issue-Key: secretsmanager|StopReplicationToReplica|4xx->4xx|$.Message
Issue-Key: secretsmanager|UpdateSecretVersionStage|4xx->4xx|$.Message
Issue-Key: secretsmanager|UpdateSecretVersionStage|4xx->4xx|$.__type
Issue-Commit: 37be8999abc11e440bb26a3acee60c47f75abac5

# Secrets Manager refusal identity and wording diverge on RotateSecret, StopReplicationToReplica and UpdateSecretVersionStage, and one rotation refusal is never raised (4 causes)

> _Note: this issue was identified against the code base at commit `37be8999abc11e440bb26a3acee60c47f75abac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Four net-new wire causes sharing one fault: when these operations refuse a request they pick the wrong error identity or their own wording instead of AWS's, and in one case they do not refuse at all. Status code and refusal placement are otherwise right, so only the error contract diverges.

Cause 1 of 4 - RotateSecret on a secret with no rotation function configured uses the emulator's own sentence (fixture secretsmanager-rotation-configuration-errors, step task#2).
AWS: Smithy: RotateSecret errors = InternalServiceError, InvalidParameterException, InvalidRequestException, ResourceNotFoundException; InvalidRequestException is documented for exactly this cause - "You tried to enable rotation on a secret that doesn't already have a Lambda function ARN configured and you didn't include such an ARN as a parameter in this call." Recorded response: HTTP 400, __type "InvalidRequestException", Message "No Lambda rotation function ARN is associated with this secret." That sentence is entirely FIXED text for this error class - it interpolates no secret name, arn or parameter value.
Emulator: HTTP 400 and the right __type, but Message "You must specify RotationLambdaARN, or the secret must already have a Lambda rotation function configured, to enable rotation." - wording AWS never emits.
Why: rotation tooling (and customer runbooks) match on this message to distinguish "rotation not configured yet" from the other InvalidRequestException causes RotateSecret shares the code with (scheduled for deletion, managed by another service, rotation already in progress); a different sentence makes those indistinguishable.
Fix: the message constant in this operation's validation branch - services/.../src/ops/rotate_secret.rs lines 279-288.
Traces: secretsmanager-rotation-configuration-errors#task#2.

Cause 2 of 4 - RotateSecret is ACCEPTED for a rotation Lambda Secrets Manager cannot invoke (same fixture, step task#3).
AWS: recorded behavior only - AccessDeniedException is not in RotateSecret's modeled error list, so AWS raises it as a service-common error. Recorded response: HTTP 400, __type "AccessDeniedException", Message "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com." - fixed text for this error class; nothing in it comes from the request. AWS verifies, at RotateSecret time, that the named function's resource policy lets principal secretsmanager.amazonaws.com invoke it, and refuses the call before storing any rotation configuration.
Emulator: HTTP 200 with a normal RotateSecretResponse (ARN/Name/VersionId): the handler never looks at the function at all - it validates only the schedule rules, deletion state, owning service, an in-progress rotation and a colliding ClientRequestToken, then stores the rotation config and stakes AWSPENDING.
Why: a misconfigured rotation is reported as configured. The caller's secret is left with rotation "enabled" against a function that can never run, which is precisely the state AWS refuses to create, and the client's error-handling path for a missing Lambda permission never executes.
Fix: this operation's validation - before configure_rotation()/start_rotation(), resolve RotationLambdaARN (or the secret's stored one) and refuse with AccessDeniedException and AWS's wording when the function does not exist or its policy does not grant secretsmanager.amazonaws.com invoke permission.
Traces: secretsmanager-rotation-configuration-errors#task#3.

Cause 3 of 4 - StopReplicationToReplica on a primary/standalone secret uses the wrong sentence (fixture secretsmanager-cross-region-replication-lifecycle, step task#4).
AWS: Smithy: StopReplicationToReplica errors = InternalServiceError, InvalidParameterException, InvalidRequestException, ResourceNotFoundException; the operation is documented as having to be called "from the Region in which you want to promote the replica to a primary secret". Recorded response: HTTP 400 with Message "Operation not permitted on a primary secret. Call must be made in replica secret's region." - fixed text for this error class, no interpolation.
Emulator: HTTP 400 with the matching error code but Message "Operation not permitted on a primary/standalone secret." - it drops the second sentence, which is the part that tells the caller what to do (re-issue the call against the replica Region).
Why: the actionable half of the refusal is lost, and any client or test asserting AWS's message fails against the emulator.
Fix: the message constant behind this branch - crate::state::NOT_A_REPLICA_MESSAGE, used at services/.../src/ops/stop_replication_to_replica.rs lines 210-222.
Traces: secretsmanager-cross-region-replication-lifecycle#task#4.

Cause 4 of 4 - UpdateSecretVersionStage refuses a stale RemoveFromVersionId with the wrong exception code AND the wrong message (fixture secretsmanager-version-stage-move-conflicts, step task#4).
AWS: Smithy: UpdateSecretVersionStage errors include InvalidParameterException ("The parameter name or value is invalid.") and InvalidRequestException ("A parameter value is not valid for the current state of the resource"); UpdateSecretVersionStageRequest$RemoveFromVersionId documents "If the label is attached and you either do not specify this parameter, or the version ID does not match, then the operation fails." A RemoveFromVersionId that does not name the holding version is a bad parameter VALUE, so AWS classifies it InvalidParameterException. Recorded response: HTTP 400, __type "InvalidParameterException", Message template - fixed text "When you move staging label ", then <the VersionStage from the request>, then ", if you specify RemoveFromVersionId, it must be set to the version that currently has the staging label ", then <the version id that currently holds that label, read from state>, then ".".
Emulator: HTTP 400 with __type "InvalidRequestException" and Message "Staging label <stage> is currently attached to version <holder>, so you can't remove it from version <named>." - a code AWS does not use for this refusal and a sentence it does not use at all.
Why: SDK clients branch on the code (`except client.exceptions.InvalidParameterException`, typed catches in Java/Go/.NET), so the lost-race retry logic written around a stale label move never runs and the call surfaces as an unhandled ClientError; message-scraping code that recovers the current holder also breaks.
Fix: the refusal classification and message construction in this operation's validation (the stale-RemoveFromVersionId branch of update_secret_version_stage.rs; the sibling refusal for an ABSENT RemoveFromVersionId in the same handler already uses InvalidParameterException, so the two halves of one rule disagree).
Traces: secretsmanager-version-stage-move-conflicts#task#4.

Related but NOT covering these: open issue 20261029T071027Z-secretsmanager-secrets-manager-updatesecretversionstage-a-stale-removefromversionid-is-refused.md describes the same classification defect as cause 4 but is grounded in a BEHAVIORAL check and carries no wire replay key, so this fixture step is unexercised by it; open issues 20261024T190718Z-secretsmanager-secrets-manager-refusals-pick-the-wrong-exception-code.md, 20261029T205254Z-secretsmanager-secrets-manager-kmskeyid-is-never-validated-against-kms.md and 20261029T180026Z-dynamodb-lambda-s3-secretsmanager-secrets-manager-rotatesecret-is-accepted-for-an-un.md bundle neighbouring wrong-code/wrong-message and rotation findings but none of them carries these four fixture steps' replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 86.5%), four mismatch clusters:

1) secretsmanager-rotation-configuration-errors#task#2 RotateSecret - both sides HTTP 400; $.Message expected "No Lambda rotation function ARN is associated with this secret.", got "You must specify RotationLambdaARN, or the secret must already have a Lambda rotation function configured, to enable rotation."
2) secretsmanager-rotation-configuration-errors#task#3 RotateSecret - expected HTTP 400 with __type "AccessDeniedException" and Message "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com."; actual HTTP 200 with no __type and no Message.
3) secretsmanager-cross-region-replication-lifecycle#task#4 StopReplicationToReplica - both sides HTTP 400; $.Message expected "Operation not permitted on a primary secret. Call must be made in replica secret's region.", got "Operation not permitted on a primary/standalone secret."
4) secretsmanager-version-stage-move-conflicts#task#4 UpdateSecretVersionStage - both sides HTTP 400; $.__type expected "InvalidParameterException", got "InvalidRequestException"; $.Message expected "When you move staging label AWSCURRENT, if you specify RemoveFromVersionId, it must be set to the version that currently has the staging label <holder>.", got "Staging label AWSCURRENT is currently attached to version <holder>, so you can't remove it from version <named>." (the recorded run's version ids d2d62afb-... and a3aa6cb1-... are per-run values filling the template, not part of the requirement).

Emulator source: services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/rotate_secret.rs lines 279-288 (cause 1 message) and 205-360 (cause 2: the whole handle body, which never resolves or authorizes the rotation function); src/ops/stop_replication_to_replica.rs lines 210-222 with crate::state::NOT_A_REPLICA_MESSAGE (cause 3); src/ops/update_secret_version_stage.rs stale-RemoveFromVersionId branch (cause 4).

## Steps to Reproduce
1. CreateSecret(Name=<unique>, SecretString="v1", ClientRequestToken=<32+ chars>).
2. RotateSecret(SecretId=<arn>) with no RotationLambdaARN and no rotation configured -> HTTP 400 InvalidRequestException; compare Message against AWS's "No Lambda rotation function ARN is associated with this secret." (cause 1).
3. RotateSecret(SecretId=<arn>, RotationLambdaARN=<a Lambda whose resource policy does NOT allow secretsmanager.amazonaws.com to invoke it, or one that does not exist>, RotateImmediately=true) -> AWS answers 400 AccessDeniedException "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com."; the emulator answers 200 and stores the rotation config (cause 2).
4. Create a secret with AddReplicaRegions so a primary exists in region A, then call StopReplicationToReplica(SecretId=<arn>) signed for region A (the PRIMARY's own region) -> HTTP 400; compare Message against "Operation not permitted on a primary secret. Call must be made in replica secret's region." (cause 3).
5. CreateSecret then PutSecretValue once, so V2 holds AWSCURRENT and V1 does not. Call UpdateSecretVersionStage(SecretId=<arn>, VersionStage="AWSCURRENT", MoveToVersionId=V1, RemoveFromVersionId=V1) -> AWS answers InvalidParameterException with "When you move staging label AWSCURRENT, if you specify RemoveFromVersionId, it must be set to the version that currently has the staging label V2."; the emulator answers InvalidRequestException with its own sentence (cause 4).
