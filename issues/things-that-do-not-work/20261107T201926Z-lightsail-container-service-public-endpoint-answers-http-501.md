Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.container-service-deployment-reaches-running
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.container-service-deployment-reaches-running|_fetch_url_body#12|AssertionError|raise AssertionError(f"container endpoint {url} never became reachable, last error: {last_error!r}")
Issue-Commit: 4eae4810055cf0c39415b7b36ad02401c0d0be96

# Lightsail container service public endpoint answers HTTP 501 on the url it just announced, so a RUNNING deployment never serves its container

> _Note: this issue was identified against the code base at commit `4eae4810055cf0c39415b7b36ad02401c0d0be96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetContainerServices

## Description
One root cause, grounded in one failing behavioral check.

NOTE: the open issue 20261022T170908Z-lightsail-container-service-public-endpoint-answers-501-so.md describes the same data-plane gap, but it does not carry this check's replay key (it was keyed on a harness timeout while polling the url; the check now fails on an explicit HTTPError 501 raised from the GET), so the suite did not mark this failure as owned and it is filed here. If that issue is closed by the same change, close this one with it.

Cause 1 of 1: the published public URL of a RUNNING container service whose currentDeployment declares a publicEndpoint is answered with HTTP 501 instead of a response produced by the deployment's public-endpoint container. Failing check: lightsail.container-service-deployment-reaches-running.

AWS: Smithy (services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json), ContainerService$url — "The publicly accessible URL of the container service. If no public endpoint is specified in the currentDeployment, this URL returns a 404 response." The model therefore states the contract in both directions: with NO publicEndpoint declared in currentDeployment a GET of the url answers 404; with one declared, the url is publicly accessible and a GET of it is answered by the container fronting the deployment with that container's own successful response (a 2xx). The specific page content is whatever the deployed image serves — it is live-run content, not a requirement. The requirement is that a GET of the announced url for a deployment with a declared publicEndpoint is NOT answered with a server-side not-implemented status.

Emulator: the control plane is faithful — CreateContainerService/GetContainerServices walk the service PENDING -> DEPLOYING -> RUNNING and settle currentDeployment to ACTIVE with the requested container image, and the announced url points at the emulator's own endpoint under /lightsail-container-service/<region>/<serviceName>/<token>, a path the emulator does claim and route. But the handler for that path, when the current deployment declares a publicEndpoint, answers HTTP 501 ("Not Implemented") with a constant text/plain notice that no container runtime is available, for every request. The check's retry loop over the url therefore records only <HTTPError 501: 'Not Implemented'> for its whole budget and fails with "container endpoint <announced url> never became reachable".

Why: a client that reads ContainerService$url and follows it — the documented use of the field, and exactly what a container-service smoke test, readiness probe or load-balancer health check does — gets a 501 from the emulator where AWS gives it the container's response. A 501 is indistinguishable from a transient server fault, so retry/readiness logic written against Lightsail either loops until its own deadline or marks the service unhealthy even though the control plane reports state RUNNING and currentDeployment ACTIVE. The data-plane half of the container-service capability is unusable even though the control plane just promised it.

Fix: the container-service public data plane in services/service-com-amazonaws-lightsail-lightsail-20161128/src/service.rs (the container-service endpoint handler reached from the service's routing override), plus whatever deployment-backed response source it needs from src/state.rs (the stored container service's current deployment, its publicEndpoint and its health-check configuration). The 404 branch for a currentDeployment with no declared publicEndpoint already matches the model and must stay; the declared-publicEndpoint branch is the one that must stop answering a not-implemented status and instead serve a successful response on behalf of the deployment. If no container runtime will ever back it, this is a capability gap to bound explicitly rather than surface as a 501 on a URL the control plane announced as publicly accessible.

Traces: lightsail.container-service-deployment-reaches-running.

## Evidence
Failing check: lightsail.container-service-deployment-reaches-running — verdict [mismatch].

Error (verbatim tail): File "check_lightsail_container_service_deployment_reaches_running.py", line 89, in _fetch_url_body / AssertionError: container endpoint https://<emulator-host:port>/lightsail-container-service/us-west-2/<serviceName>/<token> never became reachable, last error: <HTTPError 501: 'Not Implemented'>

(The host, port, generated service name and URL token differ per run and are not part of the requirement.)

Check body (abridged): create_container_service(serviceName=..., power='nano', scale=1, deployment={containers: {web: {image: 'nginx:1.30', ports: {'80': 'HTTP'}}}, publicEndpoint: {containerName: 'web', containerPort: 80}}) -> poll get_container_services until state == 'RUNNING' -> assert currentDeployment.containers.web.image == 'nginx:1.30' and currentDeployment.state == 'ACTIVE' (these all pass) -> HTTP GET of containerServices[0]['url'], retried for up to 60s.

Expected (AWS, per ContainerService$url documentation): with a publicEndpoint declared in currentDeployment, the announced url is publicly accessible and a GET is answered by the deployment's public-endpoint container with that container's own successful (2xx) response. Only when currentDeployment declares no publicEndpoint does the url answer 404.

Actual (emulator): every GET of the announced url answers HTTP 501 with a constant text/plain notice stating the emulator runs no container runtime, so the deployed image's response cannot be served. No status other than 501 is ever observed, so the polling client never gets a served response.

Scope note: the nginx welcome page body the check greps for is live-run content of the deployed image and is cited only as the recorded example. The filed divergence is the HTTP status of the announced public endpoint — a server-side 501 instead of the container's successful response.

## Steps to Reproduce
1. Boot the emulator and point a Lightsail client at it.
2. ls.create_container_service(serviceName='<name>', power='nano', scale=1, deployment={'containers': {'web': {'image': 'nginx:1.30', 'ports': {'80': 'HTTP'}}}, 'publicEndpoint': {'containerName': 'web', 'containerPort': 80}}).
3. Poll ls.get_container_services(serviceName='<name>') until containerServices[0]['state'] == 'RUNNING'; observe currentDeployment.state == 'ACTIVE' and currentDeployment.containers.web.image == 'nginx:1.30' (all correct).
4. Read containerServices[0]['url'] and issue an HTTP GET to it (the URL is the emulator's own endpoint, path /lightsail-container-service/<region>/<name>/<token>).
5. Observe HTTP 501 'Not Implemented' with the constant no-container-runtime body, on every attempt. Against AWS the same GET is answered by the deployed container with a 2xx response; only a currentDeployment with no publicEndpoint answers 404.
