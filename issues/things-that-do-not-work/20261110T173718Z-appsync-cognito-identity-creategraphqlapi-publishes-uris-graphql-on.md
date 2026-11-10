Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-identity.xsvc-appsync-3-authenticated-identity-field-grant-echoes-cognito-identity appsync__cognito-identity.xsvc-appsync-3-explicit-field-deny-overrides-api-wildcard-allow-denied appsync__cognito-identity.xsvc-appsync-3-unauthenticated-identity-role-without-grant-denied appsync__cognito-identity.xsvc-appsync-3-ungranted-query-field-denied
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-authenticated-identity-field-grant-echoes-cognito-identity|_post_signed#13|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-explicit-field-deny-overrides-api-wildcard-allow-denied|_post_signed#9|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-unauthenticated-identity-role-without-grant-denied|_post_signed#9|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-ungranted-query-field-denied|_post_signed#9|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: b89f41e6cc4db360cb043b357635b76c01311ad8

# appsync__cognito-identity: CreateGraphqlApi publishes uris.GRAPHQL on a real appsync-api.<region>.amazonaws.com host the emulator does not serve, so no AWS_IAM field-authorization check can reach the GraphQL data plane (1 cause)

> _Note: this issue was identified against the code base at commit `b89f41e6cc4db360cb043b357635b76c01311ad8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: the GraphQL endpoint AppSync publishes for a newly created API is a real-AWS hostname, not an address of the emulator, so every Cognito-vended-credential GraphQL request leaves the emulator and the whole AWS_IAM field-authorization behaviour behind these four checks is unreachable.

AWS: Smithy: CreateGraphqlApi returns GraphqlApi, whose member `uris` targets MapOfStringToString ("The URIs.") and carries the GRAPHQL key — the address a client POSTs SigV4-signed GraphQL queries to. The model does not constrain the host (recorded behavior only): on real AWS the value is a host AWS itself serves, so the endpoint handed back by the control plane is the one that answers the data-plane request. Against an emulator the equivalent requirement is that the published host be one the emulator serves (its own host/port, or a per-resource listener on an own host), so a client that uses the value verbatim reaches the API it just created.

Emulator: CreateGraphqlApi returns `uris.GRAPHQL` = `https://<26-char api id>.appsync-api.<region>.amazonaws.com/graphql` — a real-AWS AppSync host. The emulator never serves that name, so the check's routing of the published endpoint is refused before any request is sent: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiid>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The api id, region, 127.0.0.1 and the port in that text are per-run values; the fixed part of the defect is the published host family `appsync-api.<region>.amazonaws.com`, which is never an address of this emulator. All four checks in this group fail identically at that point, each immediately after a successful CreateGraphqlApi/StartSchemaCreation/CreateDataSource/CreateResolver sequence — i.e. the control plane accepts the API and then advertises an address that does not exist.

Why: a client that does what every AppSync client does — take `uris.GRAPHQL` from CreateGraphqlApi (or GetGraphqlApi) and POST its queries there — gets a DNS/connection failure against the emulator, or, worse, egresses to real AWS. Nothing downstream of the endpoint can be exercised: AWS_IAM authorization of individual GraphQL fields (appsync:GraphQL on `.../types/Query/fields/<field>`), the explicit-field-Deny-overrides-wildcard-Allow precedence, the unauthenticated-identity-role-without-grant denial, and the $context.identity echo that proves the Cognito identity reached the resolver are all untestable and, from a client's point of view, the API simply has no endpoint.

Fix: the CreateGraphqlApi handler's response projection (and the same projection used by GetGraphqlApi / ListGraphqlApis / UpdateGraphqlApi, plus the `dns` map) must build `uris.GRAPHQL` from the emulator's own listener address rather than formatting an AWS hostname template, and the emulator must route that path to its GraphQL data plane. Related open issues describe the same published-URL defect under other service headings — 20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md, 20261110T155738Z-appsync-cognito-identity-dynamodb-the-published-graphql-endpoint.md, 20261110T155328Z-appsync-cognito-idp-an-openid-connect-graphql-api.md — none of which carries these four checks' replay keys, so this issue files them.

Traces: the four failing checks listed below, each an example of the same published-endpoint defect.

## Evidence
Expected (AWS): CreateGraphqlApi -> graphqlApi.uris["GRAPHQL"] is an endpoint that answers a SigV4-signed GraphQL POST; the check then observes per-field authorization (200 with data for a granted field, 401/403 + GraphQL errors for an ungranted or explicitly denied field) and $context.identity echoing the Cognito identity (cognitoIdentityId / cognitoIdentityPoolId / cognitoIdentityAuthType).

Actual (emulator): graphqlApi.uris["GRAPHQL"] = "https://<apiid>.appsync-api.<region>.amazonaws.com/graphql". The host is not served by the emulator, and the request is refused before it is sent:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://glm5yu4cu2ot3ahe2uibidmkp7.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34203/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
Identical refusal for the other three checks with their own api ids/ports (k53ohpriyulcqfimr4pi5vckok / :36925, k5h2ocp46iqv7c72tor64aq2y6 / :38759, i7zc4v54ay44fojedkp6ekwudd / :41605) — the api id and port are per-run values, the diverging part is the published host.

Smithy citation: appsync#GraphqlApi member `uris` -> MapOfStringToString ("The URIs."); `dns` -> MapOfStringToString ("The DNS records for the API."). Per-service pass rate: appsync__cognito-identity 0/4 (0%).

## Steps to Reproduce
1. appsync.CreateGraphqlApi(name=<alnum>, authenticationType="AWS_IAM").
2. Read graphqlApi.uris["GRAPHQL"] from the response (also reproducible via GetGraphqlApi).
3. Observe the host is <apiId>.appsync-api.<region>.amazonaws.com — not an address the emulator listens on.
4. StartSchemaCreation with a two-field Query schema, CreateDataSource(type="NONE"), CreateResolver on both fields with an identity-echo request template.
5. cognito-identity: CreateIdentityPool(AllowUnauthenticatedIdentities=true, DeveloperProviderName=...), GetId/GetOpenIdTokenForDeveloperIdentity, GetCredentialsForIdentity to vend credentials; attach an IAM policy granting appsync:GraphQL on only one field.
6. SigV4-sign a GraphQL POST with those credentials to the published uris.GRAPHQL and send it: the request cannot be routed to the emulator at all (PublishedUrlRejected), so no field-authorization or $context.identity assertion is ever evaluated.
