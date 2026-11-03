Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-version-draft-composition
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-version-draft-composition
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|ListAppVersionResources|4xx->2xx|$.message
Issue-Key: resiliencehub|ListAppVersionResources|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resiliencehub|ListAppVersionResources|4xx->2xx|status
Issue-Key: resiliencehub|ListAppVersionResources|2xx->2xx|$.physicalResources[*]
Issue-Commit: 230350cd65e7d7243fd4bf98191c31977f2dc43e

# resiliencehub ListAppVersionResources reports the raw draft resource map instead of a resolved mapping

> _Note: this issue was identified against the code base at commit `230350cd65e7d7243fd4bf98191c31977f2dc43e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAppVersionResources

## Description
ListAppVersionResources answers from the app version's stored resource map directly, with no notion of whether that version's resources were ever RESOLVED and no notion of whether a mapped resource actually exists. Two causes follow from that, both on the draft version of an application.

Cause 1 of 2 — listing a draft whose resources were never resolved succeeds instead of being refused (fixture resiliencehub-app-version-draft-composition, step task#11).
AWS: ListAppVersionResources is a read over a RESOLUTION, not over the draft's edit buffer: its output member `resolutionId` is @required (Smithy: ListAppVersionResources output resolutionId -> String255, required, @length 1..255), so there is nothing to answer until ResolveAppVersionResources has produced a resolution for that appVersion. Calling it on a draft that has been composed (app components and resources added) but never resolved is refused with HTTP 409 and the modeled ConflictException (Smithy: ListAppVersionResources errors include ConflictException, httpError 409). The recorded response body is `{"message": "Resources were never resolved. Use the ResolveAppVersionResources API to resolve resources first."}` with the error type carried in `x-amzn-errortype`. That message is entirely FIXED text for this error class — it has no variable parts taken from the request or from state.
Emulator: HTTP 200 with a populated `physicalResources` list and no `message`/`x-amzn-errortype` at all. In services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/list_app_version_resources.rs the helper `resolution_of` falls back, when the version has no current resolution, to synthesizing an id via `crate::state::mapping_resolution_id(..)`, so the @required member is always fillable and the never-resolved state is never detectable.
Why: a client that uses the 409 as its signal to call ResolveAppVersionResources never gets it, so it reads an unresolved draft as an authoritative resource mapping and skips resolution entirely; against AWS the same sequence fails and the client's resolve-then-list loop is never exercised. The synthesized resolutionId is also not an id any ResolveAppVersionResources call ever returned, so passing it back in a later request addresses a resolution AWS does not have.
Fix: the ListAppVersionResources handler — gate the read on an existing resolution for (appArn, appVersion) and return the modeled ConflictException with the fixed message above instead of synthesizing a resolution identity when none exists. The synthesizing fallback is the thing to remove, not the message of some other error.

Cause 2 of 2 — the listing includes a resource that no resolution ever mapped (fixture resiliencehub-app-version-draft-composition, step task#14).
AWS: after ResolveAppVersionResources, the listing reports the resources the resolution MAPPED, so a draft entry whose underlying resource does not exist is not in it. In the recording the draft held one mappable resource and the response's `physicalResources` has exactly one element; the second draft entry (a logicalResourceId naming a nonexistent AWS::S3::Bucket) is absent, because AWS refuses to admit it at CreateAppVersionResource in the first place (Smithy: CreateAppVersionResource errors include ValidationException, httpError 400; recorded message 'Invalid resource mapping. The given resource does not exist. Resource Type: <resourceType>, Resource Id: <physicalResourceId>' — fixed text with the resource type and resource id taken from the request).
Emulator: HTTP 200 whose `physicalResources` carries a SECOND element, the unmappable draft entry, reported as a fully mapped `PhysicalResource` with `physicalResourceId.type` 'Native', an `appComponents` membership and `sourceType` 'AppTemplate' — i.e. presented as if a resolution had successfully mapped it.
Why: a client enumerating an app version's resources to drive assessment or drift checks is handed a resource that does not exist, so its counts, component memberships and any per-resource call it makes next are wrong in a way AWS cannot produce.
Fix: the ListAppVersionResources handler and the draft resource state it reads — a listing must report only entries the addressed resolution mapped. Note the related open issue 20261029T195027Z-resiliencehub-accepts-invalid-app-version-and-policy-input.md, which owns the CreateAppVersionResource side (it accepts the nonexistent resource with 200 at step task#10) and does NOT cover this listing divergence; both this listing filter and that admission check are needed.

Note: 20261030T105641Z-resiliencehub-listappversionresources-stub.md ('unimplemented scaffold stub') does not cover either cause — the operation is now implemented and serving 200s, and these are fidelity defects in that implementation.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (score 58.6%, 29 mismatched steps).

Cause 1 — fixture resiliencehub-app-version-draft-composition, step task#11, operation ListAppVersionResources:
  expected status 409, actual 200
  $.message: missing (expected 'Resources were never resolved. Use the ResolveAppVersionResources API to resolve resources first.', got None)
  $headers.x-amzn-errortype: missing (expected an error type header, got None)

Cause 2 — fixture resiliencehub-app-version-draft-composition, step task#14, operation ListAppVersionResources:
  expected status 200, actual 200
  $.physicalResources[1]: extra (expected None, got a PhysicalResource for the draft entry whose underlying AWS::S3::Bucket does not exist, with physicalResourceId.type 'Native', sourceType 'AppTemplate' and an appComponents membership)

Model cited: smithy_query query="operation" operation="ListAppVersionResources,CreateAppVersionResource,ResolveAppVersionResources" on services/service-com-amazonaws-resiliencehub-awsresiliencehub/smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json — output `resolutionId` is @required; errors include ConflictException (409) and ValidationException (400).
Code cited: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/list_app_version_resources.rs, `handle` (reads `held.resources` straight into `physicalResources`) and `resolution_of` (falls back to `crate::state::mapping_resolution_id`).

## Steps to Reproduce
1. Boot the resiliencehub emulator from this checkout.
2. Cause 1: CreateApp, then add app components and resources to the DRAFT version, and WITHOUT calling ResolveAppVersionResources call ListAppVersionResources with appVersion='draft'. Observe HTTP 200 with a populated physicalResources list; AWS answers HTTP 409 ConflictException with message 'Resources were never resolved. Use the ResolveAppVersionResources API to resolve resources first.'. Replay fixture resiliencehub-app-version-draft-composition step task#11.
3. Cause 2: on the same draft, call CreateAppVersionResource for a resource whose underlying AWS resource does not exist, then ResolveAppVersionResources, then ListAppVersionResources. Observe the nonexistent resource present in physicalResources as a fully mapped PhysicalResource; AWS's listing contains only the mappable resource. Replay fixture resiliencehub-app-version-draft-composition step task#14.
