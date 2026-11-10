Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__events.xsvc-appsync-2-near-miss-events-not-delivered-while-the-pipeline-is-live appsync__dynamodb__events.xsvc-appsync-2-role-without-graphql-denied-dead-letters-and-counts-failed-invocations appsync__dynamodb__events.xsvc-appsync-2-rule-input-transformer-mutation-writes-the-mapped-item
Issue-Key: appsync__dynamodb__events.xsvc-appsync-2-near-miss-events-not-delivered-while-the-pipeline-is-live|_create_api#13|AssertionError|assert not (p := ctx.published_url_mismatch(uri, labels=("appsync-api",))), p
Issue-Key: appsync__dynamodb__events.xsvc-appsync-2-role-without-graphql-denied-dead-letters-and-counts-failed-invocations|_create_api#16|AssertionError|assert not (p := ctx.published_url_mismatch(uri)), p
Issue-Key: appsync__dynamodb__events.xsvc-appsync-2-rule-input-transformer-mutation-writes-the-mapped-item|_create_api#13|AssertionError|assert not (p := ctx.published_url_mismatch(uri, labels=("appsync-api",), path="/graphql")), p
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# appsync__dynamodb__events: the GraphqlApi URI published to an EventBridge-driven pipeline is an appsync-api.<region>.amazonaws.com host the emulator does not serve

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: CreateGraphqlApi publishes a GRAPHQL URI on an AWS-owned host rather than on the address the emulator is reached on, so every EventBridge→AppSync→DynamoDB scenario fails before its first GraphQL call.

AWS: CreateGraphqlApi returns `graphqlApi.uris`, whose `GRAPHQL` entry is the endpoint clients (and an EventBridge rule target) send GraphQL operations to; on AWS it is a host the service itself serves. Smithy: `GraphqlApi.uris` is a `MapOfStringToString` documented only as "The URIs.", so the model states nothing about host or scheme — recorded/structural behavior only: a published endpoint must be an address of the server that published it, which for the emulator is its own listener (`http(s)://127.0.0.1:<port>/<path>` or `http(s)://<own host>:<port>/<path>`).

Emulator: publishes `https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql`. The check's own guard, `ctx.published_url_mismatch(uri)`, refuses it: "the emulator published https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql: its host is not one of the emulator's own hosts". The apiId and the harness port differ per run and are not part of the requirement; the constant defect is the AWS domain.

Why: the API cannot be driven at all, so nothing downstream of it can be exercised — a near-miss event that must NOT be delivered, a role without appsync:GraphQL that must dead-letter and count FailedInvocations, and an EventBridge InputTransformer whose mapped fields must land as a DynamoDB item are all unobservable. A client wiring EventBridge to AppSync by reading `uris.GRAPHQL` configures a target that is off-box.

Fix: the GraphqlApi URI projection in the appsync service crate — render `uris` (GRAPHQL/REALTIME) and `dns` from the emulator's own listener host and port instead of an `appsync-api.<region>.amazonaws.com` template. Open issue 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md reports the same projection defect but carries only an `appsync__dynamodb` replay key; the three checks here are additional and were not covered by it.

Traces: the three `appsync__dynamodb__events` checks in trace_ids, each failing in `_create_api` on the published-URI guard.

## Evidence
Expected: a GRAPHQL URI on one of the emulator's own hosts (e.g. http(s)://127.0.0.1:<port>/<resource path>).
Actual (appsync__dynamodb__events.xsvc-appsync-2-near-miss-events-not-delivered-while-the-pipeline-is-live):
  File "...xsvc_appsync_2_near_miss_events_not_delivered_while_the_pipeline_is_live.py", line 166, in _create_api
  assert not (p := ctx.published_url_mismatch(uri, labels=("appsync-api",))), p
  AssertionError: the emulator published https://daxisuvpck2himafl5epmh2f37.appsync-api.us-west-2.amazonaws.com/graphql: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:37329/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
Same assertion for xsvc-appsync-2-role-without-graphql-denied-dead-letters-and-counts-failed-invocations (published https://cjdxiw47rzdthjj4aa3r6u7dp2.appsync-api.us-west-2.amazonaws.com/graphql) and xsvc-appsync-2-rule-input-transformer-mutation-writes-the-mapped-item (https://djxwyladb3mcsapjfvab52h6ig.appsync-api.us-west-2.amazonaws.com/graphql).

## Steps to Reproduce
1. CreateGraphqlApi with a DynamoDB data source and a resolver.
2. Read `graphqlApi.uris["GRAPHQL"]`.
3. Observe the host is `<apiId>.appsync-api.<region>.amazonaws.com` rather than the emulator's own host:port; an EventBridge rule target or client built from it never reaches the emulator.
Replay the three checks in trace_ids.
