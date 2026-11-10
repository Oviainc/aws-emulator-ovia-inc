Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle appsync-cfg-graphql-api-limits-and-metrics appsync-cfg-graphql-api-oidc-auth-provider appsync-datasource-function-pipeline-resolver appsync-duplicate-source-association-conflict appsync-error-delete-and-operate-on-deleted-api appsync-error-list-pagination-invalid-token appsync-error-list-types-invalid-inputs appsync-graphql-api-and-type-lifecycle appsync-graphql-api-create-delete-not-found appsync-graphql-api-crud-lifecycle appsync-list-types-by-association-lifecycle appsync-list-types-by-association-not-found appsync-list-types-for-graphql-api appsync-list-types-lifecycle appsync-merge-source-api-association-lifecycle appsync-merged-api-association-from-source-side appsync-merged-api-source-association-management appsync-resource-tagging-workflow appsync-source-api-association-lifecycle appsync-unit-resolver-code-evaluation appsync-cfg-graphql-api-owner-contact appsync-datasource-function-resolver-pipeline appsync-domain-name-error-paths appsync-list-graphql-apis-paginated-and-filtered appsync-source-api-association-auto-merge-reconfigure appsync-graphql-api-core-lifecycle appsync-graphql-api-lifecycle appsync-merge-source-into-merged-api-lifecycle appsync-list-graphql-apis-baseline
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.dns.GRAPHQL
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.dns.REALTIME
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.uris.GRAPHQL
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.uris.REALTIME
Issue-Key: appsync|GetGraphqlApi|2xx->2xx|$.graphqlApi.dns.GRAPHQL
Issue-Key: appsync|GetGraphqlApi|2xx->2xx|$.graphqlApi.dns.REALTIME
Issue-Key: appsync|GetGraphqlApi|2xx->2xx|$.graphqlApi.uris.GRAPHQL
Issue-Key: appsync|GetGraphqlApi|2xx->2xx|$.graphqlApi.uris.REALTIME
Issue-Key: appsync|UpdateGraphqlApi|2xx->2xx|$.graphqlApi.dns.GRAPHQL
Issue-Key: appsync|UpdateGraphqlApi|2xx->2xx|$.graphqlApi.dns.REALTIME
Issue-Key: appsync|UpdateGraphqlApi|2xx->2xx|$.graphqlApi.uris.GRAPHQL
Issue-Key: appsync|UpdateGraphqlApi|2xx->2xx|$.graphqlApi.uris.REALTIME
Issue-Key: appsync|ListGraphqlApis|2xx->2xx|$.graphqlApis[*].dns.GRAPHQL
Issue-Key: appsync|ListGraphqlApis|2xx->2xx|$.graphqlApis[*].dns.REALTIME
Issue-Key: appsync|ListGraphqlApis|2xx->2xx|$.graphqlApis[*].uris.GRAPHQL
Issue-Key: appsync|ListGraphqlApis|2xx->2xx|$.graphqlApis[*].uris.REALTIME
Issue-Related: 20261029T200818Z-appsync-response-projections-omit-handlerconfigs-and-emit-members.md
Issue-Commit: cad593586d705c3641fe11f59f14efd9baecb330

# AppSync GraphQL API endpoints publish AWS hostnames the emulator does not serve, and UpdateGraphqlApi adds a dns map AWS omits (5 causes)

> _Note: this issue was identified against the code base at commit `cad593586d705c3641fe11f59f14efd9baecb330`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Five net-new wire causes, all one fault family: every projection of a GraphqlApi publishes its GraphQL and REALTIME endpoints as public AWS hostnames (<label>.appsync-api.<region>.amazonaws.com / <label>.appsync-realtime-api.<region>.amazonaws.com) instead of a host:port the emulator is actually reached on, so the replay checker rejects them as host_not_own on create, read, update and list; the update path additionally emits a member AWS leaves absent.

Cause 1 of 5 - CreateGraphqlApi (fixture appsync-api-key-lifecycle, step setup#1).
AWS: GraphqlApi$uris (Smithy: GraphqlApi.uris -> MapOfStringToString, doc "The URIs.") and GraphqlApi$dns (Smithy: GraphqlApi.dns -> MapOfStringToString, doc "The DNS records for the API.") carry the host at which that API's GraphQL and realtime endpoints are really served; on AWS that is https://<host-label>.appsync-api.<region>.amazonaws.com/graphql and wss://<host-label>.appsync-realtime-api.<region>.amazonaws.com/graphql. The fixed text of the value is the scheme, the "/graphql" path and the GRAPHQL/REALTIME keys; the host label is a per-API generated value and the host itself is the serving endpoint, so for this emulator the requirement is that the announced host is one the emulator answers on (the recording asserts it as the placeholders {{dns_graphql}} / {{dns_realtime}}). The emulator already does exactly this for an Event API (state::event_api_dns publishes the sockets the API is served on).
Emulator: CreateGraphqlApi answers 200 with uris.GRAPHQL='https://<label>.appsync-api.us-west-2.amazonaws.com/graphql', uris.REALTIME='wss://<label>.appsync-realtime-api.us-west-2.amazonaws.com/graphql' and the same two hosts in dns.GRAPHQL/dns.REALTIME - none of which is one of the emulator's own hosts (verdict host_not_own on all four members, 30 affected steps).
Why: a client that dials the endpoint the API handed it never reaches the emulator at all: the whole GraphQL data plane (queries, mutations, resolver execution, api-key/SigV4 authorization on the data plane) is unusable, because the only address the API advertises resolves to nothing the emulator serves.
Fix: the GraphQL API endpoint publication in the appsync service's state (api_dns, the helper CreateGraphqlApi's handler calls when it builds the GraphqlApi projection) must publish the host:port the emulator serves that API's GraphQL/realtime endpoints on, as the Event API path already does.
Traces: appsync-api-key-lifecycle#setup#1 (+29 affected steps across appsync-cfg-graphql-api-*, appsync-list-types-*, appsync-merge*-* fixtures).

Cause 2 of 5 - CreateGraphqlApi, uris-only assertion (fixture appsync-cfg-graphql-api-owner-contact, step task#1).
AWS: same members as cause 1; this cluster asserts only uris.GRAPHQL/uris.REALTIME (shape-only).
Emulator: 200 with uris.GRAPHQL/REALTIME pointing at <label>.appsync-api/appsync-realtime-api.us-west-2.amazonaws.com - host_not_own (5 affected steps).
Why: same as cause 1 - the advertised endpoint is unreachable.
Fix: same publication helper, in CreateGraphqlApi's response projection.
Traces: appsync-cfg-graphql-api-owner-contact#task#1 (+4 affected).

Cause 3 of 5 - GetGraphqlApi (fixture appsync-api-key-lifecycle, step task#7).
AWS: GetGraphqlApi returns the stored GraphqlApi, so its uris/dns must be the same serving host the create call published.
Emulator: 200, and the read-back repeats the AWS hostnames - host_not_own on uris.GRAPHQL, uris.REALTIME, dns.GRAPHQL, dns.REALTIME (14 affected steps).
Why: a client that re-reads the API to discover its endpoint gets the same unreachable address, so no retry or re-resolution path recovers.
Fix: GetGraphqlApi's response projection must render the endpoints from the same own-host publication as cause 1 (fixing the stored/published value fixes all readers).
Traces: appsync-api-key-lifecycle#task#7 (+13 affected).

Cause 4 of 5 - UpdateGraphqlApi (fixture appsync-cfg-graphql-api-owner-contact, step task#3), two faults on one response.
AWS: UpdateGraphqlApi returns the updated GraphqlApi; the recording shows uris present with the serving host and NO dns member at all on this operation's output. Recorded behavior only for the dns omission (the model marks neither member required).
Emulator: 200 with (a) uris.GRAPHQL/REALTIME at the AWS hostnames - host_not_own, and (b) a dns map {GRAPHQL: <label>.appsync-api...,REALTIME: <label>.appsync-realtime-api...} where AWS returns none (type_mismatch, expected None) (4 affected steps).
Why: the endpoint is unreachable as in cause 1, and a client that treats presence of dns as meaningful (for example to prefer DNS records over uris) sees a member AWS never sends from this call, pointing at a host that does not exist.
Fix: UpdateGraphqlApi's response serializer - publish endpoints from the own-host helper and stop emitting the dns map on this operation's output.
Traces: appsync-cfg-graphql-api-owner-contact#task#3 (+3 affected).

Cause 5 of 5 - ListGraphqlApis (fixture appsync-graphql-api-and-type-lifecycle, step task#3).
AWS: each graphqlApis[] element is a full GraphqlApi, so its uris/dns name the serving host.
Emulator: 200 with graphqlApis[0].uris.GRAPHQL/REALTIME and .dns.GRAPHQL/REALTIME at the AWS hostnames - host_not_own (4 affected steps).
Why: endpoint discovery by inventory listing yields the same unreachable address for every API in the account.
Fix: the ListGraphqlApis summary projection must use the same own-host endpoint publication.
Traces: appsync-graphql-api-and-type-lifecycle#task#3 (+3 affected).

Related but NOT covering these: the open behavioral issues 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris, 20261109T150433Z-appsync-cloudwatch-observability-activesubscriptions-never-returns-to-0 and cause 2 of 20261109T150433Z-appsync-4-net-new-causes-apicache-creating-state describe the same root fault from the behavioral corpus; none of them carries a wire replay key for these fixture steps, and none covers the UpdateGraphqlApi dns projection, so the five causes are filed here with their own keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false; 91 fixtures, 733 steps, 466 passed, score 63.6%).

[mismatch] appsync-api-key-lifecycle#setup#1 CreateGraphqlApi - expected 200, got 200:
  $.graphqlApi.uris.GRAPHQL: host_not_own (expected 'https://{{dns_graphql}}/graphql', got 'https://<label>.appsync-api.us-west-2.amazonaws.com/graphql')
  $.graphqlApi.uris.REALTIME: host_not_own (expected 'wss://{{dns_realtime}}/graphql', got 'wss://<label>.appsync-realtime-api.us-west-2.amazonaws.com/graphql')
  $.graphqlApi.dns.GRAPHQL: host_not_own (expected '{{dns_graphql}}', got '<label>.appsync-api.us-west-2.amazonaws.com')
  $.graphqlApi.dns.REALTIME: host_not_own (expected '{{dns_realtime}}', got '<label>.appsync-realtime-api.us-west-2.amazonaws.com')
  (30 affected steps)

[mismatch] appsync-cfg-graphql-api-owner-contact#task#1 CreateGraphqlApi - 200/200:
  $.graphqlApi.uris.GRAPHQL / $.graphqlApi.uris.REALTIME: host_not_own ('https://<label>.appsync-api.us-west-2.amazonaws.com/graphql', 'wss://<label>.appsync-realtime-api.us-west-2.amazonaws.com/graphql') (5 affected)

[mismatch] appsync-api-key-lifecycle#task#7 GetGraphqlApi - 200/200:
  $.graphqlApi.uris.GRAPHQL, .uris.REALTIME, .dns.GRAPHQL, .dns.REALTIME: host_not_own (same AWS hostnames) (14 affected)

[mismatch] appsync-cfg-graphql-api-owner-contact#task#3 UpdateGraphqlApi - 200/200:
  $.graphqlApi.dns: type_mismatch (expected None, got {'REALTIME': '<label>.appsync-realtime-api.us-west-2.amazonaws.com', 'GRAPHQL': '<label>.appsync-api.us-west-2.amazonaws.com'})
  $.graphqlApi.uris.GRAPHQL, .uris.REALTIME, .dns.GRAPHQL, .dns.REALTIME: host_not_own (4 affected)

[mismatch] appsync-graphql-api-and-type-lifecycle#task#3 ListGraphqlApis - 200/200:
  $.graphqlApis[0].uris.GRAPHQL, .uris.REALTIME, .dns.GRAPHQL, .dns.REALTIME: host_not_own (4 affected)

Host labels, region and port in the recorded values are per-run; the requirement is that the announced host is one the emulator is reached on, and that UpdateGraphqlApi omits dns.

## Steps to Reproduce
1. Clone the repository and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; the five clusters above diverge (all verdict mismatch, HTTP 200 on both sides except where noted).
3. By hand: CreateGraphqlApi(name=<valid name>, authenticationType=API_KEY) and read graphqlApi.uris / graphqlApi.dns - both name <label>.appsync-api.<region>.amazonaws.com and <label>.appsync-realtime-api.<region>.amazonaws.com, which the emulator does not serve (cause 1/2; fixture appsync-api-key-lifecycle step setup#1, appsync-cfg-graphql-api-owner-contact step task#1).
4. GetGraphqlApi(apiId) and ListGraphqlApis() - the same non-own hosts come back in uris/dns (causes 3 and 5; appsync-api-key-lifecycle#task#7, appsync-graphql-api-and-type-lifecycle#task#3).
5. UpdateGraphqlApi(apiId, name, authenticationType, ownerContact) - the response carries a dns map AWS does not return on this operation, plus the same non-own hosts (cause 4; appsync-cfg-graphql-api-owner-contact#task#3).
6. Dial the published uris.GRAPHQL: the request never reaches the emulator, since that host is not one of the emulator's own hosts.
