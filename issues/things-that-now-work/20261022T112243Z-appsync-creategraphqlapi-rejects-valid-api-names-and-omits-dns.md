Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-cache-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-cache-lifecycle appsync-api-key-lifecycle appsync-cfg-graphql-api-limits-and-metrics appsync-cfg-graphql-api-oidc-auth-provider appsync-cfg-graphql-api-owner-contact appsync-datasource-function-pipeline-resolver appsync-datasource-function-resolver-pipeline appsync-domain-name-error-paths appsync-duplicate-source-association-conflict appsync-error-delete-and-operate-on-deleted-api appsync-error-list-pagination-invalid-token appsync-error-list-types-invalid-inputs appsync-graphql-api-and-type-lifecycle appsync-graphql-api-crud-lifecycle appsync-list-graphql-apis-paginated-and-filtered appsync-list-types-by-association-lifecycle appsync-list-types-by-association-not-found appsync-list-types-for-graphql-api appsync-list-types-lifecycle appsync-merge-source-api-association-lifecycle appsync-merged-api-association-from-source-side appsync-merged-api-source-association-management appsync-resource-tagging-workflow appsync-source-api-association-auto-merge-reconfigure appsync-source-api-association-lifecycle appsync-unit-resolver-code-evaluation appsync-api-cache-update-flush-delete-chain appsync-associate-merged-graphql-api-duplicate-error appsync-cache-behavior-reconfiguration appsync-cfg-create-datasource-rejections appsync-cfg-datasource-type-defaults appsync-cfg-resolver-batch-and-caching-rules appsync-cfg-resolver-sync-config-rules appsync-cfg-update-datasource-rules appsync-datasource-pagination-page-boundary appsync-environment-variables-config-round-trip appsync-error-associate-with-invalid-merged-api appsync-error-association-not-found-on-merged-api appsync-eval-code-error-detail appsync-eval-schema-creation-introspection appsync-graphql-api-core-lifecycle appsync-graphql-api-lifecycle appsync-list-api-keys-contract appsync-list-graphql-apis-baseline appsync-list-types-after-create-type appsync-list-types-by-association-json-format appsync-list-types-errors appsync-merge-source-into-merged-api-lifecycle appsync-merged-api-association-lifecycle appsync-resolver-mapping-template-unicode-round-trip appsync-resource-tagging appsync-schema-creation-introspection appsync-schema-type-management appsync-source-api-association-not-found-errors appsync-type-sdl-json-format-fidelity appsync-update-data-source-http-config-persists appsync-graphql-api-create-delete-not-found
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateGraphqlApi|2xx->4xx|$.graphqlApi
Issue-Key: appsync|CreateGraphqlApi|2xx->4xx|status
Issue-Key: appsync|CreateGraphqlApi|4xx->4xx|$.message
Issue-Key: appsync|ListGraphqlApis|2xx->2xx|$.graphqlApis[*]
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.dns
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.uris.GRAPHQL
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.uris.REALTIME
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# CreateGraphqlApi rejects valid API names and omits dns

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
The AppSync CreateGraphqlApi handler applies the OUTPUT shape's name constraint to its INPUT, so almost every recorded CreateGraphqlApi is refused with 400 and nothing downstream of it can run. This bundle carries 5 causes; all 5 must be closed.

Cause 1 of 5: CreateGraphqlApi refuses a hyphenated API name (fixture appsync-api-cache-lifecycle, step setup#1; 35 steps in cluster).
AWS: Smithy: CreateGraphqlApiRequest.name targets plain `String` with NO @pattern and NO @length — the input is unconstrained. (The @pattern ^[_A-Za-z][_0-9A-Za-z]*$ / @length 1-65536 lives only on the OUTPUT member GraphqlApi.name, target ResourceName.) The recording shows AWS answering 200 for name 'sim2real-probe-appsync-cacheapi-b0d89c65' and echoing that hyphenated name back in $.graphqlApi.name, so hyphens are accepted on input.
Emulator: answers 400 and returns no body at all for $.graphqlApi (expected the full GraphqlApi structure, got None). The companion message seen on the same validation (Cause 3) is 'name must be 1-65536 characters matching ^[_A-Za-z][_0-9A-Za-z]*$, got "..."', i.e. the output-shape pattern is being enforced on the request.
Why: a client cannot create an API with any conventional hyphenated or dotted name. Every AppSync workflow begins with CreateGraphqlApi, so the 400 cascades: 509 steps of this corpus are blocked behind it and the service is effectively unusable.
Fix: CreateGraphqlApi's input validation — drop the ResourceName pattern/length check from the request path; it belongs to the response shape only.
Traces: appsync-api-cache-lifecycle#setup#1 (+34 affected, including appsync-graphql-api-crud-lifecycle#task#1, appsync-domain-name-error-paths#task#1, appsync-list-types-lifecycle#setup#1).

Cause 2 of 5: same 400 on CreateGraphqlApi reported with status-only assertions (fixture appsync-api-cache-update-flush-delete-chain, step setup#1; 37 steps in cluster).
AWS: Smithy: CreateGraphqlApi http {POST /v1/apis, code 200}; the recording expects 200.
Emulator: 400.
Why: identical client impact to Cause 1 — a separate cluster only because these steps assert status without asserting the body. Hypothesis: the same unconstrained-input defect; confirm by re-replaying after the Cause 1 fix.
Fix: CreateGraphqlApi's input validation.
Traces: appsync-api-cache-update-flush-delete-chain#setup#1 (+36 affected, including appsync-graphql-api-lifecycle#setup#1, appsync-list-graphql-apis-baseline#setup#1, appsync-eval-code-error-detail#setup#1).

Cause 3 of 5: the bogus name check pre-empts the real additionalAuthenticationProviders validation (fixture appsync-cfg-graphql-api-oidc-auth-provider, step task#1; 6 steps in cluster).
AWS: Smithy: CreateGraphqlApiRequest.additionalAuthenticationProviders targets AdditionalAuthenticationProviders; an entry whose authenticationType is OPENID_CONNECT with no openIDConnectConfig is rejected with BadRequestException (httpError 400). Recorded message: 'OpenID Connect Config is missing for additional authentication provider 1' (example of the rule: name the missing per-provider config and the 1-based provider index).
Emulator: returns 400 but with 'name must be 1-65536 characters matching ^[_A-Za-z][_0-9A-Za-z]*$, got "sim2real-probe-appsync-cfg-oidc-389579a0"' — the name check fires first and masks the provider validation entirely.
Why: a client debugging a malformed auth provider is told its API name is invalid, which is a false diagnosis; and once the name check is removed the provider validation must still produce the 400, so it has to be implemented rather than merely unmasked.
Fix: CreateGraphqlApi's input validation — remove the name check and add the per-provider openIDConnectConfig / userPoolConfig / lambdaAuthorizerConfig presence check.
Traces: appsync-cfg-graphql-api-oidc-auth-provider#task#1 (+5 affected: task#2, task#3, task#4, appsync-graphql-api-core-lifecycle#task#5, appsync-graphql-api-crud-lifecycle#task#5).

Cause 4 of 5: ListGraphqlApis returns an empty list after a create (fixture appsync-graphql-api-and-type-lifecycle, step task#3; 4 steps in cluster).
AWS: Smithy: ListGraphqlApis http {GET /v1/apis, code 200}, output.graphqlApis targets GraphqlApis; a previously created API is listed, with the same apiId/arn/name/uris/dns the create returned.
Emulator: 200 with $.graphqlApis[0] missing (got None) — the list is empty.
Why: a client that creates then enumerates cannot see its own API, so discovery, pagination and filter behavior are all unobservable.
Fix: CreateGraphqlApi state (the created API is never persisted) and ListGraphqlApis' handler. Hypothesis: downstream of Cause 1 — the create in these fixtures 400s, so there is nothing to list; verify by re-replaying this step after the create path is fixed, and only then treat any residual emptiness as a ListGraphqlApis state defect.
Traces: appsync-graphql-api-and-type-lifecycle#task#3 (+3 affected: appsync-graphql-api-lifecycle#task#6, appsync-list-graphql-apis-baseline#task#2, appsync-list-graphql-apis-paginated-and-filtered#task#2).

Cause 5 of 5: CreateGraphqlApi's response omits graphqlApi.dns (fixture appsync-graphql-api-create-delete-not-found, step task#1).
AWS: Smithy: GraphqlApi.dns targets MapOfStringToString, documented 'The DNS records for the API.'; the recording shows it populated with REALTIME and GRAPHQL hostnames, and $.graphqlApi.uris built from those same hostnames (REALTIME = wss://<realtime-host>/graphql, GRAPHQL = https://<graphql-host>/graphql). The hostnames themselves are environment-generated and are not a requirement — the recording asserts them as placeholders.
Emulator: 200, but $.graphqlApi.dns is missing (got None) while uris is present, so the two members disagree about what the API's endpoints are.
Why: clients and CDK/CloudFormation providers read graphqlApi.dns to configure custom domains and to derive the realtime endpoint; an absent map makes those reads fail, and the uris assertions that are keyed off dns cannot be satisfied either.
Fix: CreateGraphqlApi's serializer / response builder — emit dns with the REALTIME and GRAPHQL keys derived from the same generated host used to build uris.
Traces: appsync-graphql-api-create-delete-not-found#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (91 fixtures, 736 steps, 45 passed, score 6.1%).

Cause 1 — appsync-api-cache-lifecycle#setup#1, CreateGraphqlApi: expected status 200, got 400; $.graphqlApi: missing (expected the GraphqlApi structure with name 'sim2real-probe-appsync-cacheapi-b0d89c65', apiId, arn, uris, dns, visibility GLOBAL, apiType GRAPHQL, introspectionConfig ENABLED; got None). affected_count 35.
Cause 2 — appsync-api-cache-update-flush-delete-chain#setup#1, CreateGraphqlApi: status: expected 200, got 400. affected_count 37.
Cause 3 — appsync-cfg-graphql-api-oidc-auth-provider#task#1, CreateGraphqlApi: expected 400 / got 400, $.message value_mismatch: expected 'OpenID Connect Config is missing for additional authentication provider 1', got 'name must be 1-65536 characters matching ^[_A-Za-z][_0-9A-Za-z]*$, got "sim2real-probe-appsync-cfg-oidc-389579a0"'. affected_count 6.
Cause 4 — appsync-graphql-api-and-type-lifecycle#task#3, ListGraphqlApis: expected 200 / got 200, $.graphqlApis[0] missing (expected the created GraphqlApi, got None). affected_count 4.
Cause 5 — appsync-graphql-api-create-delete-not-found#task#1, CreateGraphqlApi: expected 200 / got 200, $.graphqlApi.dns missing (expected {'REALTIME': <realtime-host>, 'GRAPHQL': <graphql-host>}, got None); $.graphqlApi.uris.REALTIME and $.graphqlApi.uris.GRAPHQL mismatch against the hosts bound from dns.

Model evidence: CreateGraphqlApiRequest.name -> String (no pattern, no length); GraphqlApi.name -> ResourceName (@length 1-65536, @pattern ^[_A-Za-z][_0-9A-Za-z]*$); GraphqlApi.dns -> MapOfStringToString.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /v1/apis with {"name": "sim2real-probe-appsync-cacheapi-b0d89c65", "authenticationType": "API_KEY"} (fixture appsync-api-cache-lifecycle, step setup#1). Expect 200 with a full graphqlApi body; observe 400 and no graphqlApi.
3. Cause 2: replay appsync-api-cache-update-flush-delete-chain step setup#1 — same POST /v1/apis, expect 200, observe 400.
4. Cause 3: replay appsync-cfg-graphql-api-oidc-auth-provider step task#1 — POST /v1/apis with an additionalAuthenticationProviders entry of type OPENID_CONNECT and no openIDConnectConfig; expect the 400 to name the missing OIDC config for provider 1, observe a name-pattern message instead.
5. Cause 4: replay appsync-graphql-api-and-type-lifecycle step task#3 — GET /v1/apis after creating an API; expect graphqlApis[0] to be the created API, observe an empty list.
6. Cause 5: replay appsync-graphql-api-create-delete-not-found step task#1 — POST /v1/apis; expect graphqlApi.dns to carry REALTIME and GRAPHQL hostnames consistent with graphqlApi.uris, observe dns absent.
