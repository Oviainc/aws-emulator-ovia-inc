Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__apigatewayv2.httpapi-stage-default-throttle-returns-default-too-many-requests
Issue-Key: apigateway__apigatewayv2.httpapi-stage-default-throttle-returns-default-too-many-requests|run#31|AssertionError|assert served_during_burst, (
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# apigateway__apigatewayv2: an HTTP API $default stage throttle sheds EVERY request of a burst — a rate=1/burst=1 DefaultRouteSettings stage answers 429 to all 40 concurrent requests and serves none

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: an apigatewayv2 HTTP API stage configured with DefaultRouteSettings throttling turns into a total outage under load instead of a rate limiter — it refuses every request of the burst.

AWS: Smithy apigatewayv2 RouteSettings carries ThrottlingRateLimit (double, "Specifies the throttling rate limit") and ThrottlingBurstLimit (integer, "Specifies the throttling burst limit"); CreateStage accepts them as DefaultRouteSettings for the stage. A token-bucket throttle with rate=1 request/second and burst=1 admits the requests the bucket holds and sheds only the excess: in a 40-way concurrent burst at least one request is SERVED by the route's integration (here an HTTP_PROXY to a REST MOCK backend returning {"backend":"served"}) and the rest get HTTP 429 with the default THROTTLED body {"message":"Too Many Requests"}. Once the bucket refills the stage serves normally again. Recorded behavior only for the exact body text, which the emulator already matches.

Emulator: for the same stage every one of the 40 concurrent GETs is shed — the burst yields rejection-only results, with no 200 at all. The 429 body itself is correct ({"message":"Too Many Requests"}, asserted before this assertion and passed), and a single unburst GET before the burst is served, so the defect is the shedding DECISION under concurrency: the limiter admits zero requests rather than burst-limit-many, i.e. it behaves as a gate that closes on the first concurrent arrival instead of a bucket.

Why: a client that sizes a stage's throttle to protect a backend (and a test that asserts partial shedding) sees a full outage: every caller gets 429 while the limit is nominally 1 req/s, so no traffic at all flows through a throttled stage and a retry/backoff client cannot make progress. It also makes the inverse defect invisible — a consumer cannot distinguish "throttled correctly" from "stage broken".

Fix: the apigatewayv2 data-plane throttle evaluation for a stage's DefaultRouteSettings (the token-bucket accounting that decides admit-vs-shed per request) in the HTTP API invoke path of the apigatewayv2/execute-api data plane; the 429 body rendering is already right.

Traces: apigateway__apigatewayv2.httpapi-stage-default-throttle-returns-default-too-many-requests.

## Evidence
Replay (apigateway__apigatewayv2.httpapi-stage-default-throttle-returns-default-too-many-requests):
```
AssertionError: a throttle that sheds EVERY request in the burst is not throttling, it is broken; got rejection-only results for <stage invoke URL>/
```
Expected: in the burst that first sheds a request, the 200 results are non-empty and each carries the backend body {"backend":"served"}; the 429 results carry exactly {"message":"Too Many Requests"}; afterwards a GET recovers with 200 once the bucket refills.
Actual: of 40 concurrent GETs issued against the $default stage of an HTTP API whose DefaultRouteSettings are ThrottlingRateLimit=1.0 / ThrottlingBurstLimit=1, ALL were 429 and none was served. (The URL/host/api-id in the message are per-run values, not part of the requirement.)

## Steps to Reproduce
1. Create a REGIONAL REST API with a MOCK GET on / returning {"backend":"served"}, deploy it to a stage — this is the HTTP_PROXY backend.
2. apigatewayv2 CreateApi(ProtocolType="HTTP"); CreateIntegration(IntegrationType="HTTP_PROXY", IntegrationMethod="GET", IntegrationUri=<backend stage URL>, PayloadFormatVersion="1.0"); CreateRoute(RouteKey="GET /", Target=integrations/<id>).
3. CreateStage(StageName="$default", AutoDeploy=true, DefaultRouteSettings={ThrottlingRateLimit: 1.0, ThrottlingBurstLimit: 1}).
4. GET the stage URL once: it is served 200 {"backend":"served"}.
5. Issue 40 concurrent GETs to the same URL. Expected: some 429 {"message":"Too Many Requests"} AND at least one 200 {"backend":"served"}. Actual: all 40 answer 429.
6. Wait for the bucket to refill and GET again: it must be served 200.
