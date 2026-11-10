Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request
Issue-Key: appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request|_post_json#19|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# appsync__dynamodb__lambda: the published GraphQL endpoint is an appsync-api.<region>.amazonaws.com host, so Lambda-authorizer TTL behaviour cannot be exercised

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: CreateGraphqlApi publishes a GRAPHQL URI on an AWS-owned host instead of the emulator's own address, so the Lambda-authorizer request path is never reached.

AWS: CreateGraphqlApi returns `graphqlApi.uris`; its `GRAPHQL` entry is the endpoint clients POST to and is served by the publishing service. With `lambdaAuthorizerConfig.authorizerResultTtlInSeconds = 0` AWS re-invokes the Lambda authorizer on EVERY request instead of caching its decision. Smithy: `GraphqlApi.uris` is a `MapOfStringToString` documented only as "The URIs." — no host/scheme constraint is modelled, so the endpoint requirement is recorded/structural: a published address must belong to the server that published it.

Emulator: publishes `https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql`, which it does not serve; the harness refuses it with `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql ... its host is not one of the emulator's own hosts`. The apiId and port are per-run values, not part of the requirement.

Why: a client cannot send a single request to the API, so the authorizer is never invoked and the TTL-override-zero contract (one authorizer invocation per request) is unobservable; any application that reads `uris.GRAPHQL` and calls it is routed off-box.

Fix: the GraphqlApi URI projection in the appsync service crate — render `uris` (GRAPHQL/REALTIME) from the emulator's own listener host:port instead of an `appsync-api.<region>.amazonaws.com` template. Related open issue 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md reports the same projection defect for a different check and does not carry this one's replay key.

Traces: appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request.

## Evidence
Expected: `uris.GRAPHQL` on one of the emulator's own hosts (e.g. http(s)://127.0.0.1:<port>/<resource path>).
Actual: File "context.py", line 588, in endpoint_route -> line 602, in _published_target
  simparity.published.PublishedUrlRejected: endpoint_url was given https://epgptsryib5dfoovyfifr65lw7.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:38671/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

## Steps to Reproduce
1. CreateGraphqlApi with authenticationType AWS_LAMBDA and lambdaAuthorizerConfig.authorizerResultTtlInSeconds = 0.
2. Read `graphqlApi.uris["GRAPHQL"]`.
3. Observe the AWS-owned host; POSTing a query to it does not reach the emulator, so the authorizer is never invoked.
Replay appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request.
