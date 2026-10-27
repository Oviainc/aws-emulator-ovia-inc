Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group
Issue-Key: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet|_wait_until#8|Timeout|time.sleep(interval_s)
Issue-Key: lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group|_wait_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 903918b9e4ba7ba7fbbe78a54de34750c5938ee3

# lambda-microvms: RunMicrovm publishes a MicroVM endpoint URL that the emulator serves nothing on, so every MicroVM data-plane check hangs to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `903918b9e4ba7ba7fbbe78a54de34750c5938ee3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunMicrovm

## Description
Service attribution note: both grounding checks carry the COMPOSITE leading token `lambda-core__lambda-microvms` (their `SERVICES` list is ["lambda-core", "lambda-microvms"]), so no single leading token joins the bootstrap sets by exact equality. Neither constituent is pending — `service-com-amazonaws-lambdacore-lambdacoreapiservice` and `service-com-amazonaws-lambdamicrovms-lambdamicrovms` are both bootstrapped — and the diverging operation belongs to lambda-microvms, so the issue is filed under `lambda-microvms`. No relabelling was needed to pass the gate.

Cause 1 of 1: a RUNNING MicroVM's published HTTPS endpoint is not served by the emulator, so no request to the guest ever gets an HTTP response.

AWS: `RunMicrovm` (POST /2025-09-09/microvms) models `endpoint` as a **required** output member — Smithy: `RunMicrovmResponse$endpoint` (String, @length 1..2048, required), documented as "The HTTPS endpoint URL for communicating with the MicroVM". `CreateMicrovmAuthToken` (POST /2025-09-09/microvms/{microvmIdentifier}/auth-token) models `authToken` as a **required** `TokenParts` map (@length 1..10) whose entries are the headers a client presents to that endpoint; the recorded behavior shows the key `X-aws-proxy-auth`. In AWS, once `GetMicrovm` reports `state == RUNNING`, an HTTP GET to `<endpoint><path>` carrying that header is proxied into the guest and returns the guest's own HTTP response (any status — the proxy answers 403 on a bad/expired token, and the guest's 200 otherwise). That endpoint IS the data plane the two grounding checks exercise: they start a guest HTTP server on port 8080 and read `/health` and `/connect?host=<ip>&port=443` through it.

Emulator: `RunMicrovm` mints the endpoint purely as a string from the region and the generated MicroVM id — `format!("https://{microvm_id}.microvm.lambda.{region}.amazonaws.com")` — and nothing in the emulator listens for, routes, or proxies requests to that host or to any path under it. `CreateMicrovmAuthToken` likewise mints a token that authenticates to nothing. Consequently every GET through the endpoint fails at the transport layer (`OSError`) rather than returning any HTTP status. Both checks wrap that GET in a bounded 180 s retry loop (`_guest_get`, 5 s sleeps, breaks on ANY HTTP response including an error status), so with no response ever arriving the loop runs past the suite's 60 s per-check budget and the suite reports `Error: Check timed out after 60.0s` for both. Note this is not a slow-state-settle problem: the control plane settles promptly (lambdacore `LIFECYCLE_TRANSITION` = 500 ms for PENDING -> ACTIVE; lambdamicrovms `IMAGE_BUILD_SPAN` / `MICROVM_PROVISION_SPAN` = 500 ms), and both checks assert the MicroVM reached `RUNNING` before they touch the endpoint — so the emulator reports a RUNNING MicroVM and hands back an endpoint it does not answer on. The published host is also the REAL AWS hostname shape rather than an address the emulator is reached on, which is a divergence in its own right: a client that does not rewrite endpoints is pointed off-box.

Why: a client relying on AWS's behavior treats `endpoint` as a usable address for a RUNNING MicroVM. Against the emulator it gets a connect/resolve failure with no HTTP status and no error code, so it cannot distinguish "MicroVM not ready yet" from "wrong token" (403) from "guest not listening", and any workflow that invokes a MicroVM — the entire point of the service — cannot run. It also makes the connector contract unverifiable end-to-end: because the endpoint never answers, the assertions these checks were written for were never reached, namely (a) that a MicroVM attached to an ACTIVE VPC_EGRESS connector can reach a private address in the connector's subnet, from a link-local (169.254.0.0/16) source rather than a customer-subnet address, while an identically built MicroVM attached only to `INTERNET_EGRESS` cannot and ends `timed_out`; and (b) that the destination security group is live on the connector's path, so revoking/adding the rule flips reachability and a blocked connect times out rather than being refused. Those remain untested here — whether `egressNetworkConnectors` is merely stored rather than enforced cannot be determined while the endpoint is unserved.

Fix: the MicroVM data plane belongs in the lambda-microvms service's request routing/data-plane layer plus the `RunMicrovm` handler that publishes the address. Two parts: (1) `RunMicrovm` must publish an endpoint the emulator actually answers on, instead of a real-AWS hostname derived only from id and region; (2) the emulator must route requests on that endpoint, authenticate the `X-aws-proxy-auth` header minted by `CreateMicrovmAuthToken` (403 on an absent/expired/port-disallowed token, per that operation's `allowedPorts` and `expirationInMinutes`), and serve the MicroVM guest so a GET returns an HTTP status rather than a transport failure. Connector enforcement (`egressNetworkConnectors` as a network path, not a stored list) belongs in the same data-plane layer once it is reachable. Component file citations, for orientation only: `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs` (`microvm_endpoint`, ~line 1244-1248; `Microvm::endpoint`, ~line 710) and `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/run_microvm.rs`.

Traces: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet; lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group — both reported by the behavioral suite as `[mismatch] ... Error: Check timed out after 60.0s`.

## Evidence
Suite report (simparity-behavioral-ga-prod-v1), service group `lambda-core__lambda-microvms`, 0/2 passed:

  [mismatch] lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet
  Error: Check timed out after 60.0s

  [mismatch] lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group
  Error: Check timed out after 60.0s

Expected (AWS): after `GetMicrovm` reports state RUNNING, `GET <RunMicrovm.endpoint>/health` with header `X-aws-proxy-auth: <CreateMicrovmAuthToken.authToken["X-aws-proxy-auth"]>` returns HTTP 200 with the guest's body (`{"healthy": true}`), and `GET <endpoint>/connect?host=<private-ip>&port=443` returns HTTP 200 with the guest's socket view — `{"connected": true, "outcome": "connected", "peer": [<target ip>, 443], "local": [<169.254.x.y>, <port>]}` through an ACTIVE VPC_EGRESS connector, and `{"connected": false, "outcome": "timed_out", ...}` with no `local` member when there is no connector path into the VPC. The fixed text here is the JSON member names and the `outcome` vocabulary (`connected` / `timed_out` / `refused` / `unreachable`); the addresses, ports and ids are values taken from the request and the emulator's state, not literals.

Actual (emulator): `RunMicrovm` answers 200 with `endpoint = https://<microvmId>.microvm.lambda.<region>.amazonaws.com` (from `state::microvm_endpoint`, which composes it from the region and the minted id and nothing else) and `GetMicrovm` reports RUNNING ~500 ms later (`MICROVM_PROVISION_SPAN`). No listener or route in the emulator serves that endpoint, so each GET raises a transport-level `OSError` with NO HTTP status and NO error code at all. `_guest_get`'s bounded retry (180 s budget, 5 s interval, which only ends on an HTTP response of ANY status) therefore never terminates inside the suite's 60 s per-check budget, and both checks are killed with `Check timed out after 60.0s`.

Status/code/message divergence summary: expected an HTTP status on the data plane (200 from the guest, or 403 from the proxy for a bad token); got no HTTP response at all. The endpoint host announced is the real AWS hostname rather than the address the emulator is reached on.

## Steps to Reproduce
1. Point an AWS client at the emulator and, in one region:
   a. ec2: CreateVpc (10.90.0.0/16), CreateSubnet (10.90.1.0/24) in an AZ that offers com.amazonaws.<region>.lambda, CreateSecurityGroup, AuthorizeSecurityGroupIngress tcp/443 from the VPC CIDR.
   b. ec2: CreateVpcEndpoint VpcEndpointType=Interface, ServiceName=com.amazonaws.<region>.lambda, PrivateDnsEnabled=false, in that subnet/group; wait for State=available and read the single ENI's PrivateIpAddress as <target-ip>.
   c. iam: CreateRole trusted by lambda.amazonaws.com, AttachRolePolicy AWSLambdaVPCAccessExecutionRole -> <operator-role>.
   d. lambda-core: CreateNetworkConnector Name=<label>, Configuration.VpcEgressConfiguration={SubnetIds:[<subnet>], SecurityGroupIds:[<group>], NetworkProtocol:IPv4, AssociatedComputeResourceTypes:[MicroVm]}, OperatorRole=<operator-role>; poll GetNetworkConnector until State=ACTIVE.
2. lambda-microvms: build a guest image (CreateMicrovmImage from a managed base image + an s3 code artifact whose Dockerfile starts an HTTP server on 0.0.0.0:8080 answering /health and /connect) and poll GetMicrovmImage until state=CREATED.
3. lambda-microvms: RunMicrovm imageIdentifier=<image arn>, maximumDurationInSeconds=900, ingressNetworkConnectors=[arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:HTTP_INGRESS], egressNetworkConnectors=[<connector arn>]. Record the returned `endpoint`. Poll GetMicrovm until state=RUNNING.
4. lambda-microvms: CreateMicrovmAuthToken microvmIdentifier=<microvmId>, expirationInMinutes=15, allowedPorts=[{allPorts:{}}]; take authToken["X-aws-proxy-auth"].
5. HTTP GET `<endpoint>/health` with header `X-aws-proxy-auth: <token>`.
   Expected: HTTP 200 and the guest's JSON body.
   Observed: no HTTP response at all — the connection fails at the transport layer, because the announced host `https://<microvmId>.microvm.lambda.<region>.amazonaws.com` is not an address the emulator serves. Repeating as the checks do (5 s apart, 180 s budget) never yields a status, so the check is killed by the 60 s suite cap.
6. Same GET against `<endpoint>/connect?host=<target-ip>&port=443`, once for a MicroVM whose egressNetworkConnectors is the ACTIVE VPC_EGRESS connector and once for an otherwise identical MicroVM whose egress is the managed INTERNET_EGRESS connector: both should answer HTTP 200 with differing `outcome` values (connected vs timed_out); neither answers at all.
