Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.oauth-connection-non-https-authorization-endpoint-rejected events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.oauth-connection-non-https-authorization-endpoint-rejected|run#8|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header|_create_connection#5|Timeout|time.sleep(3)
Issue-Commit: 74cfda6b1b1701e11661fc7b5d0b64a8c57e063b

# EventBridge connections: CreateConnection's non-HTTPS AuthorizationEndpoint refusal message is rewritten, and an OAUTH_CLIENT_CREDENTIALS api-destination never fetches a token or issues the event request (2 causes)

> _Note: this issue was identified against the code base at commit `74cfda6b1b1701e11661fc7b5d0b64a8c57e063b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
Two independent root causes on the EventBridge connection / api-destination surface. Neither failing check is marked ALREADY FILED or OWNED by the replay report. Related open issues that do NOT carry these replay keys: 20261025T084539Z-events-eventbridge-api-destinations-and-connections-diverge-no-http.md (its six causes name CreateApiDestination/UpdateApiDestination/DeleteConnection/UpdateConnection/ListConnections and the generic api-destination delivery gap, and it lists the oauth check among its trace ids, but the replay key it recorded for it — `_receive#17|Timeout` — no longer matches the observed failure, which is now a top-level 60 s hang, so the report left this check unowned) and 20261025T002357Z-events-eventbridge-refusal-codes-and-messages-diverge-from-aws.md (does not name CreateConnection's AuthorizationEndpoint validation). Both are named here so a fixer can de-duplicate rather than re-discover.

Cause 1 of 2: CreateConnection does refuse a plain-http OAuth AuthorizationEndpoint, but with the wrong parameter name and the wrong message template.
AWS: Smithy: CreateConnection input {Name @required, AuthorizationType @required (enum ConnectionAuthorizationType), AuthParameters @required (CreateConnectionAuthRequestParameters)}; modeled errors AccessDeniedException, InternalException, LimitExceededException, ResourceAlreadyExistsException, ResourceNotFoundException, ThrottlingException — ValidationException is a service-common refusal and the model states no constraint on the OAuth AuthorizationEndpoint scheme, so the scheme rule is recorded behavior only. Recorded behavior: AWS refuses with HTTP 400, error code `ValidationException`, message `Failed to create the connection(s). Parameter AuthorizationEndpoint is not valid. Reason: Endpoint '<AuthorizationEndpoint exactly as submitted>' is invalid, please provide a valid HTTPS endpoint URL.` In that message the fixed text is `Failed to create the connection(s). Parameter AuthorizationEndpoint is not valid. Reason: Endpoint '` … `' is invalid, please provide a valid HTTPS endpoint URL.`; the only variable part is the AuthorizationEndpoint value taken verbatim from the request, wrapped in single quotes. Nothing is created under that name, so a follow-up DescribeConnection answers ResourceNotFoundException with `Failed to describe the connection(s). Connection '<Name as submitted>' does not exist.`
Emulator: the status and the code are right (400 / `ValidationException`) but the message is a different template: `Parameter AuthParameters is not valid. Reason: AuthorizationEndpoint "<endpoint>" is not an https endpoint.` Four separate divergences in one string — the `Failed to create the connection(s).` operation prefix is dropped; the parameter named is the container `AuthParameters` instead of the offending member `AuthorizationEndpoint`; the endpoint value is wrapped in double quotes instead of single quotes; and the reason clause is `is not an https endpoint.` instead of `is invalid, please provide a valid HTTPS endpoint URL.`
Why: the refusal does not name the member that was actually invalid, so a client (or a CloudFormation/CDK provider) that surfaces or parses `Parameter <X> is not valid` to point a user at the bad field points at the whole AuthParameters blob, and any consumer matching AWS's documented refusal text fails to classify the error at all. The missing operation prefix also breaks the uniform `Failed to <verb> the <resource>(s).` prefix the rest of this API uses.
Fix: the AuthorizationEndpoint scheme validation in the CreateConnection handler of the eventbridge service — it must build the refusal from the offending member name and the submitted endpoint value using AWS's template. The sibling path in UpdateConnection should be checked for the same rewritten template.
Traces: events.oauth-connection-non-https-authorization-endpoint-rejected.

Cause 2 of 2: an OAUTH_CLIENT_CREDENTIALS connection never performs the client-credentials token exchange, and the api-destination target therefore never issues the event request.
AWS: recorded behavior only — the Smithy model describes the connection/destination resources but not the delivery mechanics. Recorded/documented behavior: when a rule's target Arn is an api-destination whose connection has AuthorizationType `OAUTH_CLIENT_CREDENTIALS`, EventBridge first POSTs to the connection's `AuthorizationEndpoint` using its `HttpMethod`, carrying the connection's OAuthHttpParameters (QueryStringParameters on the query string, HeaderParameters as request headers) and a `application/x-www-form-urlencoded` body whose fields are exactly `client_id`, `client_secret` and any BodyParameters (here `scope`) — client_id and client_secret taken from ClientParameters as submitted. The connection then becomes AUTHORIZED, and the event request to the destination's InvocationEndpoint carries `Authorization: Bearer <access_token from the token response>` with the EventBridge envelope as the JSON body and no client secret anywhere in it.
Emulator: PutTargets accepts the api-destination target (FailedEntryCount 0) and PutEvents accepts the entry (FailedEntryCount 0), but the receiving endpoint records no request at all — neither a `/token` request nor the `/api` event request — for the whole window, so the check hangs until the 60 s harness cap. The token exchange and the delivery are both absent.
Why: every webhook/third-party integration built on an OAuth api-destination silently does nothing: the connection never reaches AUTHORIZED through a real token fetch, no Bearer credential is ever minted, and no event is delivered — while every control-plane call reports success, so a client has no signal that delivery is dead.
Fix: the api-destination branch of the eventbridge rule-target dispatcher plus the connection authorization state machine — the OAUTH_CLIENT_CREDENTIALS grant (token request construction, OAuthHttpParameters merge, token caching/refresh) and the Bearer-header application on the outbound invocation.
Traces: events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header.

## Evidence
Cause 1 — events.oauth-connection-non-https-authorization-endpoint-rejected [mismatch]:
  check_events_oauth_connection_non_https_authorization_endpoint_rejected.py, line 38, in run
  assert (code, message) == ("ValidationException", expected)
  AssertionError: an http AuthorizationEndpoint must be refused, got 'ValidationException' 'Parameter AuthParameters is not valid. Reason: AuthorizationEndpoint "http://sim2real.invalid/oauth/token" is not an https endpoint.'
Expected (AWS): code `ValidationException`, message `Failed to create the connection(s). Parameter AuthorizationEndpoint is not valid. Reason: Endpoint '<submitted AuthorizationEndpoint>' is invalid, please provide a valid HTTPS endpoint URL.`
Actual (emulator): code `ValidationException`, message `Parameter AuthParameters is not valid. Reason: AuthorizationEndpoint "<submitted AuthorizationEndpoint>" is not an https endpoint.`
The endpoint URL shown is the value the request supplied, not part of the template; the fixed text is everything around it.
The check's later assertions (DescribeConnection ResourceNotFoundException wording, and ListConnections showing only the https connection) were never reached, so only the refusal message is graded here.

Cause 2 — events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header [mismatch]: `Check timed out after 60.0s`.
The check creates an HTTPS capture endpoint, create_connection(AuthorizationType='OAUTH_CLIENT_CREDENTIALS', AuthParameters={'OAuthParameters': {ClientParameters, AuthorizationEndpoint=<base>/token, HttpMethod='POST', OAuthHttpParameters={BodyParameters: scope=orders/write, HeaderParameters: x-oauth-h=oh, QueryStringParameters: aq=a1}}}), create_api_destination(InvocationEndpoint=<base>/api), a role granting events:InvokeApiDestination, a custom bus, a rule on {"source":[<source>]} and put_targets with the destination ARN. PutTargets returns FailedEntryCount 0 and each PutEvents returns FailedEntryCount 0 with an EventId.
Expected: a POST to <base>/token with query `aq=a1`, header `x-oauth-h: oh`, `content-type: application/x-www-form-urlencoded` and a form body whose keys are exactly ['client_id','client_secret','scope'] with the submitted ClientID/ClientSecret and scope; then a request to <base>/api with `Authorization: Bearer <access_token>`, the EventBridge envelope as body (source/detail-type/detail matching the PutEvents entry) and the client secret absent.
Actual: zero requests captured at the endpoint — no /token request and no /api request — so the check never reaches any assertion and is killed at the 60 s cap.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and create an `events` client.
2. Call create_connection(Name='<any>', AuthorizationType='OAUTH_CLIENT_CREDENTIALS', AuthParameters={'OAuthParameters': {'ClientParameters': {'ClientID': 'c', 'ClientSecret': 's'}, 'AuthorizationEndpoint': 'http://example.invalid/oauth/token', 'HttpMethod': 'POST'}}).
3. Inspect Error.Code (expect ValidationException — correct) and Error.Message: it is `Parameter AuthParameters is not valid. Reason: AuthorizationEndpoint "<endpoint>" is not an https endpoint.` instead of `Failed to create the connection(s). Parameter AuthorizationEndpoint is not valid. Reason: Endpoint '<endpoint>' is invalid, please provide a valid HTTPS endpoint URL.`
4. Repeat with an https AuthorizationEndpoint to confirm the positive control is accepted, and with update_connection to check the same template there.

Cause 2:
1. Stand up an HTTPS endpoint that records each request (method, path, query, headers, body) and answers a path ending in /token with {"access_token": "<tok>", "token_type": "Bearer", "expires_in": 3600}.
2. create_connection(AuthorizationType='OAUTH_CLIENT_CREDENTIALS', AuthParameters={'OAuthParameters': {'ClientParameters': {...}, 'AuthorizationEndpoint': '<base>/token', 'HttpMethod': 'POST', 'OAuthHttpParameters': {'BodyParameters': [{'Key':'scope','Value':'orders/write'}], 'HeaderParameters': [{'Key':'x-oauth-h','Value':'oh'}], 'QueryStringParameters': [{'Key':'aq','Value':'a1'}]}}}).
3. create_api_destination(ConnectionArn=<conn arn>, InvocationEndpoint='<base>/api', HttpMethod='POST'); create an IAM role trusted by events.amazonaws.com allowing events:InvokeApiDestination on the destination ARN.
4. create_event_bus; put_rule(EventPattern={"source":["<source>"]}); put_targets(Targets=[{Id:'api', Arn:<destination arn>, RoleArn:<role arn>}]).
5. put_events(Entries=[{EventBusName:<bus>, Source:'<source>', DetailType:'oauth', Detail:'{"n":1}'}]) repeatedly.
6. Observe that the capture endpoint records no /token request and no /api request.
Replay both check ids in trace_ids to confirm a fix.
