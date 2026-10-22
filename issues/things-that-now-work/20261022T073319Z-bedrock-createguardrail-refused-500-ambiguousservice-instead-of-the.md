Issue-Source: trace-sweep-divergence
Issue-Trace: bedrockruntime-apply-guardrail-output-scope
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrockruntime-apply-guardrail-output-scope
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock|CreateGuardrail|2xx->5xx|$.createdAt
Issue-Key: bedrock|CreateGuardrail|2xx->5xx|$.guardrailArn
Issue-Key: bedrock|CreateGuardrail|2xx->5xx|$.guardrailId
Issue-Key: bedrock|CreateGuardrail|2xx->5xx|$.version
Issue-Key: bedrock|CreateGuardrail|2xx->5xx|$headers.content-type
Issue-Key: bedrock|CreateGuardrail|2xx->5xx|status
Issue-Commit: ceabbcc456fe2a783c33070041a4243561c3aa32

# Bedrock CreateGuardrail refused 500 AmbiguousService instead of the modeled 202 guardrail envelope

> _Note: this issue was identified against the code base at commit `ceabbcc456fe2a783c33070041a4243561c3aa32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGuardrail

## Description
Cause 1 of 1 — CreateGuardrail is unreachable, so no guardrail can be created and every fixture that needs one as setup cannot proceed.

AWS: Smithy (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json) — service AmazonBedrockControlPlaneService, sdkId 'Bedrock', endpointPrefix 'bedrock', sigv4Name 'bedrock', protocols ['restJson1']. CreateGuardrail is bound to POST /guardrails with @http code 202 and returns CreateGuardrailResponse whose members guardrailId, guardrailArn, version and createdAt are all @required; version is constrained to the pattern ^DRAFT$ (length exactly 5). The recording agrees: HTTP 202, content-type application/json, body carrying guardrailId, guardrailArn, createdAt and version='DRAFT'. The guardrail id, ARN and timestamp are generated per environment and are examples, not requirements; the 202 status, the content-type and the presence of all four members are the requirement, as is version='DRAFT'.

Emulator: HTTP 500 with content-type 'application/x-amz-json-1.1' and x-amzn-errortype 'AmbiguousService', body "6 emulated services all claim requests for 'bedrock': bedrock, bedrock, bedrock, bedrock, bedrock, bedrock; the request is rejected rather than served by an arbitrary one". None of guardrailId, guardrailArn, version or createdAt is present. The emulator's router resolves a request to an emulated service by its SigV4 signing name, and six emulated Bedrock-family services all declare sigv4Name 'bedrock' (bedrock, bedrock-runtime, bedrock-agent, bedrock-agent-runtime, bedrock-data-automation, bedrock-data-automation-runtime), so resolution is ambiguous and nothing is dispatched. The signing name is not a unique key; the endpoint prefix is ('bedrock' vs 'bedrock-runtime' etc.). A second, independent defect is visible in the same response: this service speaks restJson1, whose responses — errors included — carry content-type 'application/json', but the refusal is serialized as 'application/x-amz-json-1.1'.

Why: a client cannot create a guardrail at all, and the failure is reported as a retryable 500 InternalServerError rather than any modeled client error, so SDK retry logic will loop on a request that can never succeed. Because the guardrail create is the setup step for the ApplyGuardrail output-scope scenario, the downstream guardrail-apply assertions in that fixture also cannot run against a real guardrail.

Fix: the request router / service-resolution layer — dispatch on the endpoint prefix (host) and URI path rather than on the SigV4 signing name; then the CreateGuardrail handler and serializer must emit the 202 status, application/json, and the four required response members with version='DRAFT'.

Traces: bedrockruntime-apply-guardrail-output-scope#setup#1.

RELATED OPEN ISSUES (same resolver fault, other services — none covers the bedrock control plane): 20261022T042609Z-bedrock-data-automation-runtime-sigv4-name-routing-makes-all-bedrock-data-automation.md, 20261022T062300Z-forecastquery-is-entirely-unreachable-router-refuses-every-request.md, 20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md. The companion bedrock-runtime bundles filed in this same sweep share the root cause but carry their own replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1: 65 steps, 2 passed, 55 mismatched, score 3.1%. emulator_up=true (/__admin/health answered 200 and the :41535 data listener accepts), bootstrap_signal=false, state_reset_failures=0.

Divergence cluster (service 'bedrock', operation CreateGuardrail, fixture bedrockruntime-apply-guardrail-output-scope, step setup#1, verdict mismatch, affected_count 1):
  status: expected 202, got 500
  $.createdAt: missing (expected a shaped timestamp, got None)
  $.guardrailArn: missing (expected a shaped ARN, got None)
  $.guardrailId: missing (expected a shaped id, got None)
  $.version: missing (expected 'DRAFT', got None)
  $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'
  x-amzn-errortype: 'AmbiguousService'
  body $.message: "6 emulated services all claim requests for 'bedrock': bedrock, bedrock, bedrock, bedrock, bedrock, bedrock; the request is rejected rather than served by an arbitrary one"

Model evidence: smithy_query operation=CreateGuardrail on the bedrock control-plane model gives http {code: 202, method: POST, uri: /guardrails} and output members guardrailId/guardrailArn/version/createdAt all required, version pattern ^DRAFT$. smithy_query service_info on the same model gives sigv4Name 'bedrock', endpointPrefix 'bedrock', protocols ['restJson1']; the bedrock-runtime model gives the SAME sigv4Name 'bedrock' with endpointPrefix 'bedrock-runtime'.

The cluster's stub_path and owned_by are both empty, so this is not an unimplemented-operation stub and no open issue carries this replay key.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Step: fixture 'bedrockruntime-apply-guardrail-output-scope', step 'setup#1' — POST /guardrails signed for sigv4 service name 'bedrock' with a valid CreateGuardrail body (name, blockedInputMessaging, blockedOutputsMessaging and policy configs).
4. Expected: HTTP 202, content-type application/json, body with guardrailId, guardrailArn, createdAt and version='DRAFT'.
5. Observed: HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype 'AmbiguousService', none of the four required members present.
6. After changing service resolution to key on endpoint prefix / URI path rather than the SigV4 signing name, re-replay bedrockruntime-apply-guardrail-output-scope#setup#1 and confirm the 202 envelope above.
