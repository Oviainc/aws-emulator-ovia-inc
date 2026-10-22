Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-publish-topic-name-guards
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-publish-topic-name-guards
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|Publish|4xx->2xx|$
Issue-Key: iot-data|Publish|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-data|Publish|4xx->2xx|status
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# Publish accepts a reserved $-prefixed topic with HTTP 200 instead of rejecting it

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Publish has no guard for topics in the reserved `$` namespace, so a request AWS refuses is accepted and reported as a successful publish. This is a fail-open validation gap, distinct from the response-envelope defect filed separately for iot-data: here the HTTP status itself is wrong (400 -> 200) and the error identity is absent entirely.

AWS: InvalidRequestException, HTTP 400 (Smithy: Publish -> http POST /topics/{topic}, code 200; errors include InvalidRequestException @error client @httpError 400 from the service's common errors; input member `topic` is @required and @httpLabel). AWS reserves the `$`-prefixed topic namespace for its own system topics and refuses a client publish to it before the message reaches the broker. The recording shows the refusal as HTTP 400 with the restJson1 error identity in the `x-amzn-errortype` response header and a body carrying the fixed message `Topic can't start with $` plus a `traceId`.

Emulator: answers HTTP 200 with an empty JSON body `{}`. There is no 400, no `x-amzn-errortype` header, and no error body — the publish is treated as having succeeded. The emulator does implement a topic guard for MQTT wildcards (it rejects '#' with a 400 on the adjacent step), so the validation path exists but omits the reserved-prefix rule.

Why: a client that publishes to a reserved topic by mistake — a misconfigured topic template, a stray `$aws/...` path, a device that reuses a system topic — gets a success acknowledgement from the emulator and proceeds as though the message was delivered. Against AWS the same call fails with a 400 the client must handle. Code and tests developed against the emulator therefore ship with an unhandled rejection path, and any logic that distinguishes system topics from application topics is never exercised. Because this is the only cause in the corpus where the emulator's status is wrong rather than its body, it also means the failure is invisible to callers that only check the status code.

Fix: Publish's input validation in the Publish handler (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops) — extend the existing topic-name guard that already rejects MQTT wildcards so that it also refuses a topic whose first segment begins with `$`, raising InvalidRequestException (400) with the restJson1 `x-amzn-errortype` header set and the service's fixed reserved-topic message, rather than falling through to the publish path.

Traces: iotdata-publish-topic-name-guards#task#3, and iotdata-publish-topic-name-guards#task#4 in the same cluster (a second reserved-topic shape, same 400 -> 200 outcome).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up). Divergence cluster service=iot-data | Publish | status=400->200 | keys=$,$headers.x-amzn-errortype,status, 2 affected (iotdata-publish-topic-name-guards#task#3, #task#4):

  status: expected 400, got 200
  $: empty_body (expected {'message': "Topic can't start with $", 'traceId': '88d05d84-3bf2-614d-7f27-a4c51504ab03'}, got {})
  $headers.x-amzn-errortype: missing (expected 'InvalidRequestException:', got None)

Requirements from this: HTTP 400 and the InvalidRequestException error identity on `x-amzn-errortype`, for a publish whose topic starts with `$`. The message text is the service's fixed reserved-topic string; the quoted traceId value is an example from this recording, not a requirement.

Contrast within the same fixture: step task#2 publishes to a topic containing the MQTT wildcard '#' and the emulator DOES answer 400 — so the handler reaches a validation stage but has no reserved-prefix rule.

## Steps to Reproduce
1. git clone the emulator repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Failing step: fixture iotdata-publish-topic-name-guards, step task#3 — POST /topics/$<reserved-topic> with a small payload.
4. Observe HTTP 200 with body {} and no `x-amzn-errortype` response header, where the fixture records HTTP 400 with `x-amzn-errortype: InvalidRequestException:` and a JSON error body.
5. Step task#4 of the same fixture is the second reserved-topic shape and reproduces the same 400 -> 200 outcome; step task#2 (wildcard '#') shows the guard that does fire, for comparison.
