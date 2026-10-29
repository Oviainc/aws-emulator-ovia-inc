Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|CreateSecret|4xx->2xx|$.Message
Issue-Key: secretsmanager|CreateSecret|4xx->2xx|$.__type
Issue-Key: secretsmanager|CreateSecret|4xx->2xx|status
Issue-Key: secretsmanager|PutSecretValue|4xx->2xx|$.Message
Issue-Key: secretsmanager|PutSecretValue|4xx->2xx|$.__type
Issue-Key: secretsmanager|PutSecretValue|4xx->2xx|status
Issue-Key: secretsmanager|UpdateSecret|4xx->2xx|$.Message
Issue-Key: secretsmanager|UpdateSecret|4xx->2xx|$.__type
Issue-Key: secretsmanager|UpdateSecret|4xx->2xx|status
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.sim2real-crt-a1b2230e741ec402a10361dec5e8a2b9[*]
Issue-Key: secretsmanager|DescribeSecret|4xx->2xx|$.Message
Issue-Key: secretsmanager|DescribeSecret|4xx->2xx|$.__type
Issue-Key: secretsmanager|DescribeSecret|4xx->2xx|status
Issue-Commit: 37be8999abc11e440bb26a3acee60c47f75abac5

# Secrets Manager accepts CreateSecret, PutSecretValue and UpdateSecret with no ClientRequestToken and mints a version id instead of refusing (5 causes)

> _Note: this issue was identified against the code base at commit `37be8999abc11e440bb26a3acee60c47f75abac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecret

## Description
Five net-new wire causes, all one root cause: a raw HTTP request that omits ClientRequestToken on a version-creating Secrets Manager operation must be REFUSED, and the emulator instead invents a version id and succeeds. Causes 1-3 are the three missing refusals; causes 4-5 are the state divergences the accepted calls leave behind in the same fixture (same root cause, separate replay steps, so each must be re-replayed to confirm a fix).

Cause 1 of 5 - CreateSecret with SecretString and no ClientRequestToken (fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#1; cluster also covers task#2).
AWS: Smithy: CreateSecret.ClientRequestToken -> ClientRequestTokenType, @idempotencyToken, @length min 32 max 64, documented as "If you use the Amazon Web Services CLI or one of the Amazon Web Services SDKs to call this operation, then you can leave this parameter empty. The CLI or SDK generates a random UUID for you ... If you generate a raw HTTP request to the Secrets Manager service endpoint, then you must generate a ClientRequestToken and include it in the request." CreateSecret models InvalidRequestException (HTTP 400, "A parameter value is not valid for the current state of the resource"). Recorded response: HTTP 400, __type "InvalidRequestException", Message "You must provide a ClientRequestToken value. We recommend a UUID-type value." - that whole sentence is FIXED text for this error class; it interpolates nothing from the request or from state.
Emulator: HTTP 200 with a normal CreateSecretResponse. No ClientRequestToken check exists; the handler falls through to `version_id: input.client_request_token.clone().unwrap_or_else(crate::state::new_version_id)`, so a server-minted id is staked and AWSCURRENT is attached to it.
Why: a client that generates raw SigV4 requests (or any SDK path that strips the idempotency token) gets a secret version whose id the caller never chose and cannot reproduce, so the retry it would make on a timeout creates a SECOND version instead of replaying the first - the exact accidental-duplication the idempotency token exists to prevent. Code that asserts on the documented 400 never sees it.
Fix: the request validation of this operation's handler - refuse before anything is stored when ClientRequestToken is absent, with InvalidRequestException and AWS's wording, rather than defaulting the version id.
Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#1 (and #task#2 in the same cluster).

Cause 2 of 5 - PutSecretValue with no ClientRequestToken (same fixture, step task#4).
AWS: Smithy: PutSecretValue.ClientRequestToken -> ClientRequestTokenType, @idempotencyToken, @length 32..64, same raw-HTTP requirement in its docs; PutSecretValue models InvalidRequestException (HTTP 400). Recorded response: HTTP 400, __type "InvalidRequestException", Message "You must provide a ClientRequestToken value. We recommend a UUID-type value." (fixed text, no substitutions).
Emulator: HTTP 200 - the new version is stored under a minted id and AWSCURRENT is moved onto it.
Why: as cause 1, and worse: the accepted write demotes the caller's current version, so a client that expected a 400 silently rotates the live value.
Fix: same validation seam in this operation's handler.
Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#4.

Cause 3 of 5 - UpdateSecret with no ClientRequestToken (same fixture, step task#5).
AWS: Smithy: UpdateSecret.ClientRequestToken -> ClientRequestTokenType, @idempotencyToken, @length 32..64; UpdateSecret models InvalidRequestException (HTTP 400). Recorded response: HTTP 400, __type "InvalidRequestException", Message "You must provide a ClientRequestToken value. We recommend a UUID-type value." (fixed text).
Emulator: HTTP 200, new version minted and labelled AWSCURRENT.
Why: as cause 2.
Fix: same validation seam in this operation's handler.
Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#5.

Cause 4 of 5 - DescribeSecret reports the caller's own version demoted to AWSPREVIOUS (same fixture, step task#6).
AWS: DescribeSecret.VersionIdsToStages maps each stored version id to its staging labels (Smithy: DescribeSecret output VersionIdsToStages -> SecretVersionsToStagesMapType). Because the token-less PutSecretValue/UpdateSecret calls above are refused, the version the caller created WITH its own ClientRequestToken still holds AWSCURRENT: recorded response HTTP 200 with that version id mapped to ["AWSCURRENT"]. The key is the caller's own token value from the request; only the label is the requirement.
Emulator: HTTP 200, but that version is mapped to ["AWSPREVIOUS"] - it was demoted by the writes AWS rejected.
Why: AWSCURRENT is what every GetSecretValue without an explicit stage resolves, so a client reading its own secret after two refused calls gets the wrong (or no) current value. Hypothesis: this is purely downstream of causes 2-3; fixing the refusals restores the labelling, and this step is listed so a fixer re-replays it.
Fix: no separate change expected beyond causes 2-3 (the version/staging bookkeeping in this service's state is correct for the writes it is given).
Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#6.

Cause 5 of 5 - DescribeSecret answers 200 for a secret that was never created (same fixture, step task#7).
AWS: DescribeSecret models ResourceNotFoundException (HTTP 400). Recorded response: HTTP 400, __type "ResourceNotFoundException", Message "Secrets Manager can't find the specified secret." - fixed text for this error class, no substitutions. The secret does not exist precisely because its CreateSecret (cause 1) was refused for the missing token.
Emulator: HTTP 200 with a full DescribeSecret body for a secret AWS never created.
Why: existence checks invert - a client probing whether a creation succeeded is told yes. Hypothesis: downstream of cause 1; the handler's own not-found branch is present and correct, it is simply handed a secret that should not be in the store.
Fix: no separate change expected beyond cause 1; re-replay this step to confirm.
Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#7.

Related but NOT covering these: open issues 20261024T190718Z-secretsmanager-secrets-manager-refusals-pick-the-wrong-exception-code.md and 20261025T173233Z-secretsmanager-secrets-manager-updatesecret-updatesecretversionstage-awspending-is-not-evicted.md touch neighbouring Secrets Manager validation and staging behavior, but neither carries these fixture steps' replay keys nor the missing-ClientRequestToken rule, so all five causes are net-new work.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 86.5%, 13 diverged steps), all five clusters in fixture secretsmanager-create-put-update-secret-without-client-request-token:

1) task#1 CreateSecret - expected HTTP 400 / __type "InvalidRequestException" / Message "You must provide a ClientRequestToken value. We recommend a UUID-type value."; actual HTTP 200, no __type, no Message. Cluster member: task#2.
2) task#4 PutSecretValue - expected HTTP 400 / "InvalidRequestException" / same Message; actual HTTP 200.
3) task#5 UpdateSecret - expected HTTP 400 / "InvalidRequestException" / same Message; actual HTTP 200.
4) task#6 DescribeSecret - expected $.VersionIdsToStages.<caller-token-version>[0] == "AWSCURRENT"; actual "AWSPREVIOUS" (recorded key in this run: sim2real-crt-a1b2230e741ec402a10361dec5e8a2b9 - a per-run token value, not part of the requirement).
5) task#7 DescribeSecret - expected HTTP 400 / __type "ResourceNotFoundException" / Message "Secrets Manager can't find the specified secret."; actual HTTP 200 with a secret body.

Emulator source backing the root cause: services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/create_secret.rs lines 386-397 (token used only as an idempotency replay lookup) and 490-500 (`input.client_request_token.clone().unwrap_or_else(crate::state::new_version_id)`); the same `unwrap_or_else(new_version_id)` fallback pattern is what makes the token optional in put_secret_value.rs and update_secret.rs. No handler contains a "must provide a ClientRequestToken" refusal.

## Steps to Reproduce
1. Point a raw HTTP client (not an AWS SDK - SDKs auto-fill @idempotencyToken members) at the emulator's Secrets Manager endpoint, signing requests normally.
2. POST X-Amz-Target: secretsmanager.CreateSecret with body {"Name": "<unique>", "SecretString": "v1"} and NO ClientRequestToken member. AWS answers 400 InvalidRequestException "You must provide a ClientRequestToken value. We recommend a UUID-type value."; the emulator answers 200 and creates the secret (cause 1, step task#1).
3. CreateSecret a second secret WITH an explicit ClientRequestToken of your own (>=32 chars) so a known version id holds AWSCURRENT.
4. POST secretsmanager.PutSecretValue {"SecretId": "<that secret>", "SecretString": "v2"} with NO ClientRequestToken -> expect 400 InvalidRequestException, observe 200 (cause 2, task#4).
5. POST secretsmanager.UpdateSecret {"SecretId": "<that secret>", "SecretString": "v3"} with NO ClientRequestToken -> expect 400 InvalidRequestException, observe 200 (cause 3, task#5).
6. POST secretsmanager.DescribeSecret for that secret: VersionIdsToStages[<your token>] must still be ["AWSCURRENT"]; the emulator reports ["AWSPREVIOUS"] (cause 4, task#6).
7. POST secretsmanager.DescribeSecret for the name used in step 2: AWS answers 400 ResourceNotFoundException "Secrets Manager can't find the specified secret."; the emulator answers 200 (cause 5, task#7).
