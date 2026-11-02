Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__sts.malformed-access-key-rejected-consistently
Issue-Key: dynamodb__iam__sts.malformed-access-key-rejected-consistently|run#9|AssertionError|assert {svc: code for svc, (code, _) in observed.items()} == EXPECTED_CODES, (
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# A never-issued access key is refused AccessDenied by STS, IAM and DynamoDB, instead of each service's own unknown-credential code (InvalidClientTokenId / UnrecognizedClientException)

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCallerIdentity

## Description
One root cause (1 of 1), observed across the three services the check signs as nobody against (dynamodb, iam, sts).

AWS: a syntactically valid access key id that was never issued is refused by the authentication layer with the service's own unknown-credential error code, which is NOT the authorization denial AccessDenied. The awsQuery-protocol services answer InvalidClientTokenId (HTTP 403) and DynamoDB's awsJson protocol answers UnrecognizedClientException (recorded behavior — these codes are service-common auth errors and are not bound to an operation in the Smithy model, and the check records them as measured against the live services: {'sts': 'InvalidClientTokenId', 'iam': 'InvalidClientTokenId', 'dynamodb': 'UnrecognizedClientException'}). The distinction is load-bearing: an unknown key is an authentication failure, AccessDenied is an authorization failure of a key that IS known.
Emulator: all three services answer AccessDenied for the same unissued key — got {'sts': 'AccessDenied', 'iam': 'AccessDenied', 'dynamodb': 'AccessDenied'}.
Why: SDK and tooling credential logic branches on exactly this code. InvalidClientTokenId / UnrecognizedClientException makes a client re-resolve its credential chain (refresh an expired profile, re-read instance metadata, fail fast on a typo in the key id), whereas AccessDenied makes it report a policy problem and stop. With the codes collapsed, a wrong or stale key is diagnosed as a missing permission, and no credential-refresh path in a client is ever exercised. The protocol-family split also means a single hardcoded code cannot be right for all three.
Fix: the shared request-authentication layer's unknown-access-key classification (before authorization), and the per-protocol error serializer that maps that classification to the service's code — InvalidClientTokenId for the awsQuery services (sts, iam) and UnrecognizedClientException for the awsJson DynamoDB endpoint. For sts specifically this is the same classification gap as cause 8 of the open issue 20261024T065715Z-sts-assumerole-getsessiontoken-admit-requests-aws-refuses-no.md (sts.forged-credentials-rejected); the finding here additionally covers iam and dynamodb and the per-service code split, so the fix must be made in the shared layer rather than only in STS.
Traces: dynamodb__iam__sts.malformed-access-key-rejected-consistently.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1. Check dynamodb__iam__sts.malformed-access-key-rejected-consistently, verdict mismatch:

`AssertionError: each service must refuse with the code real AWS uses for it ({'sts': 'InvalidClientTokenId', 'iam': 'InvalidClientTokenId', 'dynamodb': 'UnrecognizedClientException'}); got {'sts': 'AccessDenied', 'iam': 'AccessDenied', 'dynamodb': 'AccessDenied'}`

The check signs one operation per service with a freshly generated, syntactically valid but unissued access key id; the call is refused by all three, so only the classification is wrong, not the refusal itself.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay dynamodb__iam__sts.malformed-access-key-rejected-consistently.
3. Minimal repro: build clients for sts, iam and dynamodb with a syntactically valid access key id that was never issued (and any secret), then call sts.get_caller_identity, iam.list_roles and dynamodb.list_tables.
4. Observed: Error.Code == 'AccessDenied' from all three. Expected: 'InvalidClientTokenId' from sts and iam (HTTP 403) and 'UnrecognizedClientException' from dynamodb.
