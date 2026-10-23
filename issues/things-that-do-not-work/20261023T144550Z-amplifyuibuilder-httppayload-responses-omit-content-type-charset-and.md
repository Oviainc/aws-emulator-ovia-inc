Issue-Source: trace-sweep-divergence
Issue-Trace: amplifyuibuilder-component-collection-properties-roundtrip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplifyuibuilder-component-collection-properties-roundtrip amplifyuibuilder-create-component-roundtrip-nested-properties amplifyuibuilder-theme-create-nested-values-roundtrip amplifyuibuilder-update-theme-persists-value-changes
Issue-Shape-Id: com.amazonaws.amplifyuibuilder#AmplifyUIBuilder
Issue-Key: amplifyuibuilder|GetComponent|2xx->2xx|$headers.content-type
Issue-Key: amplifyuibuilder|GetComponent|2xx->2xx|$headers.etag
Issue-Key: amplifyuibuilder|GetTheme|2xx->2xx|$headers.content-type
Issue-Key: amplifyuibuilder|GetTheme|2xx->2xx|$headers.etag
Issue-Key: amplifyuibuilder|UpdateTheme|2xx->2xx|$headers.content-type
Issue-Key: amplifyuibuilder|UpdateTheme|2xx->2xx|$headers.etag
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# amplifyuibuilder httpPayload responses omit Content-Type charset and the ETag header

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComponent

## Description
The amplifyuibuilder emulator's restJson1 response serializer emits the wrong Content-Type and emits no ETag response header on the operations whose output is an @httpPayload structure. Three distinct operation clusters diverge on exactly these two response headers; all three must be closed before this issue is resolved.

Cause 1 of 3: GetComponent (fixture amplifyuibuilder-component-collection-properties-roundtrip, step task#3).
AWS: Smithy: GetComponent GET /app/{appId}/environment/{environmentName}/components/{id}, HTTP 200, output member `component` is @httpPayload targeting the `Component` structure; the model binds no ETag member (no @httpHeader on the output), so the header requirement here is recorded behavior only. The recording shows a 200 response carrying `Content-Type: application/json; charset=utf-8` and an ETag response header (the fixture asserts ETag's presence/shape, not its value). Hypothesis: the charset parameter and the ETag are added by the fronting layer AWS serves amplifyuibuilder through, which is why they are uniform across every operation of the service.
Emulator: replies 200 with `Content-Type: application/json` (no charset parameter) and no ETag header at all.
Why: a client that selects a decoder or validates the media type by exact Content-Type string match (rather than by media-type parsing) decodes the body under the wrong assumed encoding or rejects the response; a client that does conditional GETs or caches on ETag has nothing to store, so If-None-Match revalidation is impossible to exercise against the emulator and any caching path a client builds on AWS behavior silently degrades or breaks.
Fix: the restJson1 response serializer for amplifyuibuilder (the shared response-header construction used by the operation handlers), not the operation's business logic — the status, body and payload shape all match.
Traces: amplifyuibuilder-component-collection-properties-roundtrip#task#3 (cluster representative); also affected: amplifyuibuilder-create-component-roundtrip-nested-properties#task#3.

Cause 2 of 3: GetTheme (fixture amplifyuibuilder-theme-create-nested-values-roundtrip, step task#3).
AWS: Smithy: GetTheme GET /app/{appId}/environment/{environmentName}/themes/{id}, HTTP 200, output member `theme` is @httpPayload targeting `Theme`; no ETag member is modeled, so the header is recorded behavior only. The recording shows 200 with `Content-Type: application/json; charset=utf-8` plus an ETag header.
Emulator: replies 200 with `Content-Type: application/json` and no ETag header.
Why: same client impact as Cause 1 — exact-match media-type checks and ETag-based conditional/caching logic do not behave as they do against AWS.
Fix: same restJson1 response-header serialization path, applied to GetTheme's handler.
Traces: amplifyuibuilder-theme-create-nested-values-roundtrip#task#3 (cluster representative); also affected: amplifyuibuilder-update-theme-persists-value-changes#task#4.

Cause 3 of 3: UpdateTheme (fixture amplifyuibuilder-update-theme-persists-value-changes, step task#3).
AWS: Smithy: UpdateTheme PATCH /app/{appId}/environment/{environmentName}/themes/{id}, HTTP 200, output member `entity` is @httpPayload targeting `Theme`; no ETag member is modeled, so the header is recorded behavior only. The recording shows 200 with `Content-Type: application/json; charset=utf-8` plus an ETag header. Note the write path diverges the same way as the read paths, so a fix applied only to the Get* handlers leaves this cause open.
Emulator: replies 200 with `Content-Type: application/json` and no ETag header.
Why: same client impact as Cause 1, on the mutating path — a client that reads the ETag of an updated theme to drive subsequent optimistic-concurrency or cache-invalidation logic gets nothing back.
Fix: same restJson1 response-header serialization path, applied to UpdateTheme's handler.
Traces: amplifyuibuilder-update-theme-persists-value-changes#task#3 (cluster representative).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 76.2%, 5 of 21 steps mismatched, 0 blocked, 0 errored, 0 state-reset failures, emulator up and serving — bootstrap_signal false), 3 divergence clusters, all on amplifyuibuilder and all with the identical key shape `keys=$headers.content-type,$headers.etag` at status 200->200:

1) divergence_key service=amplifyuibuilder|GetComponent|status=200->200|keys=$headers.content-type,$headers.etag
   fixture amplifyuibuilder-component-collection-properties-roundtrip step task#3 (affected_count 2)
   expected_status 200, actual_status 200
   detail: $headers.content-type: value_mismatch (expected 'application/json; charset=utf-8', got 'application/json')
           $headers.etag: missing (expected '<shaped-not-asserted>', got None)

2) divergence_key service=amplifyuibuilder|GetTheme|status=200->200|keys=$headers.content-type,$headers.etag
   fixture amplifyuibuilder-theme-create-nested-values-roundtrip step task#3 (affected_count 2)
   expected_status 200, actual_status 200
   detail: $headers.content-type: value_mismatch (expected 'application/json; charset=utf-8', got 'application/json')
           $headers.etag: missing (expected '<shaped-not-asserted>', got None)

3) divergence_key service=amplifyuibuilder|UpdateTheme|status=200->200|keys=$headers.content-type,$headers.etag
   fixture amplifyuibuilder-update-theme-persists-value-changes step task#3 (affected_count 1)
   expected_status 200, actual_status 200
   detail: $headers.content-type: value_mismatch (expected 'application/json; charset=utf-8', got 'application/json')
           $headers.etag: missing (expected '<shaped-not-asserted>', got None)

Status codes and response bodies/payload shapes match in all three; the divergence is confined to the two response headers. The ETag assertion is on presence/shape only ('<shaped-not-asserted>'), so the requirement is that an ETag header be emitted, not that it carry any particular recorded value. The Content-Type requirement is the exact media type plus the charset parameter. Smithy model consulted: services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json — GetComponent/GetTheme/UpdateTheme each return a single @httpPayload structure member with a defaulted 200 code and bind no ETag @httpHeader, so the header expectation is grounded in the recording rather than the model. Unrelated open issue 20261022T142124Z-amplifyuibuilder-listtagsforresource-stub.md (ListTagsForResource stub) does not cover any of these three causes.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (mainline) and build/boot the emulator from that checkout.
2. Replay trace set simparity-wire-aws-v1 against it.
3. Cause 1: replay fixture amplifyuibuilder-component-collection-properties-roundtrip and inspect step task#3 (GetComponent on /app/{appId}/environment/{environmentName}/components/{id}). Observe the 200 response carries `Content-Type: application/json` instead of `application/json; charset=utf-8` and carries no ETag header. Same for amplifyuibuilder-create-component-roundtrip-nested-properties#task#3.
4. Cause 2: replay fixture amplifyuibuilder-theme-create-nested-values-roundtrip and inspect step task#3 (GetTheme on .../themes/{id}). Same two header divergences. Same for amplifyuibuilder-update-theme-persists-value-changes#task#4.
5. Cause 3: replay fixture amplifyuibuilder-update-theme-persists-value-changes and inspect step task#3 (UpdateTheme PATCH on .../themes/{id}). Same two header divergences on the write path.
6. After fixing, re-replay all three representative fixture/step bindings (amplifyuibuilder-component-collection-properties-roundtrip#task#3, amplifyuibuilder-theme-create-nested-values-roundtrip#task#3, amplifyuibuilder-update-theme-persists-value-changes#task#3) and confirm each 200 response carries `Content-Type: application/json; charset=utf-8` and a non-empty ETag header.
