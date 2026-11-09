Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.api-cache-refuses-update-and-delete-while-creating appsync.create-graphql-api-limit-exceeded appsync.op-contract-create-graphql-api appsync.start-schema-creation-conflict-in-progress
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.api-cache-refuses-update-and-delete-while-creating|run#7|AssertionError|assert not pending, (
Issue-Key: appsync.create-graphql-api-limit-exceeded|run#7|AssertionError|assert limit_error == expected, (
Issue-Key: appsync.op-contract-create-graphql-api|run#11|AssertionError|assert not (p := ctx.published_url_mismatch(created["uris"]["GRAPHQL"])), p
Issue-Key: appsync.start-schema-creation-conflict-in-progress|run#8|AssertionError|assert message == _CONFLICT_MESSAGE, (
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# AppSync: 4 net-new causes — ApiCache CREATING-state gate missing, published GraphQL endpoint URIs unreachable, and two refusal messages emulator-authored

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Four independent net-new divergences in the appsync control plane, all grounded in failing behavioral checks of this service. Each must be closed before this issue is submitted.

Cause 1 of 4: UpdateApiCache (and therefore DeleteApiCache) is not refused while the API cache is still CREATING.
AWS: an ApiCache that has not reached AVAILABLE refuses reconfiguration and deletion. UpdateApiCache answers 400 BadRequestException with the fixed sentence "The cache is not yet available. Please wait until it becomes available." and DeleteApiCache answers 400 BadRequestException with the fixed sentence "Cannot delete cache as it is either not available or its creation failed."; both sentences are entirely fixed text and carry no request or state values (recorded behavior only for the wording; Smithy: UpdateApiCache and DeleteApiCache both model BadRequestException httpError 400, and ApiCacheStatus enumerates AVAILABLE, CREATING, DELETING, MODIFYING, FAILED, so the pre-AVAILABLE states exist precisely to gate these calls).
Emulator: with GetApiCache read immediately before the call still reporting status CREATING, UpdateApiCache(ttl=600, apiCachingBehavior=PER_RESOLVER_CACHING, type=SMALL) is ACCEPTED (200, no error) and the cache is then read as MODIFYING. This happened on 3 of 3 freshly created caches, so DeleteApiCache's CREATING-state refusal is never reachable either. (The out-of-range ttl=7200 refusal, which is checked before the state gate, does fire.)
Why: a client that relies on AWS serialising cache mutations has its reconfiguration applied in a state AWS rejects, so its wait/retry loop never runs and the resulting cache configuration is undefined; a negative test for the two refusals cannot pass.
Fix: the ApiCache state gate in the UpdateApiCache and DeleteApiCache handlers — reject when the stored cache status is not AVAILABLE, before applying the change or removing the cache.
Traces: appsync.api-cache-refuses-update-and-delete-while-creating. Related open issue that does NOT carry this behavioral check: 20261029T200818Z-appsync-apicache-refusal-wording-encryption-flags-and-applied.md (its cause 6 names the same UpdateApiCache gate from the wire corpus; the DeleteApiCache gate is not in it).

Cause 2 of 4: CreateGraphqlApi past the per-Region GraphQL API quota answers the right error class with an emulator-authored message.
AWS: 400 ApiLimitExceededException with the fixed sentence "The service limit for number of GraphQL APIs has been reached." — the sentence is entirely fixed text; it does NOT embed the quota number (Smithy: CreateGraphqlApi errors include ApiLimitExceededException httpError 400, documented "The GraphQL API exceeded a limit. Try your request again."; the exact refusal sentence is recorded behavior only).
Emulator: 400 ApiLimitExceededException with "an account may hold at most 25 GraphQL APIs in a Region" — the handler formats its own sentence from its quota constant.
Why: the message is a requirement; a client or test that surfaces or asserts the AWS sentence sees text that matches no AWS wording.
Fix: the CreateGraphqlApi handler's quota refusal must carry AWS's fixed sentence instead of a formatted constant.
Traces: appsync.create-graphql-api-limit-exceeded (the emulator's quota branch is in services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_graphql_api.rs around lines 232-241).

Cause 3 of 4: the GraphqlApi URIs/DNS the emulator publishes name AWS hostnames the emulator does not answer on.
AWS: GraphqlApi$uris (Smithy doc: "The URIs.") and GraphqlApi$dns publish the hostnames the API is actually served on — on AWS those are <host-label>.appsync-api.<region>.amazonaws.com and <host-label>.appsync-realtime-api.<region>.amazonaws.com. The requirement for an emulator is that the published address is an address the API is reachable at, which is exactly what this service already does for an Event API (state::event_api_dns publishes the socket the API is served on).
Emulator: CreateGraphqlApi publishes uris.GRAPHQL = https://<label>.appsync-api.<region>.amazonaws.com/graphql and uris.REALTIME = wss://<label>.appsync-realtime-api.<region>.amazonaws.com/graphql, hosts that are not any of the emulator's own hosts; the check's published-URL grading refuses them with "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<path>".
Why: the GraphQL data plane is unreachable for every client that does what the API tells it to do — it dials a public AWS hostname instead of the emulator — so queries, mutations, subscriptions and any SigV4/api-key data-plane flow built from the create response cannot run at all.
Fix: the GraphQL API endpoint publication (state::api_dns, used by CreateGraphqlApi and the uris projection) must publish the host:port the emulator serves that API's GraphQL and realtime endpoints on, as event_api_dns already does for Event APIs (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state.rs api_dns, around lines 2989-3000; the call site is create_graphql_api.rs line 264).
Traces: appsync.op-contract-create-graphql-api. The same root cause blocks appsync__dynamodb.appsync-dynamodb-unit-resolver-round-trips-item and appsync__cloudwatch.obs-appsync-2-realtime-connect-rejected-counts-client-error, filed separately because they are listed under other service headings.

Cause 4 of 4: StartSchemaCreation's in-flight conflict carries the generic error-shape sentence instead of the schema-creation sentence.
AWS: a second StartSchemaCreation while the API's previous schema-creation job is PROCESSING is refused 409 ConcurrentModificationException with the fixed sentence "Schema is currently being altered, please wait until that is complete." — entirely fixed text, no request or state values (Smithy: StartSchemaCreation errors include ConcurrentModificationException httpError 409; the per-operation sentence is recorded behavior only).
Emulator: the correct code (ConcurrentModificationException) with the message "Another modification is in progress at this time and it must complete before you can make your change." — which is verbatim the ConcurrentModificationException shape's model documentation, i.e. a shared default message rather than the operation's own.
Why: the message is a requirement; a client surfacing or branching on the refusal text cannot tell a schema-creation conflict from any other concurrent-modification refusal, and operator-facing output diverges from AWS.
Fix: the StartSchemaCreation handler's single-flight refusal must author AWS's schema-creation sentence rather than falling back to the error shape's documentation text.
Traces: appsync.start-schema-creation-conflict-in-progress. Related open issue that does NOT cover it: 20261109T022338Z-appsync-schema-creation-plane-diverges-success-detail-text.md (SUCCESS detail text and the unknown-apiId class, not the conflict message).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (130/419 passed, bootstrap_signal false — the emulator serves).

Cause 1 — [mismatch] appsync.api-cache-refuses-update-and-delete-while-creating:
  AssertionError: never observed the refusal of ['UpdateApiCache', 'DeleteApiCache'] while the cache was CREATING in 3 fresh caches: ["<apiId>: UpdateApiCache answered 'accepted' after the cache left CREATING ('MODIFYING')", ... x3]
  i.e. GetApiCache read status CREATING, UpdateApiCache was then accepted, and the following read returned MODIFYING. Expected: 400 BadRequestException 'The cache is not yet available. Please wait until it becomes available.' (UpdateApiCache) and 400 BadRequestException 'Cannot delete cache as it is either not available or its creation failed.' (DeleteApiCache).

Cause 2 — [mismatch] appsync.create-graphql-api-limit-exceeded:
  AssertionError: expected ('ApiLimitExceededException', 'The service limit for number of GraphQL APIs has been reached.') once the account/region GraphQL API ceiling is reached (created 25 APIs before this point), got ('ApiLimitExceededException', 'an account may hold at most 25 GraphQL APIs in a Region')

Cause 3 — [mismatch] appsync.op-contract-create-graphql-api:
  AssertionError: the emulator published https://<label>.appsync-api.us-west-2.amazonaws.com/graphql: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
  (the host label, region and port are per-run values; the requirement is only that the published host is one the emulator answers on.)

Cause 4 — [mismatch] appsync.start-schema-creation-conflict-in-progress:
  AssertionError: expected message 'Schema is currently being altered, please wait until that is complete.', got 'Another modification is in progress at this time and it must complete before you can make your change.'
  (the error code ConcurrentModificationException and HTTP 409 are correct; only the message diverges.)

## Steps to Reproduce
1. Clone the repository and boot the emulator built from it.
2. Replay simparity-behavioral-aws-v1; the four checks under Traces fail.
3. Cause 1 by hand: CreateGraphqlApi, CreateApiCache(ttl=300, FULL_REQUEST_CACHING, SMALL); while GetApiCache reports status CREATING call UpdateApiCache(ttl=600, PER_RESOLVER_CACHING, SMALL) and DeleteApiCache. Expect 400 BadRequestException with the two sentences above; the emulator accepts the update with 200 and moves the cache to MODIFYING.
4. Cause 2 by hand: CreateGraphqlApi 25 times in one account/Region, then once more. Expect 400 ApiLimitExceededException 'The service limit for number of GraphQL APIs has been reached.'; the emulator answers 'an account may hold at most 25 GraphQL APIs in a Region'.
5. Cause 3 by hand: CreateGraphqlApi(authenticationType=API_KEY) and read graphqlApi.uris.GRAPHQL / graphqlApi.dns, then POST a GraphQL query to that URI. The published host is <label>.appsync-api.<region>.amazonaws.com, which the emulator does not serve; it must publish the emulator's own host:port for that API.
6. Cause 4 by hand: CreateGraphqlApi, StartSchemaCreation with a large SDL (e.g. 300 types) so the job stays PROCESSING, confirm GetSchemaCreationStatus reports PROCESSING, then call StartSchemaCreation again. Expect 409 ConcurrentModificationException 'Schema is currently being altered, please wait until that is complete.'; the emulator answers the generic 'Another modification is in progress...' sentence.
