Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.workload-access-token-for-user-id-ttl-scoped
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.workload-access-token-for-user-id-ttl-scoped|run#10|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: 8d2bc7e462ed0ca11a7e6899a279b7ecb298af8b

# GetResourceApiKey is unserved (501 NotImplemented), so a minted workload identity token can never be exercised and the provider-not-found refusal is unreachable

> _Note: this issue was identified against the code base at commit `8d2bc7e462ed0ca11a7e6899a279b7ecb298af8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceApiKey

## Description
One net-new cause in the bedrock-agentcore (data-plane) AgentCore Identity surface. Note this cause is NOT covered by the open issue 20261030T013750Z-bedrock-agentcore-workload-access-token-minting-is-unserved-getworkloadaccesstokenforjwt-and.md: that issue's Issue-Trace-Ids/Issue-Keys name GetWorkloadAccessTokenForJWT and GetWorkloadAccessToken only, and no open issue names GetResourceApiKey at all (list of open bedrock-agentcore/-control issues checked). The suite did not mark this check ALREADY FILED/OWNED because the check captures the stub's ClientError through its own error helper and reports it as a code mismatch rather than letting the exception escape. GetWorkloadAccessTokenForUserId itself IS served here — the check got past minting two distinct tokens and failed only on the downstream first-party call.

Cause 1 of 1: GetResourceApiKey answers the scaffold's 501 NotImplemented for every request, so neither its success path nor its ResourceNotFoundException refusal class exists, and a freshly minted workloadIdentityToken cannot be validated against any first-party AgentCore Identity call.
AWS: Smithy: GetResourceApiKey, POST /identities/api-key, success @http code 200 (codeDefaulted); required input `workloadIdentityToken` (WorkloadIdentityTokenType, @length 1..131072, @sensitive) and `resourceCredentialProviderName` (CredentialProviderName, @length 1..128, @pattern ^[a-zA-Z0-9\-_]+$); required output `apiKey` (ApiKeyType, @length 1..65536, @sensitive); @readonly. Modeled errors: AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), UnauthorizedException(401), ValidationException(400). ResourceNotFoundException is documented as "The exception that occurs when the specified resource does not exist. This can happen when using an invalid identifier or when trying to access a resource that has been deleted." So a request carrying a VALID, currently-live workload identity token but naming an api-key credential provider that does not exist must be refused at HTTP 404 with error code `ResourceNotFoundException` and — recorded behavior for this error class — the message `Resource Credential Provider not found`, which is entirely fixed text (it carries no request- or state-derived values; the provider name is NOT interpolated). The token must be accepted first: the 404 proves the token validated, which is exactly how a caller confirms a newly minted token is immediately usable. A request whose token is invalid/expired would instead be refused with UnauthorizedException(401), and a well-formed request naming an existing provider returns 200 with the required `apiKey`.
Emulator: the call returns the scaffold's HTTP 501 with error code `NotImplemented` and the scaffold message `operation GetResourceApiKey is scaffolded but not implemented in this emulator`. services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/get_resource_api_key.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its `handle()` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("GetResourceApiKey"))`. Replay observed: `AssertionError: expected a freshly minted token to be accepted, got NotImplemented`.
Why: a client that performs the AgentCore Identity flow (mint a workload access token, then exchange it for a resource API key) cannot complete it, and because the reply is an unmodeled error code at HTTP 501 it cannot distinguish "this credential provider does not exist" (404) from "my token is not accepted" (401) from a transient server fault (500) — SDK retry/error classification sees none of the modeled classes. The guarantee that a token minted by GetWorkloadAccessTokenForUserId is independently and immediately usable against a first-party Identity call is unobservable, so a token-lifecycle/TTL test run against the emulator reports the wrong outcome; likewise any code path keyed on the provider-not-found 404 (create-the-provider-then-retry) never triggers.
Fix: implement the operation's handler, its token validation and its credential-provider lookup in services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/get_resource_api_key.rs (plus the shared workload-identity token state it must validate against): validate `workloadIdentityToken` first and refuse an invalid/expired one with UnauthorizedException(401); then resolve `resourceCredentialProviderName` against api-key-credential-provider state and refuse a missing one with ResourceNotFoundException(404) carrying the fixed message `Resource Credential Provider not found`; otherwise return 200 with the required `apiKey`. Delete the SCAFFOLDED marker line once implemented.
Traces: bedrock-agentcore.workload-access-token-for-user-id-ttl-scoped.

## Evidence
Cause 1 — check bedrock-agentcore.workload-access-token-for-user-id-ttl-scoped (verdict: mismatch).

Sequence: CreateWorkloadIdentity(name=<unique 'simreal-wat-uid-'+hex, <=60 chars>) via bedrock-agentcore-control succeeds; GetWorkloadAccessTokenForUserId(workloadName=<that name>, userId='sim2real-user-1') is called twice and returns two DISTINCT non-empty `workloadAccessToken` values (this assertion passed — minting is served). Each of those two tokens is then presented to GetResourceApiKey(workloadIdentityToken=<token>, resourceCredentialProviderName='sim2real-nonexistent-provider').

Expected (AWS): HTTP 404, error code `ResourceNotFoundException`, message exactly `Resource Credential Provider not found` (fixed text, no variable parts) — proving the freshly minted token was accepted and only the named provider was missing.

Actual (emulator): HTTP 501, botocore error code `NotImplemented`, message `operation GetResourceApiKey is scaffolded but not implemented in this emulator`.

Replay line: `check_bedrock_agentcore_workload_access_token_for_user_id_ttl_scoped.py", line 48, in run / assert code == "ResourceNotFoundException" ... AssertionError: expected a freshly minted token to be accepted, got NotImplemented`.

Code evidence: services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/get_resource_api_key.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:` marker) and line 177 (`Err(crate::scaffolded::Scaffolded::not_implemented("GetResourceApiKey"))`).

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite (simparity-behavioral-aws-v1).
2. Run bedrock-agentcore.workload-access-token-for-user-id-ttl-scoped.
3. Via bedrock-agentcore-control, CreateWorkloadIdentity(name=<unique name matching ^[A-Za-z0-9_.-]+$, 3..255>).
4. Via bedrock-agentcore, call GetWorkloadAccessTokenForUserId(workloadName=<that name>, userId='sim2real-user-1') twice; observe two distinct non-empty workloadAccessToken values (this part works).
5. For each token, call GetResourceApiKey(workloadIdentityToken=<token>, resourceCredentialProviderName='sim2real-nonexistent-provider'). Observe HTTP 501 with error code NotImplemented where AWS answers HTTP 404 ResourceNotFoundException with message `Resource Credential Provider not found`.
