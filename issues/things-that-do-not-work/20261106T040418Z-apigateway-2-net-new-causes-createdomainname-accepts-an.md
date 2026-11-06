Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-domain-name-malformed-certificate-arn-rejected apigateway.create-vpc-link-invalid-target-badrequest apigateway.vpc-link-invalid-target-arn-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-domain-name-malformed-certificate-arn-rejected|_error#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.create-vpc-link-invalid-target-badrequest|run#18|AssertionError|assert fetched["status"] == "FAILED", (
Issue-Key: apigateway.vpc-link-invalid-target-arn-rejected|run#18|AssertionError|assert fetched["status"] == "FAILED", (
Issue-Commit: 15ef1adb3fb7a9db5ce7069e30860cdce3c8580a

# apigateway: 2 net-new causes — CreateDomainName accepts an EDGE domain with no certificate at all (201), and a VpcLink whose target NLB does not exist settles AVAILABLE instead of FAILED/"NLB ARN not found"

> _Note: this issue was identified against the code base at commit `15ef1adb3fb7a9db5ce7069e30860cdce3c8580a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
Two net-new root causes under the `### apigateway` heading, neither marked ALREADY FILED nor OWNED, bundled because both are apigateway control-plane acceptance defects.

=== Cause 1 of 2: CreateDomainName accepts a request that names NO certificate for an EDGE (default) endpoint type ===
AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService) — CreateDomainName: POST /domainnames, success 201, output DomainName; certificateArn, certificateBody/certificatePrivateKey/certificateChain, certificateName, regionalCertificateArn and regionalCertificateName are all OPTIONAL members and only domainName is @required, and the model declares BadRequestException (HTTP 400) for "The submitted request is not valid, for example, the input is incomplete or incorrect". Which certificate member is required for which endpoint type is not in the model, so for that: recorded behavior only — a CreateDomainName that supplies no certificate member at all, with no endpointConfiguration (so the endpoint type defaults to EDGE), is REFUSED with HTTP 400 BadRequestException and the message "A certificate was not provided for the endpoint type EDGE." That whole sentence is fixed text except the endpoint type name, which is the type the request resolved to; nothing is created, so a following GetDomainName for that name answers 404 NotFoundException with message "Invalid domain name identifier specified". The recording also pins that this is a DISTINCT message from the malformed-ARN refusal ("Invalid ARN specified in the request", which the emulator already answers correctly): the ARN check is a format check, the certificate check is a completeness check.
Emulator: the call SUCCEEDS with HTTP 201 and returns a created record — domainName, domainNameArn, distributionDomainName, distributionHostedZoneId, endpointConfiguration {types:[EDGE]}, domainNameStatus PENDING, securityPolicy TLS_1_0 — for a request with no certificate member whatsoever. services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_domain_name.rs `handle` validates input bounds, the multi-type endpointConfiguration, the account quota and the name collision, and then stores the record with `certificate_arn: input.certificate_arn` etc. straight through; there is no check that the endpoint type's certificate member is present.
Why: a client gets an EDGE custom domain that can never serve TLS, reported as a normal PENDING domain, and the misconfiguration surfaces later as an unexplained handshake failure instead of as the 400 AWS answers synchronously. Negative tests and provisioning validators that rely on this refusal to catch an incomplete template accept the template, and an operator is never told which field is missing.
Fix: the CreateDomainName handler's validation in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_domain_name.rs — after the endpoint type is resolved (the `types`/`private`/`regional` decision), refuse with BadRequestException "A certificate was not provided for the endpoint type <TYPE>." when the request names no certificate for that endpoint type, before anything is minted or stored, so the refused create leaves no record.
Traces: apigateway.create-domain-name-malformed-certificate-arn-rejected (its malformed-ARN leg PASSES; the failure is the no-certificate leg).

=== Cause 2 of 2: a VpcLink whose well-formed target ARN names a non-existent network load balancer settles AVAILABLE with no statusMessage ===
AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService) — CreateVpcLink: POST /vpclinks, success 202, output VpcLink; VpcLink$status targets enum VpcLinkStatus with values AVAILABLE | PENDING | DELETING | FAILED, VpcLink$statusMessage is "A description about the VPC link status", and VpcLink$targetArns documents that "The network load balancer must be owned by the same Amazon Web Services account of the API owner". Which terminal state an unresolvable target produces is not modeled, so for that: recorded behavior only — CreateVpcLink validates ARN SHAPE synchronously (a malformed entry is refused 400 BadRequestException "Invalid ARN. ARN is not well formed <value>", where <value> is the offending targetArns entry — that leg already passes), accepts a well-formed but non-existent NLB ARN as status PENDING, and the asynchronous provisioning then settles the link in status FAILED with statusMessage "NLB ARN not found" (entirely fixed text — it echoes neither the ARN nor the link id). GetVpcLink reports that terminal pair.
Emulator: GetVpcLink, polled to a terminal value, reports status "AVAILABLE" and emits no statusMessage member, so the FAILED branch is unreachable: provisioning success is decided from ARN syntax alone without resolving the target against Elastic Load Balancing state.
Why: a client that gates deployment readiness on VpcLink status is told a link pointing at nothing is usable; the misconfiguration resurfaces later as an unexplained VPC_LINK/HTTP_PROXY integration failure with no resource reporting the fault, and any consumer reading statusMessage for the reason reads an absent member.
Already-open issues that describe this same cause but do NOT carry these two check ids (so a fixer closing them must re-replay the ids listed here): 20261103T135737Z-apigateway-createvpclink-s-asynchronous-provisioning-settles-a-link.md (keyed only on apigateway.create-vpc-link-empty-target-list-rejected) and 20261028T131753Z-apigateway-3-net-new-root-causes-a-vpclink.md cause 1 (keyed on apigateway.create-vpc-link-invalid-target-arn-returns-badrequest and three others). Close all three together.
Fix: the asynchronous provisioning outcome in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_vpc_link.rs — resolve each targetArn against Elastic Load Balancing state and settle the link in status FAILED with statusMessage "NLB ARN not found" when the named network load balancer does not exist; GetVpcLink's serializer must emit statusMessage whenever the stored record has one.
Traces: apigateway.create-vpc-link-invalid-target-badrequest, apigateway.vpc-link-invalid-target-arn-rejected.

## Evidence
Cause 1 — apigateway.create-domain-name-malformed-certificate-arn-rejected [mismatch], suite heading `### apigateway`:
`AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'domainName': '<generated>.example.com', 'domainNameArn': 'arn:aws:apigateway:us-west-2::/domainnames/<generated>.example.com', 'distributionDomainName': '<id>.cloudfront.net', 'distributionHostedZoneId': 'Z2FDTNDATAQYW2', 'endpointConfiguration': {'types': ['EDGE'], 'ipAddressType': 'ipv4'}, 'domainNameStatus': 'PENDING', 'securityPolicy': 'TLS_1_0', 'tags': {}}` for `create_domain_name(domainName=<name>)` with no certificate member.
The check requires Code == "BadRequestException" and Message == "A certificate was not provided for the endpoint type EDGE.", then requires GetDomainName for that name to answer NotFoundException / "Invalid domain name identifier specified", and requires that message to DIFFER from the malformed-ARN message "Invalid ARN specified in the request" (which the emulator already answers correctly, as does the positive control create with a well-formed regionalCertificateArn).

Cause 2 — two checks, both [mismatch] under `### apigateway`:
- apigateway.create-vpc-link-invalid-target-badrequest: `AssertionError: expected the link to a nonexistent NLB to settle FAILED, got 'AVAILABLE'`. Its earlier legs pass: the malformed entry "not-a-valid-nlb-arn" is refused BadRequestException "Invalid ARN. ARN is not well formed not-a-valid-nlb-arn", nothing is left behind, and the well-formed create returns status PENDING.
- apigateway.vpc-link-invalid-target-arn-rejected: `AssertionError: expected the link to a nonexistent NLB to settle FAILED for <vpcLinkId>, got AVAILABLE`.
Both then require statusMessage == "NLB ARN not found", which is unreachable while the status settles AVAILABLE.

Generated domain names, link ids, account ids and the NLB name/hash inside the target ARN are this run's values filling the templates; the requirements are the 400 + message for cause 1 and the terminal FAILED + fixed statusMessage for cause 2.

## Steps to Reproduce
Cause 1:
1. apigateway CreateDomainName(domainName="<unique>.example.com") with NO certificate member and no endpointConfiguration. Observed: HTTP 201 with domainNameStatus PENDING and endpointConfiguration.types ['EDGE']. Expected: HTTP 400 BadRequestException, message "A certificate was not provided for the endpoint type EDGE."
2. GetDomainName(domainName="<unique>.example.com"): must answer 404 NotFoundException "Invalid domain name identifier specified" (nothing was created).
3. Control that must keep passing: CreateDomainName(domainName=..., certificateArn="not-a-well-formed-acm-arn") is refused 400 BadRequestException "Invalid ARN specified in the request", and a create with a well-formed regionalCertificateArn + types [REGIONAL] succeeds.

Cause 2:
4. sts GetCallerIdentity for the account id, then CreateVpcLink(name="<unique>", targetArns=["arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/net/<nonexistent>/0123456789abcdef"]). Observed status PENDING (correct).
5. Poll GetVpcLink(vpcLinkId=<id>) to a terminal status. Observed: "AVAILABLE" with no statusMessage. Expected: "FAILED" with statusMessage "NLB ARN not found".
6. Control that must keep passing: CreateVpcLink(targetArns=["not-an-arn-at-all"]) is refused 400 BadRequestException "Invalid ARN. ARN is not well formed not-an-arn-at-all" and GetVpcLinks shows no such link.
7. Re-replay the three check ids in trace_ids.
