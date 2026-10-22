Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.container-service-deployment-reaches-running
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.container-service-deployment-reaches-running|_fetch_url_body#12|Timeout|time.sleep(interval_s)
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Lightsail container service public endpoint answers 501, so a deployment that reaches RUNNING never serves its container and the check cannot complete

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetContainerServices

## Description
One root cause, grounded in one failing behavioral check.

Cause 1 of 1: the published public URL of a RUNNING container service whose currentDeployment declares a publicEndpoint is answered with HTTP 501 and a plain-text emulator notice instead of a response from the deployed container. Failing check: lightsail.container-service-deployment-reaches-running.

AWS: Smithy (services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json), ContainerService$url — "The publicly accessible URL of the container service. If no public endpoint is specified in the currentDeployment, this URL returns a 404 response." So the model states the contract in both directions: with NO publicEndpoint in currentDeployment the URL answers 404, and with one declared the URL is "publicly accessible" — a GET of it is served by the container that fronts the deployment and returns that container's own successful response (for the recorded check, an HTTP 200 from the nginx:latest image). The specific page body (the nginx welcome HTML) is live-run content produced by a real container runtime and is NOT itself a requirement; the requirement is that a GET of the announced url for a deployment with a declared publicEndpoint is NOT answered with a server-side not-implemented status.

Emulator: CreateContainerService/GetContainerServices do stand up the control plane correctly — the service walks PENDING -> DEPLOYING -> RUNNING and the waiting deployment becomes ACTIVE (services/.../src/ops/create_container_service.rs, src/state.rs owed-phase clock). The announced url points at the emulator's own endpoint under /lightsail-container-service/<region>/<name>/<token>, and that path IS claimed (Service::can_handle) and served (Service::override_routing_error -> serve_container_service_endpoint in src/service.rs). But when the current deployment declares a publicEndpoint that handler returns status 501 with the constant body "This container service's public endpoint is not implemented: the emulator runs no container runtime, so the deployed image's response cannot be served." The behavioral check therefore polls the url until the harness kills it ("Check timed out after 60.0s"), because urllib retries the 501 for its whole 60s budget.

Why: a client that reads ContainerService$url and follows it — the documented use of the field, and what any container-service smoke test or health probe does — gets a 501 from the emulator where AWS gives it the container's response. Nothing distinguishes this from a transient server fault, so retry/readiness logic built against Lightsail either loops forever or reports the service unhealthy even though the control plane reports state RUNNING and currentDeployment ACTIVE. The data-plane half of the container service capability is thus unusable even when the control plane is fully faithful.

Fix: the container-service public data plane in services/service-com-amazonaws-lightsail-lightsail-20161128/src/service.rs (EmulatedService::serve_container_service_endpoint, reached from override_routing_error), plus whatever deployment-backed response source it needs from src/state.rs (StoredContainerService::current_deployment / its publicEndpoint and health-check configuration). The 404 branch for a currentDeployment with no publicEndpoint already matches the model and should stay; the declared-publicEndpoint branch is the one that must stop answering a not-implemented status. If no container runtime will ever back it, this is the capability gap to record and bound explicitly rather than surface as 501 on a promise the control plane just made.

Traces: lightsail.container-service-deployment-reaches-running (mismatch; harness timeout at 60.0s while polling ContainerService$url).

## Evidence
Failing check: lightsail.container-service-deployment-reaches-running — verdict [mismatch], "Error: Check timed out after 60.0s".

Check body (abridged): create_container_service(serviceName=..., power='nano', scale=1, deployment={containers:{web:{image:'nginx:latest', ports:{'80':'HTTP'}}}, publicEndpoint:{containerName:'web', containerPort:80}}) -> poll get_container_services until state == 'RUNNING' -> assert currentDeployment.containers.web.image == 'nginx:latest' and currentDeployment.state == 'ACTIVE' -> HTTP GET service['url'] and assert 'Welcome to nginx' in body, retrying for up to 60s.

Expected (AWS, per ContainerService$url documentation): the announced url is publicly accessible and is served by the deployment's public endpoint container; a GET returns that container's own successful response (HTTP 200 for nginx:latest). Only when currentDeployment declares no publicEndpoint does the url return 404.

Actual (emulator): services/service-com-amazonaws-lightsail-lightsail-20161128/src/service.rs, serve_container_service_endpoint:
  Some(true) => endpoint_response(method, 501, NO_CONTAINER_RUNTIME)
with NO_CONTAINER_RUNTIME = "This container service's public endpoint is not implemented: the emulator runs no container runtime, so the deployed image's response cannot be served." Every GET of the announced url therefore answers 501 text/plain; the check's retry loop never succeeds and the harness terminates the check at its 60s cap.

Note on scope: the exact nginx page body is live-run content and is cited only as the recorded example. The filed divergence is the HTTP status of the public endpoint (501 server-side not-implemented vs. the container's successful response), not the message text.

## Steps to Reproduce
1. Boot the emulator and point a Lightsail client at it.
2. ls.create_container_service(serviceName='<name>', power='nano', scale=1, deployment={'containers': {'web': {'image': 'nginx:latest', 'ports': {'80': 'HTTP'}}}, 'publicEndpoint': {'containerName': 'web', 'containerPort': 80}}).
3. Poll ls.get_container_services(serviceName='<name>') until containerServices[0].state == 'RUNNING' (the emulator settles there within ~1s; currentDeployment.state becomes 'ACTIVE').
4. Read containerServices[0]['url'] and issue an HTTP GET to it.
5. Observe HTTP 501 with body "This container service's public endpoint is not implemented: ...". Against AWS the same GET is served by the deployed container (HTTP 200 from nginx:latest). A client retrying the url, as the behavioral check does, never gets a served response.
