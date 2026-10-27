Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.not-authorized-identity-operations-on-foreign-pool
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.not-authorized-identity-operations-on-foreign-pool|run#42|AssertionError|assert message == expected_message, (
Issue-Commit: 9061499fb53b5c6da3e53acf5ca835db82c26990

# cognito-identity: UnlinkDeveloperIdentity resolves the identity pool before the identity, so an unknown identity is reported as an unknown POOL — ResourceNotFoundException names the wrong resource

> _Note: this issue was identified against the code base at commit `9061499fb53b5c6da3e53acf5ca835db82c26990`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UnlinkDeveloperIdentity

## Description
ONE net-new root cause (Cause 1 of 1), grounded in cognito-identity.not-authorized-identity-operations-on-foreign-pool.

=== Cause 1 of 1: UnlinkDeveloperIdentity's not-found precedence is inverted — the pool is resolved first, so the refusal names the pool instead of the identity ===

AWS: UnlinkDeveloperIdentity refuses a request whose IdentityId does not exist with HTTP 404 ResourceNotFoundException, and the message names the IDENTITY, not the pool: `Identity '<IdentityId>' not found.` Fixed text: `Identity '` ... `' not found.`; the only variable part is the IdentityId value copied verbatim from the request. This holds even when the IdentityPoolId in the same request is also unknown: of UnlinkDeveloperIdentity's two resource inputs, the identity is resolved first, so the identity is the resource reported missing. The sibling pool-scoped operations behave the opposite way and the emulator already gets them right: ListIdentities, LookupDeveloperIdentity and MergeDeveloperIdentities on an unknown pool answer ResourceNotFoundException with `IdentityPool '<IdentityPoolId>' not found.` (fixed text `IdentityPool '` ... `' not found.`, variable part the IdentityPoolId from the request) — those three assertions in the same check PASS, which is what isolates the divergence to UnlinkDeveloperIdentity's ordering rather than to the message template or the error class. Smithy (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): UnlinkDeveloperIdentity input {IdentityId required (len 1..55, pattern ^[\w-]+:[0-9a-f-]+$), IdentityPoolId required (len 1..55, same pattern), DeveloperProviderName required (len 1..128), DeveloperUserIdentifier required (len 1..1024)}; modeled errors InvalidParameterException 400, NotAuthorizedException 403, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429, InternalErrorException 500. The model states the error class (ResourceNotFoundException/404) but not which of the two resources is named first; that ordering and both message templates are recorded behavior only (measured by this check against real Cognito Identity, which asserts the pool template for the other three operations and the identity template for UnlinkDeveloperIdentity in the same run).

Emulator: for UnlinkDeveloperIdentity(IdentityId=<unknown identity>, IdentityPoolId=<unknown pool>, DeveloperProviderName=<any>, DeveloperUserIdentifier=<any>) the emulator answers the right code (ResourceNotFoundException) with the WRONG message: `IdentityPool '<IdentityPoolId>' not found.` instead of `Identity '<IdentityId>' not found.` The handler looks the pool up before the identity and returns on the first miss, so the identity-not-found branch below it is unreachable whenever the pool is also unknown.

Why: a client cannot tell which input it got wrong. Code that branches on the ResourceNotFoundException message — or an operator reading it — is told to fix the IdentityPoolId when the identity is what does not exist, and the pool-not-found message makes an otherwise valid pool look missing. Any re-provision/retry logic keyed to the resource named in the message takes the wrong action, and the two message templates become indistinguishable across the four operations, so a test that pins AWS's wording passes for the other three and fails only here (or, worse, is written to the emulator's wording and then breaks in production).

Fix: the not-found precedence in the UnlinkDeveloperIdentity handler's resolution step (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/unlink_developer_identity.rs) — resolve the IdentityId before the IdentityPoolId so an unknown identity is reported as ResourceNotFoundException with AWS's identity template, and keep the pool-not-found refusal for the case where the identity exists (or is not the deciding miss). The existing pool-scoped lookups in ListIdentities / LookupDeveloperIdentity / MergeDeveloperIdentities must not change: their pool-first precedence already matches AWS.

Related open issue that does NOT cover this: 20261027T012826Z-cognito-identity-the-identity-link-lifecycle-diverges-in.md bundles four other UnlinkIdentity/UnlinkDeveloperIdentity defects (the DISABLED sentinel, unvalidated login tokens, a developer-provider-only Logins map, and the InvalidParameterException-vs-NotAuthorizedException classification of an unsupported developer provider). None of them is this not-found precedence, and this check is not marked OWNED by it.

Traces: cognito-identity.not-authorized-identity-operations-on-foreign-pool.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator.

[mismatch] cognito-identity.not-authorized-identity-operations-on-foreign-pool
  File "check_cognito_identity_not_authorized_identity_operations_on_foreign_pool.py", line 156, in run
    assert message == expected_message, (
  AssertionError: UnlinkDeveloperIdentity: expected message "Identity 'us-west-2:41efc475-d45d-44c7-9957-a5aa0d3fc588' not found.", got "IdentityPool 'us-west-2:8d9515e9-e5f9-45de-9505-069b0a3882cb' not found."

(The two GUIDs above are per-run values generated by the check — uuid4 for the nonexistent pool and for the nonexistent identity. The requirement is the TEMPLATE: `Identity '<the IdentityId sent in the request>' not found.` The error code ResourceNotFoundException matched; only the named resource, and therefore the message, diverges.)

What PASSED earlier in the same check, on the same nonexistent pool id, and so is NOT part of this defect:
  - ListIdentities(IdentityPoolId=<unknown pool>, MaxResults=5) -> ResourceNotFoundException, message "IdentityPool '<pool>' not found."
  - LookupDeveloperIdentity(IdentityPoolId=<unknown pool>, DeveloperUserIdentifier="nobody", MaxResults=1) -> ResourceNotFoundException, same pool message.
  - MergeDeveloperIdentities(..., IdentityPoolId=<unknown pool>) -> ResourceNotFoundException, same pool message.
  - All four operations' positive controls on the real pool (ListIdentities finds the minted identity, LookupDeveloperIdentity resolves the developer user, MergeDeveloperIdentities retains a survivor that an independent lookup confirms, UnlinkDeveloperIdentity removes the developer provider from DescribeIdentity's Logins).

Handler code consulted for the Fix line (read-only): services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/unlink_developer_identity.rs:247-259 returns ResourceNotFoundException with format!("IdentityPool '{}' not found.", input.identity_pool_id) on the pool lookup, BEFORE lines 266-275 which hold the correct ResourceNotFoundException with format!("Identity '{}' not found.", input.identity_id) on the identity lookup. The identity branch is therefore dead whenever the pool is unknown too.

Model (smithy_query query=operation, model_path=services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): UnlinkDeveloperIdentity errors include ResourceNotFoundException (client, httpError 404); input requires both IdentityId and IdentityPoolId.

## Steps to Reproduce
1. Build and boot the emulator, then replay cognito-identity.not-authorized-identity-operations-on-foreign-pool.
2. Manually: CreateIdentityPool(IdentityPoolName=<name>, AllowUnauthenticatedIdentities=True, DeveloperProviderName=<devprov>) -> <pool>.
3. GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<devprov>: <devuser>}) -> <identity>.
4. Positive control: UnlinkDeveloperIdentity(IdentityId=<identity>, IdentityPoolId=<pool>, DeveloperProviderName=<devprov>, DeveloperUserIdentifier=<devuser>) succeeds and DescribeIdentity(<identity>) no longer lists <devprov>.
5. Build a pool id that does not exist, <foreign_pool> = "<region>:<uuid4>", and an identity id that does not exist, <fake_identity> = "<region>:<uuid4>".
6. Call UnlinkDeveloperIdentity(IdentityId=<fake_identity>, IdentityPoolId=<foreign_pool>, DeveloperProviderName="some.dev.provider", DeveloperUserIdentifier="nobody").
   Expect: HTTP 404 ResourceNotFoundException, message "Identity '<fake_identity>' not found."
   Observe: HTTP 404 ResourceNotFoundException, message "IdentityPool '<foreign_pool>' not found."
7. Confirm the contrast (these must keep working): ListIdentities(IdentityPoolId=<foreign_pool>, MaxResults=5), LookupDeveloperIdentity(IdentityPoolId=<foreign_pool>, DeveloperUserIdentifier="nobody", MaxResults=1) and MergeDeveloperIdentities(SourceUserIdentifier="source-user", DestinationUserIdentifier="dest-user", DeveloperProviderName="some.dev.provider", IdentityPoolId=<foreign_pool>) each answer ResourceNotFoundException with "IdentityPool '<foreign_pool>' not found."
