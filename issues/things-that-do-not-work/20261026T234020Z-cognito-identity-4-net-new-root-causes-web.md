Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.classic-flow-open-id-token-assume-role-with-web-identity cognito-identity.cloudtrail-logs-pool-management-but-not-identity-data-events cognito-identity.e2e-principal-tags-federation-to-sts-session-tags cognito-identity.server-side-token-check-refuses-a-globally-signed-out-user
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.classic-flow-open-id-token-assume-role-with-web-identity|run#21|AssertionError|assert creds["AccessKeyId"].startswith("ASIA"), (
Issue-Key: cognito-identity.cloudtrail-logs-pool-management-but-not-identity-data-events|_assert_no_data_event#30|AssertionError|assert not hits, (
Issue-Key: cognito-identity.e2e-principal-tags-federation-to-sts-session-tags|_wait_for_assume_role_with_web_identity#43|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity.server-side-token-check-refuses-a-globally-signed-out-user|run#30|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# cognito-identity: 4 net-new root causes — web-identity sessions carry a non-ASIA access key id, data-plane identity calls are logged as CloudTrail management events, and two credential scenarios hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity (also sts:AssumeRoleWithWebIdentity, GetId, cloudtrail:LookupEvents)

## Description
Four distinct net-new root causes found in the cognito-identity behavioral replay. None of them is covered by an open issue: the closest neighbours are named per cause below, and they own other checks, not these.

Cause 1 of 4: a session minted from a Cognito OpenID token has an access key id that is not a temporary-credential key id.
AWS: an access key id returned in AssumeRoleWithWebIdentity's Credentials (the classic/basic Cognito flow: GetOpenIdToken -> sts:AssumeRoleWithWebIdentity) is a TEMPORARY (STS) key id, and AWS documents the unique-identifier prefix for those as `ASIA` (IAM User Guide, "IAM identifiers" -> unique ID prefixes: `ASIA` = temporary access key id; `AKIA` = long-term IAM user key). The STS model constrains Credentials.AccessKeyId only by length/pattern (accessKeyIdType, length 16-128, pattern [\w]*), so the prefix rule is documented behavior, not a model constraint. Every key id STS vends for a web-identity session therefore begins with the fixed text `ASIA` followed by run-specific random characters.
Emulator: the vended key id begins with the literal `EMULATOR` — the replay observed `EMULATORASIE5ZSHWXMV` (only the fixed prefix `EMULATOR`/`ASIE...` is the requirement at issue here; the trailing characters are per-run values and are not part of any requirement).
Why: clients and SDK-side tooling that classify a credential by its key-id prefix (deciding whether a credential is temporary and must be refreshed, routing to a session-token-carrying signer, scrubbing/validating credentials in logs, policy conditions written against key-id shape) mis-classify a Cognito-federated session as a long-term IAM user key, and any assertion of the documented prefix fails.
Fix: the sts AssumeRoleWithWebIdentity handler's credential minting (the component that generates Credentials.AccessKeyId for an assumed session) must emit an `ASIA`-prefixed key id of legal length for every temporary session, instead of an emulator-branded literal.
Traces: cognito-identity.classic-flow-open-id-token-assume-role-with-web-identity (AssertionError: STS session key must start with ASIA, got EMULATORASIE5ZSHWXMV).

Cause 2 of 4: the five Cognito Identity data-plane operations are logged as CloudTrail MANAGEMENT events, so they appear in event history.
AWS: recorded/documented behavior ("Amazon Cognito logging in AWS CloudTrail" -> Identity pools events): GetCredentialsForIdentity, GetId, GetOpenIdToken, GetOpenIdTokenForDeveloperIdentity and UnlinkIdentity are logged as DATA events (eventCategory "Data", managementEvent false), the REMAINDER of the identity-pool API is logged as management events, and CloudTrail documents that "You can't view data events in the CloudTrail event history" — event history is a record of management events only. So LookupEvents (EventSource cognito-identity.amazonaws.com) must return the pool's management events (e.g. CreateIdentityPool, SetIdentityPoolRoles) and must return NO record for any of those five operations, whatever pool or identity they named.
Emulator: after the management event for the run's pool was delivered, LookupEvents returned a GetId record naming that run's pool and identity (the replay saw 1 GetId record; the pool id, identity id and event id in the message are per-run values, the divergence is that any record for one of the five data-plane operations is returned at all).
Why: an auditing client cannot distinguish management from data events against this emulator: event-history based audit queries over-report (every GetId/GetCredentialsForIdentity call shows up where AWS shows none), so tests and controls that assert the data/management split — including "no identity data events in event history" compliance checks — pass against AWS and fail here, and conversely a client that only reads event history would wrongly conclude it has data-plane coverage without a data event selector. Related but distinct: the FIXED issue 20261026T054653Z-cloudtrail-cognito-identity-sts-cognito-identity-control-plane-calls-emit-no-cloudtrail.md made control-plane management events appear; this cause is the opposite error on the other half of the split and is NOT a refusal of that operation.
Fix: the cognito-identity CloudTrail event emission (the state/hook that records an event per operation) must classify the five enumerated data-plane operations as data events — eventCategory "Data", managementEvent false — and keep them out of the event-history index LookupEvents reads, while continuing to emit the remainder as management events.
Traces: cognito-identity.cloudtrail-logs-pool-management-but-not-identity-data-events.

Cause 3 of 4: a developer-identity OpenID token is never admitted as a web identity, so the principal-tags federation scenario never gets a role session and hangs to the harness cap.
AWS: GetOpenIdTokenForDeveloperIdentity returns a Token that sts:AssumeRoleWithWebIdentity accepts for a role whose trust policy federates cognito-identity.amazonaws.com, returning Credentials for a session of that role (GetCallerIdentity on it reports an assumed-role ARN whose session name is the RoleSessionName supplied). Both calls answer on the first attempt; the retry loops in the check exist only to absorb IAM eventual consistency.
Emulator: the check never completes — it hits the suite's 60 s per-check cap. Its only unbounded waits are the loops that retry WHILE the emulator refuses: GetCredentialsForIdentity for the unauthenticated identity (60 s budget) and sts:AssumeRoleWithWebIdentity with the developer-identity token (90 s budget). A run in which both calls are answered finishes in seconds, so the emulator refused one of them for the entire window. Hypothesis (neither the model nor the replay states which call): the web-identity exchange is the refusing one — the related OPEN issue 20261026T091605Z-cognito-identity-4-net-new-root-causes-unsigned.md records unsigned sts:AssumeRoleWithWebIdentity answering UnknownService for a different check, which would explain this hang but does not own this check.
Why: the whole documented identity-pool basic (classic) flow is unusable: a client that mints a developer-identity token cannot exchange it for a role session, so principal tags never reach the STS session and no ABAC-gated downstream call can be exercised. A caller sees a hang/refusal loop instead of credentials.
Fix: the sts AssumeRoleWithWebIdentity handler (token validation + trust-policy evaluation for the cognito-identity.amazonaws.com federated principal) and the GetCredentialsForIdentity handler must both answer the documented success for a token this emulator itself minted.
Traces: cognito-identity.e2e-principal-tags-federation-to-sts-session-tags (Check timed out after 60.0s).

Cause 4 of 4: ServerSideTokenCheck=True on an identity pool is not enforced, so the globally-signed-out-user scenario hangs to the harness cap.
AWS: recorded/documented behavior — with CognitoIdentityProviders[].ServerSideTokenCheck=true the identity pool calls the user pool before it issues an OIDC token or AWS credentials, so after AdminUserGlobalSignOut the user's still-unexpired ID token stops vending: GetCredentialsForIdentity is refused with NotAuthorizedException (the CFN/SDK docs promise "a 400 Not Authorized error"), while a twin pool with ServerSideTokenCheck=false keeps vending for the same token. The refusal is visible on the next request, within the check's bounded 120 s propagation budget.
Emulator: the check never completes — it hits the 60 s cap. Its only waits longer than the cap are (a) the 180 s poll for the SSTC=True pool to vend for a signed-IN user and (b) the 120 s loop that retries FOR the refusal after the global sign-out; both only run while the emulator fails to do the documented thing, so the emulator either never vends for the control or (hypothesis) keeps vending for the signed-out token because the server-side token check is inert. The same root cause is recorded for the cross-service checks by the OPEN issue 20261026T054328Z-cognito-identity-cognito-idp-serversidetokencheck-is-not-enforced.md, which does not own this cognito-identity check.
Why: a security control a client configures on the pool silently does nothing: revoking a user (global sign-out, disable, delete) does not stop that user's token from vending AWS credentials, and the only difference a client can observe between SSTC=true and SSTC=false pools disappears.
Fix: the GetCredentialsForIdentity / GetOpenIdToken handlers and the identity-pool state that stores CognitoIdentityProviders[].ServerSideTokenCheck must consult the user pool's session state for a flagged provider and refuse with NotAuthorizedException (HTTP 400) when the user is signed out, leaving SSTC=false pools vending.
Traces: cognito-identity.server-side-token-check-refuses-a-globally-signed-out-user (Check timed out after 60.0s).

## Evidence
Cause 1 — cognito-identity.classic-flow-open-id-token-assume-role-with-web-identity:
  expected: Credentials.AccessKeyId of a web-identity session begins with the documented temporary-credential prefix `ASIA`.
  actual:   AssertionError: STS session key must start with ASIA, got EMULATORASIE5ZSHWXMV (fixed text diverging: the key id is prefixed `EMULATOR`; the trailing characters are per-run).

Cause 2 — cognito-identity.cloudtrail-logs-pool-management-but-not-identity-data-events:
  expected: LookupEvents(EventSource=cognito-identity.amazonaws.com) returns the pool's management events and ZERO records for GetId / GetCredentialsForIdentity / GetOpenIdToken / GetOpenIdTokenForDeveloperIdentity / UnlinkIdentity (data events are not in event history).
  actual:   AssertionError: "immediately after the management event was delivered: ... LookupEvents returned 1 GetId record(s) naming this run's pool <pool-id> / identity <identity-id>" — a data-plane operation is indexed as a management event. (pool id, identity id and event id in that message are per-run values.)

Cause 3 — cognito-identity.e2e-principal-tags-federation-to-sts-session-tags:
  expected: GetOpenIdTokenForDeveloperIdentity -> sts:AssumeRoleWithWebIdentity returns Credentials for the trusted role on the first attempt; the check completes in seconds.
  actual:   Check timed out after 60.0s — the emulator refused inside the 60 s GetCredentialsForIdentity retry loop and/or the 90 s AssumeRoleWithWebIdentity retry loop for the whole window; no credentials were ever returned.

Cause 4 — cognito-identity.server-side-token-check-refuses-a-globally-signed-out-user:
  expected: after AdminUserGlobalSignOut, GetCredentialsForIdentity on the SSTC=true pool is refused with error code NotAuthorizedException (HTTP 400) within the bounded window, while the SSTC=false twin pool keeps vending for the same token.
  actual:   Check timed out after 60.0s — the refusal never arrived inside the 120 s retry-for-refusal loop (and/or the SSTC=true control never vended), so the pool's server-side token check is not observable at all.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite against the emulator.
2. Replay the four checks listed in trace_ids.
3. Cause 1: create an identity pool with an authenticated/unauthenticated role trusting cognito-identity.amazonaws.com, GetId, GetOpenIdToken, then sts:AssumeRoleWithWebIdentity with that token; inspect Credentials.AccessKeyId — it must start with `ASIA`, the emulator returns an `EMULATOR`-prefixed id.
4. Cause 2: create a trail with a data-event selector for the pool, call CreateIdentityPool (management) and GetId (data) for the same pool, then cloudtrail:LookupEvents with EventSource=cognito-identity.amazonaws.com over the window — the management event must be present and NO GetId record may be returned; the emulator returns the GetId record.
5. Cause 3: create a pool with a DeveloperProviderName, GetOpenIdTokenForDeveloperIdentity with PrincipalTags, then sts:AssumeRoleWithWebIdentity with the returned Token for a role federating cognito-identity.amazonaws.com — expect Credentials; the emulator keeps refusing until the harness cap.
6. Cause 4: create two pools over the same user-pool provider differing only in ServerSideTokenCheck (true/false), GetId + GetCredentialsForIdentity with a fresh ID token on both (both vend), call cognito-idp AdminUserGlobalSignOut for the user, then retry GetCredentialsForIdentity on both — the SSTC=true pool must answer NotAuthorizedException while the SSTC=false pool keeps vending; the emulator keeps vending on both.
