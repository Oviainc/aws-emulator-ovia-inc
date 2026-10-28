Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.registry-catalog-display-name-round-trip-and-limit
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|PutRegistryCatalogData|UnsupportedCommandException
Issue-Commit: 7c677bf184452d3016cbf464b9a60cef63f91cb5

# ECR Public: the re-added PutRegistryCatalogData Region gate again refuses the primary client with UnsupportedCommandException, so the registry displayName cannot be written or length-validated

> _Note: this issue was identified against the code base at commit `7c677bf184452d3016cbf464b9a60cef63f91cb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRegistryCatalogData

## Description
REOPENS 20261027T223608Z-ecr-public-putregistrycatalogdata-is-unconditionally-refused-with-unsupportedcommandexception.md: its fix left the call refused.

Cause 1 of 1: PutRegistryCatalogData refuses the suite's primary ecr-public client with the modelled Region error UnsupportedCommandException, so a well-formed, in-limit registry-catalog write is never served and the operation's @length validation is unreachable. The companion read GetRegistryCatalogData is served 200 on that same client and endpoint in the same replay, so the emulator does serve ecr-public for this caller; only the write is gated off.

AWS: PutRegistryCatalogData writes the account's single public-registry catalog record and returns it. Smithy (services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json): input member `displayName` -> RegistryDisplayName with @length {min 0, max 100}; output member `registryCatalogData` -> RegistryCatalogData is @required; modelled errors are InvalidParameterException (client, httpError 400), ServerException (server, 500) and UnsupportedCommandException (client, httpError 400, documented "The action isn't supported in this Region."). From the Region that serves the action, a `displayName` of length <= 100 is answered HTTP 200 with `registryCatalogData.displayName` echoing the submitted string verbatim, and GetRegistryCatalogData then reads that same value back. A `displayName` one character past the maximum is refused HTTP 400 with Code `InvalidParameterException` and Message `Invalid parameter at 'displayName' failed to satisfy constraint: 'Member must have length less than or equal to 100'` — the fixed text is everything outside the two substituted values, namely the member name taken from the request (`displayName`) and the modelled @length max (`100`); the refusal is pre-mutation, so the previously stored value survives it. UnsupportedCommandException is reserved for a caller whose Region genuinely does not serve the action; it must not be the answer to a request the serving endpoint accepts (recorded behavior: the same endpoint answers GetRegistryCatalogData 200 for this very caller).

Emulator: every PutRegistryCatalogData issued by the check's primary client is refused with a modelled 400 error body — `UnsupportedCommandException: PutRegistryCatalogData command is only supported in us-east-1.` (botocore raised botocore.errorfactory.UnsupportedCommandException from client.py `_make_api_call`, i.e. a parsed modelled error, not a transport failure). The refusal lands on the check's FIRST mutating call, a per-run unique `displayName` padded to exactly 100 characters — in-limit and well formed. The immediately preceding GetRegistryCatalogData on the same client returned 200. Consequently the 100-character round-trip through GetRegistryCatalogData, the 101-character InvalidParameterException refusal and its message, and the "a refused over-limit write mutates nothing" guarantee are all unobservable: there is no reachable path on which the emulator could emit InvalidParameterException for `displayName`.

This is the same gate that oscillated across 20261022T192707Z (gate missing), 20261025T010839Z (gate refuses the primary client), 20261025T192924Z (gate removed again), 20261027T223608Z (unconditional refusal) and the still-open 20261028T084228Z-ecr-public-putregistrycatalogdata-is-served-200-for-a.md (no refusal for a us-west-2-signed caller). The fix for 20261027T223608Z re-introduced a refusal rather than a Region-conditional one that admits the caller the emulator is actually serving, so the primary client is refused again. Neither open issue covers this check: 20261028T084228Z is grounded in ecr-public.put-registry-catalog-data-unsupported-command-rejected, the opposite direction (an accept that should be a refusal).

Why: a client relying on AWS's behavior cannot set the public-gallery registry display name at all. UnsupportedCommandException is a terminal client-side 400, so SDK retry policies do not retry it; callers conclude the action is unavailable for them and abandon it, even while the companion read succeeds against the same endpoint. Gallery setup, CI that stamps the registry display name, and any tooling that reconciles registry catalog data fail permanently. Validation fidelity is lost in the same stroke: a client that depends on the 100-character cap being enforced server-side with InvalidParameterException instead sees a Region error, so it cannot distinguish "my input is too long" from "this action is not available here", and cannot rely on the atomicity guarantee that a rejected over-limit write leaves the prior displayName in place.

Fix: the PutRegistryCatalogData handler and its Region/support gate in the ecr-public service crate (services/service-com-amazonaws-ecrpublic-spencerfrontendservice). The gate must resolve the caller's Region from the request's SigV4 credential scope and refuse ONLY when that Region is not the one that serves the action, leaving the Region the emulator serves this caller on (the one GetRegistryCatalogData answers 200 for, and the one the suite's default client signs with) served. A served request must persist `displayName` into the account-singleton registry-catalog state and echo the stored RegistryCatalogData; the handler's validation must reject a `displayName` longer than the modelled @length max with HTTP 400 / InvalidParameterException and AWS's constraint-message template, before any state mutation. Fixing this must not re-break the wrong-Region refusal tracked by 20261028T084228Z: both the served Region and the refused Region must hold at once.

Traces: ecr-public.registry-catalog-display-name-round-trip-and-limit (verdict: error) — fails on its first PutRegistryCatalogData.

## Evidence
Check: ecr-public.registry-catalog-display-name-round-trip-and-limit [error] (suite simparity-behavioral-ga-prod-v1; ecr-public 68/74, so the service is up and answering — this is not a bootstrap gap).

Sequence the check runs with a single client, ctx.client("ecr-public"):
  1. get_registry_catalog_data() -> succeeds (used to save the pre-existing displayName for restore).
  2. put_registry_catalog_data(displayName=<per-run unique value padded to exactly 100 chars>).

Expected (AWS, per the check and the Smithy model):
  1. put_registry_catalog_data(displayName=<100 chars>) -> HTTP 200, registryCatalogData.displayName == the submitted 100-character string, verbatim.
  2. get_registry_catalog_data() -> registryCatalogData.displayName == that same string.
  3. put_registry_catalog_data(displayName=<101 chars>) -> HTTP 400,
       Error.Code    = "InvalidParameterException"
       Error.Message = "Invalid parameter at 'displayName' failed to satisfy constraint: 'Member must have length less than or equal to 100'"
         - fixed text: "Invalid parameter at '" ... "' failed to satisfy constraint: 'Member must have length less than or equal to " ... "'"
         - variable parts: the member name from the request ("displayName") and the modelled @length max ("100").
  4. get_registry_catalog_data() -> still the 100-character value: the refused write mutated nothing.

Actual (emulator), at step 2 of the sequence / step 1 of the expectations:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.UnsupportedCommandException: An error occurred (UnsupportedCommandException) when calling the PutRegistryCatalogData operation: PutRegistryCatalogData command is only supported in us-east-1.

That is a parsed modelled error body (UnsupportedCommandException, HTTP 400), not a connection or transport failure. In the diverging message, "<operation> command is only supported in us-east-1." is the fixed text with the operation name taken from the request; there are no per-run values in it. The divergence is not the message's wording but that this error class is produced at all for a caller the endpoint is serving.

Corroborating detail from the same replay: the GetRegistryCatalogData call made immediately beforehand on the same client and endpoint returned 200. The emulator therefore serves ecr-public and serves the registry-catalog READ for this caller, yet refuses the WRITE as unsupported-in-this-Region — the two answers are mutually inconsistent.

Because the refusal happens on the first mutating call, the over-limit behavior is unobservable: no code path exists on which the emulator can emit InvalidParameterException for displayName, so the @length max 100 enforcement is blocked as well as unverified.

Smithy model consulted: services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json
  PutRegistryCatalogData: input {displayName -> RegistryDisplayName, @length min 0 max 100}; output {registryCatalogData -> RegistryCatalogData, required}; errors InvalidParameterException (client/400), ServerException (server/500), UnsupportedCommandException (client/400).
  UnsupportedCommandException doc: "The action isn't supported in this Region."
  RegistryCatalogData.displayName -> RegistryDisplayName (@length 0..100).

## Steps to Reproduce
1. git_clone() the repo, then run_behavioral_suite() to build and boot the emulator.
2. Build a single boto3/botocore 'ecr-public' client against the emulator endpoint using the suite's default signing Region (the same client the check uses: ctx.client("ecr-public") with no region override).
3. Read the current value, so the round-trip is not vacuous:
     ecr.get_registry_catalog_data()["registryCatalogData"].get("displayName")
   Observe this SUCCEEDS (HTTP 200) — the endpoint serves ecr-public registry catalog data for this caller.
4. With that SAME client, write a well-formed, in-limit display name (any string of length <= 100; the check uses a per-run unique value padded to exactly 100 characters):
     ecr.put_registry_catalog_data(displayName="X" * 100)
   Expected: HTTP 200 with registryCatalogData.displayName == the submitted string.
   Actual:   HTTP 400 UnsupportedCommandException, "PutRegistryCatalogData command is only supported in us-east-1."
5. Once step 4 is served again, confirm the rest of the same cause:
   a. ecr.get_registry_catalog_data() must read back the exact 100-character value.
   b. ecr.put_registry_catalog_data(displayName="X" * 101) must fail with Code="InvalidParameterException" and Message="Invalid parameter at 'displayName' failed to satisfy constraint: 'Member must have length less than or equal to 100'".
   c. ecr.get_registry_catalog_data() must still return the value from (a): the refused over-limit write must not mutate account-global state.
6. Regression guard for the oscillation: a client whose SigV4 credential scope names a Region that does not serve the action must still be refused HTTP 400 UnsupportedCommandException with "PutRegistryCatalogData command is only supported in us-east-1." (open issue 20261028T084228Z), and GetRegistryCatalogData must remain served in every Region.
7. Replay to confirm: ecr-public.registry-catalog-display-name-round-trip-and-limit
