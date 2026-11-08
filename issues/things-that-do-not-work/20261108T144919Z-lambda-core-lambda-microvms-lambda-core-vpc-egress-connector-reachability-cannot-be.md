Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group
Issue-Key: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet|_guest_get#25|AssertionError|assert status == 200, (
Issue-Key: lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group|_guest_get#26|AssertionError|assert status == 200, (
Issue-Commit: 5bdc422e571ff089197010dd8b23a6f4d1e72235

# lambda-core VPC-egress connector reachability cannot be observed: the MicroVM endpoint answers HTTP 501 instead of the guest's response, so egressNetworkConnectors is never enforced as a network path

> _Note: this issue was identified against the code base at commit `5bdc422e571ff089197010dd8b23a6f4d1e72235`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunMicrovm

## Description
One net-new root cause found by replaying the behavioral suite. CAUSE COUNT: 1.

NOTE FOR THE FIXER: open issue 20261108T130022Z-lambda-core-lambda-microvms-lambda-core-vpc-egress-connector-data-plane-is.md describes this same 501 from these same two checks, but the suite does not report these checks as OWNED by it, i.e. it does not carry their replay keys. Treat this as that issue's replay binding rather than a second defect: fixing one fixes both, and both of the check ids below must be re-replayed before either is closed. 20261103T220031Z-lambda-microvms-6-net-new-root-causes-deletemicrovmimage.md (Cause 4) describes the same 501 from lambda-microvms-only checks; 20261107T201003Z-lambda-microvms-microvm-endpoint-listener-presents-an-untrusted.md is about the listener's TLS material, not its HTTP answer; 20261103T161149Z-lambda-core-lambda-microvms-vpc-egress-data-plane.md is about lifecycle convergence, not the endpoint's answer.

--- Cause 1 of 1: the MicroVM proxy endpoint serves no guest response (HTTP 501), so the VPC_EGRESS network-connector data path cannot be exercised ---

AWS: RunMicrovmResponse$endpoint (Smithy: com.amazonaws.lambdamicrovms#RunMicrovm, member `endpoint` String @length 1..2048 @required) is "The HTTPS endpoint URL for communicating with the MicroVM. Include a valid authentication token in the X-aws-proxy-auth header when sending requests", and CreateMicrovmAuthToken (POST /2025-09-09/microvms/{microvmIdentifier}/auth-token; input expirationInMinutes PositiveInteger @range min 1 @required, allowedPorts ListOfPortSpecification @required; output authToken TokenParts @required) mints the value for that header. The model says nothing further about the proxied exchange, so this is recorded behavior only: the request is proxied into the MicroVM and the caller receives the GUEST's own HTTP response — HTTP 200 with the guest's body (here JSON from /health and /connect?host=<ip>&port=<port>, the latter reporting one outbound TCP connect as connected/outcome/peer/local). RunMicrovm's `egressNetworkConnectors` (NetworkConnectorList @length max 10, "The list of egress network connectors to configure for the MicroVM") combined with a lambda-core network connector carrying a NetworkConnectorVpcEgressConfiguration is what gives the guest a path into the customer VPC, and the guest's response is the only way a client can observe that path. No error code is involved: an authenticated request to a RUNNING MicroVM answers 2xx with whatever the guest sent.

Emulator: every authenticated GET to the published endpoint of a RUNNING MicroVM answers HTTP 501 with the body `This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n` — that whole sentence is the emulator's own fixed text, nothing in it comes from the request, and AWS has no counterpart for it. Both replays got as far as an ACTIVE VPC_EGRESS connector, a settled image, a RUNNING MicroVM with the expected echoed egressNetworkConnectors and a minted allPorts token, and then the data-plane read returned 501. Because no guest response is ever served, the connector data plane is unverifiable: the emulator never has to show that a MicroVM on an ACTIVE VPC_EGRESS connector reaches a private address in the connector's subnet, that the guest's source address on that path is link-local (169.254.0.0/16) rather than an address inside the customer subnet, that a MicroVM attached only to INTERNET_EGRESS times out on that same address, or that revoking the target security group's ingress rule turns a reachable path back into a silent drop. egressNetworkConnectors is stored and echoed, never enforced as a network path.

Why: a client buys the connector for network isolation. Against this emulator the permitted arm and the denied arm return the identical 501, so it cannot tell an enforced path from an unenforced one, cannot distinguish "my guest is broken" from "the platform refuses", and cannot read its guest's response at all.

Fix: the MicroVM endpoint listener's request handler in the lambda-microvms service state (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state/microvm_endpoint.rs, which short-circuits every authenticated request to the NO_MICROVM_GUEST 501), together with the connector-attachment path that must make a MicroVM's egressNetworkConnectors decide whether a proxied outbound connection to an address in the connector's subnet connects, times out, or is dropped by the target security group. RunMicrovm's and GetMicrovm's handlers already publish the endpoint; the defect is behind it.

Traces: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet, lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group.

## Evidence
Suite: 20/41 passed (48.8%). Per-service: lambda-core__lambda-microvms 0/2 (0%), both failing identically; lambda-microvms 20/39 (51%).

[mismatch] lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet
  check_..._microvm_denied_the_vpc_egress_connector_cannot_reach_the_private_subnet.py line 657, in _guest_connect -> line 648, in _guest_get: assert status == 200
  AssertionError: GET '/connect?host=10.90.1.4&port=443' on microvm mvm-0000000000000001 answered HTTP 501: b"This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n"

[mismatch] lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group
  check_..._vpc_egress_connector_path_is_blocked_by_the_target_security_group.py line 691, in _guest_connect -> line 683, in _guest_get: assert status == 200
  AssertionError: GET '/connect?host=10.91.1.4&port=443' on microvm mvm-0000000000000000 answered HTTP 501: b"This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n"

  expected: an authenticated GET (X-aws-proxy-auth from CreateMicrovmAuthToken with allowedPorts=[{allPorts:{}}]) to the RunMicrovm-published `endpoint` of a RUNNING MicroVM is proxied to the guest and returns HTTP 200 with the guest's own body — for the MicroVM on the ACTIVE VPC_EGRESS connector, {"connected": true, "outcome": "connected", "peer": [<target address>, <port>], "local": [<link-local source inside 169.254.0.0/16>, <port>]}; for a MicroVM attached only to INTERNET_EGRESS, or when the target security group admits nothing, {"connected": false, "outcome": "timed_out", ...} with no "local" member; and /health answering {"healthy": true} in both.
  actual: HTTP 501 with the emulator's fixed sentence for every path (/health and /connect alike).

  Everything upstream succeeded in both checks (CreateVpc/CreateSubnet/CreateSecurityGroup/CreateVpcEndpoint, CreateNetworkConnector settling ACTIVE, CreateMicrovmImage settling CREATED/SUCCESSFUL, RunMicrovm settling RUNNING with the expected egressNetworkConnectors, CreateMicrovmAuthToken minting a token), so the 501 is the emulator's answer and not a runner or environment failure. The per-run values 10.90.1.4, 10.91.1.4 and mvm-0000000000000000/1 are not requirements.
  Source: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state/microvm_endpoint.rs:284 (NO_MICROVM_GUEST 501 returned for every authenticated request).

## Steps to Reproduce
Against the emulator (lambda-core and lambda-microvms both stood up):

1. EC2: create a VPC (e.g. 10.90.0.0/16) with one subnet, a security group admitting TCP on the target port from the VPC CIDR, and one interface VPC endpoint in that subnet with PrivateDnsEnabled=false; wait for state=available and read the private IPv4 address of its ENI as <target_ip>.
2. IAM: create the connector operator role; then lambda-core CreateNetworkConnector(Configuration={VpcEgressConfiguration:{SubnetIds:[<subnet>], SecurityGroupIds:[<group>], NetworkProtocol:"IPv4", AssociatedComputeResourceTypes:["MicroVm"]}}, OperatorRole=<role>); poll GetNetworkConnector until State=ACTIVE.
3. lambda-microvms: CreateMicrovmImage from an artifact whose guest serves JSON on port 8080 with /health and /connect?host=&port= (the latter opens one outbound TCP connection and reports connected/outcome/peer/local); poll until image state=CREATED and version state=SUCCESSFUL.
4. RunMicrovm(imageIdentifier=<imageArn>, ingressNetworkConnectors=[...:aws-network-connector:HTTP_INGRESS], egressNetworkConnectors=[<connector Arn>]); poll GetMicrovm until state=RUNNING and read `endpoint`. Run a second MicroVM from the same image with egressNetworkConnectors=[...:aws-network-connector:INTERNET_EGRESS].
5. CreateMicrovmAuthToken(microvmIdentifier=<id>, expirationInMinutes=15, allowedPorts=[{allPorts:{}}]) for each, then GET <endpoint>/health and <endpoint>/connect?host=<target_ip>&port=443 with the returned authToken map as request headers.
   Expected: HTTP 200 carrying the guest's JSON — connected:true with peer[0]==<target_ip> and a link-local local[0] for the connector-attached MicroVM; connected:false/outcome:"timed_out" with no "local" for the INTERNET_EGRESS-only MicroVM.
   Observed: HTTP 501 'This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.' for every request.
6. Repeat with a target security group that admits nothing, then AuthorizeSecurityGroupIngress on the target port, then RevokeSecurityGroupIngress: AWS turns the connect timed_out -> connected -> timed_out; the emulator answers 501 throughout, so the security group is never shown to gate the connector's path.
