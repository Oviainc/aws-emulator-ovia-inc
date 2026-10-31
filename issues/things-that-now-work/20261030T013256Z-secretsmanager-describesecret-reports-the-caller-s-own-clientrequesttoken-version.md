Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.sim2real-crt-2ad7c382fa73eacce701bffea4c39332[*]
Issue-Commit: 2a04615191c549bdab1e2f00e25668c6ae6e3d20

# DescribeSecret reports the caller's own ClientRequestToken version as AWSPREVIOUS instead of AWSCURRENT

> _Note: this issue was identified against the code base at commit `2a04615191c549bdab1e2f00e25668c6ae6e3d20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One net-new wire cause: the staging-label map DescribeSecret returns places AWSCURRENT on the wrong version after a sequence of version-creating calls that AWS refuses.

Cause 1 of 1 - DescribeSecret.VersionIdsToStages maps the caller's own ClientRequestToken version to AWSPREVIOUS (fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6).

AWS: Smithy: DescribeSecret input SecretId -> SecretIdType (@required, @length 1..2048); output VersionIdsToStages -> SecretVersionsToStagesMapType, a map from each stored version id to that version's staging labels. Smithy: PutSecretValue.ClientRequestToken and UpdateSecret.ClientRequestToken -> ClientRequestTokenType, @idempotencyToken, @length min 32 max 64; both operations model InvalidRequestException (client error, HTTP 400). Because a raw HTTP PutSecretValue / UpdateSecret that omits ClientRequestToken is refused with 400 InvalidRequestException and creates no version, the only version that ever existed is the one the caller staked with its OWN ClientRequestToken, and that version still holds the AWSCURRENT label. Recorded response: HTTP 200 with VersionIdsToStages mapping that caller-chosen version id to the single label list ["AWSCURRENT"]. The map KEY is a value taken from the request (whatever ClientRequestToken the caller sent, e.g. a UUID-shaped token) and is not part of the requirement; the requirement is that the label attached to the caller's only successfully created version is AWSCURRENT, and that no server-minted version id appears in the map at all.

Emulator: HTTP 200, same shape, but VersionIdsToStages maps that version id to ["AWSPREVIOUS"]. The emulator accepted the two token-less version-creating writes in the same fixture, minted server-side version ids for them, moved AWSCURRENT onto the newest minted version and demoted the caller's version to AWSPREVIOUS.

Why: AWSCURRENT is the label GetSecretValue resolves when no VersionId or VersionStage is given, so a client that creates a version with its own idempotency token, has two subsequent writes refused by AWS, and then reads the secret back gets a different value under the emulator than under AWS - and the version id it chose and recorded is reported as stale. Code that pins a version by its own ClientRequestToken and asserts it is current (a common pattern for confirming a write landed) inverts. Hypothesis: this is downstream of the missing ClientRequestToken refusals - the per-version staging bookkeeping is self-consistent for the writes it is actually handed; it is handed writes AWS rejects.

Fix: the request validation of PutSecretValue's and UpdateSecret's handlers (refuse a version-creating call with no ClientRequestToken before any version is staked) and, if any residue remains after that, the AWSCURRENT/AWSPREVIOUS promotion bookkeeping in this service's secret-version state. No change to DescribeSecret's own serializer is expected - it reports the state it is given faithfully.

Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#6.

Related but NOT covering this: open issue 20261029T224105Z-secretsmanager-secrets-manager-accepts-createsecret-putsecretvalue-and-updatesecret-with.md describes the same root cause and even discusses this step in prose, but its recorded replay key pins the map path to ONE run's generated token value (VersionIdsToStages.sim2real-crt-a1b2230e741ec402a10361dec5e8a2b9), which is a per-run value and does not match this run's key, so the replay reports this cluster as unowned and its confirmation replay would not run under that issue. Open issue 20261025T173233Z-secretsmanager-secrets-manager-updatesecret-updatesecretversionstage-awspending-is-not-evicted.md concerns AWSPENDING eviction, a different label, and does not carry this step either.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 155 steps, 135 passed, 16 mismatched, score 87.1%, 9 divergence clusters).

Cluster service=secretsmanager|DescribeSecret|status=200->200|keys=$.VersionIdsToStages.<caller-token-version>[0], fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6, verdict mismatch, affected_count 1, owned_by [] (unowned), stub_path empty:

  expected status 200, actual status 200
  $.VersionIdsToStages.<caller-token-version>[0]: value_mismatch - expected 'AWSCURRENT', got 'AWSPREVIOUS'

The map key in this run was the generated token sim2real-crt-2ad7c382fa73eacce701bffea4c39332; that is a per-run value supplied by the fixture's request and is NOT part of the requirement. The requirement that diverges is the staging label on the caller's own version: AWSCURRENT (AWS) vs AWSPREVIOUS (emulator).

Upstream in the same fixture, and consistent with this: task#4 PutSecretValue and task#5 UpdateSecret, both sent with no ClientRequestToken, were expected to answer HTTP 400 / __type 'InvalidRequestException' / Message 'You must provide a ClientRequestToken value. We recommend a UUID-type value.' and both answered HTTP 200 instead, each minting a version. Those two refusals are already filed; only this staging-label state divergence is unowned.

Model grounding: DescribeSecret output VersionIdsToStages -> SecretVersionsToStagesMapType; PutSecretValue/UpdateSecret ClientRequestToken -> ClientRequestTokenType @idempotencyToken @length 32..64; both ops model InvalidRequestException with client-error kind and HTTP 400.

## Steps to Reproduce
1. Point a raw HTTP client (not an AWS SDK - SDKs auto-fill @idempotencyToken members, which hides this) at the emulator's Secrets Manager endpoint, signing requests normally.
2. POST X-Amz-Target: secretsmanager.CreateSecret with {"Name": "<unique>", "SecretString": "v1", "ClientRequestToken": "<your own token, 32..64 chars>"}. Record the token you sent; it is the version id that must hold AWSCURRENT.
3. POST secretsmanager.PutSecretValue with {"SecretId": "<that secret>", "SecretString": "v2"} and NO ClientRequestToken member. AWS refuses with HTTP 400 InvalidRequestException and creates no version; the emulator answers 200 and mints one.
4. POST secretsmanager.UpdateSecret with {"SecretId": "<that secret>", "SecretString": "v3"} and NO ClientRequestToken member. AWS refuses the same way; the emulator answers 200 and mints another version.
5. POST secretsmanager.DescribeSecret with {"SecretId": "<that secret>"}. Inspect VersionIdsToStages: the entry keyed by the token from step 2 must be ["AWSCURRENT"] and must be the only version present. The emulator reports it as ["AWSPREVIOUS"], with AWSCURRENT sitting on a server-minted version id the caller never chose.
6. Confirm the user-visible consequence: POST secretsmanager.GetSecretValue with only {"SecretId": "<that secret>"} (no VersionId/VersionStage). AWS returns "v1" (the caller's only version, still current); the emulator returns the value written by the call AWS rejected.
7. Replay fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6, to confirm a fix.
