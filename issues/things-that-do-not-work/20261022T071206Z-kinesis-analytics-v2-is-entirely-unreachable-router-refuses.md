Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisanalyticsv2.add-application-input-on-flink-application-rejected kinesisanalyticsv2.snapshot-of-ready-application-rejected kinesisanalyticsv2.update-with-stale-version-id-rejected
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesis-analytics-v2|CreateApplication|AmbiguousService
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# Kinesis Analytics V2 is entirely unreachable: router refuses every request with AmbiguousService because kinesisanalytics (v1) and kinesisanalyticsv2 share the 'kinesisanalytics' signing name

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Cause 1 of 1: request routing for the Kinesis Analytics V2 control plane resolves the target service by sigv4 signing name / endpoint prefix alone, and two emulated services both claim 'kinesisanalytics', so the router refuses rather than dispatching. Every Kinesis Analytics V2 operation is therefore unreachable; the three behavioral checks below all die on their very first call (CreateApplication) and nothing further in them is exercised.

AWS: Smithy model services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json declares service KinesisAnalytics_20180523 (version 2018-05-23), protocol awsJson1_1, awsService.endpointPrefix 'kinesisanalytics', arnNamespace 'kinesisanalytics', sigv4Name 'kinesisanalytics'. The v1 model services/service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814/... declares KinesisAnalytics_20150814 with the IDENTICAL endpointPrefix 'kinesisanalytics' and sigv4Name 'kinesisanalytics'. Real AWS serves both API versions behind that one endpoint and signing name and disambiguates them by the awsJson1_1 X-Amz-Target prefix (KinesisAnalytics_20180523.<Operation> vs KinesisAnalytics_20150814.<Operation>), so a sigv4-name collision is normal and is NOT an error condition. CreateApplication is modeled to answer 200 with a required ApplicationDetail, or one of CodeValidationException / ConcurrentModificationException (409) / InvalidArgumentException / InvalidRequestException / LimitExceededException / ResourceInUseException / TooManyTagsException / UnsupportedOperationException (400). 'AmbiguousService' is not a modeled error of this service and does not exist in AWS.

Emulator: every call signed for 'kinesisanalytics' is rejected with error code AmbiguousService and the message '2 emulated services all claim requests for \'kinesisanalytics\': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one' (recorded message, example only). botocore retried to its ceiling ('reached max retries: 3'), i.e. the response is classified retryable/server-side rather than a modeled client error.

Why: no client can use Kinesis Analytics V2 against this emulator at all — create, update, snapshot, describe and list all fail before any business logic runs. Callers see a non-AWS error code that their SDK retries and then surfaces as an unknown fault, instead of either a successful ApplicationDetail or a modeled 400/409 error. All downstream fidelity (version-id optimistic concurrency, runtime-specific configuration rejection, snapshot state rules) is untestable while routing is broken.

Fix: the shared request router / service-resolution layer, which must key awsJson1_1 dispatch on the X-Amz-Target service-shape prefix (KinesisAnalytics_20180523 vs KinesisAnalytics_20150814) rather than on the sigv4 signing name or endpoint prefix, and must never answer AmbiguousService for a signing name that AWS legitimately shares between API versions. Note: open issues report the same root cause for other shared signing names (e.g. 20261022T062300Z-forecastquery-is-entirely-unreachable-router-refuses-every-request.md, 20261022T035032Z-events-every-eventbridge-events-request-is-refused-with-ambiguousservice.md, 20261022T042904Z-chime-sdk-identity-is-entirely-unreachable-the-router.md); none of them names kinesisanalytics, so this instance is filed separately.

Traces: kinesis-analytics-v2.add-application-input-on-flink-application-rejected, kinesis-analytics-v2.snapshot-of-ready-application-rejected, kinesis-analytics-v2.update-with-stale-version-id-rejected — all three error on the first CreateApplication call.

## Evidence
Expected (AWS, per Smithy KinesisAnalytics_20180523 awsJson1_1): CreateApplication returns HTTP 200 with ApplicationDetail, or a modeled client error (InvalidArgumentException/InvalidRequestException/ResourceInUseException/LimitExceededException/CodeValidationException/TooManyTagsException/UnsupportedOperationException at 400, ConcurrentModificationException at 409). Dispatch is by X-Amz-Target prefix, so sharing the 'kinesisanalytics' signing name with the 2015-08-14 API is expected and harmless.

Actual (emulator replay): botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateApplication operation (reached max retries: 3): 2 emulated services all claim requests for 'kinesisanalytics': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one.

Same error on all 3 kinesis-analytics-v2 checks; 0/3 passed (0%).

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Point a botocore client at it: client = boto3.client('kinesisanalyticsv2', endpoint_url=<emulator>).
3. Call client.create_application(ApplicationName='probe-app', RuntimeEnvironment='FLINK-1_15', ServiceExecutionRole=<role arn>, ApplicationConfiguration={...}).
4. Observe error code AmbiguousService naming a 'kinesisanalytics' collision instead of an ApplicationDetail or a modeled 400/409 error. Any other kinesisanalyticsv2 operation (describe_application, update_application, create_application_snapshot) reproduces it identically.
5. Replay kinesis-analytics-v2.add-application-input-on-flink-application-rejected, kinesis-analytics-v2.snapshot-of-ready-application-rejected and kinesis-analytics-v2.update-with-stale-version-id-rejected to confirm the fix.
