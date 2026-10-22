Issue-Source: trace-sweep-divergence
Issue-Trace: amplifyuibuilder-component-collection-properties-roundtrip
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplifyuibuilder-component-collection-properties-roundtrip amplifyuibuilder-create-component-roundtrip-nested-properties
Issue-Shape-Id: com.amazonaws.amplifyuibuilder#AmplifyUIBuilder
Issue-Key: amplifyuibuilder|GetComponent|2xx->2xx|$.createdAt
Issue-Key: amplifyuibuilder|GetComponent|2xx->2xx|$.modifiedAt
Issue-Key: amplifyuibuilder|GetComponent|2xx->2xx|$.tags
Issue-Key: amplifyuibuilder|GetComponent|2xx->2xx|$headers.content-type
Issue-Key: amplifyuibuilder|GetComponent|2xx->2xx|$headers.etag
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# GetComponent response serializes timestamps as epoch numbers and omits modifiedAt/tags

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComponent

## Description
The GetComponent response payload (the `Component` structure bound with @httpPayload) is serialized incorrectly by the emulator. One root cause: the Component output serializer does not honour the shape's wire traits and drops members the service always returns. Cause 1 of 1: Component timestamp members are emitted in the wrong wire form and the optional-but-always-populated members (modifiedAt, tags) plus the response's content-type parameters/ETag header are not produced (fixture amplifyuibuilder-component-collection-properties-roundtrip, step task#3; same cause also reached via fixture amplifyuibuilder-create-component-roundtrip-nested-properties, step task#3).

AWS: Smithy model services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json — GetComponent is `GET /app/{appId}/environment/{environmentName}/components/{id}`, HTTP 200, output member `component` is @httpPayload of shape `Component`. `Component$createdAt` is `@required` with `@timestampFormat("date-time")`, and `Component$modifiedAt` also carries `@timestampFormat("date-time")`; both must therefore appear on the wire as RFC3339/ISO-8601 date-time STRINGS, not as restJson1's default epoch-seconds number. `Component$tags` is a `Tags` map. The recorded responses show the service returning `modifiedAt` on a component that has just been created/updated, `tags` as an empty map `{}` when no tags were supplied, a `content-type` of `application/json; charset=utf-8`, and an `etag` response header (the etag value itself is environment-specific and is only an example; the model says nothing about it — recorded behavior only).

Emulator: status is correct (200) but the body/headers diverge: `$.createdAt` is a bare number (recorded example `1792638447.645931`) instead of a date-time string; `$.modifiedAt` is absent entirely; `$.tags` is absent instead of `{}`; `$headers.content-type` is `application/json` without the `charset=utf-8` parameter; `$headers.etag` is absent.

Why: a client built against AWS deserializes `createdAt`/`modifiedAt` with a date-time parser, so an epoch number either fails to parse or yields a wrong instant — any caller that reads component creation/modification time breaks, and the @required `createdAt` guarantee is effectively violated. Callers that read `modifiedAt` to detect updates see nothing, and callers that iterate `component.tags` get a null map instead of an empty one, turning a normal empty-tag component into a null-dereference or "field not present" error path. The missing ETag removes the conditional-request/cache handle clients use for component revisions.

Fix: the amplifyuibuilder GetComponent output path — the `Component` payload serializer must honour @timestampFormat("date-time") for createdAt/modifiedAt and emit the map member `tags` as `{}` rather than omitting it; the GetComponent handler/state must carry and return `modifiedAt` for the stored component; and the response writer must set the full `application/json; charset=utf-8` content-type and the `etag` header for the component payload.

Traces: amplifyuibuilder-component-collection-properties-roundtrip#task#3 (representative) and amplifyuibuilder-create-component-roundtrip-nested-properties#task#3 — both GetComponent, expected 200 / actual 200 with body+header key divergence on $.createdAt, $.modifiedAt, $.tags, $headers.content-type, $headers.etag.

## Evidence
Replay cluster service=amplifyuibuilder|GetComponent|status=200->200|keys=$.createdAt,$.modifiedAt,$.tags,$headers.content-type,$headers.etag (verdict: mismatch, affected_count 2).

Expected (recorded AWS response, HTTP 200):
  $.createdAt  -> ISO-8601 date-time string (shape-asserted, @timestampFormat date-time)
  $.modifiedAt -> present, ISO-8601 date-time string
  $.tags       -> {}
  $headers.content-type -> application/json; charset=utf-8
  $headers.etag -> present (value environment-specific, example only)

Actual (emulator, HTTP 200):
  $.createdAt  -> 1792638447.645931   (epoch-seconds number)
  $.modifiedAt -> missing (None)
  $.tags       -> missing (None)
  $headers.content-type -> application/json
  $headers.etag -> missing (None)

Model citation: Smithy — Component$createdAt @required @timestampFormat("date-time"); Component$modifiedAt @timestampFormat("date-time"); Component$tags -> Tags (map); GetComponent output member `component` @httpPayload, http GET /app/{appId}/environment/{environmentName}/components/{id}, code 200.

## Steps to Reproduce
1. Boot the emulator from this checkout (health on /__admin/health, data listener accepting).
2. Replay fixture `amplifyuibuilder-component-collection-properties-roundtrip` and stop at step `task#3` (the GetComponent read-back after the component is created/updated): GET /app/{appId}/environment/{environmentName}/components/{id}.
3. Inspect the 200 response: `createdAt` comes back as a JSON number (epoch seconds with fractional part) rather than an ISO-8601 date-time string; `modifiedAt` and `tags` are absent from the payload; the response carries `content-type: application/json` with no charset parameter and no `etag` header.
4. Same divergence on fixture `amplifyuibuilder-create-component-roundtrip-nested-properties`, step `task#3`.
