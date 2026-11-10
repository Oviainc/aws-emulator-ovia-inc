Issue-Source: trace-sweep-divergence
Issue-Trace: accessanalyzer-resource-scan-and-lookup
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: accessanalyzer-resource-scan-and-lookup
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: access-analyzer|GetAnalyzedResource|2xx->2xx|$.resource.analyzedAt
Issue-Key: access-analyzer|GetAnalyzedResource|2xx->2xx|$.resource.createdAt
Issue-Key: access-analyzer|GetAnalyzedResource|2xx->2xx|$.resource.updatedAt
Issue-Key: access-analyzer|GetAnalyzedResource|4xx->2xx|$
Issue-Key: access-analyzer|GetAnalyzedResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: access-analyzer|GetAnalyzedResource|4xx->2xx|status
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# GetAnalyzedResource response diverges: epoch timestamps and missing ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAnalyzedResource

## Description
Two distinct causes in the emulator's IAM Access Analyzer GetAnalyzedResource response path. Both must be fixed and verified before this issue is closed.

Cause 1 of 2: AnalyzedResource timestamp members are serialized as epoch numbers instead of RFC3339 date-time strings.
- AWS: Smithy (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json) models GetAnalyzedResource output as GetAnalyzedResourceResponse{resource: AnalyzedResource}, and AnalyzedResource.createdAt, AnalyzedResource.analyzedAt and AnalyzedResource.updatedAt are all `Timestamp` members carrying @timestampFormat("date-time") and @required. On the wire (restJson1) each therefore MUST be a JSON string in RFC3339/ISO-8601 date-time form (e.g. "2024-05-01T12:34:56Z" / with fractional seconds), never a JSON number. The instant itself is state-derived and is not a requirement; only the lexical form is.
- Emulator: all three members are emitted as bare JSON numbers (epoch seconds with a fractional part), e.g. $.resource.createdAt, $.resource.analyzedAt and $.resource.updatedAt each came back as a float value rather than a quoted date-time string, on an otherwise 200 OK response.
- Why: an SDK/client that follows the model parses these members with a date-time (string) deserializer. A JSON number where a date-time string is modeled is a hard deserialization failure in strict SDKs, or silently yields an epoch-0 / garbage instant in lenient ones, so every consumer of GetAnalyzedResource gets a broken or wrong analyzedAt/createdAt/updatedAt even though the call "succeeded".
- Fix: the GetAnalyzedResource output serializer for AnalyzedResource (the member-level timestamp formatting in that operation's response serialization), which must honor the @timestampFormat("date-time") override rather than falling back to the restJson1 epoch-seconds default.
- Traces: fixture accessanalyzer-resource-scan-and-lookup, step task#8 (expected shaped date-time values, got numeric epoch values for all three members).

Cause 2 of 2: GetAnalyzedResource for a resource that was never analyzed returns 200 with an empty body instead of the modeled 404 ResourceNotFoundException.
- AWS: Smithy binds GetAnalyzedResource (GET /analyzed-resource, success code 200) to the error ResourceNotFoundException, errorKind client, httpError 404, documented as "The specified resource could not be found." The error structure requires `message`, `resourceId` and `resourceType`. A restJson1 error response carries the HTTP status 404 and the `x-amzn-errortype` header naming the shape (ResourceNotFoundException). The recorded AWS response for this step is HTTP 404 with body member `message` = "Resource not found" — "Resource not found" is the fixed text of this error class for this operation; the variable parts of the error payload (`resourceId`, `resourceType`) are taken from the request's resourceArn / the resource being looked up, and must not be hardcoded from any one run.
- Emulator: returns HTTP 200 with an empty JSON object body ({}), no `resource` member at all, and no `x-amzn-errortype` header (expected 404 + error payload, got 200 + `{}`).
- Why: a client cannot distinguish "this resource has no analysis" from "this resource is analyzed" — it receives a success with a response missing the @required `resource` member, which either fails deserialization or presents as an empty/absent analysis. Code that branches on ResourceNotFoundException (retry, create-analysis-then-poll, negative-cache) never takes that branch, and error-handling paths in the emulated workload go completely unexercised.
- Fix: the GetAnalyzedResource handler's lookup/error path — a miss in the analyzed-resource state must raise the modeled ResourceNotFoundException (404, x-amzn-errortype: ResourceNotFoundException, message/resourceId/resourceType populated) instead of falling through to an empty 200 success envelope.
- Traces: fixture accessanalyzer-resource-scan-and-lookup, step task#10.

Note: the only other open issue naming this service (20261030T072417Z-service-quotas-listawsdefaultservicequotas-omits-most-default-quotas-for-access-analyzer.md) is a service-quotas issue that merely mentions the access-analyzer service code; it does not cover either cause above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, /__admin/health 200):

Cause 1 — fixture accessanalyzer-resource-scan-and-lookup, step task#8, operation GetAnalyzedResource, verdict mismatch, expected status 200 / actual status 200:
  $.resource.createdAt: value_mismatch (expected '<shaped-not-asserted>' date-time string, got 1794323865.757313)
  $.resource.analyzedAt: value_mismatch (expected '<shaped-not-asserted>' date-time string, got 1794323865.757313)
  $.resource.updatedAt: value_mismatch (expected '<shaped-not-asserted>' date-time string, got 1794323865.757313)
  Expected per model: quoted RFC3339 date-time (@timestampFormat("date-time") on AnalyzedResource.createdAt/analyzedAt/updatedAt). Actual: unquoted epoch-seconds float.

Cause 2 — fixture accessanalyzer-resource-scan-and-lookup, step task#10, operation GetAnalyzedResource, verdict mismatch, expected status 404 / actual status 200:
  status: expected 404, got 200
  $: empty_body (expected {'message': 'Resource not found'}, got {})
  $headers.x-amzn-errortype: missing (expected ResourceNotFoundException shape name, got None)

No stub_path and no owned_by on either cluster, so neither is already filed.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `accessanalyzer-resource-scan-and-lookup` from trace set simparity-wire-aws-v1.
2. Cause 1: at step `task#8`, call GetAnalyzedResource with the analyzerArn and resourceArn of a resource the fixture has already analyzed. Inspect the 200 body: $.resource.createdAt / $.resource.analyzedAt / $.resource.updatedAt come back as JSON numbers; the model requires quoted RFC3339 date-time strings.
3. Cause 2: at step `task#10`, call GetAnalyzedResource with a resourceArn that has no analyzed-resource entry. The emulator answers 200 with body `{}` and no x-amzn-errortype header; AWS answers 404 with x-amzn-errortype: ResourceNotFoundException and a body whose `message` is "Resource not found" (plus resourceId/resourceType from the request).
4. Re-replay both steps after the fix; both must pass.
