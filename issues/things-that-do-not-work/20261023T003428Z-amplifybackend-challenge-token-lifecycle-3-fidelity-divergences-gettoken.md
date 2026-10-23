Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifybackend.create-backend-challenge-token-retrieval amplifybackend.delete-token-unknown-session-rejected amplifybackend.get-token-after-delete-rejected
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend.create-backend-challenge-token-retrieval|run#14|AssertionError|assert fetched["Ttl"] == "", repr(fetched.get("Ttl"))
Issue-Key: amplifybackend|DeleteToken|NotFoundException
Issue-Key: amplifybackend.get-token-after-delete-rejected|run#11|AssertionError|assert error["Code"] == "ResourceNotFoundException", repr(error)
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# AmplifyBackend challenge-token lifecycle: 3 fidelity divergences (GetToken echoes Ttl, DeleteToken rejects unknown session, GetToken-after-delete uses wrong error code)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetToken, DeleteToken

## Description
Three distinct root causes in the AmplifyBackend challenge-token (CreateToken/GetToken/DeleteToken) path, all grounded in failing behavioral checks of this suite. They must be fixed as THREE causes; closing one does not close the others.

NOTE ON EXISTING ISSUES: none of the three causes below is covered by the open issues. 20261022T052247Z-amplifybackend-control-plane-7-fidelity-divergences-no-environment.md covers CreateBackend/CreateBackendAuth/CloneBackend/GetBackendJob/DeleteBackend only and never mentions the token operations. 20261022T142056Z-amplifybackend-accepts-unregistered-appids-resource-not-found-or.md touches CreateToken/GetToken/DeleteToken but ONLY for the case of an appId that is not a registered Amplify app (its Cause 1/Cause 2 app-registration gate) and for the error envelope's missing `code` member (its Cause 3). Every cause below happens on a LIVE, registered app with a correctly-resolving appId, so the app-registration gate does not reach any of them.

=== Cause 1 of 3: GetToken echoes the token's Ttl instead of returning it empty ===
AWS: recorded behavior only. Smithy: GetToken GET /backend/{AppId}/challenge/{SessionId}, output AppId/ChallengeCode/SessionId/Ttl, all bare `__string` with jsonName appId/challengeCode/sessionId/ttl, no constraints and no documented value for Ttl on the read path. The recorded behavior is that CreateToken's response carries a non-empty Ttl, but a subsequent GetToken for the same (AppId, SessionId) does NOT echo it back: the Ttl member comes back as the empty string while AppId, SessionId and ChallengeCode round-trip unchanged. The check documents this explicitly ("Empirical: GetToken's Ttl field is not populated with the original TTL").
Emulator: GetToken populates Ttl from stored token state and returns the minted value, '300'. See services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/get_token.rs:179-184 (`ttl: Some(token.ttl())` in the success path).
Why: a client that reads Ttl off GetToken to compute an expiry deadline gets a plausible-looking lifetime from the emulator and an empty string from AWS, so expiry/refresh arithmetic written against the emulator raises or mis-parses in production; conversely a client that treats a non-empty Ttl as "this read reports a fresh lifetime" infers a liveness guarantee AWS does not give.
Fix: GetToken's output construction in src/ops/get_token.rs — the read path must report Ttl as the empty string rather than the stored lifetime; CreateToken's Ttl stays as-is.
Traces: amplifybackend.create-backend-challenge-token-retrieval (AssertionError: '300' at `assert fetched["Ttl"] == ""`).

=== Cause 2 of 3: DeleteToken rejects an unknown session id instead of succeeding as a no-op ===
AWS: recorded behavior only. Smithy: DeleteToken POST /backend/{AppId}/challenge/{SessionId}/remove, output IsSuccess (__boolean, jsonName isSuccess), errors BadRequestException(400)/GatewayTimeoutException(504)/NotFoundException(404)/TooManyRequestsException(429) — the model does not state which of these applies to an absent session. The recording shows real AmplifyBackend validating only APP identity on this path, never session existence: DeleteToken for a well-formed session id that this live app holds no token for answers HTTP 200 with `IsSuccess: true` (a permissive no-op, i.e. delete is idempotent over session ids). Session absence is not an error on DeleteToken.
Emulator: answers HTTP 404 with error code `NotFoundException`, raising `botocore.errorfactory.NotFoundException: ... when calling the DeleteToken operation` (example message: 'no challenge token with session id 00000000-0000-0000-0000-000000000000 exists for app d0cmslu6k83i0p'). See src/ops/delete_token.rs:157-166, the `if !store.remove_challenge_token(..)` branch that converts "nothing removed" into state::not_found(RESOURCE_TYPE_CHALLENGE_TOKEN, ..); the module doc at delete_token.rs:5-8 states this stricter contract deliberately ("there is nothing to delete, which is a different answer from 'deleted'"), so the doc needs correcting alongside the code.
Why: an overly-strict rejection. Any idempotent cleanup — a retried DeleteToken, a cleanup handler that runs after another path already consumed the session, a teardown that deletes a session id it never confirmed exists — succeeds against AWS and raises against the emulator. Client code therefore cannot be exercised on the emulator at all on this path, and the status class is wrong too (404 where AWS reports 200).
Fix: DeleteToken's handler (src/ops/delete_token.rs) — treat "no such live session for this app" as success and return `IsSuccess: true`, reserving the 404/NotFoundException channel for the cases AWS actually refuses (per the existing unregistered-appId issue, an invalid app is a 400 ValidationException, not this 404).
Traces: amplifybackend.delete-token-unknown-session-rejected (errored at the second DeleteToken call, before the check's app-deletion assertions were ever reached).

=== Cause 3 of 3: GetToken for a consumed/deleted session reports NotFoundException where AWS reports ResourceNotFoundException ===
AWS: recorded behavior only for the code. Smithy binds GetToken to BadRequestException(400)/GatewayTimeoutException(504)/NotFoundException(404)/TooManyRequestsException(429) and declares no `ResourceNotFoundException` shape at all, so the model does not state this rule. The recording shows that after a successful DeleteToken consumes a session, a GetToken for that same (AppId, SessionId) on the live app is refused with error code `ResourceNotFoundException`; the message follows the service's fixed prefix rule, "An error occurred while processing your request: " + the service sentence (example: "An error occurred while processing your request: Login code not found").
Emulator: answers error code `NotFoundException` with a locally invented message — recorded actual: {'Message': 'no challenge token with session id emulatorasjnkriknqkg-0000000000000000 exists for app d1pxuvsog1nb0n', 'Code': 'NotFoundException'} — from the same not_found site at src/ops/get_token.rs:166-178. The positive control (GetToken before the delete) passes, so the delete-visibility semantics are right and only the refusal's classification is wrong. This is the same wrong-code family as Cause 5 of 20261022T052247Z-...-7-fidelity-divergences (GetBackendJob), but on a different operation and code path, and that issue's Cause 5 names only GetBackendJob.
Why: the error code is the only machine-readable discriminator AmplifyBackend gives here; a client that catches ResourceNotFoundException to mean "this login code is spent, start a new pairing" never matches and surfaces an unexpected NotFoundException instead, aborting the flow. The message prefix rule is also not applied (see the already-filed message-prefix cause), but the code itself is the requirement here.
Fix: GetToken's not-found path in src/ops/get_token.rs together with the service's error channel in src/state.rs — a session that is absent because it was never minted, was deleted, or expired must be refused with the `ResourceNotFoundException` code (and the service's prefixed message form), not the generic NotFoundException the backend-resource lookups use.
Traces: amplifybackend.get-token-after-delete-rejected (AssertionError at `assert error["Code"] == "ResourceNotFoundException"`).

## Evidence
Suite simparity-behavioral-aws-v1: amplifybackend 4/15 passed, 11 failed. Of those 11, 6 are OWNED by 20261022T052247Z-amplifybackend-control-plane-7-fidelity-divergences-no-environment.md and 2 are ALREADY FILED against unimplemented stubs (CreateBackendStorage, RemoveAllBackends); the 3 clustered here are the remainder.

Cause 1 — amplifybackend.create-backend-challenge-token-retrieval [mismatch]:
  CreateToken(AppId=<live app>) -> AppId/SessionId/ChallengeCode/Ttl all non-empty; GetToken(AppId, SessionId) round-trips AppId, SessionId and ChallengeCode correctly.
  expected: fetched["Ttl"] == "" (empty string on the read path)
  actual:   AssertionError: '300'  (at check line 33: assert fetched["Ttl"] == "", repr(fetched.get("Ttl")))

Cause 2 — amplifybackend.delete-token-unknown-session-rejected [error]:
  Positive control passes: DeleteToken of the real session returns IsSuccess=True.
  expected: DeleteToken(AppId=<live app>, SessionId="00000000-0000-0000-0000-000000000000") -> HTTP 200, IsSuccess=True
  actual:   botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DeleteToken operation: no challenge token with session id 00000000-0000-0000-0000-000000000000 exists for app d0cmslu6k83i0p  (HTTP 404; the raised exception aborted the check before its later app-identity assertions ran)

Cause 3 — amplifybackend.get-token-after-delete-rejected [mismatch]:
  Positive control passes: GetToken before the delete returns the same SessionId and ChallengeCode; DeleteToken returns IsSuccess=True.
  expected: Code == "ResourceNotFoundException" (message example: "An error occurred while processing your request: Login code not found")
  actual:   {'Message': 'no challenge token with session id emulatorasjnkriknqkg-0000000000000000 exists for app d1pxuvsog1nb0n', 'Code': 'NotFoundException'}  (at check line 31)

Model citations (services/service-com-amazonaws-amplifybackend-amplifybackend/smithy-model/com-amazonaws-amplifybackend-amplifybackend.json): CreateToken POST /backend/{AppId}/challenge, output AppId/ChallengeCode/SessionId/Ttl (all __string); GetToken GET /backend/{AppId}/challenge/{SessionId}, same output shape; DeleteToken POST /backend/{AppId}/challenge/{SessionId}/remove, output IsSuccess (__boolean). All three bind exactly BadRequestException(400), GatewayTimeoutException(504), NotFoundException(404), TooManyRequestsException(429); no ResourceNotFoundException/ValidationException shape is modeled, and no member carries @length/@pattern — hence "recorded behavior only" for the Ttl rule, the DeleteToken no-op rule and the ResourceNotFoundException code.

Emulator code sites: src/ops/get_token.rs:166-184 (not_found branch + `ttl: Some(token.ttl())`); src/ops/delete_token.rs:157-166 (`if !store.remove_challenge_token(..)` -> not_found) and its module doc at lines 5-8.

## Steps to Reproduce
Boot the emulator and replay these amplifybackend behavioral checks (each first creates a real Amplify app via amplify.CreateApp and uses that live appId, so the unregistered-appId gate filed elsewhere is not involved):

1. amplifybackend.create-backend-challenge-token-retrieval (Cause 1) — CreateToken(AppId=<live appId>); record SessionId/ChallengeCode/Ttl. Then GetToken(AppId, SessionId) and inspect Ttl: the emulator returns the minted lifetime ('300') where AWS returns the empty string.

2. amplifybackend.delete-token-unknown-session-rejected (Cause 2) — CreateToken(AppId=<live appId>), then DeleteToken(AppId, SessionId) and confirm IsSuccess=True (passes today). Now call DeleteToken(AppId, SessionId="00000000-0000-0000-0000-000000000000") on the same live app: the emulator raises HTTP 404 NotFoundException where AWS answers HTTP 200 with IsSuccess=True.

3. amplifybackend.get-token-after-delete-rejected (Cause 3) — CreateToken(AppId=<live appId>), GetToken(AppId, SessionId) to confirm it is retrievable, DeleteToken(AppId, SessionId) -> IsSuccess=True, then GetToken(AppId, SessionId) again: the emulator refuses with Code='NotFoundException' where AWS refuses with Code='ResourceNotFoundException'.
