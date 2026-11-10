Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-identity__dynamodb.deep-appsync-2-appsync-authz-caller-credentials-getitem-denied-item-unchanged
Issue-Key: appsync__cognito-identity__dynamodb.deep-appsync-2-appsync-authz-caller-credentials-getitem-denied-item-unchanged|_signed_post#29|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# appsync__cognito-identity__dynamodb: the published GraphQL endpoint is an appsync-api.<region>.amazonaws.com host, so AWS_IAM caller-credentials authorization cannot be exercised

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: CreateGraphqlApi publishes a GRAPHQL URI on an AWS-owned host instead of the emulator's own address, so the Cognito-identity caller-credentials authorization path is never reached.

AWS: CreateGraphqlApi returns `graphqlApi.uris`; its `GRAPHQL` entry is the endpoint a SigV4-signed client POSTs to, served by the service that published it. Smithy: `GraphqlApi.uris` is a `MapOfStringToString` documented only as "The URIs." — the model constrains neither host nor scheme, so this is recorded/structural behavior: a published endpoint must be an address of the publishing server, i.e. for the emulator its own listener host and port.

Emulator: publishes `https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql`, a host it does not serve. The harness refuses the value with `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql ... its host is not one of the emulator's own hosts`. The apiId and port vary per run; the AWS domain is the invariant defect.

Why: a client that signs with Cognito-identity credentials and posts to the documented endpoint never reaches the emulator, so the AWS_IAM authorization decision (an unauthorized caller's GetItem being denied and the item left unchanged) cannot be observed or relied on.

Fix: the GraphqlApi URI projection in the appsync service crate — build `uris` (GRAPHQL/REALTIME) from the emulator's own listener host:port rather than an `appsync-api.<region>.amazonaws.com` template. Related open issue 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md describes the same projection defect but carries an `appsync__dynamodb` replay key only and does not cover this check.

Traces: appsync__cognito-identity__dynamodb.deep-appsync-2-appsync-authz-caller-credentials-getitem-denied-item-unchanged.

## Evidence
Expected: `uris.GRAPHQL` on one of the emulator's own hosts (e.g. http(s)://127.0.0.1:<port>/<resource path>).
Actual: File "context.py", line 588, in endpoint_route -> line 602, in _published_target
  simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

## Steps to Reproduce
1. CreateGraphqlApi with authenticationType AWS_IAM and a DynamoDB data source.
2. Read `graphqlApi.uris["GRAPHQL"]`.
3. Observe the AWS-owned host and that a SigV4-signed POST to it does not reach the emulator.
Replay appsync__cognito-identity__dynamodb.deep-appsync-2-appsync-authz-caller-credentials-getitem-denied-item-unchanged.
