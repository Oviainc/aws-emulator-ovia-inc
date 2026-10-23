Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-featurestore-runtime-batch-get-record-malformed-name
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-featurestore-runtime-batch-get-record-malformed-name sagemaker-featurestore-runtime-batch-write-record-malformed-name sagemaker-featurestore-runtime-delete-record-malformed-name sagemaker-featurestore-runtime-get-record-malformed-name sagemaker-featurestore-runtime-list-records-malformed-name-and-limit sagemaker-featurestore-runtime-put-record-malformed-name sagemaker-featurestore-runtime-update-record-malformed-name
Issue-Shape-Id: com.amazonaws.sagemakerfeaturestoreruntime#AmazonSageMakerFeatureStoreRuntime
Issue-Key: featurestore-runtime.sagemaker|BatchGetRecord|4xx->5xx|$.message
Issue-Key: featurestore-runtime.sagemaker|BatchGetRecord|4xx->5xx|$headers.content-type
Issue-Key: featurestore-runtime.sagemaker|BatchGetRecord|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: featurestore-runtime.sagemaker|BatchGetRecord|4xx->5xx|status
Issue-Key: featurestore-runtime.sagemaker|BatchWriteRecord|4xx->5xx|$.message
Issue-Key: featurestore-runtime.sagemaker|BatchWriteRecord|4xx->5xx|$headers.content-type
Issue-Key: featurestore-runtime.sagemaker|BatchWriteRecord|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: featurestore-runtime.sagemaker|BatchWriteRecord|4xx->5xx|status
Issue-Key: featurestore-runtime.sagemaker|DeleteRecord|4xx->5xx|$.message
Issue-Key: featurestore-runtime.sagemaker|DeleteRecord|4xx->5xx|$headers.content-type
Issue-Key: featurestore-runtime.sagemaker|DeleteRecord|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: featurestore-runtime.sagemaker|DeleteRecord|4xx->5xx|status
Issue-Key: featurestore-runtime.sagemaker|GetRecord|4xx->5xx|$.message
Issue-Key: featurestore-runtime.sagemaker|GetRecord|4xx->5xx|$headers.content-type
Issue-Key: featurestore-runtime.sagemaker|GetRecord|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: featurestore-runtime.sagemaker|GetRecord|4xx->5xx|status
Issue-Key: featurestore-runtime.sagemaker|ListRecords|4xx->5xx|$.message
Issue-Key: featurestore-runtime.sagemaker|ListRecords|4xx->5xx|$headers.content-type
Issue-Key: featurestore-runtime.sagemaker|ListRecords|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: featurestore-runtime.sagemaker|ListRecords|4xx->5xx|status
Issue-Key: featurestore-runtime.sagemaker|PutRecord|4xx->5xx|$.message
Issue-Key: featurestore-runtime.sagemaker|PutRecord|4xx->5xx|$headers.content-type
Issue-Key: featurestore-runtime.sagemaker|PutRecord|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: featurestore-runtime.sagemaker|PutRecord|4xx->5xx|status
Issue-Key: featurestore-runtime.sagemaker|UpdateRecord|4xx->5xx|$.message
Issue-Key: featurestore-runtime.sagemaker|UpdateRecord|4xx->5xx|$headers.content-type
Issue-Key: featurestore-runtime.sagemaker|UpdateRecord|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: featurestore-runtime.sagemaker|UpdateRecord|4xx->5xx|status
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# Router refuses every featurestore-runtime.sagemaker request as AmbiguousService (sagemaker signing-name collision across 9 services), so no operation is reachable

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetRecord

## Description
This bundle has 7 causes — one per bound operation of SageMaker FeatureStore Runtime. All seven share a single root cause in the shared request router, and every one must be closed before submitting.

SHARED ROOT CAUSE: the router resolves an incoming request to an emulated service by its sigv4 signing name. Smithy (services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime, query=service_info) gives this service sigv4Name "sagemaker" and arnNamespace "sagemaker", while its endpointPrefix is "featurestore-runtime.sagemaker". Nine emulated SageMaker-family services share the signing name "sagemaker" (sagemaker, sagemaker-runtime, sagemaker-runtime-http2, sagemaker-a2i-runtime, sagemaker-featurestore-runtime, sagemaker-metrics, sagemaker-edge, sagemaker-geospatial, sagemaker-training-session-runtime), so the router finds nine claimants, refuses to pick one, and answers 500 with x-amzn-errortype AmbiguousService and message "9 emulated services all claim requests for 'sagemaker': sagemaker, sagemaker, ...; the request is rejected rather than served by an arbitrary one". There is no AmbiguousService error in any AWS model and AWS never answers 500 to a malformed-name request. Two further defects ride along: the refusal is serialized with content-type application/x-amz-json-1.1 (awsJson1_1) although this service's protocol is restJson1, whose error documents are application/json; and because the request is never routed, the modeled input validation never runs at all.

AWS (common to all seven causes): every operation takes the feature group name as FeatureGroupNameOrArn — Smithy shape com.amazonaws.sagemakerfeaturestoreruntime#FeatureGroupNameOrArn, @length min 1 max 150, @pattern ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$. A value violating that pattern is rejected before any store lookup. The recorded responses show HTTP 400, content-type application/json, x-amzn-errortype ValidationException, with a message built from the request: "1 validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — the message RULE (member path + the modeled pattern) is the requirement; the recorded text is an example of it. (The model also declares a service-common ValidationError with httpError 400; the recordings show the framework's ValidationException errortype, which is what a client sees.)

Why (common to all seven): a client gets a 500 server-fault instead of a 400 client-fault, so SDKs classify it as retryable and retry a request that can never succeed, and no ValidationException reaches the caller's error branch. The content-type/errortype are wrong for restJson1, so error shape matching fails too. Because the refusal happens in routing, the service is entirely unreachable — not just on malformed input — so every FeatureStore Runtime workload fails.

Fix (common to all seven): the shared request router / service-registry claim resolution — disambiguate claimants by endpointPrefix and the request host/URI (restJson1 path bindings such as /FeatureGroup/{FeatureGroupName} are also distinctive) rather than by sigv4 signing name alone; and the refusal serializer must emit the target service's protocol (restJson1: application/json, x-amzn-errortype) instead of an awsJson1_1 __type body. Once routing lands, each operation's input validation must enforce the FeatureGroupNameOrArn pattern and return 400 ValidationException naming the offending member path.

Cause 1 of 7: BatchGetRecord. AWS: 400 ValidationException naming member path 'identifiers.1.member.featureGroupName' against the FeatureGroupNameOrArn pattern. Emulator: 500, x-amzn-errortype AmbiguousService, content-type application/x-amz-json-1.1. Trace: sagemaker-featurestore-runtime-batch-get-record-malformed-name step task#1.

Cause 2 of 7: BatchWriteRecord. AWS: 400 ValidationException naming member path 'entries.1.member.featureGroupName'. Emulator: 500 AmbiguousService as above. Trace: sagemaker-featurestore-runtime-batch-write-record-malformed-name step task#1.

Cause 3 of 7: DeleteRecord (DELETE /FeatureGroup/{FeatureGroupName}). AWS: 400 ValidationException naming member 'featureGroupName'. Emulator: 500 AmbiguousService as above. Trace: sagemaker-featurestore-runtime-delete-record-malformed-name step task#1.

Cause 4 of 7: GetRecord (GET /FeatureGroup/{FeatureGroupName}). AWS: 400 ValidationException naming member 'featureGroupName'. Emulator: 500 AmbiguousService as above. Trace: sagemaker-featurestore-runtime-get-record-malformed-name step task#1.

Cause 5 of 7: ListRecords (POST /FeatureGroup/{FeatureGroupName}/ListRecords). AWS: 400 ValidationException naming member 'featureGroupName'. Emulator: 500 AmbiguousService as above; this cluster covers two recorded steps (task#1 and task#2), so the limit-bearing step must be replayed too. Trace: sagemaker-featurestore-runtime-list-records-malformed-name-and-limit steps task#1 and task#2.

Cause 6 of 7: PutRecord (PUT /FeatureGroup/{FeatureGroupName}). AWS: 400 ValidationException naming member 'featureGroupName'. Emulator: 500 AmbiguousService as above. Trace: sagemaker-featurestore-runtime-put-record-malformed-name step task#1.

Cause 7 of 7: UpdateRecord (POST /FeatureGroup/{FeatureGroupName}/Record). AWS: 400 ValidationException naming member 'featureGroupName'. Emulator: 500 AmbiguousService as above. Trace: sagemaker-featurestore-runtime-update-record-malformed-name step task#1.

Related open issue (does NOT cover these steps): '20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice' reports the same router defect for the iam/iam-toolbox signing-name collision. It carries no replay key for any featurestore-runtime.sagemaker fixture, and the 'sagemaker' collision has nine claimants rather than two, so fixing the iam case alone will not close these seven. A fix that generalises claim resolution should close both.

## Evidence
Replay of simparity-wire-aws-v1: 8 of 8 steps mismatched, score 0.0%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

All seven clusters report the identical diff:
  status: expected 400, got 500
  $.message: expected "1 validation error detected: Value 'not a valid name!!' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})"
            got "9 emulated services all claim requests for 'sagemaker': sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker; the request is rejected rather than served by an arbitrary one"
  $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'
  $headers.x-amzn-errortype: expected 'ValidationException', got 'AmbiguousService'

Member path per operation: BatchGetRecord 'identifiers.1.member.featureGroupName'; BatchWriteRecord 'entries.1.member.featureGroupName'; DeleteRecord/GetRecord/ListRecords/PutRecord/UpdateRecord 'featureGroupName'.

Smithy evidence:
- service_info: sdkId "SageMaker FeatureStore Runtime", endpointPrefix "featurestore-runtime.sagemaker", sigv4Name "sagemaker", arnNamespace "sagemaker", protocol restJson1, common_errors include ValidationError (errorKind client, httpError 400).
- shape FeatureGroupNameOrArn: string, @length 1..150, @pattern ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$.
- No AWS model defines an 'AmbiguousService' error, and restJson1 error documents are application/json, not application/x-amz-json-1.1.

Failing fixtures/steps: sagemaker-featurestore-runtime-batch-get-record-malformed-name#task#1; sagemaker-featurestore-runtime-batch-write-record-malformed-name#task#1; sagemaker-featurestore-runtime-delete-record-malformed-name#task#1; sagemaker-featurestore-runtime-get-record-malformed-name#task#1; sagemaker-featurestore-runtime-list-records-malformed-name-and-limit#task#1 and #task#2; sagemaker-featurestore-runtime-put-record-malformed-name#task#1; sagemaker-featurestore-runtime-update-record-malformed-name#task#1.

## Steps to Reproduce
1. Clone the repo and boot the emulator (health endpoint /__admin/health answers 200 and the data listener accepts).
2. Point an SDK client for SageMaker FeatureStore Runtime (sigv4 signing name 'sagemaker', endpointPrefix 'featurestore-runtime.sagemaker', protocol restJson1) at the emulator's data listener.
3. Call any of the seven operations with a feature group name that violates FeatureGroupNameOrArn, e.g. GetRecord(FeatureGroupName="not a valid name!!", RecordIdentifierValueAsString="1").
4. Observe HTTP 500 with content-type application/x-amz-json-1.1 and x-amzn-errortype AmbiguousService, message "9 emulated services all claim requests for 'sagemaker': ...", instead of HTTP 400 application/json x-amzn-errortype ValidationException naming 'featureGroupName' and the modeled pattern.
5. Repeat for BatchGetRecord ('identifiers.1.member.featureGroupName'), BatchWriteRecord ('entries.1.member.featureGroupName'), DeleteRecord, ListRecords, PutRecord and UpdateRecord — each refuses identically, confirming the service is unreachable rather than mis-validating.
6. Replay the wire corpus simparity-wire-aws-v1 and re-run all seven fixtures listed under Traces (including both steps of sagemaker-featurestore-runtime-list-records-malformed-name-and-limit) to confirm the fix.
