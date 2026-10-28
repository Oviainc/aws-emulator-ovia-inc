Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.registry-catalog-display-name-round-trip-and-limit
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|PutRegistryCatalogData|UnsupportedCommandException
Issue-Commit: 57a6b20e9d97ec60071d2a16759239ef9fac6a5c

# ECR Public: PutRegistryCatalogData is unconditionally refused with UnsupportedCommandException, so the registry displayName can never be written or length-validated (1 cause)

> _Note: this issue was identified against the code base at commit `57a6b20e9d97ec60071d2a16759239ef9fac6a5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRegistryCatalogData

## Description
Cause 1 of 1: PutRegistryCatalogData is gated off entirely and answers every request — including a well-formed, in-limit one — with the modelled Region error, so the account-singleton registry catalog cannot be mutated at all.

AWS: PutRegistryCatalogData writes the account's single public-registry catalog record and returns it. Smithy (services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json): PutRegistryCatalogData input member `displayName` -> RegistryDisplayName with @length {min: 0, max: 100}; output member `registryCatalogData` (RegistryCatalogData) is @required; modelled errors are InvalidParameterException (client, HTTP 400), ServerException (server, HTTP 500) and UnsupportedCommandException (client, HTTP 400, documented as "The action isn't supported in this Region"). On a region where the ECR Public API is served, a `displayName` of length <= 100 succeeds: HTTP 200 with `registryCatalogData.displayName` echoing the submitted string verbatim, and GetRegistryCatalogData then reads that same value back. A `displayName` one character past the maximum is refused with HTTP 400, error code `InvalidParameterException` and message `Invalid parameter at 'displayName' failed to satisfy constraint: 'Member must have length less than or equal to 100'` — the fixed text is everything outside the two substituted values, namely the member name taken from the request (`displayName`) and the modelled @length max (`100`); a rejected write must leave the previously stored value untouched. UnsupportedCommandException is reserved for the case where the caller's Region genuinely does not serve the action — not for a request the serving endpoint accepted; note that the SAME emulator endpoint answers GetRegistryCatalogData successfully in this very replay, so by its own account the Region does serve ecr-public.

Emulator: every PutRegistryCatalogData call is refused. The replay's first mutating call, a 100-character in-limit `displayName`, came back as `UnsupportedCommandException: PutRegistryCatalogData command is only supported in us-east-1.` (botocore raised `botocore.errorfactory.UnsupportedCommandException` out of `client.py` `_make_api_call`, i.e. a modelled 400 error body, not a transport failure). The preceding GetRegistryCatalogData on the same client/endpoint returned 200, so the refusal is specific to the Put operation rather than a Region the emulator does not serve. Two further consequences follow from the same gate: the `displayName` round-trip through GetRegistryCatalogData is never exercised, and the @length max 100 boundary is never enforced — the emulator has no reachable path on which it could emit InvalidParameterException for an over-long `displayName`, nor can it demonstrate that a rejected over-limit write leaves state intact.

Why: a client relying on AWS's behavior cannot set the public-gallery author/display name at all. Because UnsupportedCommandException is a client-side 400 that SDK retry policies treat as terminal, callers do not retry; they conclude the API is unavailable in their Region and abandon the call, even though the same endpoint is serving the companion read. Any workflow that publishes or re-brands a public registry (gallery setup, CI that stamps the registry display name, infrastructure tooling that reconciles registry catalog data) fails permanently and non-recoverably. Validation fidelity is lost in the same stroke: a client that depends on the 100-character cap being rejected server-side with InvalidParameterException instead sees a Region error, so it cannot distinguish "my input is too long" from "this action is not available here", and it cannot rely on the documented guarantee that an over-limit write is atomic and leaves the prior displayName in place.

Fix: the PutRegistryCatalogData handler and its Region/support gate in the ecr-public service crate. Remove the unconditional UnsupportedCommandException refusal so the operation is served wherever the emulator serves ecr-public (consistently with GetRegistryCatalogData, which is already served); then implement the operation proper — persist `displayName` into the account-singleton registry-catalog state and echo the stored RegistryCatalogData in the response, add the input validation that rejects `displayName` longer than the modelled @length max with HTTP 400 / `InvalidParameterException` and AWS's constraint message template, and make that rejection occur before any state mutation so the stored value survives a refused write. UnsupportedCommandException should remain reachable only from a genuine unsupported-Region condition.

Traces: ecr-public.registry-catalog-display-name-round-trip-and-limit (verdict: error) — fails on its first PutRegistryCatalogData, which also leaves the GetRegistryCatalogData read-back, the 101-character InvalidParameterException refusal and its message, and the no-mutation-on-refusal assertion all unreached.

## Evidence
Check: ecr-public.registry-catalog-display-name-round-trip-and-limit [error]

Expected (AWS, per the check and the Smithy model):
  1. put_registry_catalog_data(displayName=<100 chars>) -> HTTP 200,
     registryCatalogData.displayName == the submitted 100-character string (verbatim)
  2. get_registry_catalog_data() -> registryCatalogData.displayName == that same string
  3. put_registry_catalog_data(displayName=<101 chars>) -> HTTP 400,
     Error.Code = "InvalidParameterException"
     Error.Message = "Invalid parameter at 'displayName' failed to satisfy constraint: 'Member must have length less than or equal to 100'"
       - fixed text: "Invalid parameter at '" ... "' failed to satisfy constraint: 'Member must have length less than or equal to " ... "'"
       - variable parts: the member name from the request ("displayName") and the modelled @length max ("100")
  4. get_registry_catalog_data() -> still the 100-character value: the refused write mutated nothing

Actual (emulator), at step 1:
  botocore.errorfactory.UnsupportedCommandException: An error occurred
  (UnsupportedCommandException) when calling the PutRegistryCatalogData operation:
  PutRegistryCatalogData command is only supported in us-east-1.

  Raised from client.py:1094 `_make_api_call` -> `raise error_class(parsed_response, operation_name)`,
  i.e. the emulator returned a modelled error body (UnsupportedCommandException, HTTP 400),
  not a connection/transport failure.

Corroborating detail from the same replay: the GetRegistryCatalogData call the check makes
immediately beforehand (to save the pre-existing displayName for restore) returned 200 from the
same client and endpoint. So the emulator serves ecr-public and serves the registry-catalog READ,
yet refuses the WRITE as unsupported-in-this-Region — the two answers are mutually inconsistent.

Smithy model consulted: services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json
  PutRegistryCatalogData: input {displayName -> RegistryDisplayName, @length min 0 max 100};
    output {registryCatalogData -> RegistryCatalogData, required};
    errors: InvalidParameterException (client/400), ServerException (server/500),
            UnsupportedCommandException (client/400)
  UnsupportedCommandException documentation: "The action isn't supported in this Region."

Because the refusal happens on the first mutating call, the emulator's behavior for the
over-limit case is unobservable: no code path exists on which it can emit InvalidParameterException
for displayName, so the @length max 100 enforcement is missing as well as blocked.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator (run_behavioral_suite builds and boots it).
2. Point a botocore/boto3 'ecr-public' client at the emulator endpoint.
3. Read the current value so the round-trip is not vacuous:
     ecr.get_registry_catalog_data()["registryCatalogData"].get("displayName")
   -> observe this SUCCEEDS (HTTP 200), proving the endpoint serves ecr-public registry catalog data.
4. Write a well-formed, in-limit display name (any string of length <= 100; the check uses a
   per-run unique value padded to exactly 100 characters):
     ecr.put_registry_catalog_data(displayName="X" * 100)
   Expected: HTTP 200 with registryCatalogData.displayName == the submitted string.
   Actual:   HTTP 400 UnsupportedCommandException,
             "PutRegistryCatalogData command is only supported in us-east-1."
5. (Unreachable today, and part of the same cause) Once step 4 is served, confirm the rest:
   a. ecr.get_registry_catalog_data() must read back the exact 100-character value.
   b. ecr.put_registry_catalog_data(displayName="X" * 101) must fail with
      Code="InvalidParameterException" and Message=
      "Invalid parameter at 'displayName' failed to satisfy constraint: 'Member must have length less than or equal to 100'".
   c. ecr.get_registry_catalog_data() must still return the 100-character value from (a):
      the refused over-limit write must not mutate account-global state.
6. Replay to confirm: ecr-public.registry-catalog-display-name-round-trip-and-limit
