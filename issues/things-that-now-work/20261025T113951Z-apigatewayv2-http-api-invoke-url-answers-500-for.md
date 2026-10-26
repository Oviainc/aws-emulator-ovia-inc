Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves apigatewayv2.create-stage-and-deployment apigatewayv2.invoke-http-api-httpproxy apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized apigatewayv2.disable-portal-revokes-access-server-side
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served|run#20|AssertionError|assert (status, text) == (200, marker), (
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves|run#19|AssertionError|assert twin_status == 200, f"GET /twin should serve 200, got {twin_status}: {twin_text[:200]!r}"
Issue-Key: apigatewayv2.create-stage-and-deployment|_poll_http#15|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.invoke-http-api-httpproxy|_get_until_200#12|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized|_poll_http#17|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.disable-portal-revokes-access-server-side|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# apigatewayv2: HTTP API invoke URL answers 500 for proxy routes, and three readiness probes never converge (3 causes)

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three distinct net-new root causes on the apigatewayv2 data plane. They are NOT covered by the open issue 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md (nor by 20261025T030504Z-apigatewayv2-an-http-api-s-invoke-url-never.md): the replay report did not mark any of these six checks as owned by them, and in Cause 1 the execute-api endpoint DOES answer — it answers 500 — which is a different defect from "nothing serves". A fixer must close all three before submitting.

Cause 1 of 3: an HTTP API AWS_PROXY (Lambda proxy) route answers 500 instead of the integration's response.
AWS: recorded behavior only for the data plane (execute-api has no Smithy model). The recordings show a deployed HTTP API route whose AWS_PROXY integration targets a Lambda function returning {"statusCode": 200, body: <marker>} serves HTTP 200 with that body, and that a route with AuthorizationType=NONE serves an UNSIGNED request. Smithy (apigatewayv2 control plane) grounds the setup: CreateIntegration input IntegrationType (enum, required) with PayloadFormatVersion, TimeoutInMillis @range 50..30000; CreateStage AutoDeploy.
Emulator: both checks get HTTP 500 with body {"message":"Internal Server Error"} from the invoke URL. In the AWS_IAM check the unsigned request to GET /open (AuthorizationType=NONE, marker backend) returns 500 instead of 200+marker; in the integration-timeout check the control route GET /twin (TimeoutInMillis=30000, same backend) returns 500 instead of 200. So the listener accepts the request, resolves a route, and then fails internally rather than invoking the integration — the 503-on-timeout behaviour under test is never reached because its positive control is already broken.
Why: a client gets an unconditional 5xx from every HTTP API backed by a Lambda proxy integration. No application can be exercised against the emulator, and 500 is indistinguishable from a genuine backend fault, so callers retry a request AWS would have served.
Fix: the execute-api HTTP data-plane request handler for AWS_PROXY integrations — route resolution to Lambda invocation (event construction and the proxy-response translation back to an HTTP response), plus its error mapping, which must not collapse an internal failure into 500 for a well-formed request.
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served ("GET /open (AuthorizationType=NONE) must serve an unsigned request with marker ...; got 500: '{\"message\":\"Internal Server Error\"}'"); apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves ("GET /twin should serve 200, got 500"). Handler area: services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/.

Cause 2 of 3: an invoke URL never reaches the expected status, so three data-plane checks hang to the harness cap.
AWS: recorded behavior only. A stage created with CreateStage(DeploymentId=<id>) at creation time, and a $default stage with AutoDeploy=true, both serve their routes' integrations through the stage invoke URL within seconds; an HTTP_PROXY route serves the upstream body with 200; an AWS_IAM-protected route answers an unsigned request 403 {"message":"Forbidden"}.
Emulator: each check polls its invoke URL for the expected status (90 s / 120 s poll budgets) and never gets it, so the check is killed by the 60 s runner cap with "Check timed out after 60.0s". A correct target satisfies the first probe, so the timeout is the emulator's: the endpoint never produces the expected status (consistent with Cause 1's unconditional 500, or with nothing answering). Hypothesis: one and the same data-plane gap underlies Cause 1 and Cause 2, but they are listed separately because the evidence differs (observed 500 vs no observed response at all) and fixing the 500 alone would not prove these three converge.
Why: every workflow that creates an API/route/stage and then calls it — the normal use of API Gateway — never completes. The AuthorizationType gate (403 for unsigned on AWS_IAM) and CORS preflight ordering are unobservable.
Fix: the execute-api HTTP data-plane listener and its stage/deployment wiring (a stage must serve the routes of the deployment it names, including $default+AutoDeploy), plus the HTTP_PROXY integration forwarder and the AWS_IAM authorization step in front of route dispatch.
Traces: apigatewayv2.create-stage-and-deployment (polls GET <invoke>/items for 200 on both $default and a stage created with DeploymentId); apigatewayv2.invoke-http-api-httpproxy (polls the invoke URL for 200 from an HTTP_PROXY upstream); apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized (polls GET /orders for 403 on an AWS_IAM ANY /{proxy+} route before any CORS assertion).

Cause 3 of 3: a published portal's default domain never serves, so DisablePortal's server-side effect cannot be observed.
AWS: recorded behavior only (the portal website has no Smithy model; the control plane ops are modeled: PublishPortal/DisablePortal/GetPortal with PublishStatus, and GetPortal.EndpointConfiguration.PortalDefaultDomainName). The recording shows PublishPortal drives PublishStatus to PUBLISHED and the portal's PortalDefaultDomainName then serves HTTP 200, and after DisablePortal the same URL stops serving 200 and stays non-200.
Emulator: the check is killed at the 60 s cap before its first assertion. Its blocking polls are (a) PublishStatus == PUBLISHED and (b) a 200 from the portal default domain. Hypothesis: the emulator serves no portal website at PortalDefaultDomainName at all, so poll (b) spins forever; a non-converging PublishStatus would produce the same symptom and should be checked at the same time.
Why: a caller cannot tell whether DisablePortal actually took a developer portal offline. An emulator that only flips PublishStatus while the site (or no site) is unchanged lets a security-relevant revocation silently pass.
Fix: the portal data plane (serving PortalDefaultDomainName) and the PublishPortal/DisablePortal state transitions that gate it.
Traces: apigatewayv2.disable-portal-revokes-access-server-side ("Check timed out after 60.0s"). Handler area: services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/ (publish_portal.rs, disable_portal.rs, get_portal.rs).

## Evidence
Cause 1 — expected: GET /open (AuthorizationType=NONE, AWS_PROXY to a Lambda returning 200+marker) => 200 with the marker body; GET /twin (TimeoutInMillis=30000) => 200. Actual: both => 500 with body {"message":"Internal Server Error"}.
Cause 2 — expected: first probe of the stage invoke URL returns the asserted status (200 for the $default/explicit-DeploymentId stages and for the HTTP_PROXY route; 403 {"message":"Forbidden"} for the unsigned request on the AWS_IAM route). Actual: the poll loop never observes it and the check is terminated: "Error: Check timed out after 60.0s" for create-stage-and-deployment, invoke-http-api-httpproxy and http-api-cors-preflight-caught-by-any-route-is-authorized.
Cause 3 — expected: after PublishPortal, PublishStatus becomes PUBLISHED and GET https://<PortalDefaultDomainName>/ returns 200; after DisablePortal it returns non-200 and stays non-200 for a 30 s hold window. Actual: "Error: Check timed out after 60.0s" before the first assertion was reached.

## Steps to Reproduce
Cause 1: CreateApi(ProtocolType=HTTP); create a Lambda function returning {"statusCode":200,"body":"<marker>"}; CreateIntegration(IntegrationType=AWS_PROXY, IntegrationUri=<fn arn>, PayloadFormatVersion=2.0, TimeoutInMillis=30000); CreateRoute(RouteKey="GET /open", AuthorizationType=NONE, Target=integrations/<id>); CreateStage(StageName="$default", AutoDeploy=true); then GET <invoke-url>/open unsigned. Observe 500 {"message":"Internal Server Error"} instead of 200 and the marker.
Cause 2: (a) same HTTP API with an HTTP_PROXY integration to any reachable upstream, CreateStage($default, AutoDeploy=true) and CreateStage(<name>, DeploymentId=<CreateDeployment id>), then GET both invoke URLs — neither returns 200; (b) with CorsConfiguration set and an AWS_IAM "ANY /{proxy+}" route, GET <invoke-url>/orders unsigned — never 403.
Cause 3: CreatePortal(Authorization={None:{}}, EndpointConfiguration={None:{}}, PortalContent=...); PublishPortal; poll GetPortal.PublishStatus for PUBLISHED; GET https://<GetPortal.EndpointConfiguration.PortalDefaultDomainName>/ — never 200; then DisablePortal and re-probe.
Replay: run_behavioral_suite and re-run the six check ids in trace_ids.

## Status note (Cause 3 resolved; Causes 1 and 2 remain, with their blockers named)

**Cause 3 (the portal data plane) is implemented.** A portal is now served on the address its own
`EndpointConfigurationResponse` reports, and that address is one the client that created the portal
can reach: a create addressed at this service's AWS host is announced on the generated
`{portal-id}.portal.{region}.amazonaws.com` host, and a create addressed at the EMULATOR's own
endpoint — the only address a client on this machine resolves — is announced on that endpoint,
behind the same reserved path marker this emulator's API Gateway (REST) stages are reached at
(`crate::state::announced_portal_default_domain_name`). Whichever it is, the build the portal's last
SETTLED `PublishPortal` made is served there as the portal's page (200, rendered from that build's
stored `PortalContent`, logo and included portal products with their product pages and product REST
endpoint pages), a portal with no live publication — never published, publish still building, or
disabled — serves `403` `{"message":"Forbidden"}`, a path the portal has no page at is `404`
`{"message":"Not Found"}`, and the preview `PreviewPortal` builds is served at the path
`Preview$PreviewUrl` announces once it is ready
(`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/portal.rs`, plus
`Portal::live_site` / `PublishedSite` in `src/state.rs`). So `DisablePortal`'s server-side effect is
observable: the domain stops serving 200 and stays non-200. Pinned end to end by
`tests/all/portal_data_plane.rs`, and the "answers on the FIRST probe" property every one of these
checks depends on by `tests/all/probe.rs`.

Replay of `disable-portal-revokes-access-server-side` now gets PAST both of its blocking polls — the
`PublishStatus` poll and the poll for a 200 from the portal's domain — and reaches the check's own
30 s hold window (`run#15`, `time.sleep(10)`), where the suite's 60 s runner cap ends it. Nothing in
this emulator is waited on there: the publish and the disable settle in `PORTAL_BUILD` (500 ms).

**Cause 2 is two thirds closed by the data plane that already landed** (replay: the
`create-stage-and-deployment` and `invoke-http-api-httpproxy` keys now pass). Its remaining check,
`http-api-cors-preflight-caught-by-any-route-is-authorized`, cannot pass from this repository: the
GET half it probes for is answered `403` on the first probe (pinned in `tests/all/probe.rs`, first
row: an `AWS_IAM` `ANY /{proxy+}` route on an api carrying a `CorsConfiguration`, probed unsigned),
but a CORS PREFLIGHT never reaches an emulated service at all — the core listener answers every
`OPTIONS` request itself before the handler runs (`vendor/server/src/listeners.rs`, the preflight
branch of `serve_http`), and `vendor/` is read-only here. No route of an api can therefore catch a
preflight, which is the property that check is about.

Not done here, and worth its own report: `Api$ApiEndpoint` still announces the AWS host for every
api, however the control plane was addressed. The same "announce what the caller can reach" rule
would apply, but that value is parsed by other services as an AWS invoke host — see
`20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md`, which is
exactly that defect for the REST data plane's `_aws` form — so changing it belongs in a change that
can fix those parsers with it.

**Cause 1 (an `AWS_PROXY` route answering the function's response) is NOT fixed and is not
re-worded.** A Lambda proxy route still answers `500 {"message":"Internal Server Error"}` — the
honest mapping for an integration that produced no response — because no Lambda deployment package
can run in this emulator (`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`
answers 501 for `RequestResponse`). Nothing short of the function actually running can close it: a
200 and a marker body invented for a backend that never ran would be indistinguishable from a real
one, and the `503`-on-timeout behaviour the second check is about needs an integration that really
ran. The runtime is owned by
`issues/things-that-do-not-work/20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`
and
`issues/things-that-do-not-work/20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`,
and the dispatch half (proxy-event construction and response mapping) by
`issues/things-that-do-not-work/20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md`,
which carries this same cause for three other checks. The two replay keys of this report's Cause 1
stay attributed to that report.

**Cause 1 (an `AWS_PROXY` route answering the function's response) is NOT fixed and is not
re-worded.** A Lambda proxy route still answers `500 {"message":"Internal Server Error"}` — the
honest mapping for an integration that produced no response — because no Lambda deployment package
can run in this emulator (`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`
answers 501 for `RequestResponse`). Nothing short of the function actually running can close it: a
200 and a marker body invented for a backend that never ran would be indistinguishable from a real
one, and the `503`-on-timeout behaviour the second check is about needs an integration that really
ran. The runtime is owned by
`issues/things-that-do-not-work/20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`
and
`issues/things-that-do-not-work/20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`,
and the dispatch half (proxy-event construction and response mapping) by
`issues/things-that-do-not-work/20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md`,
which carries this same cause for three other checks. The two replay keys of this report's Cause 1
stay attributed to that report.
