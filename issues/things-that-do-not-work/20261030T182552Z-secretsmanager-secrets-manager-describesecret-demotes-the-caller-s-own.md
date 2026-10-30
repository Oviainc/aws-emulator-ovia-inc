Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.sim2real-crt-feaa3781363869b83db3319ee3dc3d4a[*]
Issue-Commit: 4226368beaf76fbb89a0358ac027d2e5fc79db7f

# Secrets Manager DescribeSecret demotes the caller's own AWSCURRENT version to AWSPREVIOUS

> _Note: this issue was identified against the code base at commit `4226368beaf76fbb89a0358ac027d2e5fc79db7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One cause.

Cause 1 of 1: DescribeSecret reports the version the caller staked with its OWN ClientRequestToken as AWSPREVIOUS instead of AWSCURRENT (fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6).

AWS: Smithy: DescribeSecret (input SecretId -> SecretIdType, required, @length 1..2048; output VersionIdsToStages -> SecretVersionsToStagesMapType, a map whose key is SecretVersionIdType @length 32..64 and whose value is SecretVersionStagesType, a list @length 1..20 of SecretVersionStageType). The map reports, for each stored version of the secret, the staging labels currently attached to it. In this fixture the caller creates a secret supplying its own ClientRequestToken, then issues a PutSecretValue and an UpdateSecret that each OMIT ClientRequestToken; AWS refuses both of those version-creating calls (HTTP 400 InvalidRequestException, "You must provide a ClientRequestToken value. We recommend a UUID-type value."), so no new version is ever created and the caller's own version still holds AWSCURRENT. The recorded response for this step is HTTP 200 with that version id mapped to ["AWSCURRENT"]. The requirement is the LABEL: the map key is whatever ClientRequestToken value the caller sent (in this run sim2real-crt-feaa3781363869b83db3319ee3dc3d4a, a per-run value that is not part of the requirement), and the only version that exists must carry AWSCURRENT. DescribeSecret models no error for this path (its modeled errors are InternalServiceError 500, InvalidParameterException 400, ResourceNotFoundException 400), and the recording shows no error: status, code and message all match; only the staging label diverges.

Emulator: HTTP 200, same status and shape, but $.VersionIdsToStages.<caller-token-version>[0] is "AWSPREVIOUS" where AWS reports "AWSCURRENT". The emulator accepted the two token-less writes, minted server-side version ids for them, and moved AWSCURRENT onto the last of those versions, demoting the caller's version.

Why: AWSCURRENT is the label every GetSecretValue resolves when the caller names no VersionId or VersionStage. A client that has just had two writes refused by AWS, and then reads back its secret, is told its version is no longer current, so it either reads a value AWS never stored or concludes its own version was superseded. Any code that pins, promotes or reconciles staging labels off DescribeSecret (rotation tooling in particular, which decides what to move AWSPENDING/AWSCURRENT onto by reading this map) acts on an inverted picture of which version is live.

Fix: this is the version/staging bookkeeping reached through the version-creating handlers (PutSecretValue and UpdateSecret) and the state that owns a secret's version-to-staging-label map, not DescribeSecret's own serializer -- DescribeSecret faithfully reports the labels it is given. Hypothesis: the label is wrong purely because those two handlers admitted writes AWS rejects for a missing ClientRequestToken; refusing them at their request-validation seam leaves the caller's version holding AWSCURRENT and makes this step pass with no change to DescribeSecret. Confirm by re-replaying this step, because nothing else in the corpus exercises the label after a refused write.

Related, and why this is filed separately: open issue 20261029T224105Z-secretsmanager-secrets-manager-accepts-createsecret-putsecretvalue-and-updatesecret-with.md describes this same step as its "Cause 4 of 5". It does NOT own this divergence's replay key as the replay reports it: its Issue-Key for this cluster embeds a version id from an earlier run (sim2real-crt-a1b2230e741ec402a10361dec5e8a2b9[*]) while this run's key is sim2real-crt-feaa3781363869b83db3319ee3dc3d4a[0], so the per-run ClientRequestToken value prevents the keys from joining and the cluster came back with an empty owned_by. A fixer closing that issue should close this one with the same edit; it is filed so the step is not left unowned and unreplayed. Separately, that key-matching fragility is itself worth noting: an ownership key that embeds a caller-supplied per-run token can never re-match a later replay.

Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#6 (cluster of 1; affected_count 1).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, 155 steps, 135 passed, 16 mismatched, 4 blocked, score 87.1%, 9 divergence clusters).

Cluster divergence_key secretsmanager|DescribeSecret|status=200->200|keys=$.VersionIdsToStages.sim2real-crt-feaa3781363869b83db3319ee3dc3d4a[0], verdict mismatch, owned_by empty, stub_path empty.

Fixture: secretsmanager-create-put-update-secret-without-client-request-token, step task#6, operation DescribeSecret.
Expected status 200; actual status 200 (status, __type and Message all agree -- this is a body-value divergence only).
Detail as reported: "$.VersionIdsToStages.sim2real-crt-feaa3781363869b83db3319ee3dc3d4a[0]: value_mismatch (expected 'AWSCURRENT', got 'AWSPREVIOUS')".

Expected: the caller's own ClientRequestToken-keyed version mapped to ["AWSCURRENT"].
Actual: the same version mapped to ["AWSPREVIOUS"].

Preceding steps in the same fixture show the mechanism: task#4 (PutSecretValue, no ClientRequestToken) and task#5 (UpdateSecret, no ClientRequestToken) each recorded HTTP 400 InvalidRequestException "You must provide a ClientRequestToken value. We recommend a UUID-type value." but the emulator answered HTTP 200, minting a version for each. Those two refusals are already filed under 20261029T224105Z-secretsmanager-secrets-manager-accepts-createsecret-putsecretvalue-and-updatesecret-with.md (owned_by non-empty) and are NOT re-filed here.

Model facts used (services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): DescribeSecret output member VersionIdsToStages targets SecretVersionsToStagesMapType; key SecretVersionIdType @length 32..64; value SecretVersionStagesType, list @length 1..20 of SecretVersionStageType @length 1..256. DescribeSecret errors: InternalServiceError (500), InvalidParameterException (400), ResourceNotFoundException (400).

## Steps to Reproduce
1. Point a raw HTTP client (not an AWS SDK -- SDKs auto-fill @idempotencyToken members, so the divergence cannot be reached through one) at the emulator's Secrets Manager endpoint, signing requests normally.
2. POST X-Amz-Target: secretsmanager.CreateSecret with body {"Name": "<unique>", "SecretString": "v1", "ClientRequestToken": "<your own token, 32-64 chars>"}. Note the version id returned -- it is your token value, and it holds AWSCURRENT.
3. POST X-Amz-Target: secretsmanager.DescribeSecret {"SecretId": "<that secret>"} and confirm VersionIdsToStages maps your token to ["AWSCURRENT"] at this point.
4. POST X-Amz-Target: secretsmanager.PutSecretValue {"SecretId": "<that secret>", "SecretString": "v2"} with NO ClientRequestToken member. AWS answers HTTP 400 InvalidRequestException; the emulator answers HTTP 200 and mints a version.
5. POST X-Amz-Target: secretsmanager.UpdateSecret {"SecretId": "<that secret>", "SecretString": "v3"} with NO ClientRequestToken member. AWS answers HTTP 400 InvalidRequestException; the emulator answers HTTP 200 and mints another version.
6. POST X-Amz-Target: secretsmanager.DescribeSecret {"SecretId": "<that secret>"} again. AWS reports VersionIdsToStages[<your token>] == ["AWSCURRENT"], because steps 4 and 5 never happened. The emulator reports ["AWSPREVIOUS"] -- the divergence (fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6).
7. Re-replay that fixture step to confirm a fix; substitute your own ClientRequestToken value for the map key, which differs per run.
