Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.concurrent-update-secret-version-stage-last-write-wins
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.concurrent-update-secret-version-stage-last-write-wins|run#28|AssertionError|assert stale_code == "InvalidParameterException", (
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# Secrets Manager UpdateSecretVersionStage: a stale RemoveFromVersionId is refused with InvalidRequestException and the wrong message instead of InvalidParameterException (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSecretVersionStage

## Description
Cause 1 of 1: a move of a staging label whose RemoveFromVersionId does not name the version that currently holds the label is refused with the wrong modeled exception and the wrong message.

AWS: UpdateSecretVersionStage models both InvalidParameterException and InvalidRequestException (HTTP 400 each; Smithy: UpdateSecretVersionStage errors = InternalServiceError, InvalidParameterException, InvalidRequestException, LimitExceededException, ResourceNotFoundException). The model's UpdateSecretVersionStageRequest$RemoveFromVersionId says: "If the staging label you are trying to attach to one version is already attached to a different version, then you must include this parameter and specify the version that the label is to be removed from. If the label is attached and you either do not specify this parameter, or the version ID does not match, then the operation fails." A mismatched version ID is a bad PARAMETER VALUE, not a resource-state conflict, so AWS classifies it as InvalidParameterException ("The parameter name or value is invalid."), reserving InvalidRequestException for "a parameter value is not valid for the current state of the resource" (secret scheduled for deletion, rotation not configured, secret owned by another service). Recorded behavior: HTTP 400, Error.Code `InvalidParameterException`, Error.Message template -- fixed text "When you move staging label ", <the VersionStage from the request>, ", if you specify RemoveFromVersionId, it must be set to the version that currently has the staging label ", <the version id that currently holds that label, from the emulator's state>, "." Everything outside those two substitutions is fixed text; the label name comes from the request and the version id from state, and neither one's value from any single run is part of the requirement.

Emulator: answers HTTP 400 with Error.Code `InvalidRequestException` and the message "Staging label <stage> is currently attached to version <holder>, so you can't remove it from version <named>." -- a code AWS does not use for this refusal and wording AWS does not use at all. The refusal itself happens, and the AWSCURRENT holder is correctly left unchanged; only the error identity and text diverge.

Why: botocore/SDK clients branch on the error code. `except client.exceptions.InvalidParameterException` (and any equivalent catch in Java/Go/.NET) does not catch InvalidRequestException, so the rotation and label-swap retry logic AWS customers write around a lost race -- re-read the current holder and retry -- never runs; the call surfaces as an unhandled ClientError instead. Code that matches on the message to find the current holder also fails, since the emulator's sentence has a different shape.

Fix: the refusal classification and message construction in this operation's validation -- the `stale_remove_from` helper in the UpdateSecretVersionStage handler, which is reached from both the move path (MoveToVersionId plus a RemoveFromVersionId that does not match the holder) and the removal-only path. The move path must raise InvalidParameterException with AWS's wording above. Note the neighbouring refusal in the same handler for an ABSENT RemoveFromVersionId already uses InvalidParameterException, so the two sibling refusals of one rule currently disagree with each other.

Related but NOT covering this: open issue 20261024T190718Z-secretsmanager-secrets-manager-refusals-pick-the-wrong-exception-code.md bundles other wrong-code refusals in this service but does not carry this check's replay key, so this cause is net-new work.

Traces: failing check secretsmanager.concurrent-update-secret-version-stage-last-write-wins (verdict: mismatch). Emulator source for the diverging branch: services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret_version_stage.rs lines 246-268 (branch selection) and 337-353 (`stale_remove_from`).

## Evidence
Expected (AWS): UpdateSecretVersionStage(VersionStage="AWSCURRENT", MoveToVersionId=V_loser, RemoveFromVersionId=V_loser) while V_winner holds AWSCURRENT -> HTTP 400, Error.Code="InvalidParameterException", Error.Message="When you move staging label AWSCURRENT, if you specify RemoveFromVersionId, it must be set to the version that currently has the staging label <V_winner>." (fixed text as shown; "AWSCURRENT" is the request's VersionStage and <V_winner> is the holding version id from state).

Actual (emulator): HTTP 400, Error.Code="InvalidRequestException", Error.Message="Staging label AWSCURRENT is currently attached to version <V_winner>, so you can't remove it from version <V_loser>."

Replay assertion that failed: `AssertionError: a move of AWSCURRENT to 165bff11-5c72-456c-a819-e0f3ff089afd with stale RemoveFromVersionId=165bff11-5c72-456c-a819-e0f3ff089afd while 672f26fa-d6cd-4f2c-8684-39c461572401 holds it must be rejected with InvalidParameterException, got 'InvalidRequestException'` (the version ids are per-run values, not part of the requirement). The message assertion that follows it also fails for the same call.

Unaffected and already correct: the swap resolves with exactly one AWSCURRENT holder, GetSecretValue(VersionStage=AWSCURRENT) agrees with it, and the rejected stale move leaves the holder unchanged.

## Steps to Reproduce
1. CreateSecret(Name=<unique>, SecretString="version-1") -> version V1 holds AWSCURRENT.
2. PutSecretValue(SecretId=<arn>, SecretString="version-2") -> version V2 now holds AWSCURRENT, V1 does not.
3. Race two opposite UpdateSecretVersionStage calls on VersionStage="AWSCURRENT": (MoveToVersionId=V1, RemoveFromVersionId=V2) and (MoveToVersionId=V2, RemoveFromVersionId=V1). One wins; call the holder W and the other L.
4. Call UpdateSecretVersionStage(SecretId=<arn>, VersionStage="AWSCURRENT", MoveToVersionId=L, RemoveFromVersionId=L) -- i.e. a stale RemoveFromVersionId that does not name W.
5. Observe the raised error: the emulator returns Error.Code "InvalidRequestException" with its own wording; AWS returns "InvalidParameterException" with "When you move staging label AWSCURRENT, if you specify RemoveFromVersionId, it must be set to the version that currently has the staging label W."

Step 4 alone reproduces it without the race: create a secret, PutSecretValue once, then call UpdateSecretVersionStage(VersionStage="AWSCURRENT", MoveToVersionId=V1, RemoveFromVersionId=V1) while V2 holds AWSCURRENT.
