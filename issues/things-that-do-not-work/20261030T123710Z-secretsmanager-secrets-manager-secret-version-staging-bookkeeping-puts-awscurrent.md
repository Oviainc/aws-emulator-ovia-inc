Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-create-put-update-secret-without-client-request-token
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.sim2real-crt-9ccf09c63069a51da84558524f2106c9[*]
Issue-Commit: 2aeffd5e5ca9a10c283c8a29444e9b7abf34c8f4

# Secrets Manager secret-version staging bookkeeping puts AWSCURRENT on a server-minted version, demoting the caller's own version to AWSPREVIOUS (1 cause)

> _Note: this issue was identified against the code base at commit `2aeffd5e5ca9a10c283c8a29444e9b7abf34c8f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One net-new wire cause: the staging-label map returned by DescribeSecret attaches AWSCURRENT to the wrong secret version after a sequence of version-creating writes that AWS refuses, so the only version the caller ever legitimately created is reported as stale.

Cause 1 of 1: DescribeSecret.VersionIdsToStages reports the caller's own ClientRequestToken-staked version with the staging label AWSPREVIOUS instead of AWSCURRENT (fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6).

AWS: Smithy (services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model): DescribeSecret input SecretId -> SecretIdType (@required, @length 1..2048); output VersionIdsToStages -> SecretVersionsToStagesMapType, a map keyed by each stored version id whose value is that version's list of staging labels; errors InvalidParameterException / ResourceNotFoundException (client, HTTP 400) and InternalServiceError (server, 500). PutSecretValue.ClientRequestToken and UpdateSecret.ClientRequestToken both target ClientRequestTokenType with @idempotencyToken and @length min 32 max 64, and both operations model InvalidRequestException (client error, HTTP 400). On a raw HTTP call that omits ClientRequestToken, AWS refuses PutSecretValue and UpdateSecret with HTTP 400, __type InvalidRequestException and the message 'You must provide a ClientRequestToken value. We recommend a UUID-type value.' (that whole sentence is fixed text, with no variable parts) and stakes NO new version. Consequently the only version that exists is the one the caller created with its own ClientRequestToken, and that version still carries AWSCURRENT. Recorded response for this step: HTTP 200 with VersionIdsToStages mapping the caller-supplied version id to exactly ["AWSCURRENT"]. The map KEY is a value taken from the request (whatever ClientRequestToken the caller sent; in this run a generated sim2real-crt-<hex> token) and is NOT part of the requirement -- the requirement is that the version the caller staked holds AWSCURRENT and that no server-minted version id appears in the map.

Emulator: HTTP 200 with the same shape, but VersionIdsToStages maps the caller's version id to ["AWSPREVIOUS"]. Replay detail for the cluster: '$.VersionIdsToStages.<caller-token-version>[0]: value_mismatch (expected \'AWSCURRENT\', got \'AWSPREVIOUS\')', expected status 200, actual status 200. Upstream in the same fixture the emulator accepted both token-less version-creating writes (task#4 PutSecretValue and task#5 UpdateSecret answered HTTP 200 where AWS answers 400 InvalidRequestException), minted server-side version ids for them, promoted AWSCURRENT onto the newest minted version and demoted the caller's version.

Why: AWSCURRENT is the label GetSecretValue resolves when neither VersionId nor VersionStage is supplied. A client that stakes a version with its own idempotency token, has its subsequent token-less writes refused by AWS, and then reads the secret back therefore gets a different value from the emulator than from AWS, and the version id it recorded is reported as superseded. The common pattern of pinning a version by the ClientRequestToken one sent and asserting that version is current inverts, and so does any reconciliation that treats AWSPREVIOUS as 'safe to discard'. Hypothesis: the staging bookkeeping is self-consistent for the writes it is handed; it is handed writes AWS rejects, so the residue is downstream of the missing refusals.

Fix: the request validation in PutSecretValue's and UpdateSecret's handlers (refuse a version-creating call carrying no ClientRequestToken with 400 InvalidRequestException before any version is staked), and, if any residue survives that, the AWSCURRENT/AWSPREVIOUS promotion bookkeeping in this service's secret-version state. DescribeSecret's own serializer needs no change -- it reports the state it is handed.

Traces: secretsmanager-create-put-update-secret-without-client-request-token#task#6.

Related but NOT covering this cause: open issue 20261030T013256Z-secretsmanager-describesecret-reports-the-caller-s-own-clientrequesttoken-version.md describes this same behavior, but its recorded replay key pins the JSON path to ONE run's generated token value (VersionIdsToStages.sim2real-crt-2ad7c382fa73eacce701bffea4c39332), which is a per-run value; this run's key is VersionIdsToStages.sim2real-crt-9ccf09c63069a51da84558524f2106c9, so the replay reports this cluster as UNOWNED (owned_by []) and a confirmation replay driven from that issue does not cover it. Open issue 20261029T224105Z-secretsmanager-secrets-manager-accepts-createsecret-putsecretvalue-and-updatesecret-with.md owns the upstream missing ClientRequestToken refusals (task#1/#2/#4/#5/#7) but not this staging-label state divergence. Open issue 20261025T173233Z-secretsmanager-secrets-manager-updatesecret-updatesecretversionstage-awspending-is-not-evicted.md concerns AWSPENDING eviction, a different label, and does not carry this step.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, 155 steps total, 135 passed, 16 mismatched, 4 blocked, score 87.1%, 9 divergence clusters.

Cluster divergence_key 'service=secretsmanager|DescribeSecret|status=200->200|keys=$.VersionIdsToStages.<caller-token-version>[0]', fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6, operation DescribeSecret, verdict mismatch, affected_count 1, stub_path empty, owned_by [] (unowned):

  expected status 200, actual status 200
  $.VersionIdsToStages.<caller-token-version>[0]: value_mismatch - expected 'AWSCURRENT', got 'AWSPREVIOUS'

The map key in this run was the generated token sim2real-crt-9ccf09c63069a51da84558524f2106c9. That is a per-run value supplied by the fixture's request and is not part of the requirement; the diverging requirement is the staging label on the caller's own version -- AWSCURRENT (AWS) vs AWSPREVIOUS (emulator).

Consistent upstream context in the same fixture (already filed elsewhere, cited only as corroboration): task#4 PutSecretValue and task#5 UpdateSecret, each sent with no ClientRequestToken, were expected to answer HTTP 400 with __type 'InvalidRequestException' and Message 'You must provide a ClientRequestToken value. We recommend a UUID-type value.' and both answered HTTP 200, each minting a version.

Model grounding: DescribeSecret output VersionIdsToStages -> SecretVersionsToStagesMapType (map version id -> staging label list); PutSecretValue/UpdateSecret ClientRequestToken -> ClientRequestTokenType @idempotencyToken @length 32..64; both operations model InvalidRequestException, errorKind client, httpError 400.

## Steps to Reproduce
1. Point a raw HTTP client (not an AWS SDK -- SDKs auto-fill @idempotencyToken members, which hides this) at the emulator's Secrets Manager endpoint, signing requests normally.
2. POST X-Amz-Target: secretsmanager.CreateSecret with {"Name": "<unique>", "SecretString": "v1", "ClientRequestToken": "<your own 32..64 char token>"}. Record that token: it is the version id that must hold AWSCURRENT.
3. POST secretsmanager.PutSecretValue with {"SecretId": "<that secret>", "SecretString": "v2"} and NO ClientRequestToken member. AWS refuses with HTTP 400 InvalidRequestException and stakes no version; the emulator answers 200 and mints one.
4. POST secretsmanager.UpdateSecret with {"SecretId": "<that secret>", "SecretString": "v3"} and NO ClientRequestToken member. AWS refuses the same way; the emulator answers 200 and mints another version.
5. POST secretsmanager.DescribeSecret with {"SecretId": "<that secret>"} and inspect VersionIdsToStages. The entry keyed by the token from step 2 must be ["AWSCURRENT"] and must be the only entry. The emulator reports it as ["AWSPREVIOUS"], with AWSCURRENT on a server-minted version id the caller never chose.
6. Confirm the user-visible consequence: POST secretsmanager.GetSecretValue with only {"SecretId": "<that secret>"} (no VersionId/VersionStage). AWS returns the step-2 value; the emulator returns the value written by a call AWS rejects.
7. Replay fixture secretsmanager-create-put-update-secret-without-client-request-token, step task#6, to confirm a fix.
