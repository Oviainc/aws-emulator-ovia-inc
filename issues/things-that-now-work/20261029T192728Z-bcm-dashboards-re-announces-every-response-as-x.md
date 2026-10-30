Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-dashboards-dashboard-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-dashboards-dashboard-error-paths bcm-dashboards-dashboard-tagging bcm-dashboards-list-dashboards-max-results-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|CreateDashboard|4xx->4xx|$headers.content-type
Issue-Key: bcm-dashboards|ListTagsForResource|4xx->4xx|$headers.content-type
Issue-Key: bcm-dashboards|ListTagsForResource|2xx->2xx|$headers.content-type
Issue-Key: bcm-dashboards|TagResource|2xx->2xx|$headers.content-type
Issue-Key: bcm-dashboards|UntagResource|2xx->2xx|$headers.content-type
Issue-Key: bcm-dashboards|ListDashboards|4xx->4xx|$headers.content-type
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# bcm-dashboards re-announces every response as x-amz-json-1.1 instead of the model's x-amz-json-1.0

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDashboard

## Description
Every bcm-dashboards response — success and refusal alike — announces `content-type: application/x-amz-json-1.1`, while the service's protocol is awsJson1_0 and the recorded wire announces `application/x-amz-json-1.0`. The service deliberately rewrites the protocol-implied media type on the way out: the per-service pair in the bcm-dashboards crate maps AWS_JSON_1_0 -> AWS_JSON_1_1 and the framework seam stamps the rewritten value on every response it renders. That single rewrite is the one defect; the six causes below are the six distinct response seams the corpus catches it on, and each must stop re-announcing before the issue is closed. NOTE: the resolved issue `20261023T081740Z-bcm-dashboards-responses-carry-content-type-application-x` is what introduced this rewrite (in the opposite direction, from an older recording); it is fixed and does NOT cover these failures.

Cause 1 of 6: CreateDashboard refusal path.
AWS: the modelled protocol is awsJson1_0 (Smithy: service AWSBCMDashboardsService, protocols [awsJson1_0], sdkId "BCM Dashboards"), so every response, including a modelled client error, announces `content-type: application/x-amz-json-1.0`. The recorded response for this step is HTTP 400 under that media type; the status, error code and error message of the refusal itself are unchanged and correct.
Emulator: same HTTP 400 and same body, but the header reads `content-type: application/x-amz-json-1.1`.
Why: a client or proxy that picks its JSON-protocol decoder from the response media type selects the awsJson1.1 decoder for an awsJson1_0 service, and a strict protocol conformance check on the response envelope fails even though the modelled payload is right.
Fix: the service's response media-type seam (the per-service ResponseMediaType pair and the operation-seam wrapper that applies it) must leave the protocol-implied `application/x-amz-json-1.0` in place rather than re-announcing it.
Traces: bcm-dashboards-dashboard-error-paths#task#3.

Cause 2 of 6: ListTagsForResource refusal path.
AWS: `application/x-amz-json-1.0` on the modelled 400 refusal (Smithy: ListTagsForResource errors ValidationException 400, ResourceNotFoundException 404, awsJson1_0 protocol).
Emulator: HTTP 400 with `content-type: application/x-amz-json-1.1`.
Why: as cause 1 — the refusal is decoded under the wrong protocol version.
Fix: same seam; the refusal tail must not rewrite the media type either.
Traces: bcm-dashboards-dashboard-error-paths#task#4 (cluster also covers bcm-dashboards-dashboard-tagging#teardown#8).

Cause 3 of 6: ListTagsForResource success path.
AWS: `application/x-amz-json-1.0` on the HTTP 200 reply carrying `resourceTags`.
Emulator: HTTP 200 with `content-type: application/x-amz-json-1.1`.
Why: as cause 1, on a read a client makes on every tag round-trip.
Fix: same seam, success renderer.
Traces: bcm-dashboards-dashboard-tagging#task#2 (cluster also covers task#4 and task#6 of the same fixture).

Cause 4 of 6: TagResource success path.
AWS: `application/x-amz-json-1.0` on the HTTP 200 reply.
Emulator: HTTP 200 with `content-type: application/x-amz-json-1.1`.
Why: as cause 1.
Fix: same seam, success renderer.
Traces: bcm-dashboards-dashboard-tagging#task#3.

Cause 5 of 6: UntagResource success path.
AWS: `application/x-amz-json-1.0` on the HTTP 200 reply.
Emulator: HTTP 200 with `content-type: application/x-amz-json-1.1`.
Why: as cause 1.
Fix: same seam, success renderer.
Traces: bcm-dashboards-dashboard-tagging#task#5.

Cause 6 of 6: ListDashboards input-validation refusal path.
AWS: `application/x-amz-json-1.0` on the HTTP 400 refusal of an out-of-range `maxResults` (Smithy: ListDashboards.maxResults @range min 1 max 100, default 20; error ValidationException 400).
Emulator: HTTP 400 with `content-type: application/x-amz-json-1.1`.
Why: as cause 1; the validation verdict is right, only the announced protocol version is wrong.
Fix: same seam, validation refusal renderer.
Traces: bcm-dashboards-list-dashboards-max-results-out-of-range-rejected#task#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, six divergence clusters, all with the same detail string:
  `$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.0', got 'application/x-amz-json-1.1')`

- CreateDashboard, status 400 -> 400: bcm-dashboards-dashboard-error-paths#task#3
- ListTagsForResource, status 400 -> 400: bcm-dashboards-dashboard-error-paths#task#4 (+ bcm-dashboards-dashboard-tagging#teardown#8)
- ListTagsForResource, status 200 -> 200: bcm-dashboards-dashboard-tagging#task#2 (+ task#4, task#6)
- TagResource, status 200 -> 200: bcm-dashboards-dashboard-tagging#task#3
- UntagResource, status 200 -> 200: bcm-dashboards-dashboard-tagging#task#5
- ListDashboards, status 400 -> 400: bcm-dashboards-list-dashboards-max-results-out-of-range-rejected#task#1

Status codes and bodies match throughout; only the announced media type differs. Model corroboration: smithy_query(query="service_info", model_path="services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json") -> protocols ["awsJson1_0"], which implies the response media type `application/x-amz-json-1.0`. The rewrite is stated in services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/service/response_media_type.rs lines 42-43 (ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1)) and pinned by tests/all/response_media_type.rs.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture bcm-dashboards-dashboard-error-paths step task#3 (CreateDashboard refusal) and inspect the response `content-type` header: it reads application/x-amz-json-1.1 where the recorded wire reads application/x-amz-json-1.0.
3. Repeat for bcm-dashboards-dashboard-error-paths#task#4, bcm-dashboards-dashboard-tagging#task#2, #task#3, #task#5 and bcm-dashboards-list-dashboards-max-results-out-of-range-rejected#task#1 — every success and every refusal seam of the service shows the same header.
