Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.container-service-deployment-reaches-running
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.container-service-deployment-reaches-running|_fetch_url_body#12|EgressRefused|with urllib.request.urlopen(url, timeout=10.0) as resp:
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# Lightsail container service announces an unreachable cs.amazonlightsail.com url and serves no container data plane

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetContainerServices

## Description
One root cause, Cause 1 of 1.

Cause 1 of 1: `ContainerService$url` is synthesized as a real-AWS public domain, so the endpoint the emulator advertises is not an address the emulator can be reached on, and no container data plane answers there.

AWS: Smithy (services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json) models `ContainerService$url` (target `string`) as "The publicly accessible URL of the container service. If no public endpoint is specified in the currentDeployment, this URL returns a 404 response." `CreateContainerService$serviceName` documents the default-domain form `https://<service>.<token>.<region>.cs.amazonlightsail.com`. The contract that matters to a client is that the URL returned in `url` is actually reachable and, when the current deployment declares a `publicEndpoint`, serves that container's HTTP response (here the nginx image's default page); only a missing public endpoint is modeled as producing a 404. The textual shape of the hostname is NOT a requirement — it is environment-specific — the reachability of the announced address is.

Emulator: `CreateContainerService`/`GetContainerServices` report the control-plane record correctly (the service does transition PENDING -> DEPLOYING -> RUNNING, `currentDeployment.state` is ACTIVE and the container image round-trips), but `url` is built unconditionally from the real AWS suffix: `services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs` defines `CONTAINER_SERVICE_DOMAIN_SUFFIX = "cs.amazonlightsail.com"` and `container_service_url(name, token, region) -> format!("https://{name}.{token}.{region}.{CONTAINER_SERVICE_DOMAIN_SUFFIX}")`, which `StoredContainerService::view()` puts into `url`. Replaying the check, a client that follows the returned `url` is sent to `sim2real-ls-cs-<id>.<token>.us-west-2.cs.amazonlightsail.com` instead of the emulator endpoint (127.0.0.1), and the run is refused: `simparity.egress.EgressRefused: egress refused: '...cs.amazonlightsail.com' is not the endpoint under test (127.0.0.1); ... the emulator announces an address other than the endpoint it is reached on`. Nothing in the emulator serves the deployed container on any address.

Why: a client that does what AWS clients do with this field — read `containerServices[].url` and make an HTTP request to it to reach the deployed container — leaves the emulator entirely. Against real AWS the request returns the container's response (or a modeled 404 when there is no public endpoint); against the emulator it resolves to a public AWS DNS name that the account does not own, so the call fails with a DNS/connection error (or, in a sandboxed run, is refused as egress). Any integration that smoke-tests a deployment through its public endpoint, or that templates the service URL into downstream configuration, is unusable.

Fix: belongs in the container-service state/view layer and its data plane: `container_service_url` and `StoredContainerService::view()` in services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs must derive the announced host from the endpoint the emulator is actually served on (the request's own host/authority, as the other endpoint-returning resources must), instead of a hard-coded `cs.amazonlightsail.com`, and the emulator needs a handler bound to that announced address that answers the current deployment's `publicEndpoint` container (and the modeled 404 when the current deployment declares none).

Traces: lightsail.container-service-deployment-reaches-running (the check reaches RUNNING and asserts on `service["url"]`; it fails at the fetch of that URL).

Note: the open issue 20261022T050650Z-lightsail-key-pair-control-plane-createkeypair-and-deletekeypair.md covers the key-pair mismatches and does not cover this; the scaffold-stub issues filed for GetBlueprints/CreateBucket/CreateDisk/CreateDomainEntry/CreateKeyPair/GetContainerImages/GetRelationalDatabaseBundles do not cover this either — CreateContainerService and GetContainerServices are implemented and answered here.

## Evidence
Check lightsail.container-service-deployment-reaches-running.

Expected (AWS): CreateContainerService(serviceName=<name>, power=nano, scale=1, deployment={containers:{web:{image:nginx:latest, ports:{"80":"HTTP"}}}, publicEndpoint:{containerName:web, containerPort:80}}) then GetContainerServices -> state RUNNING, currentDeployment.state ACTIVE, and an HTTP GET of containerServices[0].url returns the container's body (nginx default page). The URL returned is an address the caller can actually reach; only a deployment with no publicEndpoint yields a 404 there.

Actual (emulator): the control-plane record is right (states seen PENDING -> DEPLOYING -> RUNNING, currentDeployment.containers.web.image == nginx:latest, currentDeployment.state == ACTIVE), but `url` is `https://sim2real-ls-cs-09ef5f81328d.SUpAJW0MgDn1o.us-west-2.cs.amazonlightsail.com` — a real-AWS public hostname, not the endpoint under test. Fetching it fails before any HTTP exchange:

  File "socket.py", line 983, in getaddrinfo
  File "egress.py", line 81, in _hook
  simparity.egress.EgressRefused: egress refused: 'sim2real-ls-cs-09ef5f81328d.SUpAJW0MgDn1o.us-west-2.cs.amazonlightsail.com' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on

(The specific host/token/region above are one run's values and are not the requirement; the requirement is that the announced address be the emulator's own and that it serve the deployment's public endpoint.)

Source of the divergence: services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:1779 (CONTAINER_SERVICE_DOMAIN_SUFFIX), :1801-1803 (container_service_url), :1937-1941 (StoredContainerService::view sets url).

## Steps to Reproduce
1. Point a botocore lightsail client at the emulator endpoint.
2. create_container_service(serviceName="ls-cs-demo", power="nano", scale=1, deployment={"containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}}, "publicEndpoint": {"containerName": "web", "containerPort": 80}}).
3. Poll get_container_services(serviceName="ls-cs-demo") until containerServices[0]["state"] == "RUNNING".
4. Read containerServices[0]["url"] -> it is an https://<name>.<token>.<region>.cs.amazonlightsail.com address, not the emulator's own endpoint.
5. HTTP GET that url -> DNS resolution leaves the emulator (egress refused / name resolution failure); no container response is served. Expected: the announced URL is on the emulator's endpoint and returns the nginx default page.
