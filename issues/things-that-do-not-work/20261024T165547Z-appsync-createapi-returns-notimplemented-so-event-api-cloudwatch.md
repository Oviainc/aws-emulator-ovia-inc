Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters
Issue-Key: appsync__cloudwatch|CreateApi|NotImplemented
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# AppSync CreateApi returns NotImplemented, so Event-API CloudWatch namespace lifecycle counters are never emitted

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: the Event-API observability path cannot be exercised at all because AppSync `CreateApi` refuses every call.
AWS: Smithy CreateApi (POST /v2/apis) takes a required `name` (ApiName, length 1\u201350, pattern ^[A-Za-z0-9_\\-\\ ]+$) and a required `eventConfig`, and returns HTTP 200 with an `api` member; its modeled errors are BadRequestException (400), ConcurrentModificationException (409), ServiceQuotaExceededException (402), UnauthorizedException (401), InternalFailureException (500) \u2014 `NotImplemented` is not among them. Recorded behavior only for the downstream claim: the check (measured against us-west-2) expects an Event API's lifecycle counters in the CloudWatch namespace `AWS/AppSync` under two dimension sets \u2014 namespace-scoped counters (SubscribeSuccess, UnsubscribeSuccess, PublishHandlerInvocations, SuccessfulEvents, DroppedEvents, FailedEvents) under [EventAPIId, ChannelNamespaceName], and connection-scoped counters (ConnectSuccess, DisconnectSuccess, ActiveConnections, ActiveSubscriptions) under [EventAPIId] only.
Emulator: the first call, CreateApi, fails with error code `NotImplemented` (HTTP 501, "operation CreateApi is scaffolded but not implemented in this emulator"), so no Event API, channel namespace, publish or CloudWatch metric is ever produced.
Why: a client (or the AWS console/CDK) creating an Event API gets an unmodeled server error instead of an Api resource, and anything that watches AWS/AppSync metrics for an Event API sees no data at all.
Fix: the CreateApi handler in the appsync service crate (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_api.rs) and the Api/ChannelNamespace state behind it, plus the AWS/AppSync metric emission on the Event-API publish/subscribe path.
Note on duplication: open issue 20261024T032443Z-appsync-createapi-stub.md asks for CreateApi to be implemented and lists 15 blocked checks, but it does NOT carry this check's replay key (the check id's leading token is the composite `appsync__cloudwatch`, so the stub census did not bind it). Fix them together; this issue exists to register the Event-API metrics replay and the metric-emission half, which the stub issue does not describe.
Traces: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters.

## Evidence
appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters (verdict: error):
  expected: CreateApi -> HTTP 200 with `api`, then the Event API's counters readable from CloudWatch GetMetricData/GetMetricStatistics in namespace AWS/AppSync under [EventAPIId, ChannelNamespaceName] and [EventAPIId].
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateApi operation: operation CreateApi is scaffolded but not implemented in this emulator
  Smithy: CreateApi errors = BadRequestException(400), ConcurrentModificationException(409), InternalFailureException(500), ServiceQuotaExceededException(402), UnauthorizedException(401); no NotImplemented, and the success code is 200.
  Handler: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_api.rs is still a scaffold stub.

## Steps to Reproduce
1. Boot the emulator.
2. Call appsync CreateApi with a valid name and eventConfig (e.g. an API_KEY auth provider for connect/publish/subscribe). Observe error code NotImplemented (HTTP 501) where AWS returns 200 with the `api` member.
3. (Once CreateApi works) create a channel namespace with an onPublish handler, open a realtime connection, subscribe, publish 4 events of which 1 is filtered, then read namespace AWS/AppSync from CloudWatch and observe the namespace-scoped counters under [EventAPIId, ChannelNamespaceName] and the connection-scoped counters under [EventAPIId].
4. Replay appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters.
